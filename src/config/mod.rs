//! Config-модуль: пользовательские настройки (serde + toml).

pub mod settings;

pub use settings::{
    load, save, shared, AudioMode, CaptureSource, Container, CursorMode, EncoderHint, RegionMode,
    Settings, SharedSettings, TranscriptionModel, VideoCodec,
};
