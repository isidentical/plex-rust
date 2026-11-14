//! What the app remembers between launches.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// The viewer's standing choices. Each field falls back to its default when
/// the file is missing it, so a new setting never invalidates an old file.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Playback speed in tenths of normal: 15 is one and a half times.
    pub speed: u32,
    /// Volume out of 100.
    pub volume: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self { speed: 10, volume: 100.0 }
    }
}

impl Settings {
    /// Reads the settings, or the defaults if none were ever saved.
    pub fn load() -> Result<Self> {
        let path = path()?;
        match fs::read_to_string(&path) {
            Ok(json) => {
                serde_json::from_str(&json).map_err(|source| Error::Settings { path, source })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(Error::File { path, source }),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = path()?;
        let json = serde_json::to_string_pretty(self)
            .map_err(|source| Error::Settings { path: path.clone(), source })?;
        fs::write(&path, json).map_err(|source| Error::File { path, source })
    }
}

fn path() -> Result<PathBuf> {
    Ok(crate::account::support_directory()?.join("settings.json"))
}
