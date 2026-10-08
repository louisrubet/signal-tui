//! User settings, saved as `key = value` lines in `~/.config/signal-tui/settings`.

use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Desktop notification for messages received in a discussion that is not open.
    pub notifications: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { notifications: true }
    }
}

impl Settings {
    /// `$XDG_CONFIG_HOME/signal-tui/settings`, or `~/.config/signal-tui/settings`.
    pub fn default_path() -> io::Result<PathBuf> {
        let config_dir = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .ok_or_else(|| io::Error::other("neither XDG_CONFIG_HOME nor HOME is set"))?;
        Ok(config_dir.join("signal-tui").join("settings"))
    }

    /// Loads the settings; a missing file or unknown lines leave the defaults.
    pub fn load(path: &Path) -> Self {
        let mut settings = Settings::default();
        for line in std::fs::read_to_string(path).unwrap_or_default().lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            if let ("notifications", Ok(value)) = (key.trim(), value.trim().parse()) {
                settings.notifications = value;
            }
        }
        settings
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, format!("notifications = {}\n", self.notifications))
    }
}

#[cfg(test)]
mod tests {
    use super::Settings;

    #[test]
    fn save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("signal-tui").join("settings");
        assert_eq!(Settings::load(&path), Settings::default());

        let settings = Settings { notifications: false };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path), settings);
    }
}
