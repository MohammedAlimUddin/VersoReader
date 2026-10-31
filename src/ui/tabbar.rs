use std::path::Path;

use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, UiBuilder, Vec2};

use crate::ui::theme::Theme;

pub const MIN_TAB_WIDTH: f32 = 48.0;
pub const MAX_TAB_WIDTH: f32 = 210.0;

/// Actions emitted by user interactions on the tab bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabBarAction {
    Select(usize),
    Close(usize),
    NewTab,
    CloseWindow,
    OpenAbout,
    MoveTab { from: usize, to: usize },
    DetachTab { index: usize },
}

/// Transient state of a tab drag interaction (either reordering or detaching into a new window).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TabDragState {
    /// Index of the tab being dragged.
    pub src_idx: usize,
    /// Starting pointer position when the drag gesture began.
    pub start_pos: Pos2,
    /// Current pointer position.
    pub current_pos: Pos2,
    /// Target insertion index in the tab strip.
    pub target_idx: usize,
    /// Whether the pointer has moved outside the tab strip into detachment territory.
    pub is_detaching: bool,
}

/// Computes the optimal tab width to fit `tab_count` tabs into `available_width`.
pub fn compute_tab_width(available_width: f32, tab_count: usize) -> f32 {
    if tab_count == 0 {
        return MAX_TAB_WIDTH;
    }
    // Reserve space for brand mark (26 + 3), plus button (28 + 3), and minimum window drag handle (20)
    let non_tab_reserved = 29.0 + 31.0 + 20.0;
    let tab_strip_available = (available_width - non_tab_reserved).max(0.0);
    let item_spacing = 2.0;
    let total_spacing = item_spacing * (tab_count.saturating_sub(1) as f32);
    let net_for_tabs = (tab_strip_available - total_spacing).max(0.0);
    (net_for_tabs / tab_count as f32).clamp(MIN_TAB_WIDTH, MAX_TAB_WIDTH)
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
    let is_dark = ui.visuals().dark_mode;
    let icon_rect = Rect::from_center_size(rect.center(), Vec2::new(12.0, 15.0));
    let fold = 3.5_f32;

    let outline_col = if is_active {
        Color32::from_rgb(220, 70, 70) // Edge PDF red accent
    } else if is_dark {
        Color32::from_rgb(170, 80, 80)
    } else {
        Color32::from_rgb(190, 90, 90)
    };

    let (body_fill, fold_fill) = if is_dark {
        (Color32::from_rgb(32, 32, 34), Color32::from_rgb(45, 45, 48))
    } else {
        (Color32::WHITE, Color32::from_rgb(225, 228, 234))
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
        body_fill,
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
        fold_fill,
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

    let is_dark = ui.visuals().dark_mode;
    let palette = Theme::palette(is_dark);

    let row_height = Theme::TAB_HEIGHT;
    let bar_rect = ui.available_rect_before_wrap();
    let bar_rect = Rect::from_min_size(bar_rect.min, Vec2::new(bar_rect.width(), row_height));

    let right_controls_width = 164.0;
    let right_rect = Rect::from_min_max(
        Pos2::new(
            (bar_rect.right() - right_controls_width).max(bar_rect.left()),
            bar_rect.top(),
        ),
        bar_rect.right_bottom(),
    );

    // -------------------------------------------------------------
    // 1. Right side controls: locked to right edge, never overlapped
    // -------------------------------------------------------------
    let mut right_ui = ui.new_child(
        UiBuilder::new()
            .max_rect(right_rect)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    right_ui.spacing_mut().item_spacing = Vec2::ZERO;

    let win_btn_size = Vec2::new(44.0, row_height);

    // Close Window Button (Vector ✕ with red hover)
    let (close_rect, close_resp) = right_ui.allocate_exact_size(win_btn_size, Sense::click());
    let close_hovered = close_resp.hovered();
    if close_hovered {
        right_ui
            .painter()
            .rect_filled(close_rect, CornerRadius::ZERO, Theme::CLOSE_RED);
    }
    let c_center = close_rect.center();
    let c_half = 4.5_f32;
    let c_stroke = Stroke::new(
        1.2_f32,
        if close_hovered {
            Color32::WHITE
        } else {
            palette.text_secondary
        },
    );
    right_ui.painter().line_segment(
        [
            Pos2::new(c_center.x - c_half, c_center.y - c_half),
            Pos2::new(c_center.x + c_half, c_center.y + c_half),
        ],
        c_stroke,
    );
    right_ui.painter().line_segment(
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
    let is_max = right_ui.input(|i| i.viewport().maximized.unwrap_or(false));
    let (max_rect, max_resp) = right_ui.allocate_exact_size(win_btn_size, Sense::click());
    let max_hovered = max_resp.hovered();
    if max_hovered {
        right_ui
            .painter()
            .rect_filled(max_rect, CornerRadius::ZERO, palette.bg_panel_hover);
    }
    let m_center = max_rect.center();
    let m_stroke = Stroke::new(
        1.1_f32,
        if max_hovered {
            palette.icon_fg_hover
        } else {
            palette.text_secondary
        },
    );

    if is_max {
        let back_sq = Rect::from_min_size(
            Pos2::new(m_center.x - 2.5, m_center.y - 4.5),
            Vec2::new(7.5, 7.5),
        );
        right_ui
            .painter()
            .rect_stroke(back_sq, CornerRadius::ZERO, m_stroke, StrokeKind::Inside);
        let front_sq = Rect::from_min_size(
            Pos2::new(m_center.x - 5.0, m_center.y - 2.0),
            Vec2::new(7.5, 7.5),
        );
        let fill = if max_hovered {
            palette.bg_panel_hover
        } else {
            palette.bg_panel
        };
        right_ui
            .painter()
            .rect_filled(front_sq, CornerRadius::ZERO, fill);
        right_ui
            .painter()
            .rect_stroke(front_sq, CornerRadius::ZERO, m_stroke, StrokeKind::Inside);
    } else {
        let sq = Rect::from_center_size(m_center, Vec2::new(10.0, 10.0));
        right_ui
            .painter()
            .rect_stroke(sq, CornerRadius::ZERO, m_stroke, StrokeKind::Inside);
    }

    if max_resp.clicked() {
        right_ui
            .ctx()
            .send_viewport_cmd(egui::ViewportCommand::Maximized(!is_max));
    }
    max_resp.on_hover_text(if is_max { "Restore" } else { "Maximize" });

    // Minimize Button (Crisp horizontal line)
    let (min_rect, min_resp) = right_ui.allocate_exact_size(win_btn_size, Sense::click());
    let min_hovered = min_resp.hovered();
    if min_hovered {
        right_ui
            .painter()
            .rect_filled(min_rect, CornerRadius::ZERO, palette.bg_panel_hover);
    }
    let min_center = min_rect.center();
    let min_stroke = Stroke::new(
        1.2_f32,
        if min_hovered {
            palette.icon_fg_hover
        } else {
            palette.text_secondary
        },
    );
    right_ui.painter().line_segment(
        [
            Pos2::new(min_center.x - 5.0, min_center.y),
            Pos2::new(min_center.x + 5.0, min_center.y),
        ],
        min_stroke,
    );
    if min_resp.clicked() {
        right_ui
            .ctx()
            .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
    }
    min_resp.on_hover_text("Minimize");

    // Chevron Button for path bar toggle
    right_ui.add_space(4.0);
    let (chev_rect, chev_resp) =
        right_ui.allocate_exact_size(Vec2::new(28.0, 28.0), Sense::click());
    let chev_hovered = chev_resp.hovered();
    if chev_hovered {
        right_ui
            .painter()
            .rect_filled(chev_rect, Theme::RADIUS, palette.bg_panel_hover);
        right_ui
            .ctx()
            .set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let chev_center = chev_rect.center();
    let chev_stroke = Stroke::new(
        1.3_f32,
        if chev_hovered {
            palette.icon_fg_hover
        } else {
            palette.text_secondary
        },
    );
    if path_bar_collapsed {
        // Down arrow / chevron
        right_ui.painter().line_segment(
            [
                Pos2::new(chev_center.x - 4.0, chev_center.y - 2.0),
                Pos2::new(chev_center.x, chev_center.y + 2.5),
            ],
            chev_stroke,
        );
        right_ui.painter().line_segment(
            [
                Pos2::new(chev_center.x, chev_center.y + 2.5),
                Pos2::new(chev_center.x + 4.0, chev_center.y - 2.0),
            ],
            chev_stroke,
        );
    } else {
        // Up arrow / chevron
        right_ui.painter().line_segment(
            [
                Pos2::new(chev_center.x - 4.0, chev_center.y + 2.0),
                Pos2::new(chev_center.x, chev_center.y - 2.5),
            ],
            chev_stroke,
        );
        right_ui.painter().line_segment(
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

    // -------------------------------------------------------------
    // 2. Left side & Tab strip: brand, dynamic tabs, "+", drag spacer
    // -------------------------------------------------------------
    let left_max_x = (right_rect.left() - 4.0).max(bar_rect.left());
    let left_rect = Rect::from_min_max(
        bar_rect.left_top(),
        Pos2::new(left_max_x, bar_rect.bottom()),
    );

    let mut left_ui = ui.new_child(
        UiBuilder::new()
            .max_rect(left_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    left_ui.spacing_mut().item_spacing = Vec2::new(2.0, 0.0);

    // Verso Reader brand mark (click to open About modal)
    let (brand_rect, brand_resp) =
        left_ui.allocate_exact_size(Vec2::new(26.0, 26.0), Sense::click());
    let brand_resp = brand_resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("About Verso Reader");
    if brand_resp.hovered() {
        left_ui
            .painter()
            .rect_filled(brand_rect, Theme::RADIUS, palette.bg_panel_hover);
    }
    crate::ui::icons::paint_verso_logo(left_ui.painter(), brand_rect);
    if brand_resp.clicked() {
        action = Some(TabBarAction::OpenAbout);
    }
    left_ui.add_space(3.0);

    // Retrieve active drag state (if any) from egui context
    let drag_id = egui::Id::new("verso_tab_drag_state");
    let mut drag_state: Option<TabDragState> = left_ui.ctx().data_mut(|d| d.get_temp(drag_id));

    let pointer_pos = left_ui.input(|i| i.pointer.hover_pos().or(i.pointer.interact_pos()));
    let is_primary_down = left_ui.input(|i| i.pointer.primary_down());
    let is_primary_pressed =
        left_ui.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary));
    let is_primary_released =
        left_ui.input(|i| i.pointer.button_released(egui::PointerButton::Primary));
    let is_middle_clicked =
        left_ui.input(|i| i.pointer.button_clicked(egui::PointerButton::Middle));

    let tab_w = compute_tab_width(left_rect.width(), tabs.len());
    let mut tab_rects = Vec::with_capacity(tabs.len());
    let mut clicked_close_idx = None;

    // Render tabs
    for (idx, tab) in tabs.iter().enumerate() {
        let is_being_dragged = drag_state
            .as_ref()
            .is_some_and(|d| d.src_idx == idx && (d.current_pos - d.start_pos).length() > 4.0);

        let bg_color = if tab.is_active {
            palette.tab_active_bg
        } else {
            palette.tab_inactive_bg
        };

        let text_color = if tab.is_active {
            palette.text_primary
        } else {
            palette.text_secondary
        };

        let inner_margin_x = if tab_w < 80.0 {
            4
        } else if tab_w < 130.0 {
            6
        } else {
            8
        };
        let item_spacing_x = if tab_w < 80.0 { 3.0 } else { 5.0 };
        let show_close = tab_w >= 70.0 || tab.is_active;

        let tab_frame = egui::Frame::new()
            .fill(if is_being_dragged {
                bg_color.gamma_multiply(0.6)
            } else {
                bg_color
            })
            .corner_radius(Theme::TAB_RADIUS)
            .inner_margin(egui::Margin::symmetric(inner_margin_x, 5));

        let tab_inner_w = (tab_w - (inner_margin_x * 2) as f32).max(12.0);

        let tab_response = tab_frame.show(&mut left_ui, |ui| {
            ui.set_width(tab_inner_w);
            ui.set_height(row_height - 6.0);
            let inner_rect = ui.max_rect();

            // 1. Close tab button (permanently anchored on the far right end of the tab)
            let close_rect = Rect::from_center_size(
                Pos2::new(inner_rect.right() - 9.0, inner_rect.center().y),
                Vec2::new(18.0, 18.0),
            );
            if show_close {
                let close_id = ui.id().with(("tab_close_btn", idx));
                let c_resp = ui
                    .interact(close_rect, close_id, Sense::click())
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("Close tab (Ctrl+W)");

                let c_hovered = c_resp.hovered();
                let (btn_bg, cross_col) = if c_hovered {
                    (Theme::CLOSE_RED, Color32::WHITE)
                } else if tab.is_active {
                    (Color32::TRANSPARENT, palette.icon_fg)
                } else {
                    (Color32::TRANSPARENT, palette.text_muted)
                };

                if btn_bg != Color32::TRANSPARENT {
                    ui.painter()
                        .rect_filled(close_rect, CornerRadius::same(4), btn_bg);
                }

                let center = close_rect.center();
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
                    clicked_close_idx = Some(idx);
                }
            }

            // 2. Unsaved changes subtle dot badge (anchored to the left of close button)
            let dot_right = if show_close {
                close_rect.left() - 3.0
            } else {
                inner_rect.right() - 2.0
            };
            let dot_center = Pos2::new(dot_right - 4.0, inner_rect.center().y);
            if tab.is_dirty {
                ui.painter()
                    .circle_filled(dot_center, 2.8, Color32::from_rgb(0, 120, 212));
            }

            // 3. Document icon / Loading spinner / Settings gear icon (left-anchored)
            let icon_w = 14.0;
            let icon_rect = Rect::from_center_size(
                Pos2::new(inner_rect.left() + icon_w * 0.5, inner_rect.center().y),
                Vec2::new(icon_w, 16.0),
            );
            if tab.is_loading {
                let spinner_rect =
                    Rect::from_center_size(icon_rect.center(), Vec2::new(14.0, 14.0));
                let mut spinner_ui = ui.new_child(UiBuilder::new().max_rect(spinner_rect).layout(
                    egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                ));
                spinner_ui.spinner();
            } else if tab.is_settings {
                let icon_col = if tab.is_active {
                    Color32::from_rgb(0, 160, 255)
                } else {
                    Color32::from_rgb(160, 160, 165)
                };
                crate::ui::icons::paint_gear(ui.painter(), icon_rect, icon_col);
            } else {
                paint_pdf_icon(ui, icon_rect, tab.is_active);
            }

            // 4. Document title (occupies middle space between icon and right controls)
            let title_left = icon_rect.right() + item_spacing_x;
            let title_right = if tab.is_dirty {
                dot_center.x - 7.0
            } else if show_close {
                close_rect.left() - 4.0
            } else {
                inner_rect.right()
            };
            let title_avail_w = (title_right - title_left).max(0.0);
            let max_title_chars = (title_avail_w / 6.8).floor() as usize;

            if max_title_chars >= 3 {
                let truncated_title = truncate_filename(tab.title, max_title_chars);
                let title_rect = Rect::from_min_max(
                    Pos2::new(title_left, inner_rect.top()),
                    Pos2::new(title_right, inner_rect.bottom()),
                );
                let mut title_ui = ui.new_child(
                    UiBuilder::new()
                        .max_rect(title_rect)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                let mut title_label = egui::RichText::new(&truncated_title)
                    .color(text_color)
                    .size(12.0);
                if tab.is_dormant && !tab.is_active {
                    title_label = title_label.color(palette.text_muted).italics();
                }
                title_ui.label(title_label);
            }
        });

        let tab_rect = tab_response.response.rect;
        tab_rects.push(tab_rect);

        // Draw active tab top accent line
        if tab.is_active {
            let accent_line = Rect::from_min_size(
                Pos2::new(tab_rect.left(), tab_rect.top()),
                Vec2::new(tab_rect.width(), 2.0),
            );
            left_ui.painter().rect_filled(
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

        let is_tab_hovered = pointer_pos.is_some_and(|p| tab_rect.contains(p));
        if is_tab_hovered && !tab.is_active && !is_being_dragged {
            let hover_frame = tab_rect.shrink(0.5);
            left_ui.painter().rect_stroke(
                hover_frame,
                Theme::TAB_RADIUS,
                Stroke::new(1.0_f32, palette.border_subtle),
                StrokeKind::Inside,
            );
        }

        if is_tab_hovered {
            if drag_state.is_none() {
                left_ui
                    .ctx()
                    .set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let tip = if tab.is_dormant && !tab.is_active {
                format!(
                    "{} (Sleeping tab to save memory — click to wake)",
                    tab.tooltip
                )
            } else {
                tab.tooltip.to_string()
            };
            tab_response.response.on_hover_text(tip);

            if is_middle_clicked {
                action = Some(TabBarAction::Close(idx));
            }
        }
    }

    // -------------------------------------------------------------
    // Drag-to-reorder and drag-to-detach event handling
    // -------------------------------------------------------------
    if let Some(pos) = pointer_pos {
        if is_primary_pressed && clicked_close_idx.is_none() {
            for (idx, rect) in tab_rects.iter().enumerate() {
                let show_close = tab_w >= 70.0 || tabs[idx].is_active;
                let on_close_btn = show_close && {
                    let c_rect = Rect::from_center_size(
                        Pos2::new(rect.right() - 4.0 - 9.0, rect.center().y),
                        Vec2::new(18.0, 18.0),
                    );
                    c_rect.contains(pos)
                };
                if rect.contains(pos) && !on_close_btn {
                    drag_state = Some(TabDragState {
                        src_idx: idx,
                        start_pos: pos,
                        current_pos: pos,
                        target_idx: idx,
                        is_detaching: false,
                    });
                    break;
                }
            }
        }
    }

    if let Some(ref mut drag) = drag_state {
        if is_primary_down {
            if let Some(pos) = pointer_pos {
                drag.current_pos = pos;
                let dist = (pos - drag.start_pos).length();

                if dist > 4.0 {
                    // Dragging outside tab bar bounds vertically triggers detachment (only if multiple tabs exist)
                    let is_outside_y =
                        pos.y > bar_rect.bottom() + 25.0 || pos.y < bar_rect.top() - 25.0;
                    let can_detach =
                        tabs.len() > 1 && !tabs.get(drag.src_idx).is_some_and(|t| t.is_settings);
                    drag.is_detaching = is_outside_y && can_detach;

                    if drag.is_detaching {
                        left_ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
                    } else {
                        left_ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);

                        // Find target reordering slot
                        let mut target = drag.src_idx;
                        for (i, rect) in tab_rects.iter().enumerate() {
                            if pos.x < rect.center().x {
                                target = i;
                                break;
                            }
                            target = i;
                        }
                        drag.target_idx = target;
                    }
                    left_ui.ctx().request_repaint();
                }
            }
        }
    }

    if is_primary_released {
        if let Some(drag) = drag_state.take() {
            let dist = (drag.current_pos - drag.start_pos).length();
            if dist > 4.0 {
                if drag.is_detaching {
                    action = Some(TabBarAction::DetachTab {
                        index: drag.src_idx,
                    });
                } else if drag.target_idx != drag.src_idx {
                    action = Some(TabBarAction::MoveTab {
                        from: drag.src_idx,
                        to: drag.target_idx,
                    });
                }
            } else if clicked_close_idx.is_none() && !tabs[drag.src_idx].is_active {
                action = Some(TabBarAction::Select(drag.src_idx));
            }
        }
    }

    if let Some(close_idx) = clicked_close_idx {
        action = Some(TabBarAction::Close(close_idx));
        drag_state = None;
    }

    // Visual feedback while dragging
    if let Some(ref drag) = drag_state {
        let dist = (drag.current_pos - drag.start_pos).length();
        if dist > 4.0 {
            if drag.is_detaching {
                // Floating detach preview following cursor
                let p = left_ui.ctx().layer_painter(egui::LayerId::new(
                    egui::Order::Tooltip,
                    egui::Id::new("tab_detach_preview_layer"),
                ));
                let preview_rect = Rect::from_min_size(
                    drag.current_pos + Vec2::new(14.0, 14.0),
                    Vec2::new(175.0, 34.0),
                );
                p.rect_filled(
                    preview_rect,
                    CornerRadius::same(6),
                    palette.bg_panel.gamma_multiply(0.95),
                );
                p.rect_stroke(
                    preview_rect,
                    CornerRadius::same(6),
                    Stroke::new(1.2_f32, Color32::from_rgb(0, 160, 255)),
                    StrokeKind::Outside,
                );

                let tab_title = tabs
                    .get(drag.src_idx)
                    .map(|t| t.title)
                    .unwrap_or("PDF Document");
                let trunc = truncate_filename(tab_title, 18);
                p.text(
                    Pos2::new(preview_rect.left() + 8.0, preview_rect.top() + 6.0),
                    egui::Align2::LEFT_TOP,
                    format!("📄 {trunc}"),
                    egui::FontId::proportional(12.0),
                    palette.text_primary,
                );
                p.text(
                    Pos2::new(preview_rect.left() + 8.0, preview_rect.top() + 20.0),
                    egui::Align2::LEFT_TOP,
                    "Detach into new window",
                    egui::FontId::proportional(9.5),
                    Color32::from_rgb(0, 160, 255),
                );
            } else if let Some(target_rect) = tab_rects.get(drag.target_idx) {
                // Vertical insertion indicator line at target drop position
                let drop_x = if drag.target_idx <= drag.src_idx {
                    target_rect.left() - 1.0
                } else {
                    target_rect.right() + 1.0
                };
                let p = left_ui.painter();
                let top_y = bar_rect.top() + 4.0;
                let bot_y = bar_rect.bottom() - 4.0;
                let ind_col = Color32::from_rgb(0, 160, 255);
                p.line_segment(
                    [Pos2::new(drop_x, top_y), Pos2::new(drop_x, bot_y)],
                    Stroke::new(2.5_f32, ind_col),
                );
                p.circle_filled(Pos2::new(drop_x, top_y + 1.0), 2.5, ind_col);
                p.circle_filled(Pos2::new(drop_x, bot_y - 1.0), 2.5, ind_col);
            }
        }
    }

    // Persist drag state back to egui memory
    left_ui.ctx().data_mut(|d| {
        if let Some(state) = drag_state {
            d.insert_temp(drag_id, state);
        } else {
            d.remove_temp::<TabDragState>(drag_id);
        }
    });

    // "+" button to open a new tab immediately
    left_ui.add_space(3.0);
    let (plus_rect, plus_resp) = left_ui.allocate_exact_size(Vec2::new(28.0, 28.0), Sense::click());
    let plus_hovered = plus_resp.hovered();
    if plus_hovered {
        left_ui
            .painter()
            .rect_filled(plus_rect, Theme::RADIUS, palette.bg_panel_hover);
        left_ui
            .ctx()
            .set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let plus_center = plus_rect.center();
    let plus_half = 4.5_f32;
    let plus_col = if plus_hovered {
        palette.icon_fg_hover
    } else {
        palette.text_secondary
    };
    let plus_stroke = Stroke::new(1.4_f32, plus_col);
    left_ui.painter().line_segment(
        [
            Pos2::new(plus_center.x - plus_half, plus_center.y),
            Pos2::new(plus_center.x + plus_half, plus_center.y),
        ],
        plus_stroke,
    );
    left_ui.painter().line_segment(
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

    // Middle draggable window area (allows moving the borderless window)
    let drag_width = left_ui.available_width().max(0.0);
    if drag_width > 0.0 {
        let (_, drag_resp) =
            left_ui.allocate_exact_size(Vec2::new(drag_width, row_height), Sense::click_and_drag());
        if drag_resp.drag_started_by(egui::PointerButton::Primary) {
            left_ui
                .ctx()
                .send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if drag_resp.double_clicked() {
            let is_max = left_ui.input(|i| i.viewport().maximized.unwrap_or(false));
            left_ui
                .ctx()
                .send_viewport_cmd(egui::ViewportCommand::Maximized(!is_max));
        }
    }

    ui.allocate_rect(bar_rect, Sense::hover());
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

        let move_act = TabBarAction::MoveTab { from: 1, to: 3 };
        assert_eq!(move_act, TabBarAction::MoveTab { from: 1, to: 3 });

        let detach_act = TabBarAction::DetachTab { index: 2 };
        assert_eq!(detach_act, TabBarAction::DetachTab { index: 2 });
    }

    #[test]
    fn test_compute_tab_width_few_tabs() {
        // Wide window (1000px) with 2 tabs -> clamped to MAX_TAB_WIDTH (210)
        let w = compute_tab_width(1000.0, 2);
        assert_eq!(w, MAX_TAB_WIDTH);
    }

    #[test]
    fn test_compute_tab_width_many_tabs() {
        // Normal window (800px) with 20 tabs -> clamped to MIN_TAB_WIDTH (48)
        let w = compute_tab_width(800.0, 20);
        assert_eq!(w, MIN_TAB_WIDTH);

        // Moderate tab count (800px with 6 tabs) -> within (MIN_TAB_WIDTH..=MAX_TAB_WIDTH)
        let w_med = compute_tab_width(800.0, 6);
        assert!((MIN_TAB_WIDTH..=MAX_TAB_WIDTH).contains(&w_med));
    }

    #[test]
    fn test_compute_tab_width_zero_tabs() {
        assert_eq!(compute_tab_width(800.0, 0), MAX_TAB_WIDTH);
    }
}
