use std::path::Path;

use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::ui::theme::Theme;

/// Actions emitted by user interactions on the tab bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabBarAction {
    Select(usize),
    Close(usize),
    NewTab,
    CloseWindow,
    OpenAbout,
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
    pub is_settings: bool,
    pub is_dormant: bool,
}

/// Draws a crisp vector PDF document icon with a folded corner.
fn paint_pdf_icon(ui: &mut egui::Ui, rect: Rect, is_active: bool) {
    let p = ui.painter();
    let icon_rect = Rect::from_center_size(rect.center(), Vec2::new(12.0, 15.0));
    let fold = 3.5_f32;

    let outline_col = if is_active {
        Color32::from_rgb(220, 70, 70) // Edge PDF red accent
    } else {
        Color32::from_rgb(170, 80, 80)
    };

    // Body of the page with top-right fold
    let points = [
        icon_rect.left_top(),
        Pos2::new(icon_rect.right() - fold, icon_rect.top()),
        Pos2::new(icon_rect.right(), icon_rect.top() + fold),
        icon_rect.right_bottom(),
        icon_rect.left_bottom(),
    ];

    p.add(egui::Shape::convex_polygon(
        points.to_vec(),
        Color32::from_rgb(32, 32, 34),
        Stroke::new(1.0_f32, outline_col),
    ));

    // Folded corner triangle
    let fold_points = [
        Pos2::new(icon_rect.right() - fold, icon_rect.top()),
        Pos2::new(icon_rect.right() - fold, icon_rect.top() + fold),
        Pos2::new(icon_rect.right(), icon_rect.top() + fold),
    ];
    p.add(egui::Shape::convex_polygon(
        fold_points.to_vec(),
        Color32::from_rgb(45, 45, 48),
        Stroke::new(1.0_f32, outline_col),
    ));

    // Red left accent indicator inside page
    let bar_rect = Rect::from_min_size(
        Pos2::new(icon_rect.left() + 2.0, icon_rect.top() + 3.0),
        Vec2::new(2.0, icon_rect.height() - 6.0),
    );
    p.rect_filled(bar_rect, CornerRadius::same(1), outline_col);
}

/// Renders the tab bar at the top of the application window, acting as the window title bar.
pub fn render_tab_bar(
    ui: &mut egui::Ui,
    tabs: &[TabItem<'_>],
    path_bar_collapsed: bool,
) -> (Option<TabBarAction>, bool) {
    let mut action = None;
    let mut toggle_path_bar = false;

    let row_height = Theme::TAB_HEIGHT;
    ui.set_height(row_height);

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(2.0, 0.0);

        // 0. Verso Reader brand mark (click to open About modal)
        let (brand_rect, brand_resp) =
            ui.allocate_exact_size(Vec2::new(26.0, 26.0), Sense::click());
        let brand_resp = brand_resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("About Verso Reader");
        if brand_resp.hovered() {
            ui.painter()
                .rect_filled(brand_rect, Theme::RADIUS, Theme::BG_PANEL_HOVER);
        }
        crate::ui::icons::paint_verso_logo(ui.painter(), brand_rect);
        if brand_resp.clicked() {
            action = Some(TabBarAction::OpenAbout);
        }
        ui.add_space(3.0);

        // 1. Render tabs
        for (idx, tab) in tabs.iter().enumerate() {
            let bg_color = if tab.is_active {
                Theme::TAB_ACTIVE_BG
            } else {
                Theme::TAB_INACTIVE_BG
            };

            let text_color = if tab.is_active {
                Color32::WHITE
            } else {
                Theme::TEXT_SECONDARY
            };

            let truncated_title = truncate_filename(tab.title, 24);

            let mut close_clicked = false;
            let mut close_btn_rect = Rect::NOTHING;

            let tab_frame = egui::Frame::new()
                .fill(bg_color)
                .corner_radius(Theme::TAB_RADIUS)
                .inner_margin(egui::Margin::symmetric(10, 6));

            let tab_response = tab_frame.show(ui, |ui| {
                ui.set_height(row_height - 6.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(7.0, 0.0);

                    // Document icon / Loading spinner / Settings gear icon
                    if tab.is_loading {
                        ui.spinner();
                    } else if tab.is_settings {
                        let (icon_rect, _) =
                            ui.allocate_exact_size(Vec2::new(14.0, 16.0), Sense::hover());
                        let icon_col = if tab.is_active {
                            Color32::from_rgb(0, 160, 255)
                        } else {
                            Color32::from_rgb(160, 160, 165)
                        };
                        crate::ui::icons::paint_gear(ui.painter(), icon_rect, icon_col);
                    } else {
                        let (icon_rect, _) =
                            ui.allocate_exact_size(Vec2::new(14.0, 16.0), Sense::hover());
                        paint_pdf_icon(ui, icon_rect, tab.is_active);
                    }

                    // Tab document title
                    let mut title_label = egui::RichText::new(&truncated_title)
                        .color(text_color)
                        .size(12.5);
                    if tab.is_dormant && !tab.is_active {
                        title_label = title_label
                            .color(Color32::from_rgb(130, 130, 135))
                            .italics();
                    }
                    ui.label(title_label);

                    // Unsaved changes subtle dot badge
                    if tab.is_dirty {
                        let (dot_rect, _) =
                            ui.allocate_exact_size(Vec2::new(8.0, 8.0), Sense::hover());
                        ui.painter().circle_filled(
                            dot_rect.center(),
                            2.8,
                            Color32::from_rgb(0, 120, 212),
                        );
                    }

                    // Close tab button with dedicated red hover state
                    let (c_rect, c_resp) =
                        ui.allocate_exact_size(Vec2::new(18.0, 18.0), Sense::click());
                    close_btn_rect = c_rect;

                    let c_hovered = c_resp.hovered();
                    let (btn_bg, cross_col) = if c_hovered {
                        (Theme::CLOSE_RED, Color32::WHITE)
                    } else if tab.is_active {
                        (Color32::TRANSPARENT, Color32::from_rgb(175, 175, 175))
                    } else {
                        (Color32::TRANSPARENT, Color32::from_rgb(125, 125, 125))
                    };

                    if btn_bg != Color32::TRANSPARENT {
                        ui.painter()
                            .rect_filled(c_rect, CornerRadius::same(4), btn_bg);
                    }

                    // Vector "✕" for tab close
                    let center = c_rect.center();
                    let half = 3.5_f32;
                    let cross_stroke = Stroke::new(1.2_f32, cross_col);
                    ui.painter().line_segment(
                        [
                            Pos2::new(center.x - half, center.y - half),
                            Pos2::new(center.x + half, center.y + half),
                        ],
                        cross_stroke,
                    );
                    ui.painter().line_segment(
                        [
                            Pos2::new(center.x - half, center.y + half),
                            Pos2::new(center.x + half, center.y - half),
                        ],
                        cross_stroke,
                    );

                    if c_resp.clicked() {
                        close_clicked = true;
                    }
                });
            });

            let tab_rect = tab_response.response.rect;

            // Draw active tab top accent line (Microsoft Edge signature accent indicator)
            if tab.is_active {
                let accent_line = Rect::from_min_size(
                    Pos2::new(tab_rect.left(), tab_rect.top()),
                    Vec2::new(tab_rect.width(), 2.0),
                );
                ui.painter().rect_filled(
                    accent_line,
                    CornerRadius {
                        nw: 4,
                        ne: 4,
                        sw: 0,
                        se: 0,
                    },
                    Theme::TAB_INDICATOR,
                );
            }

            let pointer_pos = ui.input(|i| i.pointer.hover_pos());
            let is_hovered = pointer_pos.is_some_and(|p| tab_rect.contains(p));

            if is_hovered {
                if !tab.is_active {
                    // Hover highlight for inactive tabs
                    let hover_frame = tab_rect.shrink(0.5);
                    ui.painter().rect_stroke(
                        hover_frame,
                        Theme::TAB_RADIUS,
                        Stroke::new(1.0_f32, Theme::BORDER_SUBTLE),
                        StrokeKind::Inside,
                    );
                }

                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                let tip = if tab.is_dormant && !tab.is_active {
                    format!(
                        "{} (Sleeping tab to save memory — click to wake)",
                        tab.tooltip
                    )
                } else {
                    tab.tooltip.to_string()
                };
                tab_response.response.on_hover_text(tip);

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

        // 2. Polished "+" button to open a new tab immediately
        ui.add_space(3.0);
        let (plus_rect, plus_resp) = ui.allocate_exact_size(Vec2::new(28.0, 28.0), Sense::click());
        let plus_hovered = plus_resp.hovered();
        if plus_hovered {
            ui.painter()
                .rect_filled(plus_rect, Theme::RADIUS, Theme::BG_PANEL_HOVER);
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let plus_center = plus_rect.center();
        let plus_half = 4.5_f32;
        let plus_col = if plus_hovered {
            Color32::WHITE
        } else {
            Theme::TEXT_SECONDARY
        };
        let plus_stroke = Stroke::new(1.4_f32, plus_col);
        ui.painter().line_segment(
            [
                Pos2::new(plus_center.x - plus_half, plus_center.y),
                Pos2::new(plus_center.x + plus_half, plus_center.y),
            ],
            plus_stroke,
        );
        ui.painter().line_segment(
            [
                Pos2::new(plus_center.x, plus_center.y - plus_half),
                Pos2::new(plus_center.x, plus_center.y + plus_half),
            ],
            plus_stroke,
        );
        if plus_resp.clicked() {
            action = Some(TabBarAction::NewTab);
        }
        plus_resp.on_hover_text("New Tab (Ctrl+T)");

        // 3. Middle draggable window area (allows moving the borderless window)
        let right_controls_width = 170.0;
        let drag_width = (ui.available_width() - right_controls_width).max(10.0);
        let (_, drag_resp) =
            ui.allocate_exact_size(Vec2::new(drag_width, row_height), Sense::click_and_drag());
        if drag_resp.drag_started_by(egui::PointerButton::Primary) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if drag_resp.double_clicked() {
            let is_max = ui.input(|i| i.viewport().maximized.unwrap_or(false));
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Maximized(!is_max));
        }

        // 4. Right side controls: path bar chevron + authentic desktop window control buttons
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(0.0, 0.0);

            let win_btn_size = Vec2::new(44.0, row_height);

            // Close Window Button (Vector ✕ with red hover)
            let (close_rect, close_resp) = ui.allocate_exact_size(win_btn_size, Sense::click());
            let close_hovered = close_resp.hovered();
            if close_hovered {
                ui.painter()
                    .rect_filled(close_rect, CornerRadius::ZERO, Theme::CLOSE_RED);
            }
            let c_center = close_rect.center();
            let c_half = 4.5_f32;
            let c_stroke = Stroke::new(
                1.2_f32,
                if close_hovered {
                    Color32::WHITE
                } else {
                    Theme::TEXT_SECONDARY
                },
            );
            ui.painter().line_segment(
                [
                    Pos2::new(c_center.x - c_half, c_center.y - c_half),
                    Pos2::new(c_center.x + c_half, c_center.y + c_half),
                ],
                c_stroke,
            );
            ui.painter().line_segment(
                [
                    Pos2::new(c_center.x - c_half, c_center.y + c_half),
                    Pos2::new(c_center.x + c_half, c_center.y - c_half),
                ],
                c_stroke,
            );
            if close_resp.clicked() {
                action = Some(TabBarAction::CloseWindow);
            }
            close_resp.on_hover_text("Close");

            // Maximize / Restore Button (Vector overlapping squares when max, single square when normal)
            let is_max = ui.input(|i| i.viewport().maximized.unwrap_or(false));
            let (max_rect, max_resp) = ui.allocate_exact_size(win_btn_size, Sense::click());
            let max_hovered = max_resp.hovered();
            if max_hovered {
                ui.painter()
                    .rect_filled(max_rect, CornerRadius::ZERO, Theme::BG_PANEL_HOVER);
            }
            let m_center = max_rect.center();
            let m_stroke = Stroke::new(
                1.1_f32,
                if max_hovered {
                    Color32::WHITE
                } else {
                    Theme::TEXT_SECONDARY
                },
            );

            if is_max {
                // Two overlapping squares for restore
                // Back square
                let back_sq = Rect::from_min_size(
                    Pos2::new(m_center.x - 2.5, m_center.y - 4.5),
                    Vec2::new(7.5, 7.5),
                );
                ui.painter()
                    .rect_stroke(back_sq, CornerRadius::ZERO, m_stroke, StrokeKind::Inside);
                // Front square
                let front_sq = Rect::from_min_size(
                    Pos2::new(m_center.x - 5.0, m_center.y - 2.0),
                    Vec2::new(7.5, 7.5),
                );
                let fill = if max_hovered {
                    Theme::BG_PANEL_HOVER
                } else {
                    Theme::BG_PANEL
                };
                ui.painter().rect_filled(front_sq, CornerRadius::ZERO, fill);
                ui.painter().rect_stroke(
                    front_sq,
                    CornerRadius::ZERO,
                    m_stroke,
                    StrokeKind::Inside,
                );
            } else {
                // Single square for maximize
                let sq = Rect::from_center_size(m_center, Vec2::new(10.0, 10.0));
                ui.painter()
                    .rect_stroke(sq, CornerRadius::ZERO, m_stroke, StrokeKind::Inside);
            }

            if max_resp.clicked() {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Maximized(!is_max));
            }
            max_resp.on_hover_text(if is_max { "Restore" } else { "Maximize" });

            // Minimize Button (Crisp horizontal line)
            let (min_rect, min_resp) = ui.allocate_exact_size(win_btn_size, Sense::click());
            let min_hovered = min_resp.hovered();
            if min_hovered {
                ui.painter()
                    .rect_filled(min_rect, CornerRadius::ZERO, Theme::BG_PANEL_HOVER);
            }
            let min_center = min_rect.center();
            let min_stroke = Stroke::new(
                1.2_f32,
                if min_hovered {
                    Color32::WHITE
                } else {
                    Theme::TEXT_SECONDARY
                },
            );
            ui.painter().line_segment(
                [
                    Pos2::new(min_center.x - 5.0, min_center.y),
                    Pos2::new(min_center.x + 5.0, min_center.y),
                ],
                min_stroke,
            );
            if min_resp.clicked() {
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
            min_resp.on_hover_text("Minimize");

            // Chevron Button for path bar toggle
            ui.add_space(4.0);
            let (chev_rect, chev_resp) =
                ui.allocate_exact_size(Vec2::new(28.0, 28.0), Sense::click());
            let chev_hovered = chev_resp.hovered();
            if chev_hovered {
                ui.painter()
                    .rect_filled(chev_rect, Theme::RADIUS, Theme::BG_PANEL_HOVER);
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let chev_center = chev_rect.center();
            let chev_stroke = Stroke::new(
                1.3_f32,
                if chev_hovered {
                    Color32::WHITE
                } else {
                    Theme::TEXT_SECONDARY
                },
            );
            if path_bar_collapsed {
                // Down arrow / chevron
                ui.painter().line_segment(
                    [
                        Pos2::new(chev_center.x - 4.0, chev_center.y - 2.0),
                        Pos2::new(chev_center.x, chev_center.y + 2.5),
                    ],
                    chev_stroke,
                );
                ui.painter().line_segment(
                    [
                        Pos2::new(chev_center.x, chev_center.y + 2.5),
                        Pos2::new(chev_center.x + 4.0, chev_center.y - 2.0),
                    ],
                    chev_stroke,
                );
            } else {
                // Up arrow / chevron
                ui.painter().line_segment(
                    [
                        Pos2::new(chev_center.x - 4.0, chev_center.y + 2.0),
                        Pos2::new(chev_center.x, chev_center.y - 2.5),
                    ],
                    chev_stroke,
                );
                ui.painter().line_segment(
                    [
                        Pos2::new(chev_center.x, chev_center.y - 2.5),
                        Pos2::new(chev_center.x + 4.0, chev_center.y + 2.0),
                    ],
                    chev_stroke,
                );
            }

            if chev_resp.clicked() {
                toggle_path_bar = true;
            }
            chev_resp.on_hover_text(if path_bar_collapsed {
                "Expand path bar (Ctrl+L)"
            } else {
                "Collapse path bar"
            });
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

    #[test]
    fn test_tabbar_action_variants() {
        let action = TabBarAction::OpenAbout;
        assert_eq!(action, TabBarAction::OpenAbout);
        assert_ne!(action, TabBarAction::NewTab);
    }
}
