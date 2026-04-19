//! Recorder-модуль: GStreamer pipeline, state machine, захват аудио/видео.

pub mod audio;
pub mod encoders;
pub mod output;
pub mod pipeline;

use async_channel::{Receiver, Sender};

use crate::config::SharedSettings;
use crate::portal::screencast::{open_or_cancel, OpenOutcome, ScreenCastSession};
use crate::portal::state::PortalState;
use crate::ui::events::{RecorderEvent, UiCommand};

pub use pipeline::{attach_bus_watch, build_pipeline, start, stop_graceful, RecordRequest};

pub async fn run(
    cmd_rx: Receiver<UiCommand>,
    evt_tx: Sender<RecorderEvent>,
    settings: SharedSettings,
) {
    let mut current_session: Option<ScreenCastSession> = None;

    while let Ok(cmd) = cmd_rx.recv().await {
        match cmd {
            UiCommand::StartRequested { sources, parent } => {
                tracing::info!(?sources, "start requested");

                let snapshot = { settings.read().unwrap().clone() };

                let saved = PortalState::load();
                let token = saved.screencast_restore_token.clone();
                if token.is_some() {
                    tracing::debug!("restore_token loaded from state.json");
                }

                let _ = evt_tx.send(RecorderEvent::PortalOpened).await;

                match open_or_cancel(parent, token, snapshot.cursor_mode, snapshot.capture_source).await {
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

                        let final_path = match output::build_output_path(
                            &snapshot.output_dir,
                            snapshot.container,
                        ) {
                            Ok(p) => p,
                            Err(e) => {
                                tracing::error!(%e, "cannot resolve output path");
                                let _ = evt_tx.send(RecorderEvent::Error(e.to_string())).await;
                                let _ = session.close().await;
                                continue;
                            }
                        };
                        // Пишем всегда в MKV (crash-safe); remux в целевой контейнер —
                        // в UI на стороне RecordingStopped.
                        let output_path =
                            output::intermediate_mkv_path(&final_path, snapshot.container);

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
            UiCommand::TranscribeRequested { video_path } => {
                let snapshot = { settings.read().unwrap().clone() };
                let _ = evt_tx
                    .send(RecorderEvent::TranscriptionStarted {
                        video_path: video_path.clone(),
                    })
                    .await;
                // Мостим progress-канал транскрипции → UI-события.
                let (prog_tx, prog_rx) = async_channel::unbounded::<(u32, u32)>();
                let evt_tx_progress = evt_tx.clone();
                let video_for_progress = video_path.clone();
                let progress_forwarder = tokio::spawn(async move {
                    while let Ok((part, total)) = prog_rx.recv().await {
                        let _ = evt_tx_progress
                            .send(RecorderEvent::TranscriptionProgress {
                                video_path: video_for_progress.clone(),
                                part,
                                total,
                            })
                            .await;
                    }
                });
                let result =
                    crate::transcription::transcribe_file(&video_path, &snapshot, Some(&prog_tx))
                        .await;
                drop(prog_tx);
                let _ = progress_forwarder.await;
                match result {
                    Ok(result) => {
                        if result.text.trim().is_empty() {
                            tracing::warn!(
                                video = %video_path.display(),
                                "transcription came back empty — likely silent audio; .txt not written"
                            );
                            let _ = evt_tx
                                .send(RecorderEvent::TranscriptionFailed {
                                    video_path,
                                    message:
                                        "Речь не распознана — возможно, запись беззвучная \
                                         (монитор динамика, если ничего не играло). \
                                         Попробуйте включить микрофон."
                                            .to_string(),
                                })
                                .await;
                            continue;
                        }
                        let text_path = crate::transcription::text_output_path(&video_path);
                        match std::fs::write(&text_path, &result.text) {
                            Ok(()) => {
                                tracing::info!(
                                    video = %video_path.display(),
                                    text = %text_path.display(),
                                    model = ?result.model,
                                    chunks = result.chunks,
                                    "transcription saved"
                                );
                                let _ = evt_tx
                                    .send(RecorderEvent::TranscriptionFinished {
                                        video_path,
                                        text_path,
                                        model: result.model,
                                        chunks: result.chunks,
                                    })
                                    .await;
                            }
                            Err(e) => {
                                tracing::warn!(%e, "failed to write transcription .txt");
                                let _ = evt_tx
                                    .send(RecorderEvent::TranscriptionFailed {
                                        video_path,
                                        message: format!("write .txt: {e}"),
                                    })
                                    .await;
                            }
                        }
                    }
                    Err(e) => {
                        let message = crate::transcription::friendly_message(&e);
                        tracing::warn!(err = %e, "transcription failed");
                        let _ = evt_tx
                            .send(RecorderEvent::TranscriptionFailed { video_path, message })
                            .await;
                    }
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
