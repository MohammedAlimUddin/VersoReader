use crate::annotations::{AnnotationColor, PRESET_HIGHLIGHT_COLORS};

/// Active annotation tool mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveTool {
    None,
    Highlighter,
    Eraser,
}

/// Actions emitted by user interactions in the toolbar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ToolbarAction {
    ToggleOutlineSidebar,
    ToggleFind,
    JumpToPage(usize),
    ZoomIn,
    ZoomOut,
    FitWidth,
    FitPage,
    RotateClockwise,
    ToggleFullscreen,
    ToggleHighlighter,
    ToggleEraser,
    SetHighlighterColor(AnnotationColor),
    SetHighlighterThickness(f32),
    Save,
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
    pub active_tool: ActiveTool,
    pub highlighter_color: AnnotationColor,
    pub highlighter_thickness: f32,
    pub can_save: bool,
    pub has_unsaved_changes: bool,
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
    pub fn ui(&mut self, ui: &mut egui::Ui, props: ToolbarProps) -> Option<ToolbarAction> {
        let mut action = None;

        if !self.is_editing_page {
            self.page_input_text = props.current_page.to_string();
        }

        ui.horizontal(|ui| {
            // TOC sidebar toggle button at the far left
            let sidebar_btn = ui
                .add(
                    egui::Button::new(egui::RichText::new("☰").size(14.0))
                        .selected(props.sidebar_open),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Table of contents (outline)");

            if sidebar_btn.clicked() {
                action = Some(ToolbarAction::ToggleOutlineSidebar);
            }

            // Find in document toggle button
            let find_btn = ui
                .add(
                    egui::Button::new(egui::RichText::new("🔍").size(13.0))
                        .selected(props.find_open),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Find in document (Ctrl+F)");

            if find_btn.clicked() {
                action = Some(ToolbarAction::ToggleFind);
            }

            ui.separator();

            // Page navigation
            ui.add_space(2.0);
            ui.label(egui::RichText::new("Page").strong());

            let text_edit = egui::TextEdit::singleline(&mut self.page_input_text)
                .desired_width(45.0)
                .margin(egui::Margin::symmetric(4, 2));
            let response = ui.add(text_edit);

            if response.gained_focus() {
                self.is_editing_page = true;
            }

            let enter_pressed =
                response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

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

            ui.label(format!("of {}", props.total_pages));

            ui.separator();

            // Zoom controls
            if ui
                .button("➖")
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Zoom out (Ctrl + -)")
                .clicked()
            {
                action = Some(ToolbarAction::ZoomOut);
            }

            let zoom_percent = (props.zoom * 100.0).round();
            ui.label(format!("{zoom_percent:.0}%"));

            if ui
                .button("➕")
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Zoom in (Ctrl + =)")
                .clicked()
            {
                action = Some(ToolbarAction::ZoomIn);
            }

            if ui
                .button("Fit Width")
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Fit page width to window (Ctrl + 0)")
                .clicked()
            {
                action = Some(ToolbarAction::FitWidth);
            }

            if ui
                .button("Fit Page")
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Fit entire page to window")
                .clicked()
            {
                action = Some(ToolbarAction::FitPage);
            }

            ui.separator();

            // Rotate button
            if ui
                .button("⟳ Rotate")
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Rotate all pages 90° clockwise")
                .clicked()
            {
                action = Some(ToolbarAction::RotateClockwise);
            }

            ui.separator();

            // Highlighter tool & settings dropdown
            let hl_active = props.active_tool == ActiveTool::Highlighter;
            let hl_btn = ui
                .add(
                    egui::Button::new(egui::RichText::new("🖍 Highlight").size(13.0))
                        .selected(hl_active),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Highlighter (drag over text to highlight)");
            if hl_btn.clicked() {
                action = Some(ToolbarAction::ToggleHighlighter);
            }

            let popup_id = ui.make_persistent_id("highlighter_popup_menu");
            let chevron_btn = ui
                .add(egui::Button::new(egui::RichText::new("▾").size(10.0)))
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Highlighter color and thickness");
            if chevron_btn.clicked() {
                ui.memory_mut(|m| m.toggle_popup(popup_id));
            }

            egui::popup_below_widget(
                ui,
                popup_id,
                &chevron_btn,
                egui::PopupCloseBehavior::CloseOnClickOutside,
                |ui| {
                    ui.set_min_width(170.0);
                    ui.add_space(2.0);
                    ui.label(egui::RichText::new("Color").strong().size(12.0));
                    ui.horizontal(|ui| {
                        for color in PRESET_HIGHLIGHT_COLORS {
                            let is_sel = props.highlighter_color == color;
                            let c = egui::Color32::from_rgb(color.r, color.g, color.b);
                            let (rect, resp) = ui
                                .allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
                            let p = ui.painter();
                            p.rect_filled(rect, 4.0, c);
                            if is_sel {
                                p.rect_stroke(
                                    rect,
                                    4.0,
                                    egui::Stroke::new(2.0_f32, egui::Color32::WHITE),
                                    egui::StrokeKind::Outside,
                                );
                            } else if resp.hovered() {
                                p.rect_stroke(
                                    rect,
                                    4.0,
                                    egui::Stroke::new(1.0_f32, egui::Color32::LIGHT_GRAY),
                                    egui::StrokeKind::Outside,
                                );
                            }
                            if resp.clicked() {
                                action = Some(ToolbarAction::SetHighlighterColor(color));
                            }
                        }
                    });

                    ui.add_space(8.0);
                    ui.label(egui::RichText::new("Thickness").strong().size(12.0));
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
                        ui.allocate_exact_size(egui::vec2(160.0, 14.0), egui::Sense::hover());
                    let p = ui.painter();
                    p.rect_filled(preview_rect, 2.0, egui::Color32::from_gray(40));
                    let band_h =
                        (preview_rect.height() * thickness).clamp(3.0, preview_rect.height());
                    let band_rect = egui::Rect::from_center_size(
                        preview_rect.center(),
                        egui::vec2(preview_rect.width() - 8.0, band_h),
                    );
                    p.rect_filled(
                        band_rect,
                        1.0,
                        egui::Color32::from_rgba_unmultiplied(
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
            let eraser_btn = ui
                .add(
                    egui::Button::new(egui::RichText::new("⌫ Eraser").size(13.0))
                        .selected(eraser_active),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Eraser (click or drag across highlights to delete)");
            if eraser_btn.clicked() {
                action = Some(ToolbarAction::ToggleEraser);
            }

            ui.separator();

            // Save button
            let save_label = if props.has_unsaved_changes {
                "💾 Save*"
            } else {
                "💾 Save"
            };
            let save_btn = ui
                .add_enabled(
                    props.can_save,
                    egui::Button::new(egui::RichText::new(save_label).size(13.0)),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Save annotations directly into the PDF file (Ctrl + S)");
            if save_btn.clicked() {
                action = Some(ToolbarAction::Save);
            }

            ui.separator();

            // Fullscreen toggle button
            let fs_label = if props.is_fullscreen {
                "🗗 Exit Fullscreen"
            } else {
                "⛶ Fullscreen"
            };
            if ui
                .button(fs_label)
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Toggle fullscreen (F11)")
                .clicked()
            {
                action = Some(ToolbarAction::ToggleFullscreen);
            }
        });

        action
    }
}

impl Default for Toolbar {
    fn default() -> Self {
        Self::new()
    }
}
