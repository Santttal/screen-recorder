use std::path::PathBuf;

use ashpd::WindowIdentifier;

use crate::ui::window::Sources;

#[allow(dead_code)]
#[derive(Debug)]
pub enum UiCommand {
    StartRequested {
        sources: Sources,
        parent: WindowIdentifier,
    },
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
