//! Settings struct and settings.toml persistence (Milestone 8 and 10).

use std::fs;
use std::path::PathBuf;

use directories::BaseDirs;
use serde::{Deserialize, Serialize};

use crate::annotations::{AnnotationColor, PRESET_HIGHLIGHT_COLORS};

fn default_highlighter_color() -> AnnotationColor {
    PRESET_HIGHLIGHT_COLORS[0] // Classic Yellow
}

fn default_highlighter_thickness() -> f32 {
    1.0
}

fn default_touchpad_scroll_speed() -> f32 {
    3.5
}

fn default_mouse_wheel_speed() -> f32 {
    1.0
}

fn default_text_font_size() -> f32 {
    16.0
}

fn default_text_color() -> AnnotationColor {
    AnnotationColor::new(0, 0, 0)
}

/// Persistent user settings saved in ~/.config/pdfview/settings.toml.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    #[serde(default = "default_highlighter_color")]
    pub highlighter_color: AnnotationColor,
    #[serde(default = "default_highlighter_thickness")]
    pub highlighter_thickness: f32,
    #[serde(default = "default_text_font_size")]
    pub text_font_size: f32,
    #[serde(default = "default_text_color")]
    pub text_color: AnnotationColor,
    #[serde(default)]
    pub sidebar_open: bool,
    #[serde(default)]
    pub pathbar_collapsed: bool,
    #[serde(default = "default_touchpad_scroll_speed")]
    pub touchpad_scroll_speed: f32,
    #[serde(default = "default_mouse_wheel_speed")]
    pub mouse_wheel_speed: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            highlighter_color: default_highlighter_color(),
            highlighter_thickness: default_highlighter_thickness(),
            text_font_size: default_text_font_size(),
            text_color: default_text_color(),
            sidebar_open: false,
            pathbar_collapsed: false,
            touchpad_scroll_speed: default_touchpad_scroll_speed(),
            mouse_wheel_speed: default_mouse_wheel_speed(),
        }
    }
}

impl Settings {
    /// Returns the canonical path `~/.config/verso-reader/settings.toml` (falling back to legacy path if present).
    pub fn config_path() -> Option<PathBuf> {
        BaseDirs::new().map(|base| {
            let primary = base.config_dir().join("verso-reader").join("settings.toml");
            let legacy = base.config_dir().join("pdfview").join("settings.toml");
            if primary.exists() || !legacy.exists() {
                primary
            } else {
                legacy
            }
        })
    }

    /// Loads settings from `~/.config/verso-reader/settings.toml`, returning defaults on error or missing file.
    pub fn load() -> Self {
        if let Some(path) = Self::config_path() {
            if path.exists() {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(settings) = toml::from_str::<Settings>(&content) {
                        return settings;
                    }
                }
            }
        }
        Self::default()
    }

    /// Saves settings to `~/.config/verso-reader/settings.toml`.
    pub fn save(&self) -> Result<(), std::io::Error> {
        if let Some(path) = Self::config_path() {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let content = toml::to_string_pretty(self).map_err(std::io::Error::other)?;
            fs::write(path, content)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_settings_defaults() {
        let s = Settings::default();
        assert_eq!(s.highlighter_color, PRESET_HIGHLIGHT_COLORS[0]);
        assert_eq!(s.highlighter_thickness, 1.0);
        assert_eq!(s.text_font_size, 16.0);
        assert_eq!(s.text_color, AnnotationColor::new(0, 0, 0));
        assert!(!s.sidebar_open);
        assert!(!s.pathbar_collapsed);
        assert_eq!(s.touchpad_scroll_speed, 3.5);
        assert_eq!(s.mouse_wheel_speed, 1.0);
    }

    #[test]
    fn test_settings_toml_roundtrip() {
        let s = Settings {
            highlighter_color: PRESET_HIGHLIGHT_COLORS[3],
            highlighter_thickness: 0.65,
            text_font_size: 20.0,
            text_color: AnnotationColor::new(192, 57, 43),
            sidebar_open: true,
            pathbar_collapsed: true,
            touchpad_scroll_speed: 3.0,
            mouse_wheel_speed: 1.5,
        };
        let serialized = toml::to_string_pretty(&s).expect("serialize settings");
        let deserialized: Settings = toml::from_str(&serialized).expect("deserialize settings");
        assert_eq!(s, deserialized);
    }

    #[test]
    fn test_settings_deserializes_missing_fields_with_defaults() {
        let empty_toml = "";
        let s: Settings = toml::from_str(empty_toml).expect("deserialize empty toml");
        assert_eq!(s, Settings::default());
    }
}
