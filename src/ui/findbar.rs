//! Find/search bar UI component (Milestone 6).

use egui::{vec2, Color32, RichText, Sense, Ui};

use crate::pdf::types::SearchMatch;
use crate::ui::icons::{
    paint_chevron_down, paint_chevron_up, paint_close, paint_search, render_icon_button,
};
use crate::ui::theme::Theme;

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
        // Vector search icon
        let (icon_rect, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
        paint_search(ui.painter(), icon_rect, Theme::TEXT_SECONDARY);

        let text_edit = egui::TextEdit::singleline(query)
            .hint_text("Find in document...")
            .desired_width(140.0)
            .margin(egui::Margin::symmetric(8, 6))
            .min_size(vec2(140.0, 28.0));
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
            ui.add_space(2.0);
            ui.label(
                RichText::new(count_text)
                    .size(12.0)
                    .color(Color32::from_rgb(180, 180, 185)),
            );
            ui.add_space(2.0);
        }

        // Prev and Next navigation buttons with vector chevrons
        let has_matches = match_count > 0;

        let prev_btn = render_icon_button(
            ui,
            vec2(26.0, 26.0),
            false,
            has_matches,
            "Previous match (Shift+Enter)",
            paint_chevron_up,
        );
        if prev_btn.clicked() {
            action = Some(FindBarAction::PrevMatch);
        }

        let next_btn = render_icon_button(
            ui,
            vec2(26.0, 26.0),
            false,
            has_matches,
            "Next match (Enter)",
            paint_chevron_down,
        );
        if next_btn.clicked() {
            action = Some(FindBarAction::NextMatch);
        }

        // Keyboard navigation: Enter moves to next match (Shift+Enter to previous), and retains focus
        let enter_pressed = (response.has_focus()
            || (response.lost_focus() && !ui.input(|i| i.key_pressed(egui::Key::Escape))))
            && ui.input(|i| i.key_pressed(egui::Key::Enter));

        if enter_pressed {
            response.request_focus();
            if ui.input(|i| i.modifiers.shift) {
                action = Some(FindBarAction::PrevMatch);
            } else {
                action = Some(FindBarAction::NextMatch);
            }
        } else if response.has_focus() {
            if ui.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                action = Some(FindBarAction::NextMatch);
            } else if ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                action = Some(FindBarAction::PrevMatch);
            } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                action = Some(FindBarAction::Close);
            }
        }

        if ui.input(|i| i.key_pressed(egui::Key::F3)) {
            if ui.input(|i| i.modifiers.shift) {
                action = Some(FindBarAction::PrevMatch);
            } else {
                action = Some(FindBarAction::NextMatch);
            }
        }

        ui.add_space(4.0);
        ui.separator();
        ui.add_space(2.0);

        // Close button with vector cross
        let close_btn = render_icon_button(
            ui,
            vec2(24.0, 24.0),
            false,
            true,
            "Close find bar (Esc)",
            paint_close,
        );
        if close_btn.clicked() {
            action = Some(FindBarAction::Close);
        }
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

    #[test]
    fn test_find_bar_action_variants() {
        assert_eq!(FindBarAction::NextMatch, FindBarAction::NextMatch);
        assert_eq!(FindBarAction::PrevMatch, FindBarAction::PrevMatch);
        assert_eq!(FindBarAction::Close, FindBarAction::Close);
        assert_eq!(
            FindBarAction::QueryChanged("search".to_string()),
            FindBarAction::QueryChanged("search".to_string())
        );
        assert_ne!(FindBarAction::NextMatch, FindBarAction::PrevMatch);
    }
}
