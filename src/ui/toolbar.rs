use crate::annotations::{AnnotationColor, PRESET_HIGHLIGHT_COLORS};
use crate::ui::icons::{
    paint_chevron_down, paint_eraser, paint_fit_page, paint_fit_width, paint_fullscreen,
    paint_gear, paint_highlighter, paint_nav_back, paint_nav_forward, paint_redo, paint_rotate,
    paint_save, paint_search, paint_sidebar, paint_text_tool, paint_undo, paint_zoom_in,
    paint_zoom_out, render_icon_button,
};
use crate::ui::theme::Theme;
use egui::{
    pos2, vec2, Color32, CornerRadius, Margin, Rect, RichText, Stroke, StrokeKind, TextEdit,
    UiBuilder,
};

/// Active annotation tool mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveTool {
    None,
    Highlighter,
    Eraser,
    Text,
}

/// Actions emitted by user interactions in the toolbar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ToolbarAction {
    ToggleOutlineSidebar,
    ToggleFind,
    JumpToPage(usize),
    GoBack,
    GoForward,
    ZoomIn,
    ZoomOut,
    FitWidth,
    FitPage,
    RotateClockwise,
    ToggleFullscreen,
    ToggleHighlighter,
    ToggleEraser,
    ToggleText,
    Undo,
    Redo,
    SetHighlighterColor(AnnotationColor),
    SetHighlighterThickness(f32),
    SetTextFontSize(f32),
    SetTextColor(AnnotationColor),
    Save,
    OpenSettings,
}

/// Properties passed to the toolbar for rendering.
#[derive(Debug, Clone, Copy)]
pub struct ToolbarProps {
    pub current_page: usize,
    pub total_pages: usize,
    pub zoom: f32,
    pub is_fullscreen: bool,
    pub sidebar_open: bool,
    pub find_open: bool,
    pub settings_open: bool,
    pub active_tool: ActiveTool,
    pub highlighter_color: AnnotationColor,
    pub highlighter_thickness: f32,
    pub text_font_size: f32,
    pub text_color: AnnotationColor,
    pub can_save: bool,
    pub has_unsaved_changes: bool,
    pub can_undo: bool,
    pub can_redo: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
}

/// Main top toolbar component containing page navigation, zoom, rotation, annotation tools, and fullscreen controls.
pub struct Toolbar {
    pub page_input_text: String,
    pub is_editing_page: bool,
}

impl Toolbar {
    pub fn new() -> Self {
        Self {
            page_input_text: "1".to_string(),
            is_editing_page: false,
        }
    }

    /// Renders the toolbar and returns any user-triggered action.
    /// Distributed in 3 clusters matching Microsoft Edge:
    /// - Left: TOC outline, Highlighter (+options), Eraser, Text tool (+options), Undo, Redo
    /// - Center: Zoom controls, Fit width/page, Page jump box [ N ] of M, Rotate
    /// - Right: Search, Save, Fullscreen, Preferences
    pub fn ui(&mut self, ui: &mut egui::Ui, props: ToolbarProps) -> Option<ToolbarAction> {
        let mut action = None;

        if !self.is_editing_page {
            self.page_input_text = props.current_page.to_string();
        }

        let bar_rect = ui.available_rect_before_wrap();
        let bar_rect = Rect::from_min_size(bar_rect.min, vec2(bar_rect.width(), 34.0));

        // -------------------------------------------------------------
        // Cluster 1: Left (TOC, Annotations, Undo/Redo)
        // -------------------------------------------------------------
        let mut left_ui = ui.new_child(
            UiBuilder::new()
                .max_rect(bar_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        left_ui.spacing_mut().item_spacing = vec2(4.0, 0.0);

        let sidebar_btn = render_icon_button(
            &mut left_ui,
            vec2(32.0, 32.0),
            props.sidebar_open,
            true,
            "Table of contents (outline)",
            |p, r, c| paint_sidebar(p, r, c, props.sidebar_open),
        );
        if sidebar_btn.clicked() {
            action = Some(ToolbarAction::ToggleOutlineSidebar);
        }

        left_ui.add_space(2.0);
        left_ui.separator();
        left_ui.add_space(2.0);

        let hl_active = props.active_tool == ActiveTool::Highlighter;
        let hl_color = Color32::from_rgb(
            props.highlighter_color.r,
            props.highlighter_color.g,
            props.highlighter_color.b,
        );
        let hl_btn = render_icon_button(
            &mut left_ui,
            vec2(32.0, 32.0),
            hl_active,
            true,
            "Highlighter (drag over text or whitespace to highlight)",
            |p, r, c| paint_highlighter(p, r, c, hl_color),
        );
        if hl_btn.clicked() {
            action = Some(ToolbarAction::ToggleHighlighter);
        }

        let popup_id = left_ui.make_persistent_id("highlighter_popup_menu");
        let chevron_btn = render_icon_button(
            &mut left_ui,
            vec2(16.0, 32.0),
            false,
            true,
            "Highlighter color and thickness",
            paint_chevron_down,
        );
        if chevron_btn.clicked() {
            left_ui.memory_mut(|m| m.toggle_popup(popup_id));
        }

        egui::popup_below_widget(
            &left_ui,
            popup_id,
            &chevron_btn,
            egui::PopupCloseBehavior::CloseOnClickOutside,
            |ui| {
                ui.set_min_width(170.0);
                ui.add_space(2.0);
                ui.label(RichText::new("Color").strong().size(12.0));
                ui.horizontal(|ui| {
                    for color in PRESET_HIGHLIGHT_COLORS {
                        let is_sel = props.highlighter_color == color;
                        let c = Color32::from_rgb(color.r, color.g, color.b);
                        let (rect, resp) =
                            ui.allocate_exact_size(vec2(22.0, 22.0), egui::Sense::click());
                        let p = ui.painter();
                        p.rect_filled(rect, 4.0, c);
                        if is_sel {
                            p.rect_stroke(
                                rect,
                                4.0,
                                Stroke::new(2.0_f32, Color32::WHITE),
                                StrokeKind::Outside,
                            );
                        } else if resp.hovered() {
                            p.rect_stroke(
                                rect,
                                4.0,
                                Stroke::new(1.0_f32, Color32::LIGHT_GRAY),
                                StrokeKind::Outside,
                            );
                        }
                        if resp.clicked() {
                            action = Some(ToolbarAction::SetHighlighterColor(color));
                        }
                    }
                });

                ui.add_space(8.0);
                ui.label(RichText::new("Thickness").strong().size(12.0));
                let mut thickness = props.highlighter_thickness;
                let slider = egui::Slider::new(&mut thickness, 0.25..=1.0)
                    .custom_formatter(|val, _| format!("{:.0}%", val * 100.0))
                    .show_value(true);
                if ui.add(slider).changed() {
                    action = Some(ToolbarAction::SetHighlighterThickness(thickness));
                }

                // Visual thickness preview band
                ui.add_space(4.0);
                let (preview_rect, _) =
                    ui.allocate_exact_size(vec2(160.0, 14.0), egui::Sense::hover());
                let p = ui.painter();
                p.rect_filled(preview_rect, 2.0, Color32::from_gray(40));
                let band_h = (preview_rect.height() * thickness).clamp(3.0, preview_rect.height());
                let band_rect = Rect::from_center_size(
                    preview_rect.center(),
                    vec2(preview_rect.width() - 8.0, band_h),
                );
                p.rect_filled(
                    band_rect,
                    1.0,
                    Color32::from_rgba_unmultiplied(
                        props.highlighter_color.r,
                        props.highlighter_color.g,
                        props.highlighter_color.b,
                        200,
                    ),
                );
                ui.add_space(2.0);
            },
        );

        // Eraser tool
        let eraser_active = props.active_tool == ActiveTool::Eraser;
        let eraser_btn = render_icon_button(
            &mut left_ui,
            vec2(32.0, 32.0),
            eraser_active,
            true,
            "Eraser (click or drag across highlights or text boxes to delete)",
            |p, r, c| paint_eraser(p, r, c, eraser_active),
        );
        if eraser_btn.clicked() {
            action = Some(ToolbarAction::ToggleEraser);
        }

        left_ui.add_space(2.0);
        left_ui.separator();
        left_ui.add_space(2.0);

        // Text tool
        let text_active = props.active_tool == ActiveTool::Text;
        let text_btn = render_icon_button(
            &mut left_ui,
            vec2(32.0, 32.0),
            text_active,
            true,
            "Text tool (click on page to add or edit text boxes)",
            |p, r, c| paint_text_tool(p, r, c, text_active),
        );
        if text_btn.clicked() {
            action = Some(ToolbarAction::ToggleText);
        }

        if text_active {
            // Font size picker
            egui::ComboBox::from_id_salt("text_font_size_combo")
                .width(55.0)
                .selected_text(format!("{:.0} pt", props.text_font_size))
                .show_ui(&mut left_ui, |ui| {
                    ui.visuals_mut().selection.bg_fill =
                        Color32::from_rgba_unmultiplied(255, 255, 255, 35);
                    ui.visuals_mut().selection.stroke = Stroke::NONE;

                    for &size in &[10.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 32.0, 36.0] {
                        let is_sel = (props.text_font_size - size).abs() < 0.1;
                        let text =
                            egui::RichText::new(format!("{:.0} pt", size)).color(Color32::WHITE);
                        if ui.selectable_label(is_sel, text).clicked() {
                            action = Some(ToolbarAction::SetTextFontSize(size));
                        }
                    }
                });

            // Text color picker popup
            let text_color_popup_id = left_ui.make_persistent_id("text_color_popup");
            let cur_col =
                Color32::from_rgb(props.text_color.r, props.text_color.g, props.text_color.b);
            let (col_rect, col_resp) =
                left_ui.allocate_exact_size(vec2(18.0, 18.0), egui::Sense::click());
            let col_resp = col_resp
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Text color");
            let p = left_ui.painter();
            p.rect_filled(col_rect, CornerRadius::same(3), cur_col);
            p.rect_stroke(
                col_rect,
                CornerRadius::same(3),
                Stroke::new(1.0_f32, Color32::LIGHT_GRAY),
                StrokeKind::Outside,
            );
            if col_resp.clicked() {
                left_ui.memory_mut(|m| m.toggle_popup(text_color_popup_id));
            }

            egui::popup_below_widget(
                &left_ui,
                text_color_popup_id,
                &col_resp,
                egui::PopupCloseBehavior::CloseOnClickOutside,
                |ui| {
                    ui.label(RichText::new("Text Color").strong().size(12.0));
                    ui.horizontal(|ui| {
                        for color in crate::annotations::PRESET_TEXT_COLORS {
                            let is_sel = props.text_color == color;
                            let c = Color32::from_rgb(color.r, color.g, color.b);
                            let (r, resp) =
                                ui.allocate_exact_size(vec2(20.0, 20.0), egui::Sense::click());
                            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                            let p = ui.painter();
                            p.rect_filled(r, CornerRadius::same(3), c);
                            if is_sel {
                                p.rect_stroke(
                                    r,
                                    CornerRadius::same(3),
                                    Stroke::new(2.0_f32, Color32::WHITE),
                                    StrokeKind::Outside,
                                );
                            }
                            if resp.clicked() {
                                action = Some(ToolbarAction::SetTextColor(color));
                            }
                        }
                    });
                },
            );
        }

        left_ui.add_space(2.0);
        left_ui.separator();
        left_ui.add_space(2.0);

        // Undo & Redo controls
        let undo_btn = render_icon_button(
            &mut left_ui,
            vec2(32.0, 32.0),
            false,
            props.can_undo,
            "Undo annotation change (Ctrl+Z)",
            paint_undo,
        );
        if undo_btn.clicked() {
            action = Some(ToolbarAction::Undo);
        }

        let redo_btn = render_icon_button(
            &mut left_ui,
            vec2(32.0, 32.0),
            false,
            props.can_redo,
            "Redo annotation change (Ctrl+Shift+Z)",
            paint_redo,
        );
        if redo_btn.clicked() {
            action = Some(ToolbarAction::Redo);
        }

        let left_end_x = left_ui.min_rect().right();

        // -------------------------------------------------------------
        // Cluster 2: Right (Search, Save, Fullscreen, Preferences)
        // -------------------------------------------------------------
        let mut right_ui = ui.new_child(
            UiBuilder::new()
                .max_rect(bar_rect)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
        );
        right_ui.spacing_mut().item_spacing = vec2(4.0, 0.0);

        // Settings / Preferences button (rightmost)
        let settings_btn = render_icon_button(
            &mut right_ui,
            vec2(32.0, 32.0),
            props.settings_open,
            true,
            "Preferences",
            paint_gear,
        );
        if settings_btn.clicked() {
            action = Some(ToolbarAction::OpenSettings);
        }

        // Fullscreen toggle button
        let fs_tooltip = if props.is_fullscreen {
            "Exit fullscreen (F11 / Esc)"
        } else {
            "Enter fullscreen (F11)"
        };
        let fs_btn = render_icon_button(
            &mut right_ui,
            vec2(32.0, 32.0),
            props.is_fullscreen,
            true,
            fs_tooltip,
            |p, r, c| paint_fullscreen(p, r, c, props.is_fullscreen),
        );
        if fs_btn.clicked() {
            action = Some(ToolbarAction::ToggleFullscreen);
        }

        // Save button
        let save_tooltip = if props.has_unsaved_changes {
            "Save annotations directly into the PDF file (unsaved changes!) (Ctrl + S)"
        } else {
            "Save annotations directly into the PDF file (Ctrl + S)"
        };
        let save_btn = render_icon_button(
            &mut right_ui,
            vec2(32.0, 32.0),
            false,
            props.can_save,
            save_tooltip,
            |p, r, c| paint_save(p, r, c, props.has_unsaved_changes),
        );
        if save_btn.clicked() {
            action = Some(ToolbarAction::Save);
        }

        // Search button
        let find_btn = render_icon_button(
            &mut right_ui,
            vec2(32.0, 32.0),
            props.find_open,
            true,
            "Find in document (Ctrl + F)",
            paint_search,
        );
        if find_btn.clicked() {
            action = Some(ToolbarAction::ToggleFind);
        }

        let right_start_x = right_ui.min_rect().left();

        // -------------------------------------------------------------
        // Cluster 3: Center (Zoom, Page box [ N ] of M, Rotate)
        // -------------------------------------------------------------
        let center_w = 410.0;
        let ideal_center_min = (bar_rect.center().x - center_w / 2.0).max(left_end_x + 8.0);
        let center_max_x = (ideal_center_min + center_w).min(right_start_x - 8.0);
        let center_rect = Rect::from_min_max(
            pos2(ideal_center_min, bar_rect.top()),
            pos2(center_max_x.max(ideal_center_min + 10.0), bar_rect.bottom()),
        );

        let mut center_ui = ui.new_child(
            UiBuilder::new()
                .max_rect(center_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        center_ui.spacing_mut().item_spacing = vec2(4.0, 0.0);

        let zoom_out_btn = render_icon_button(
            &mut center_ui,
            vec2(32.0, 32.0),
            false,
            true,
            "Zoom out (Ctrl + -)",
            paint_zoom_out,
        );
        if zoom_out_btn.clicked() {
            action = Some(ToolbarAction::ZoomOut);
        }

        let zoom_percent = (props.zoom * 100.0).round();
        center_ui.add_space(1.0);
        center_ui.label(
            RichText::new(format!("{zoom_percent:.0}%"))
                .size(12.0)
                .color(Theme::TEXT_PRIMARY),
        );
        center_ui.add_space(1.0);

        let zoom_in_btn = render_icon_button(
            &mut center_ui,
            vec2(32.0, 32.0),
            false,
            true,
            "Zoom in (Ctrl + =)",
            paint_zoom_in,
        );
        if zoom_in_btn.clicked() {
            action = Some(ToolbarAction::ZoomIn);
        }

        let fit_width_btn = render_icon_button(
            &mut center_ui,
            vec2(32.0, 32.0),
            false,
            true,
            "Fit page width to window (Ctrl + 0)",
            paint_fit_width,
        );
        if fit_width_btn.clicked() {
            action = Some(ToolbarAction::FitWidth);
        }

        let fit_page_btn = render_icon_button(
            &mut center_ui,
            vec2(32.0, 32.0),
            false,
            true,
            "Fit entire page to window",
            paint_fit_page,
        );
        if fit_page_btn.clicked() {
            action = Some(ToolbarAction::FitPage);
        }

        center_ui.add_space(2.0);
        center_ui.separator();
        center_ui.add_space(2.0);

        let back_btn = render_icon_button(
            &mut center_ui,
            vec2(28.0, 28.0),
            false,
            props.can_go_back,
            "Go back (Alt+Left)",
            paint_nav_back,
        );
        if back_btn.clicked() {
            action = Some(ToolbarAction::GoBack);
        }

        let fwd_btn = render_icon_button(
            &mut center_ui,
            vec2(28.0, 28.0),
            false,
            props.can_go_forward,
            "Go forward (Alt+Right)",
            paint_nav_forward,
        );
        if fwd_btn.clicked() {
            action = Some(ToolbarAction::GoForward);
        }

        center_ui.add_space(2.0);

        // Centered Page Navigation [ N ] of M
        center_ui.label(
            RichText::new("Page")
                .size(12.5)
                .color(Theme::TEXT_SECONDARY),
        );

        let text_edit = TextEdit::singleline(&mut self.page_input_text)
            .desired_width(54.0)
            .margin(Margin::symmetric(6, 4))
            .horizontal_align(egui::Align::Center);
        let response = center_ui.add(text_edit);

        if response.gained_focus() {
            self.is_editing_page = true;
            let mut state = egui::text_edit::TextEditState::load(center_ui.ctx(), response.id)
                .unwrap_or_default();
            let len = self.page_input_text.chars().count();
            state
                .cursor
                .set_char_range(Some(egui::text_selection::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(len),
                )));
            state.store(center_ui.ctx(), response.id);
        }

        let enter_pressed = (response.has_focus() || response.lost_focus())
            && center_ui.input(|i| i.key_pressed(egui::Key::Enter));

        if enter_pressed || (response.lost_focus() && self.is_editing_page) {
            if enter_pressed {
                response.surrender_focus();
            }
            if let Ok(num) = self.page_input_text.trim().parse::<usize>() {
                if num >= 1 && num <= props.total_pages {
                    action = Some(ToolbarAction::JumpToPage(num));
                }
            }
            self.is_editing_page = false;
            self.page_input_text = props.current_page.to_string();
        }

        center_ui.label(
            RichText::new(format!("of {}", props.total_pages))
                .size(12.5)
                .color(Theme::TEXT_MUTED),
        );

        center_ui.add_space(2.0);
        center_ui.separator();
        center_ui.add_space(2.0);

        let rot_btn = render_icon_button(
            &mut center_ui,
            vec2(32.0, 32.0),
            false,
            true,
            "Rotate all pages 90° clockwise",
            paint_rotate,
        );
        if rot_btn.clicked() {
            action = Some(ToolbarAction::RotateClockwise);
        }

        ui.allocate_rect(bar_rect, egui::Sense::hover());

        action
    }
}

impl Default for Toolbar {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toolbar_new_defaults() {
        let tb = Toolbar::new();
        assert_eq!(tb.page_input_text, "1");
        assert!(!tb.is_editing_page);
    }

    #[test]
    fn test_toolbar_undo_redo_actions() {
        assert_eq!(ToolbarAction::Undo, ToolbarAction::Undo);
        assert_eq!(ToolbarAction::Redo, ToolbarAction::Redo);
        assert_ne!(ToolbarAction::Undo, ToolbarAction::Redo);
        assert_eq!(ToolbarAction::GoBack, ToolbarAction::GoBack);
        assert_eq!(ToolbarAction::GoForward, ToolbarAction::GoForward);
        assert_ne!(ToolbarAction::GoBack, ToolbarAction::GoForward);
        assert_eq!(ToolbarAction::OpenSettings, ToolbarAction::OpenSettings);
    }
}
