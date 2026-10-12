/// Main top toolbar component containing page navigation and tool controls.
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

    /// Renders the toolbar.
    ///
    /// - `current_page`: 1-indexed current page.
    /// - `total_pages`: total pages in document.
    ///
    /// Returns `Some(target_page)` (1-indexed) if user requests jumping to a page via Enter.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        current_page: usize,
        total_pages: usize,
    ) -> Option<usize> {
        let mut jump_target = None;

        if !self.is_editing_page {
            self.page_input_text = current_page.to_string();
        }

        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Page").strong());

            let text_edit = egui::TextEdit::singleline(&mut self.page_input_text)
                .desired_width(48.0)
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
                        jump_target = Some(num);
                    }
                }
                self.is_editing_page = false;
                self.page_input_text = current_page.to_string();
            }

            ui.label(format!("of {}", total_pages));
        });

        jump_target
    }
}

impl Default for Toolbar {
    fn default() -> Self {
        Self::new()
    }
}
