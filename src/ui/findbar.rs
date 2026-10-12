//! Find/search bar UI component (Milestone 6).

use egui::{Color32, RichText, Ui};

use crate::pdf::types::SearchMatch;

/// Actions emitted by the Find bar.
#[derive(Debug, Clone, PartialEq)]
pub enum FindBarAction {
    QueryChanged(String),
    NextMatch,
    PrevMatch,
    Close,
}

/// State of the Find/Search feature per tab.
#[derive(Debug, Clone, Default)]
pub struct FindState {
    pub is_open: bool,
    pub query: String,
    pub active_query_id: u64,
    pub matches: Vec<SearchMatch>,
    pub current_match_idx: Option<usize>,
    pub is_searching: bool,
    pub last_edit_time: Option<std::time::Instant>,
    pub focus_requested: bool,
}

impl FindState {
    /// Advances to the next match (wrapping around to the first).
    pub fn next_match(&mut self) -> Option<usize> {
        if self.matches.is_empty() {
            self.current_match_idx = None;
            return None;
        }
        let next_idx = match self.current_match_idx {
            Some(i) => (i + 1) % self.matches.len(),
            None => 0,
        };
        self.current_match_idx = Some(next_idx);
        Some(next_idx)
    }

    /// Moves to the previous match (wrapping around to the last).
    pub fn prev_match(&mut self) -> Option<usize> {
        if self.matches.is_empty() {
            self.current_match_idx = None;
            return None;
        }
        let prev_idx = match self.current_match_idx {
            Some(0) | None => self.matches.len() - 1,
            Some(i) => i - 1,
        };
        self.current_match_idx = Some(prev_idx);
        Some(prev_idx)
    }

    /// Resets all search data and query.
    pub fn clear_matches(&mut self) {
        self.matches.clear();
        self.current_match_idx = None;
        self.is_searching = false;
        self.last_edit_time = None;
    }
}

/// Renders the Find bar UI row below the toolbar.
pub fn render_find_bar(
    ui: &mut Ui,
    query: &mut String,
    match_count: usize,
    current_match_idx: Option<usize>,
    is_searching: bool,
    focus_requested: &mut bool,
) -> Option<FindBarAction> {
    let mut action = None;

    ui.horizontal(|ui| {
        ui.label(RichText::new("🔍").size(13.0));

        let text_edit = egui::TextEdit::singleline(query)
            .hint_text("Find in document...")
            .desired_width(180.0);
        let response = ui.add(text_edit);

        if *focus_requested {
            response.request_focus();
            *focus_requested = false;
        }

        if response.changed() {
            action = Some(FindBarAction::QueryChanged(query.clone()));
        }

        // Match count label
        let count_text = if query.trim().is_empty() {
            String::new()
        } else if match_count == 0 {
            if is_searching {
                "Searching...".to_string()
            } else {
                "No matches".to_string()
            }
        } else {
            let current_display = current_match_idx.map(|i| i + 1).unwrap_or(0);
            if is_searching {
                format!("{current_display} of {match_count} (searching...)")
            } else {
                format!("{current_display} of {match_count}")
            }
        };

        if !count_text.is_empty() {
            ui.label(
                RichText::new(count_text)
                    .size(12.0)
                    .color(Color32::from_rgb(180, 180, 185)),
            );
        }

        // Prev and Next navigation buttons
        let has_matches = match_count > 0;

        let prev_btn = ui
            .add_enabled(
                has_matches,
                egui::Button::new(RichText::new("▲").size(11.0)),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Previous match (Shift+Enter)");
        if prev_btn.clicked() {
            action = Some(FindBarAction::PrevMatch);
        }

        let next_btn = ui
            .add_enabled(
                has_matches,
                egui::Button::new(RichText::new("▼").size(11.0)),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Next match (Enter)");
        if next_btn.clicked() {
            action = Some(FindBarAction::NextMatch);
        }

        // Keyboard navigation while search input is focused
        if response.has_focus() {
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                if ui.input(|i| i.modifiers.shift) {
                    action = Some(FindBarAction::PrevMatch);
                } else {
                    action = Some(FindBarAction::NextMatch);
                }
            } else if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                action = Some(FindBarAction::NextMatch);
            } else if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                action = Some(FindBarAction::PrevMatch);
            }
        }
        if ui.input(|i| i.key_pressed(egui::Key::F3)) {
            if ui.input(|i| i.modifiers.shift) {
                action = Some(FindBarAction::PrevMatch);
            } else {
                action = Some(FindBarAction::NextMatch);
            }
        }

        // Close button on the right
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let close_btn = ui
                .add(
                    egui::Button::new(RichText::new("×").size(14.0))
                        .frame(false)
                        .min_size(egui::vec2(18.0, 18.0)),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Close find bar (Esc)");
            if close_btn.clicked() {
                action = Some(FindBarAction::Close);
            }
        });
    });

    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::types::CanonicalRect;

    #[test]
    fn test_find_state_navigation() {
        let mut state = FindState::default();
        assert_eq!(state.next_match(), None);
        assert_eq!(state.prev_match(), None);

        state.matches = vec![
            SearchMatch {
                page: 1,
                rects: vec![CanonicalRect::new(10.0, 10.0, 50.0, 20.0)],
            },
            SearchMatch {
                page: 2,
                rects: vec![CanonicalRect::new(20.0, 30.0, 60.0, 40.0)],
            },
            SearchMatch {
                page: 3,
                rects: vec![CanonicalRect::new(30.0, 50.0, 70.0, 60.0)],
            },
        ];

        // First next goes to index 0
        assert_eq!(state.next_match(), Some(0));
        assert_eq!(state.current_match_idx, Some(0));

        // Next goes to 1, then 2
        assert_eq!(state.next_match(), Some(1));
        assert_eq!(state.next_match(), Some(2));

        // Wraps around to 0
        assert_eq!(state.next_match(), Some(0));

        // Prev from 0 wraps around to 2
        assert_eq!(state.prev_match(), Some(2));
        assert_eq!(state.prev_match(), Some(1));
        assert_eq!(state.prev_match(), Some(0));
    }

    #[test]
    fn test_find_state_clear() {
        let mut state = FindState {
            query: "test".to_string(),
            matches: vec![SearchMatch {
                page: 1,
                rects: vec![],
            }],
            current_match_idx: Some(0),
            is_searching: true,
            ..Default::default()
        };

        state.clear_matches();
        assert!(state.matches.is_empty());
        assert_eq!(state.current_match_idx, None);
        assert!(!state.is_searching);
    }
}
