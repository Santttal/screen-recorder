//! Модель очереди транскрипции (phase 19.c.1).
//! Используется AI-страницей для отображения текущих и ожидающих задач.
//! Источник данных — `RecorderEvent::Transcription{Started,Progress,Finished,Failed}`,
//! которые уже генерируются recorder-loop. Мы просто собираем их в in-memory-список
//! и позволяем UI перестроить рендер по уведомлению.

#![allow(dead_code)]

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum QueueStatus {
    Queued,
    Processing,
    Done,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct QueueItem {
    pub video_path: PathBuf,
    pub status: QueueStatus,
    /// `0.0..=1.0`. Для chunk-based задач — доля обработанных чанков.
    pub progress: f64,
}

impl QueueItem {
    pub fn title(&self) -> String {
        self.video_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.video_path.display().to_string())
    }

    pub fn status_label(&self) -> String {
        match &self.status {
            QueueStatus::Queued => "Ожидает".into(),
            QueueStatus::Processing => {
                let pct = (self.progress * 100.0).round() as i64;
                format!("Обрабатывается · {pct}%")
            }
            QueueStatus::Done => "Готово".into(),
            QueueStatus::Failed(msg) => format!("Ошибка: {msg}"),
        }
    }
}
