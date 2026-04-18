use std::os::fd::RawFd;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use async_channel::Sender;
use gstreamer as gst;
use gstreamer::prelude::*;
use gtk::glib;
use gtk4 as gtk;

use crate::config::{AudioMode, EncoderHint, Settings};
use crate::recorder::audio::{detect_audio_devices, ensure_source_volume_full, AudioDevices};
use crate::recorder::encoders::{
    preencoder_converter_factory, requires_nv12_caps, Codec, HwHint, VideoEncoder,
};
use crate::ui::events::RecorderEvent;

#[derive(Debug, Clone)]
pub struct RecordRequest {
    pub capture_screen: bool,
    pub capture_system_audio: bool,
    pub capture_mic: bool,
    pub output_path: PathBuf,
    pub fd: RawFd,
    pub node_id: u32,
    pub settings: Settings,
}

pub fn build_pipeline(req: &RecordRequest) -> Result<gst::Pipeline> {
    if !req.capture_screen {
        return Err(anyhow!("capture_screen=false не поддерживается на MVP"));
    }
    let pipeline = build_video_pipeline(req.fd, req.node_id, &req.output_path, &req.settings)?;

    if req.capture_system_audio || req.capture_mic {
        let AudioDevices {
            monitor_source,
            mic_source,
        } = detect_audio_devices()?;
        let audio_bitrate_bps = (req.settings.audio_bitrate as i32).saturating_mul(1000);
        let prefer_mixed = req.settings.audio_mode == AudioMode::Mixed;
        match (req.capture_system_audio, req.capture_mic) {
            (true, true) if prefer_mixed => add_mixed_audio_branch(
                &pipeline,
                &monitor_source,
                &mic_source,
                audio_bitrate_bps,
            )?,
            (true, true) => {
                add_system_audio_branch(&pipeline, &monitor_source, audio_bitrate_bps)?;
                add_mic_branch(&pipeline, &mic_source, audio_bitrate_bps)?;
            }
            (true, false) => add_system_audio_branch(&pipeline, &monitor_source, audio_bitrate_bps)?,
            (false, true) => add_mic_branch(&pipeline, &mic_source, audio_bitrate_bps)?,
            (false, false) => unreachable!(),
        }
    }

    dump_dot(&pipeline, "pipeline-ready");
    Ok(pipeline)
}

pub fn build_video_pipeline(
    fd: RawFd,
    node_id: u32,
    output_path: &Path,
    settings: &Settings,
) -> Result<gst::Pipeline> {
    let pipeline = gst::Pipeline::new(Some("screen_record"));

    let src = gst::ElementFactory::make("pipewiresrc")
        .name("src")
        .property("fd", fd)
        .property("path", node_id.to_string())
        .property("do-timestamp", true)
        .build()
        .context("pipewiresrc not available (gstreamer1.0-pipewire?)")?;

    let vconv = gst::ElementFactory::make("videoconvert")
        .name("vconv")
        .build()
        .context("videoconvert missing")?;

    let vrate = gst::ElementFactory::make("videorate")
        .name("vrate")
        .build()
        .context("videorate missing")?;

    let vrate_caps = gst::Caps::builder("video/x-raw")
        .field("framerate", gst::Fraction::new(settings.fps as i32, 1))
        .build();
    let vrate_filter = gst::ElementFactory::make("capsfilter")
        .name("vrate_filter")
        .property("caps", &vrate_caps)
        .build()
        .context("capsfilter missing")?;

    let vqueue = gst::ElementFactory::make("queue")
        .name("vqueue")
        .property("max-size-time", 2_000_000_000u64)
        .property("max-size-buffers", 0u32)
        .property("max-size-bytes", 0u32)
        .build()
        .context("queue missing")?;

    let hint = match settings.encoder_hint {
        EncoderHint::Auto => HwHint::Auto,
        EncoderHint::Hardware => HwHint::ForceHw,
        EncoderHint::Software => HwHint::ForceSw,
    };
    let encoder = match VideoEncoder::for_codec(Codec::H264, hint, settings.video_bitrate) {
        Ok(e) => e,
        Err(err) if hint != HwHint::ForceHw => {
            tracing::warn!(%err, "HW encoder unavailable, falling back to x264enc");
            VideoEncoder::for_codec(Codec::H264, HwHint::ForceSw, settings.video_bitrate)?
        }
        Err(err) => return Err(err),
    };
    let venc = encoder.element.clone();
    let backend = encoder.info.backend;
    // key-int-max зависит от fps для всех энкодеров, где оно поддерживается
    if venc.has_property("key-int-max", None) {
        venc.set_property("key-int-max", settings.fps * 10);
    } else if venc.has_property("keyframe-period", None) {
        venc.set_property("keyframe-period", settings.fps * 10);
    }

    let vparse = gst::ElementFactory::make("h264parse")
        .name("vparse")
        .build()
        .context("h264parse missing (gstreamer1.0-plugins-bad?)")?;

    let mux = gst::ElementFactory::make("matroskamux")
        .name("mux")
        .build()
        .context("matroskamux missing")?;

    let fsink = gst::ElementFactory::make("filesink")
        .name("fsink")
        .property("location", output_path.to_string_lossy().as_ref())
        .build()
        .context("filesink missing")?;

    // Для HW-бекендов вставляем videoconvert + capsfilter(NV12) перед энкодером.
    let hw_pre: Option<gst::Element> = if backend.is_hw() {
        let factory = preencoder_converter_factory(backend);
        gst::ElementFactory::make(factory)
            .name("hw_pre")
            .build()
            .ok()
            .or_else(|| {
                tracing::warn!(%factory, "HW pre-encoder element missing, HW path may fail");
                None
            })
    } else {
        None
    };

    let hw_caps: Option<gst::Element> = if requires_nv12_caps(backend) {
        let caps = gst::Caps::builder("video/x-raw")
            .field("format", "NV12")
            .build();
        gst::ElementFactory::make("capsfilter")
            .name("hw_caps")
            .property("caps", &caps)
            .build()
            .ok()
    } else {
        None
    };

    let mut elements: Vec<&gst::Element> = vec![&src, &vconv, &vrate, &vrate_filter, &vqueue];
    if let Some(ref e) = hw_pre {
        elements.push(e);
    }
    if let Some(ref e) = hw_caps {
        elements.push(e);
    }
    elements.extend([&venc, &vparse, &mux, &fsink]);

    pipeline.add_many(&elements)?;
    gst::Element::link_many(&elements)?;

    tracing::debug!(
        fd,
        node_id,
        output = %output_path.display(),
        "video pipeline built"
    );

    Ok(pipeline)
}

fn audio_caps() -> gst::Caps {
    gst::Caps::builder("audio/x-raw")
        .field("format", "S16LE")
        .field("channels", 2i32)
        .field("rate", 48000i32)
        .build()
}

pub fn add_system_audio_branch(
    pipeline: &gst::Pipeline,
    monitor_source: &str,
    bitrate_bps: i32,
) -> Result<()> {
    if let Err(e) = ensure_source_volume_full(monitor_source) {
        tracing::warn!(%e, "failed to ensure monitor source volume");
    }
    let src = gst::ElementFactory::make("pulsesrc")
        .name("sys_src")
        .property("device", monitor_source)
        .property("provide-clock", false)
        .property("do-timestamp", true)
        .build()
        .context("pulsesrc missing")?;
    src.set_property_from_str("slave-method", "skew");
    let aconv = gst::ElementFactory::make("audioconvert")
        .name("sys_aconv")
        .build()
        .context("audioconvert missing")?;
    let ares = gst::ElementFactory::make("audioresample")
        .name("sys_ares")
        .build()
        .context("audioresample missing")?;
    let arate = gst::ElementFactory::make("audiorate")
        .name("sys_arate")
        .build()
        .context("audiorate missing")?;
    let capsf = gst::ElementFactory::make("capsfilter")
        .name("sys_capsf")
        .property("caps", audio_caps())
        .build()
        .context("capsfilter missing")?;
    let aqueue = gst::ElementFactory::make("queue")
        .name("sys_aqueue")
        .property("max-size-time", 2_000_000_000u64)
        .property("max-size-buffers", 0u32)
        .property("max-size-bytes", 0u32)
        .build()
        .context("queue missing")?;
    let aenc = gst::ElementFactory::make("opusenc")
        .name("sys_aenc")
        .property("bitrate", bitrate_bps)
        .build()
        .context("opusenc missing (gstreamer1.0-plugins-base?)")?;

    pipeline.add_many(&[&src, &aconv, &ares, &arate, &capsf, &aqueue, &aenc])?;
    gst::Element::link_many(&[&src, &aconv, &ares, &arate, &capsf, &aqueue, &aenc])?;

    link_to_mux(pipeline, &aenc)?;
    tracing::info!(device = %monitor_source, "system audio branch attached");
    Ok(())
}

pub fn add_mic_branch(
    pipeline: &gst::Pipeline,
    mic_source: &str,
    bitrate_bps: i32,
) -> Result<()> {
    let src = gst::ElementFactory::make("pulsesrc")
        .name("mic_src")
        .property("device", mic_source)
        .property("provide-clock", false)
        .property("do-timestamp", true)
        .build()
        .context("pulsesrc missing")?;
    src.set_property_from_str("slave-method", "skew");
    let aconv = gst::ElementFactory::make("audioconvert")
        .name("mic_aconv")
        .build()?;
    let ares = gst::ElementFactory::make("audioresample")
        .name("mic_ares")
        .build()?;
    let arate = gst::ElementFactory::make("audiorate")
        .name("mic_arate")
        .build()
        .context("audiorate missing")?;
    let capsf = gst::ElementFactory::make("capsfilter")
        .name("mic_capsf")
        .property("caps", audio_caps())
        .build()?;
    let aqueue = gst::ElementFactory::make("queue")
        .name("mic_aqueue")
        .property("max-size-time", 2_000_000_000u64)
        .property("max-size-buffers", 0u32)
        .property("max-size-bytes", 0u32)
        .build()?;
    let aenc = gst::ElementFactory::make("opusenc")
        .name("mic_aenc")
        .property("bitrate", bitrate_bps)
        .build()
        .context("opusenc missing (gstreamer1.0-plugins-base?)")?;

    pipeline.add_many(&[&src, &aconv, &ares, &arate, &capsf, &aqueue, &aenc])?;
    gst::Element::link_many(&[&src, &aconv, &ares, &arate, &capsf, &aqueue, &aenc])?;

    link_to_mux(pipeline, &aenc)?;
    tracing::info!(device = %mic_source, "mic branch attached");
    Ok(())
}

pub fn add_mixed_audio_branch(
    pipeline: &gst::Pipeline,
    monitor_source: &str,
    mic_source: &str,
    bitrate_bps: i32,
) -> Result<()> {
    if let Err(e) = ensure_source_volume_full(monitor_source) {
        tracing::warn!(%e, "failed to ensure monitor source volume");
    }

    let (sys_src, sys_tail) = build_audio_preproc("sys", monitor_source)?;
    let (mic_src, mic_tail) = build_audio_preproc("mic", mic_source)?;

    // Аттенюация перед суммированием, чтобы пики двух источников не клипили.
    let sys_vol = gst::ElementFactory::make("volume")
        .name("sys_vol")
        .property("volume", 0.7f64)
        .build()
        .context("volume missing")?;
    let mic_vol = gst::ElementFactory::make("volume")
        .name("mic_vol")
        .property("volume", 1.0f64)
        .build()
        .context("volume missing")?;

    let mixer = gst::ElementFactory::make("audiomixer")
        .name("amix")
        .build()
        .context("audiomixer missing (gstreamer1.0-plugins-bad?)")?;
    let mix_capsf = gst::ElementFactory::make("capsfilter")
        .name("mix_capsf")
        .property("caps", audio_caps())
        .build()?;
    let aqueue = gst::ElementFactory::make("queue")
        .name("mix_aqueue")
        .property("max-size-time", 2_000_000_000u64)
        .property("max-size-buffers", 0u32)
        .property("max-size-bytes", 0u32)
        .build()?;
    let aenc = gst::ElementFactory::make("opusenc")
        .name("mix_aenc")
        .property("bitrate", bitrate_bps)
        .build()
        .context("opusenc missing (gstreamer1.0-plugins-base?)")?;

    pipeline.add_many(&[
        &sys_src,
        &sys_tail.convert,
        &sys_tail.resample,
        &sys_tail.rate,
        &sys_tail.capsf,
        &sys_vol,
        &mic_src,
        &mic_tail.convert,
        &mic_tail.resample,
        &mic_tail.rate,
        &mic_tail.capsf,
        &mic_vol,
        &mixer,
        &mix_capsf,
        &aqueue,
        &aenc,
    ])?;

    gst::Element::link_many(&[
        &sys_src,
        &sys_tail.convert,
        &sys_tail.resample,
        &sys_tail.rate,
        &sys_tail.capsf,
        &sys_vol,
    ])?;
    gst::Element::link_many(&[
        &mic_src,
        &mic_tail.convert,
        &mic_tail.resample,
        &mic_tail.rate,
        &mic_tail.capsf,
        &mic_vol,
    ])?;

    sys_vol.link(&mixer)?;
    mic_vol.link(&mixer)?;

    gst::Element::link_many(&[&mixer, &mix_capsf, &aqueue, &aenc])?;

    link_to_mux(pipeline, &aenc)?;
    tracing::info!(
        %monitor_source,
        %mic_source,
        "mixed audio branch attached (sys + mic → single opus track)"
    );
    Ok(())
}

struct AudioPreprocTail {
    convert: gst::Element,
    resample: gst::Element,
    rate: gst::Element,
    capsf: gst::Element,
}

fn build_audio_preproc(prefix: &str, device: &str) -> Result<(gst::Element, AudioPreprocTail)> {
    let src = gst::ElementFactory::make("pulsesrc")
        .name(format!("{prefix}_src"))
        .property("device", device)
        .property("provide-clock", false)
        .property("do-timestamp", true)
        .build()
        .context("pulsesrc missing")?;
    src.set_property_from_str("slave-method", "skew");

    let convert = gst::ElementFactory::make("audioconvert")
        .name(format!("{prefix}_aconv"))
        .build()
        .context("audioconvert missing")?;
    let resample = gst::ElementFactory::make("audioresample")
        .name(format!("{prefix}_ares"))
        .build()
        .context("audioresample missing")?;
    let rate = gst::ElementFactory::make("audiorate")
        .name(format!("{prefix}_arate"))
        .build()
        .context("audiorate missing")?;
    let capsf = gst::ElementFactory::make("capsfilter")
        .name(format!("{prefix}_capsf"))
        .property("caps", audio_caps())
        .build()
        .context("capsfilter missing")?;

    Ok((
        src,
        AudioPreprocTail {
            convert,
            resample,
            rate,
            capsf,
        },
    ))
}

fn link_to_mux(pipeline: &gst::Pipeline, aenc: &gst::Element) -> Result<()> {
    let mux = pipeline
        .by_name("mux")
        .ok_or_else(|| anyhow!("mux element not found in pipeline"))?;
    let mux_pad = mux
        .request_pad_simple("audio_%u")
        .ok_or_else(|| anyhow!("matroskamux did not grant audio sink pad"))?;
    let src_pad = aenc
        .static_pad("src")
        .ok_or_else(|| anyhow!("opusenc has no src pad"))?;
    src_pad.link(&mux_pad)?;
    tracing::debug!(pad = %mux_pad.name(), "linked audio branch to mux");
    Ok(())
}

pub fn start(pipeline: &gst::Pipeline) -> Result<()> {
    pipeline.set_state(gst::State::Playing)?;
    tracing::info!("pipeline started");
    Ok(())
}

pub fn stop_graceful(pipeline: &gst::Pipeline) {
    pipeline.send_event(gst::event::Eos::new());
    tracing::info!("EOS sent");
}

pub fn attach_bus_watch(
    pipeline: &gst::Pipeline,
    tx: Sender<RecorderEvent>,
    output_path: std::path::PathBuf,
) -> Result<glib::SourceId> {
    let bus = pipeline.bus().context("pipeline has no bus")?;
    let weak = pipeline.downgrade();
    let source_id = bus
        .add_watch(move |_, msg| {
            use gst::MessageView::*;
            match msg.view() {
                Eos(_) => {
                    tracing::info!("EOS received on bus");
                    if let Some(p) = weak.upgrade() {
                        if let Err(e) = p.set_state(gst::State::Null) {
                            tracing::warn!(%e, "failed set_state(Null) after EOS");
                        } else {
                            tracing::info!("pipeline reached Null");
                        }
                    }
                    let _ = tx.send_blocking(RecorderEvent::RecordingStopped {
                        output_path: output_path.clone(),
                    });
                }
                Error(err) => {
                    tracing::error!(
                        src = ?err.src().map(|s| s.name()),
                        error = %err.error(),
                        debug = ?err.debug(),
                        "pipeline error"
                    );
                    if let Some(p) = weak.upgrade() {
                        dump_dot(&p, "pipeline-error");
                        let _ = p.set_state(gst::State::Null);
                    }
                    let _ = tx.send_blocking(RecorderEvent::Error(err.error().to_string()));
                }
                StateChanged(sc) => {
                    if let Some(p) = weak.upgrade() {
                        if sc.src().map(|s| s.as_ptr()) == Some(p.upcast_ref::<gst::Object>().as_ptr()) {
                            tracing::debug!(
                                old = ?sc.old(),
                                new = ?sc.current(),
                                "pipeline state changed"
                            );
                            if sc.current() == gst::State::Playing {
                                dump_dot(&p, "pipeline-playing");
                                let _ = tx.send_blocking(RecorderEvent::RecordingStarted);
                            }
                        }
                    }
                }
                _ => {}
            }
            glib::Continue(true)
        })
        .context("failed to attach bus watch")?;
    Ok(source_id)
}

fn dump_dot(pipeline: &gst::Pipeline, name: &str) {
    if std::env::var_os("GST_DEBUG_DUMP_DOT_DIR").is_some() {
        gst::debug_bin_to_dot_file_with_ts(pipeline, gst::DebugGraphDetails::all(), name);
    }
}
