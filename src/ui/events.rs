use std::path::PathBuf;

use crate::ui::window::Sources;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum UiCommand {
    StartRequested(Sources),
    StopRequested,
    Quit,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum RecorderEvent {
    PortalOpened,
    RecordingStarted,
    RecordingStopped { output_path: PathBuf },
    Error(String),
    Cancelled,
}
