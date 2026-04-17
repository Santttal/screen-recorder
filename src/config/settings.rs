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
    pub hotkey_start_stop: String,
}

impl Default for Settings {
    fn default() -> Self {
        let output_dir = directories::UserDirs::new()
            .and_then(|d| d.video_dir().map(|v| v.join("Recordings")))
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
            hotkey_start_stop: "<Ctrl><Alt>R".to_owned(),
        }
    }
}

pub type SharedSettings = Arc<RwLock<Settings>>;

pub fn shared(s: Settings) -> SharedSettings {
    Arc::new(RwLock::new(s))
}

pub fn config_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("dev", "local", "ScreenRecord")
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
    Ok(())
}

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
}
