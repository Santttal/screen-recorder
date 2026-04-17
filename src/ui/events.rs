use std::os::fd::RawFd;
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
}
