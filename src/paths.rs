//! Locations of the files the daemon owns, all under `$HOME/.cache/shpi`.

use anyhow::{Result, anyhow};
use std::path::PathBuf;

pub fn data_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
    Ok(PathBuf::from(home).join(".cache").join("shpi"))
}

pub fn pid_file() -> Result<PathBuf> {
    Ok(data_dir()?.join("shpi.pid"))
}

pub fn fifo_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("output.pipe"))
}

pub fn control_sock() -> Result<PathBuf> {
    Ok(data_dir()?.join("control.sock"))
}

pub fn default_log() -> Result<PathBuf> {
    Ok(data_dir()?.join("shpi.log"))
}

/// Create the data directory if absent and return it.
pub fn ensure_data_dir() -> Result<PathBuf> {
    let dir = data_dir()?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}
