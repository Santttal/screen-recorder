use std::process::Command;

use anyhow::{anyhow, Context, Result};

#[derive(Debug, Clone)]
pub struct AudioDevices {
    pub monitor_source: String,
    pub mic_source: String,
}

pub fn detect_audio_devices() -> Result<AudioDevices> {
    let default_sink = pactl(&["get-default-sink"])?;
    if default_sink.is_empty() {
        return Err(anyhow!("pactl returned empty default-sink"));
    }
    let monitor_source = format!("{default_sink}.monitor");

    let mic_source = pactl(&["get-default-source"])?;
    if mic_source.is_empty() {
        return Err(anyhow!("pactl returned empty default-source"));
    }

    // Проверить, что monitor реально присутствует в списке sources.
    let list = pactl(&["list", "short", "sources"])?;
    if !list.lines().any(|line| line.contains(&monitor_source)) {
        tracing::warn!(
            %monitor_source,
            "monitor source not found in `pactl list short sources`, continuing anyway"
        );
    }

    tracing::info!(%monitor_source, %mic_source, "audio devices detected");
    Ok(AudioDevices {
        monitor_source,
        mic_source,
    })
}

fn pactl(args: &[&str]) -> Result<String> {
    let out = Command::new("pactl")
        .args(args)
        .output()
        .with_context(|| format!("spawn pactl {args:?}"))?;
    if !out.status.success() {
        return Err(anyhow!(
            "pactl {args:?} exited with {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8(out.stdout)
        .context("pactl output is not utf-8")?
        .trim()
        .to_owned())
}
