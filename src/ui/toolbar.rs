/// Actions emitted by user interactions in the toolbar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ToolbarAction {
    ToggleOutlineSidebar,
    JumpToPage(usize),
    ZoomIn,
    ZoomOut,
    FitWidth,
    FitPage,
    RotateClockwise,
    ToggleFullscreen,
}

/// Main top toolbar component containing page navigation, zoom, rotation, and fullscreen controls.
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
    ///
    /// - `current_page`: 1-indexed current page.
    /// - `total_pages`: total pages in document.
    /// - `zoom`: current zoom multiplier (e.g. 1.0 = 100%).
    /// - `is_fullscreen`: whether fullscreen mode is currently active.
    /// - `sidebar_open`: whether the table of contents sidebar is currently open.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        current_page: usize,
        total_pages: usize,
        zoom: f32,
        is_fullscreen: bool,
        sidebar_open: bool,
    ) -> Option<ToolbarAction> {
        let mut action = None;

        if !self.is_editing_page {
            self.page_input_text = current_page.to_string();
        }

        ui.horizontal(|ui| {
            // TOC sidebar toggle button at the far left
            let sidebar_btn = ui
                .add(egui::Button::new(egui::RichText::new("☰").size(14.0)).selected(sidebar_open))
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Table of contents (outline)");

            if sidebar_btn.clicked() {
                action = Some(ToolbarAction::ToggleOutlineSidebar);
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
                if let Ok(num) = self.page_input_text.trim().parse::<usize>() {
                    if num >= 1 && num <= total_pages {
                        action = Some(ToolbarAction::JumpToPage(num));
                    }
                }
                self.is_editing_page = false;
                self.page_input_text = current_page.to_string();
            }

            ui.label(format!("of {total_pages}"));

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

            let zoom_percent = (zoom * 100.0).round();
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

            // Fullscreen toggle button
            let fs_label = if is_fullscreen {
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
