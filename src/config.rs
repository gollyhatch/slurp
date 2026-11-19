//! Configuration file handling.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

/// Built-in theme used when the configuration does not name one.
pub const DEFAULT_THEME: &str = "silkcircuit-neon";

/// Runtime configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Name of a built-in theme, or a path to a theme TOML file.
    pub theme: String,
    /// Maximum width of the text column in cells.
    pub text_width: u16,
    /// Minimum empty cells between the text column and the terminal edge.
    pub margin: u16,
    /// Empty rows between text blocks.
    pub paragraph_spacing: u16,
    /// Maximum height of an inline image in rows.
    pub image_max_height: u16,
    /// Draw inline images. When `false`, images show as a text placeholder.
    pub images: bool,
    /// Auto-scroll reading speed in words per minute.
    pub auto_scroll_wpm: u16,
    /// Per-role color overrides, for example `text = "#cdd6f4"`.
    pub colors: BTreeMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: DEFAULT_THEME.to_string(),
            text_width: 80,
            margin: 2,
            paragraph_spacing: 1,
            image_max_height: 20,
            images: true,
            auto_scroll_wpm: 200,
            colors: BTreeMap::new(),
        }
    }
}

impl Config {
    /// Load the configuration.
    ///
    /// A missing default file is not an error. A missing file that was named
    /// on the command line is an error.
    pub fn load(explicit: Option<&Path>) -> Result<Self> {
        let (path, required) = match explicit {
            Some(path) => (path.to_path_buf(), true),
            None => match default_path() {
                Some(path) => (path, false),
                None => return Ok(Self::default()),
            },
        };

        if !path.exists() {
            if required {
                bail!("config file not found: {}", path.display());
            }
            return Ok(Self::default());
        }

        let text = fs::read_to_string(&path).with_context(|| {
            format!("cannot read config file {}", path.display())
        })?;
        let config: Self = toml::from_str(&text).with_context(|| {
            format!("invalid config file {}", path.display())
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Check all values for sane bounds.
    pub fn validate(&self) -> Result<()> {
        if self.text_width < 20 || self.text_width > 400 {
            bail!("text_width must be between 20 and 400");
        }
        if self.margin > 20 {
            bail!("margin must be at most 20");
        }
        if self.paragraph_spacing > 4 {
            bail!("paragraph_spacing must be at most 4");
        }
        if self.image_max_height < 2 || self.image_max_height > 100 {
            bail!("image_max_height must be between 2 and 100");
        }
        if !(25..=1000).contains(&self.auto_scroll_wpm) {
            bail!("auto_scroll_wpm must be between 25 and 1000");
        }
        if self.theme.trim().is_empty() {
            bail!("theme must not be empty");
        }
        crate::theme::validate_overrides(&self.colors)?;
        Ok(())
    }

    /// Path of the configuration file, if a home directory is known.
    pub fn default_path() -> Option<PathBuf> {
        default_path()
    }
}

/// `$XDG_CONFIG_HOME/slurp/config.toml` or `~/.config/slurp/config.toml`.
pub fn default_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("slurp").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_config() {
        let text = r##"
            theme = "catppuccin-mocha"
            text_width = 100
            margin = 4
            paragraph_spacing = 0
            image_max_height = 12
            images = false
            auto_scroll_wpm = 300

            [colors]
            text = "#cdd6f4"
            accent = "#cba6f7"
        "##;
        let config: Config = toml::from_str(text).unwrap();
        config.validate().unwrap();
        assert_eq!(config.theme, "catppuccin-mocha");
        assert_eq!(config.text_width, 100);
        assert!(!config.images);
        assert_eq!(config.auto_scroll_wpm, 300);
        assert_eq!(config.colors.len(), 2);
    }

    #[test]
    fn rejects_unknown_fields() {
        let result: Result<Config, _> = toml::from_str("width = 80");
        assert!(result.is_err());
    }

    #[test]
    fn rejects_bad_bounds() {
        let config = Config {
            text_width: 5,
            ..Config::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_unknown_color_role() {
        let mut config = Config::default();
        config.colors.insert("nope".into(), "#ffffff".into());
        assert!(config.validate().is_err());
    }
}
