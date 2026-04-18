use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};

use crate::config::Container;

/// Записываем всегда в MKV (crash-safe). После EOS — опциональный ремукс.
/// Имя файла формируется с финальным расширением (mkv/mp4/webm) — промежуточный
/// файл получает расширение `.mkv`, а финальное имя вычисляется из `container.ext()`.
pub fn build_output_path(dir: &Path, container: Container) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let ts = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
    Ok(dir.join(format!("{ts}.{}", container.ext())))
}

/// Путь для промежуточного MKV-файла, в который пишет GStreamer.
/// Если финальный контейнер тоже MKV — возвращается тот же путь (remux не нужен).
pub fn intermediate_mkv_path(final_path: &Path, container: Container) -> PathBuf {
    if matches!(container, Container::Mkv) {
        return final_path.to_path_buf();
    }
    final_path.with_extension("mkv")
}

/// Переупаковывает MKV в выбранный контейнер без перекодирования.
/// Возвращает путь к финальному файлу. На успех удаляет промежуточный MKV.
pub fn remux_to(input_mkv: &Path, final_container: Container) -> Result<PathBuf> {
    if matches!(final_container, Container::Mkv) {
        return Ok(input_mkv.to_path_buf());
    }
    let final_path = input_mkv.with_extension(final_container.ext());

    let mut args: Vec<&str> = vec!["-hide_banner", "-y", "-i"];
    let input_str = input_mkv.to_string_lossy().into_owned();
    let final_str = final_path.to_string_lossy().into_owned();
    args.push(&input_str);
    args.extend_from_slice(&["-c", "copy"]);
    if matches!(final_container, Container::Mp4) {
        args.extend_from_slice(&["-movflags", "+faststart"]);
    }
    args.push(&final_str);

    tracing::info!(
        input = %input_mkv.display(),
        output = %final_path.display(),
        container = ?final_container,
        "remuxing via ffmpeg"
    );

    let status = Command::new("ffmpeg")
        .args(&args)
        .output()
        .context("failed to spawn ffmpeg (not installed?)")?;

    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        return Err(anyhow!(
            "ffmpeg remux failed ({}): {}",
            status.status,
            stderr.lines().last().unwrap_or("").trim()
        ));
    }

    // Успех — удаляем промежуточный MKV.
    if let Err(e) = std::fs::remove_file(input_mkv) {
        tracing::warn!(%e, "failed to delete intermediate mkv");
    }

    Ok(final_path)
}
