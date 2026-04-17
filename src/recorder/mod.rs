//! Recorder-модуль: GStreamer pipeline, state machine, захват аудио/видео.

pub mod audio;
pub mod output;
pub mod pipeline;

use async_channel::{Receiver, Sender};

use crate::portal::screencast::{open_or_cancel, OpenOutcome, ScreenCastSession};
use crate::portal::state::PortalState;
use crate::ui::events::{RecorderEvent, UiCommand};

pub use output::default_output_path;
pub use pipeline::{attach_bus_watch, build_pipeline, start, stop_graceful, RecordRequest};

pub async fn run(cmd_rx: Receiver<UiCommand>, evt_tx: Sender<RecorderEvent>) {
    let mut current_session: Option<ScreenCastSession> = None;

    while let Ok(cmd) = cmd_rx.recv().await {
        match cmd {
            UiCommand::StartRequested { sources, parent } => {
                tracing::info!(?sources, "start requested");

                let saved = PortalState::load();
                let token = saved.screencast_restore_token.clone();
                if token.is_some() {
                    tracing::debug!("restore_token loaded from state.json");
                }

                let _ = evt_tx.send(RecorderEvent::PortalOpened).await;

                match open_or_cancel(parent, token).await {
                    Ok(OpenOutcome::Opened(session)) => {
                        let Some(node_id) = session.primary_node_id() else {
                            tracing::warn!("portal returned no streams");
                            let _ = evt_tx
                                .send(RecorderEvent::Error("no streams from portal".into()))
                                .await;
                            continue;
                        };

                        tracing::info!(
                            node_id,
                            size = ?session.primary_size(),
                            fd = session.pipewire_fd,
                            "screencast ready"
                        );

                        if let Some(new_token) = &session.restore_token {
                            if saved.screencast_restore_token.as_deref() != Some(new_token.as_str())
                            {
                                let new_state = PortalState {
                                    screencast_restore_token: Some(new_token.clone()),
                                };
                                if let Err(e) = new_state.save() {
                                    tracing::warn!(%e, "failed to save state.json");
                                } else {
                                    tracing::debug!("restore_token saved to state.json");
                                }
                            }
                        }

                        let output_path = match default_output_path() {
                            Ok(p) => p,
                            Err(e) => {
                                tracing::error!(%e, "cannot resolve output path");
                                let _ = evt_tx.send(RecorderEvent::Error(e.to_string())).await;
                                let _ = session.close().await;
                                continue;
                            }
                        };

                        let fd = session.pipewire_fd;
                        current_session = Some(session);

                        let _ = evt_tx
                            .send(RecorderEvent::ScreenCastReady {
                                fd,
                                node_id,
                                output_path,
                            })
                            .await;
                    }
                    Ok(OpenOutcome::Cancelled) => {
                        tracing::debug!("user cancelled screencast dialog");
                        let _ = evt_tx.send(RecorderEvent::Cancelled).await;
                    }
                    Err(e) => {
                        tracing::error!(%e, "portal error");
                        let mut fresh = saved.clone();
                        if fresh.screencast_restore_token.is_some() {
                            fresh.screencast_restore_token = None;
                            let _ = fresh.save();
                        }
                        let _ = evt_tx.send(RecorderEvent::Error(e.to_string())).await;
                    }
                }
            }
            UiCommand::StopRequested => {
                if let Some(session) = current_session.take() {
                    if let Err(e) = session.close().await {
                        tracing::warn!(%e, "failed to close portal session");
                    } else {
                        tracing::debug!("portal session closed");
                    }
                } else {
                    tracing::warn!("StopRequested without active session");
                }
            }
            UiCommand::Quit => break,
        }
    }

    if let Some(session) = current_session.take() {
        let _ = session.close().await;
    }
    tracing::info!("recorder loop exited");
}
