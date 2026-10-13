use std::path::Path;

/// Actions emitted by user interactions on the tab bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabBarAction {
    Select(usize),
    Close(usize),
    NewTab,
}

/// Helper function to truncate long file names with an ellipsis while preserving the extension.
pub fn truncate_filename(name: &str, max_len: usize) -> String {
    if name.chars().count() <= max_len {
        return name.to_string();
    }

    let path = Path::new(name);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();

    let ext_len = ext.chars().count();
    let ellipsis = "…";

    if max_len <= ext_len + 3 {
        // Very tight constraint: simple truncate
        return format!(
            "{}…",
            name.chars()
                .take(max_len.saturating_sub(1))
                .collect::<String>()
        );
    }

    let stem_allowed = max_len - ext_len - 1; // 1 for ellipsis
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(name);

    let prefix: String = stem.chars().take(stem_allowed).collect();
    format!("{prefix}{ellipsis}{ext}")
}

/// Information describing a tab to display in the tab bar.
pub struct TabItem<'a> {
    pub title: &'a str,
    pub tooltip: &'a str,
    pub is_active: bool,
    pub is_loading: bool,
    pub is_dirty: bool,
}

/// Renders the tab bar at the top of the application window, acting as the window title bar.
pub fn render_tab_bar(
    ui: &mut egui::Ui,
    tabs: &[TabItem<'_>],
    path_bar_collapsed: bool,
) -> (Option<TabBarAction>, bool) {
    let mut action = None;
    let mut toggle_path_bar = false;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(2.0, 0.0);

        // 1. Render tabs
        for (idx, tab) in tabs.iter().enumerate() {
            let bg_color = if tab.is_active {
                egui::Color32::from_rgb(45, 45, 48)
            } else {
                egui::Color32::from_rgb(28, 28, 30)
            };

            let text_color = if tab.is_active {
                egui::Color32::WHITE
            } else {
                egui::Color32::from_rgb(170, 170, 175)
            };

            let truncated_title = truncate_filename(tab.title, 24);
            let display_title = if tab.is_dirty {
                format!("* {truncated_title}")
            } else {
                truncated_title
            };

            let frame = egui::Frame::new()
                .fill(bg_color)
                .corner_radius(egui::CornerRadius {
                    nw: 4,
                    ne: 4,
                    sw: 0,
                    se: 0,
                })
                .inner_margin(egui::Margin::symmetric(8, 5));

            let mut close_clicked = false;
            let mut close_btn_rect = egui::Rect::NOTHING;

            let tab_response = frame.show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);

                    if tab.is_loading {
                        ui.spinner();
                    } else {
                        ui.label(egui::RichText::new("📄").size(11.0));
                    }

                    let label_text = egui::RichText::new(&display_title)
                        .color(text_color)
                        .size(12.0);
                    ui.label(label_text);

                    // Close button "×"
                    let close_btn = ui
                        .add(
                            egui::Button::new(egui::RichText::new("×").size(13.0).color(
                                if tab.is_active {
                                    egui::Color32::LIGHT_GRAY
                                } else {
                                    egui::Color32::GRAY
                                },
                            ))
                            .frame(false)
                            .min_size(egui::vec2(16.0, 16.0)),
                        )
                        .on_hover_cursor(egui::CursorIcon::PointingHand);

                    close_btn_rect = close_btn.rect;
                    if close_btn.clicked() {
                        close_clicked = true;
                    }
                });
            });

            let tab_rect = tab_response.response.rect;
            let pointer_pos = ui.input(|i| i.pointer.hover_pos());
            let is_hovered = pointer_pos.is_some_and(|p| tab_rect.contains(p));

            if is_hovered {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                tab_response.response.on_hover_text(tab.tooltip);

                if ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Middle)) {
                    action = Some(TabBarAction::Close(idx));
                } else if ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Primary)) {
                    let is_over_close = pointer_pos.is_some_and(|p| close_btn_rect.contains(p));
                    if close_clicked || is_over_close {
                        action = Some(TabBarAction::Close(idx));
                    } else if !tab.is_active {
                        action = Some(TabBarAction::Select(idx));
                    }
                }
            } else if close_clicked {
                action = Some(TabBarAction::Close(idx));
            }
        }

        // 2. "+" button to open a new tab immediately
        ui.add_space(2.0);
        let new_tab_btn = ui
            .add(
                egui::Button::new(egui::RichText::new("+").size(15.0).strong())
                    .frame(false)
                    .min_size(egui::vec2(24.0, 24.0)),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if new_tab_btn.clicked() {
            action = Some(TabBarAction::NewTab);
        }
        new_tab_btn.on_hover_text("New tab");

        // 3. Middle draggable window area
        let right_controls_width = 140.0;
        let drag_width = (ui.available_width() - right_controls_width).max(10.0);
        let (_, drag_resp) =
            ui.allocate_exact_size(egui::vec2(drag_width, 24.0), egui::Sense::click_and_drag());
        if drag_resp.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if drag_resp.double_clicked() {
            let is_max = ui.input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Maximized(!is_max));
        }

        // 4. Right side controls: path bar chevron + window control buttons
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Close window button "✕"
            let close_win_btn = ui
                .add(
                    egui::Button::new(egui::RichText::new("✕").size(12.0))
                        .frame(false)
                        .min_size(egui::vec2(28.0, 22.0)),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if close_win_btn.clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
            close_win_btn.on_hover_text("Close window");

            // Maximize / Restore button
            let is_max = ui.input(|i| i.viewport().maximized.unwrap_or(false));
            let max_icon = if is_max { "❐" } else { "🗖" };
            let max_win_btn = ui
                .add(
                    egui::Button::new(egui::RichText::new(max_icon).size(12.0))
                        .frame(false)
                        .min_size(egui::vec2(28.0, 22.0)),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if max_win_btn.clicked() {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Maximized(!is_max));
            }
            max_win_btn.on_hover_text(if is_max { "Restore" } else { "Maximize" });

            // Minimize button "—"
            let min_win_btn = ui
                .add(
                    egui::Button::new(egui::RichText::new("—").size(12.0))
                        .frame(false)
                        .min_size(egui::vec2(28.0, 22.0)),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if min_win_btn.clicked() {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
            min_win_btn.on_hover_text("Minimize");

            // Chevron button for path bar
            let chevron_icon = if path_bar_collapsed { "⏷" } else { "⏶" };
            let chevron_tooltip = if path_bar_collapsed {
                "Expand path bar (Ctrl+L)"
            } else {
                "Collapse path bar"
            };

            let chevron_btn = ui
                .add(
                    egui::Button::new(egui::RichText::new(chevron_icon).size(13.0))
                        .frame(false)
                        .min_size(egui::vec2(22.0, 22.0)),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if chevron_btn.clicked() {
                toggle_path_bar = true;
            }
            chevron_btn.on_hover_text(chevron_tooltip);
        });
    });

    (action, toggle_path_bar)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_short_filename() {
        assert_eq!(truncate_filename("MLE.pdf", 20), "MLE.pdf");
        assert_eq!(truncate_filename("doc.pdf", 7), "doc.pdf");
    }

    #[test]
    fn test_truncate_long_filename_preserves_ext() {
        let long_name = "Modern Operating Systems 4th Edition--Andrew Tanenbaum.pdf";
        let res = truncate_filename(long_name, 22);
        assert!(res.ends_with(".pdf"), "Must preserve .pdf extension: {res}");
        assert!(res.contains('…'), "Must contain ellipsis: {res}");
        assert!(res.chars().count() <= 22, "Length must be <= 22: {res}");
    }

    #[test]
    fn test_truncate_no_extension() {
        let long_name = "VeryLongDocumentTitleWithoutAnyExtension";
        let res = truncate_filename(long_name, 15);
        assert!(res.contains('…'));
        assert!(res.chars().count() <= 15);
    }
}
