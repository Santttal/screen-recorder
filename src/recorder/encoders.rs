use std::sync::OnceLock;

use anyhow::{anyhow, Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    H264,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Software,
    Vaapi,
    Nvenc,
    Qsv,
    VaNew, // vah264enc из gst-plugins-bad va-plugin
}

impl Backend {
    pub fn is_hw(self) -> bool {
        matches!(self, Self::Vaapi | Self::Nvenc | Self::Qsv | Self::VaNew)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Software => "Software",
            Self::Vaapi => "VAAPI (legacy)",
            Self::VaNew => "VAAPI (va-plugin)",
            Self::Nvenc => "NVENC",
            Self::Qsv => "Intel QSV",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct EncoderInfo {
    pub factory_name: &'static str,
    pub codec: Codec,
    pub backend: Backend,
}

/// Порядок = приоритет при HwHint::Auto (сверху вниз).
const CANDIDATES: &[EncoderInfo] = &[
    EncoderInfo {
        factory_name: "vah264enc",
        codec: Codec::H264,
        backend: Backend::VaNew,
    },
    EncoderInfo {
        factory_name: "vaapih264enc",
        codec: Codec::H264,
        backend: Backend::Vaapi,
    },
    EncoderInfo {
        factory_name: "nvh264enc",
        codec: Codec::H264,
        backend: Backend::Nvenc,
    },
    EncoderInfo {
        factory_name: "qsvh264enc",
        codec: Codec::H264,
        backend: Backend::Qsv,
    },
    EncoderInfo {
        factory_name: "x264enc",
        codec: Codec::H264,
        backend: Backend::Software,
    },
];

static CACHE: OnceLock<Vec<EncoderInfo>> = OnceLock::new();

pub fn detect_available_encoders() -> &'static [EncoderInfo] {
    CACHE
        .get_or_init(|| {
            let list: Vec<_> = CANDIDATES
                .iter()
                .filter(|info| gst::ElementFactory::find(info.factory_name).is_some())
                .copied()
                .collect();
            let labels: Vec<_> = list.iter().map(|e| e.factory_name).collect();
            tracing::info!(encoders = ?labels, "detected video encoders");
            list
        })
        .as_slice()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HwHint {
    Auto,
    ForceHw,
    ForceSw,
}

pub struct VideoEncoder {
    pub element: gst::Element,
    pub info: EncoderInfo,
}

impl VideoEncoder {
    pub fn for_codec(codec: Codec, hint: HwHint, bitrate_kbps: u32) -> Result<Self> {
        let all = detect_available_encoders();
        let candidates = all
            .iter()
            .filter(|e| e.codec == codec)
            .filter(|e| match hint {
                HwHint::Auto => true,
                HwHint::ForceHw => e.backend.is_hw(),
                HwHint::ForceSw => !e.backend.is_hw(),
            });
        let info = candidates
            .copied()
            .next()
            .ok_or_else(|| anyhow!("no encoder for codec={:?} hint={:?}", codec, hint))?;

        let element = gst::ElementFactory::make(info.factory_name)
            .name("venc")
            .build()
            .with_context(|| format!("failed to create {}", info.factory_name))?;

        apply_properties(&element, &info, bitrate_kbps);

        tracing::info!(
            factory = info.factory_name,
            backend = info.backend.label(),
            bitrate_kbps,
            "video encoder selected"
        );

        Ok(Self { element, info })
    }
}

fn apply_properties(element: &gst::Element, info: &EncoderInfo, bitrate_kbps: u32) {
    match info.factory_name {
        "x264enc" => {
            element.set_property("bitrate", bitrate_kbps);
            element.set_property("key-int-max", bitrate_kbps.clamp(10, 100)); // будет переопределено в pipeline
            element.set_property("bframes", 0u32);
            element.set_property("byte-stream", false);
            element.set_property_from_str("tune", "zerolatency");
            element.set_property_from_str("speed-preset", "veryfast");
        }
        "vaapih264enc" => {
            // bitrate в vaapih264enc в kbps
            element.set_property("bitrate", bitrate_kbps);
            element.set_property("keyframe-period", 100u32);
            // Не все Intel iGPU поддерживают CBR — используем VBR (шире совместимость)
            element.set_property_from_str("rate-control", "vbr");
        }
        "vah264enc" => {
            // va-plugin принимает bitrate в kbps
            element.set_property("bitrate", bitrate_kbps);
            element.set_property_from_str("rate-control", "vbr");
        }
        "nvh264enc" => {
            element.set_property("bitrate", bitrate_kbps);
            element.set_property_from_str("preset", "low-latency-hq");
            element.set_property_from_str("rc-mode", "cbr");
        }
        "qsvh264enc" => {
            element.set_property("bitrate", bitrate_kbps);
            element.set_property("target-usage", 4u32);
        }
        _ => {}
    }
}

/// Какой элемент преобразования цвета нужен перед HW-энкодером.
/// Для VAAPI на многих Intel iGPU VPP недоступен — используем SW videoconvert + явный NV12 caps.
pub fn preencoder_converter_factory(backend: Backend) -> &'static str {
    match backend {
        // VPP (vaapipostproc/vapostproc) часто не работает на встроенных GPU →
        // полагаемся на SW videoconvert для преобразования в NV12.
        Backend::Vaapi | Backend::VaNew => "videoconvert",
        Backend::Nvenc => "nvvidconv",
        Backend::Qsv => "videoconvert",
        Backend::Software => "videoconvert",
    }
}

/// Нужен ли явный capsfilter format=NV12 перед HW-энкодером.
pub fn requires_nv12_caps(backend: Backend) -> bool {
    matches!(backend, Backend::Vaapi | Backend::VaNew | Backend::Qsv)
}
