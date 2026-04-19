use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Container {
    Mkv,
    Mp4,
    Webm,
}

impl Container {
    pub fn ext(self) -> &'static str {
        match self {
            Self::Mkv => "mkv",
            Self::Mp4 => "mp4",
            Self::Webm => "webm",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VideoCodec {
    H264,
    H265,
    Vp9,
    Av1,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AudioMode {
    Separate,
    Mixed,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CursorMode {
    Hidden,
    Embedded,
    Metadata,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RegionMode {
    FullScreen,
    Monitor,
    Window,
}

/// Источник захвата экрана (UI-вариант). Phase 19.a.5.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CaptureSource {
    /// Весь экран (MONITOR в portal-терминах).
    Screen,
    /// Конкретное окно — portal покажет window-picker.
    Window,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EncoderHint {
    Auto,
    Hardware,
    Software,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TranscriptionModel {
    Whisper1,
    Gpt4oTranscribe,
    Gpt4oMiniTranscribe,
    Gpt4oTranscribeDiarize,
}

impl TranscriptionModel {
    pub fn api_id(self) -> &'static str {
        match self {
            Self::Whisper1 => "whisper-1",
            Self::Gpt4oTranscribe => "gpt-4o-transcribe",
            Self::Gpt4oMiniTranscribe => "gpt-4o-mini-transcribe",
            Self::Gpt4oTranscribeDiarize => "gpt-4o-transcribe-diarize",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Whisper1 => "Whisper-1",
            Self::Gpt4oTranscribe => "GPT-4o Transcribe",
            Self::Gpt4oMiniTranscribe => "GPT-4o Mini Transcribe",
            Self::Gpt4oTranscribeDiarize => "GPT-4o Transcribe + Diarize",
        }
    }

    /// true — ответ приходит как plain text (response_format=text).
    /// false — как JSON (response_format=json). Для `gpt-4o-transcribe-diarize`
    /// нужен json, потому что там сегменты по дикторам. Остальные модели
    /// поддерживают text (speech-to-text guide OpenAI).
    pub fn supports_text_response(self) -> bool {
        !matches!(self, Self::Gpt4oTranscribeDiarize)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    pub output_dir: PathBuf,
    pub container: Container,
    pub video_codec: VideoCodec,
    pub fps: u32,
    pub video_bitrate: u32,
    pub audio_bitrate: u32,
    pub audio_mode: AudioMode,
    pub cursor_mode: CursorMode,
    pub region_mode: RegionMode,
    pub capture_source: CaptureSource,
    pub encoder_hint: EncoderHint,
    pub hotkey_start_stop: String,
    pub transcription_enabled: bool,
    pub transcription_model: TranscriptionModel,
    pub openai_api_key: String,
    pub transcription_language: String,
}

impl Default for Settings {
    fn default() -> Self {
        let output_dir = directories::UserDirs::new()
            .and_then(|d| d.video_dir().map(|v| v.join("Ralume")))
            .unwrap_or_else(std::env::temp_dir);

        Self {
            output_dir,
            container: Container::Mkv,
            video_codec: VideoCodec::H264,
            fps: 10,
            video_bitrate: 2500,
            audio_bitrate: 128,
            audio_mode: AudioMode::Mixed,
            cursor_mode: CursorMode::Embedded,
            region_mode: RegionMode::Monitor,
            capture_source: CaptureSource::Screen,
            encoder_hint: EncoderHint::Auto,
            hotkey_start_stop: "<Ctrl><Alt>R".to_owned(),
            transcription_enabled: false,
            transcription_model: TranscriptionModel::Gpt4oMiniTranscribe,
            openai_api_key: String::new(),
            transcription_language: String::new(),
        }
    }
}

pub type SharedSettings = Arc<RwLock<Settings>>;

pub fn shared(s: Settings) -> SharedSettings {
    Arc::new(RwLock::new(s))
}

pub fn config_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("dev", "local", "Ralume")
        .map(|d| d.config_dir().join("settings.toml"))
}

pub fn load() -> Settings {
    let Some(path) = config_path() else {
        tracing::warn!("no project dirs, using default settings");
        return Settings::default();
    };

    match std::fs::read_to_string(&path) {
        Ok(text) => match toml::from_str::<Settings>(&text) {
            Ok(s) => {
                tracing::info!(path = %path.display(), "settings loaded");
                s
            }
            Err(err) => {
                tracing::warn!(%err, path = %path.display(), "failed to parse settings.toml, using defaults");
                Settings::default()
            }
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!(path = %path.display(), "settings.toml not found, creating with defaults");
            let s = Settings::default();
            if let Err(e) = save(&s) {
                tracing::warn!(%e, "failed to write default settings.toml");
            }
            s
        }
        Err(err) => {
            tracing::warn!(%err, "failed to read settings.toml, using defaults");
            Settings::default()
        }
    }
}

pub fn save(settings: &Settings) -> Result<()> {
    let path = config_path().context("no project dirs")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }
    let text = toml::to_string_pretty(settings).context("serialize settings to toml")?;
    write_atomic(&path, &text).with_context(|| format!("write {}", path.display()))
}

fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)?;
    restrict_permissions(path);
    Ok(())
}

/// Сужаем права до `0600` — в `settings.toml` теперь может лежать OpenAI API-ключ.
/// На не-Unix-платформах no-op.
#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
        tracing::warn!(%e, path = %path.display(), "failed to chmod 0600 on settings.toml");
    }
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_default() {
        let a = Settings::default();
        let text = toml::to_string_pretty(&a).unwrap();
        let b: Settings = toml::from_str(&text).unwrap();
        assert_eq!(a.container, b.container);
        assert_eq!(a.fps, b.fps);
        assert_eq!(a.video_bitrate, b.video_bitrate);
        assert_eq!(a.audio_mode, b.audio_mode);
        assert_eq!(a.hotkey_start_stop, b.hotkey_start_stop);
    }

    #[test]
    fn missing_fields_fall_back_to_default() {
        // Only partial fields — serde(default) fills the rest.
        let partial = r#"fps = 24"#;
        let s: Settings = toml::from_str(partial).unwrap();
        assert_eq!(s.fps, 24);
        assert_eq!(s.video_bitrate, Settings::default().video_bitrate);
    }

    #[test]
    fn transcription_defaults_and_roundtrip() {
        let a = Settings::default();
        assert!(!a.transcription_enabled);
        assert_eq!(a.transcription_model, TranscriptionModel::Gpt4oMiniTranscribe);
        assert!(a.openai_api_key.is_empty());
        assert!(a.transcription_language.is_empty());

        let text = toml::to_string_pretty(&a).unwrap();
        let b: Settings = toml::from_str(&text).unwrap();
        assert_eq!(a.transcription_enabled, b.transcription_enabled);
        assert_eq!(a.transcription_model, b.transcription_model);
        assert_eq!(a.openai_api_key, b.openai_api_key);
    }

    #[test]
    fn transcription_model_api_ids() {
        assert_eq!(TranscriptionModel::Whisper1.api_id(), "whisper-1");
        assert_eq!(TranscriptionModel::Gpt4oTranscribe.api_id(), "gpt-4o-transcribe");
        assert_eq!(TranscriptionModel::Gpt4oMiniTranscribe.api_id(), "gpt-4o-mini-transcribe");
        assert_eq!(
            TranscriptionModel::Gpt4oTranscribeDiarize.api_id(),
            "gpt-4o-transcribe-diarize"
        );
        assert!(TranscriptionModel::Whisper1.supports_text_response());
        assert!(TranscriptionModel::Gpt4oMiniTranscribe.supports_text_response());
        // diarize — единственная модель с json-ответом (формат diarized_json).
        assert!(!TranscriptionModel::Gpt4oTranscribeDiarize.supports_text_response());
    }

    #[test]
    fn missing_stt_fields_fall_back() {
        // Старые settings.toml без полей транскрипции парсятся, значения — дефолтные.
        let legacy = r#"
fps = 30
video_bitrate = 5000
audio_bitrate = 128
container = "mkv"
video_codec = "h264"
audio_mode = "mixed"
cursor_mode = "embedded"
region_mode = "monitor"
encoder_hint = "auto"
hotkey_start_stop = "<Ctrl><Alt>R"
output_dir = "/tmp/ralume"
"#;
        let s: Settings = toml::from_str(legacy).unwrap();
        assert!(!s.transcription_enabled);
        assert!(s.openai_api_key.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn save_sets_0600_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!(
            "ralume-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.toml");
        write_atomic(&path, "fps = 1\n").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {mode:o}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
