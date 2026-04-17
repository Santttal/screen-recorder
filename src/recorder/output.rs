use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::Container;

pub fn build_output_path(dir: &Path, container: Container) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let ts = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
    Ok(dir.join(format!("{ts}.{}", container.ext())))
}
