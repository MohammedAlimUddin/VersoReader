//! Settings struct and settings.toml persistence (Milestone 8 and 10).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

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
    4.5
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

fn default_cache_size_mb() -> usize {
    150
}

fn default_restore_session() -> bool {
    true
}

fn default_remember_reading_position() -> bool {
    true
}

fn default_inactivity_timeout_mins() -> u32 {
    5
}

/// Persistent user settings saved in ~/.config/verso-reader/settings.toml.
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
    #[serde(default = "default_cache_size_mb")]
    pub cache_size_mb: usize,
    #[serde(default = "default_restore_session")]
    pub restore_session: bool,
    #[serde(default = "default_remember_reading_position")]
    pub remember_reading_position: bool,
    #[serde(default = "default_inactivity_timeout_mins")]
    pub inactivity_timeout_mins: u32,
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
            cache_size_mb: default_cache_size_mb(),
            restore_session: default_restore_session(),
            remember_reading_position: default_remember_reading_position(),
            inactivity_timeout_mins: default_inactivity_timeout_mins(),
        }
    }
}

/// A single saved tab state in a session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionTab {
    pub path: PathBuf,
    pub scroll_offset_y: f32,
    pub current_page: usize,
    pub zoom: f32,
    pub rotation: u16,
}

/// The overall session state containing open tabs.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SessionState {
    pub tabs: Vec<SessionTab>,
    pub active_tab_index: usize,
}

/// Reading position for a specific document.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DocumentPosition {
    pub scroll_offset_y: f32,
    pub current_page: usize,
    pub zoom: f32,
    pub rotation: u16,
    pub updated_at: u64,
}

/// Reading history map across documents.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ReadingHistory {
    pub entries: HashMap<String, DocumentPosition>,
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

    /// Returns the path to `~/.config/verso-reader/session.json`.
    pub fn session_path() -> Option<PathBuf> {
        BaseDirs::new().map(|base| base.config_dir().join("verso-reader").join("session.json"))
    }

    /// Returns the path to `~/.config/verso-reader/history.json`.
    pub fn history_path() -> Option<PathBuf> {
        BaseDirs::new().map(|base| base.config_dir().join("verso-reader").join("history.json"))
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

    /// Loads session state from `~/.config/verso-reader/session.json`.
    pub fn load_session() -> Option<SessionState> {
        let path = Self::session_path()?;
        if path.exists() {
            let content = fs::read_to_string(&path).ok()?;
            serde_json::from_str(&content).ok()
        } else {
            None
        }
    }

    /// Saves session state to `~/.config/verso-reader/session.json`.
    pub fn save_session(session: &SessionState) {
        if let Some(path) = Self::session_path() {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Ok(content) = serde_json::to_string_pretty(session) {
                let _ = fs::write(path, content);
            }
        }
    }

    /// Loads document reading history from `~/.config/verso-reader/history.json`.
    pub fn load_history() -> ReadingHistory {
        if let Some(path) = Self::history_path() {
            if path.exists() {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(history) = serde_json::from_str(&content) {
                        return history;
                    }
                }
            }
        }
        ReadingHistory::default()
    }

    /// Saves document reading history to `~/.config/verso-reader/history.json`.
    pub fn save_history(history: &ReadingHistory) {
        if let Some(path) = Self::history_path() {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Ok(content) = serde_json::to_string_pretty(history) {
                let _ = fs::write(path, content);
            }
        }
    }

    /// Computes canonical storage key for a file path.
    pub fn canonical_path_key(path: &Path) -> String {
        fs::canonicalize(path)
            .unwrap_or_else(|_| path.to_path_buf())
            .to_string_lossy()
            .to_string()
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
        assert_eq!(s.touchpad_scroll_speed, 4.5);
        assert_eq!(s.mouse_wheel_speed, 1.0);
        assert_eq!(s.cache_size_mb, 150);
        assert!(s.restore_session);
        assert!(s.remember_reading_position);
        assert_eq!(s.inactivity_timeout_mins, 5);
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
            cache_size_mb: 200,
            restore_session: false,
            remember_reading_position: true,
            inactivity_timeout_mins: 15,
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

    #[test]
    fn test_session_state_json_roundtrip() {
        let session = SessionState {
            tabs: vec![
                SessionTab {
                    path: PathBuf::from("/home/user/book1.pdf"),
                    scroll_offset_y: 1250.0,
                    current_page: 5,
                    zoom: 1.25,
                    rotation: 90,
                },
                SessionTab {
                    path: PathBuf::from("/home/user/paper.pdf"),
                    scroll_offset_y: 0.0,
                    current_page: 1,
                    zoom: 1.0,
                    rotation: 0,
                },
            ],
            active_tab_index: 0,
        };
        let json = serde_json::to_string_pretty(&session).expect("serialize session");
        let restored: SessionState = serde_json::from_str(&json).expect("deserialize session");
        assert_eq!(session, restored);
    }

    #[test]
    fn test_reading_history_json_roundtrip() {
        let mut history = ReadingHistory::default();
        history.entries.insert(
            "/home/user/book1.pdf".to_string(),
            DocumentPosition {
                scroll_offset_y: 4500.0,
                current_page: 42,
                zoom: 1.5,
                rotation: 0,
                updated_at: 1700000000,
            },
        );
        let json = serde_json::to_string(&history).expect("serialize history");
        let restored: ReadingHistory = serde_json::from_str(&json).expect("deserialize history");
        assert_eq!(history, restored);
    }
}
