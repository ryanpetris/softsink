use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::{env, fs};

use serde::{Deserialize, Serialize};

use crate::capture::Mode;
use crate::orientation::Orientation;
use crate::scale::Scale;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    #[default]
    Framed,
    Immersive,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub source: Option<String>,
    pub modes: BTreeMap<String, Mode>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub orientations: BTreeMap<String, Orientation>,
    pub scale: Scale,
    pub layout: Layout,
    pub window_size: Option<[u32; 2]>,
    pub stats: bool,
    pub muted: bool,
    pub volume: f32,
    pub breakdown: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            source: None,
            modes: BTreeMap::new(),
            orientations: BTreeMap::new(),
            scale: Scale::default(),
            layout: Layout::default(),
            window_size: None,
            stats: false,
            muted: false,
            volume: 1.0,
            breakdown: false,
        }
    }
}

pub fn path() -> Option<PathBuf> {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .map(|base| base.join("softsink/config.toml"))
}

impl Settings {
    pub fn load(path: Option<&Path>) -> Self {
        path.and_then(|path| fs::read_to_string(path).ok())
            .and_then(|data| toml::from_str(&data).ok())
            .unwrap_or_default()
    }
}

pub fn save_changes(path: Option<PathBuf>, changes: Receiver<Settings>) {
    let Some(path) = path else { return };
    let parent = path.parent().unwrap();
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    while let Ok(mut settings) = changes.recv() {
        while let Ok(newer) = changes.try_recv() {
            settings = newer;
        }
        let result = (|| {
            let data = toml::to_string_pretty(&settings)?;
            fs::create_dir_all(parent)?;
            fs::write(&temp, data)?;
            fs::rename(&temp, &path)?;
            Ok::<_, Box<dyn std::error::Error>>(())
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&temp);
            eprintln!("Could not save settings: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orientation::Rotation;

    #[test]
    fn orientations_round_trip_without_a_default_device_entry() {
        let mut settings: Settings = toml::from_str("source = 'Capture A'").unwrap();
        assert!(settings.orientations.is_empty());
        assert!(!toml::to_string(&settings).unwrap().contains("orientations"));

        settings.orientations.insert(
            "Capture B".into(),
            Orientation {
                rotation: Rotation::Deg270,
                flip_horizontal: true,
                flip_vertical: false,
            },
        );
        let saved = toml::to_string(&settings).unwrap();
        assert_eq!(toml::from_str::<Settings>(&saved).unwrap(), settings);
    }
}
