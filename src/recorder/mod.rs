//! Recorder-модуль: GStreamer pipeline, state machine, захват аудио/видео.

use async_channel::{Receiver, Sender};

use crate::ui::events::{RecorderEvent, UiCommand};

pub async fn run(cmd_rx: Receiver<UiCommand>, _evt_tx: Sender<RecorderEvent>) {
    while let Ok(cmd) = cmd_rx.recv().await {
        tracing::info!(?cmd, "recorder received command");
        // TODO: фазы 3+ — portal, pipeline, lifecycle.
    }
    tracing::info!("recorder loop exited");
}
