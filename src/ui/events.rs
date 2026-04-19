use std::os::fd::RawFd;
use std::path::PathBuf;

use ashpd::WindowIdentifier;

use crate::config::TranscriptionModel;
use crate::ui::shell::Sources;

#[allow(dead_code)]
#[derive(Debug)]
pub enum UiCommand {
    StartRequested {
        sources: Sources,
        parent: WindowIdentifier,
    },
    StopRequested,
    TranscribeRequested {
        video_path: PathBuf,
    },
    Quit,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum RecorderEvent {
    PortalOpened,
    ScreenCastReady {
        fd: RawFd,
        node_id: u32,
        output_path: PathBuf,
    },
    RecordingStarted,
    RecordingStopped {
        output_path: PathBuf,
    },
    Error(String),
    Cancelled,
    TranscriptionStarted {
        video_path: PathBuf,
    },
    TranscriptionProgress {
        video_path: PathBuf,
        part: u32,
        total: u32,
    },
    TranscriptionFinished {
        video_path: PathBuf,
        text_path: PathBuf,
        model: TranscriptionModel,
        chunks: u32,
    },
    TranscriptionFailed {
        video_path: PathBuf,
        message: String,
    },
}
