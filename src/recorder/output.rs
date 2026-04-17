use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};

pub fn default_output_path() -> Result<PathBuf> {
    let dirs =
        directories::UserDirs::new().ok_or_else(|| anyhow!("cannot resolve user directories"))?;
    let videos = dirs
        .video_dir()
        .map(|p| p.to_path_buf())
        .or_else(|| dirs.home_dir().to_path_buf().into())
        .ok_or_else(|| anyhow!("no $XDG_VIDEOS_DIR and no $HOME"))?;
    let base = videos.join("Recordings");
    std::fs::create_dir_all(&base).with_context(|| format!("create {}", base.display()))?;

    let ts = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
    Ok(base.join(format!("{ts}.mkv")))
}
