use anyhow::{Context, Result};
use serde::{de::DeserializeOwned, Serialize};
use std::path::PathBuf;

use crate::model::{Fix, Record, Settings, Tracking};

/// JSON files in the app's data directory.
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn read<T: DeserializeOwned + Default>(&self, name: &str) -> T {
        let path = self.dir.join(name);
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                // Keep the unreadable file for inspection rather than overwriting it silently.
                let _ = std::fs::rename(&path, path.with_extension("corrupt"));
                eprintln!("warden: {} was unreadable ({e}); starting fresh", path.display());
                T::default()
            }),
            Err(_) => T::default(),
        }
    }

    fn write<T: Serialize>(&self, name: &str, value: &T) -> Result<()> {
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("Could not create {}", self.dir.display()))?;
        let path = self.dir.join(name);
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)
            .with_context(|| format!("Could not write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("Could not write {}", path.display()))
    }

    pub fn settings(&self) -> Settings {
        self.read("settings.json")
    }

    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        self.write("settings.json", settings)
    }

    pub fn history(&self) -> Vec<Record> {
        self.read("history.json")
    }

    pub fn save_history(&self, history: &[Record]) -> Result<()> {
        self.write("history.json", &history)
    }

    pub fn fixes(&self) -> Vec<Fix> {
        self.read("fixes.json")
    }

    pub fn save_fixes(&self, fixes: &[Fix]) -> Result<()> {
        self.write("fixes.json", &fixes)
    }

    pub fn tracking(&self) -> Tracking {
        self.read("tracking.json")
    }

    pub fn save_tracking(&self, tracking: &Tracking) -> Result<()> {
        self.write("tracking.json", tracking)
    }
}
