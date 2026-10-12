use std::path::PathBuf;

/// Parses and validates a path entered by the user.
///
/// Accepts:
/// - Quoted paths (`"..."` or `'...'`)
/// - `~` (home directory) expansion
/// - `file://` URI prefix
/// - Surrounding whitespace trimming
///
/// Returns `Ok(PathBuf)` if file exists and is a PDF, or `Err(String)` with an inline explanation.
pub fn parse_pdf_path(input: &str) -> Result<PathBuf, String> {
    let mut trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Please enter a file path.".to_string());
    }

    // Strip surrounding quotes
    if (trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2)
        || (trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2)
    {
        trimmed = trimmed[1..trimmed.len() - 1].trim();
    }

    // Strip file:// prefix
    let path_str = if let Some(stripped) = trimmed.strip_prefix("file://") {
        stripped
    } else {
        trimmed
    };

    // Expand ~
    let path_buf = if path_str == "~" {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "Could not determine home directory".to_string())?
    } else if let Some(rest) = path_str.strip_prefix("~/") {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "Could not determine home directory".to_string())?;
        home.join(rest)
    } else {
        PathBuf::from(path_str)
    };

    if !path_buf.exists() {
        return Err(format!("File not found: {}", path_buf.display()));
    }

    if !path_buf.is_file() {
        return Err(format!(
            "Path is not a regular file: {}",
            path_buf.display()
        ));
    }

    let is_pdf = path_buf
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false);

    if !is_pdf {
        return Err("Invalid file type: expected a PDF file (.pdf extension).".to_string());
    }

    Ok(path_buf)
}

/// UI component for the document path bar.
pub struct PathBar {
    pub text: String,
    pub inline_error: Option<String>,
}

impl PathBar {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            inline_error: None,
        }
    }

    pub fn set_text(&mut self, text: String) {
        self.text = text;
    }

    pub fn set_error(&mut self, err: Option<String>) {
        self.inline_error = err;
    }

    /// Renders the path bar. Returns `Some(PathBuf)` if Enter was pressed or Open was clicked with a valid path.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<PathBuf> {
        let mut triggered_path = None;

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("PDF Path:").strong());

            let text_edit = egui::TextEdit::singleline(&mut self.text)
                .desired_width(ui.available_width() - 80.0)
                .hint_text("Enter path to PDF (e.g. ~/book.pdf, /path/to/doc.pdf)...");
            let response = ui.add(text_edit);

            let enter_pressed =
                response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let open_clicked = ui.button("Open").clicked();

            if enter_pressed || open_clicked {
                match parse_pdf_path(&self.text) {
                    Ok(path) => {
                        self.inline_error = None;
                        triggered_path = Some(path);
                    }
                    Err(err) => {
                        self.inline_error = Some(err);
                    }
                }
            }
        });

        if let Some(ref err) = self.inline_error {
            ui.add_space(2.0);
            ui.colored_label(egui::Color32::LIGHT_RED, format!("⚠ {err}"));
        }

        triggered_path
    }
}

impl Default for PathBar {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty_path() {
        assert!(parse_pdf_path("").is_err());
        assert!(parse_pdf_path("   ").is_err());
    }

    #[test]
    fn test_parse_missing_file() {
        let res = parse_pdf_path("fixtures/non_existent_file.pdf");
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("File not found"));
    }

    #[test]
    fn test_parse_non_pdf() {
        let res = parse_pdf_path("Cargo.toml");
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("expected a PDF file"));
    }

    #[test]
    fn test_parse_valid_fixture() {
        let res = parse_pdf_path("fixtures/MLE.pdf");
        assert!(res.is_ok());
    }

    #[test]
    fn test_parse_quoted_path() {
        let res_double = parse_pdf_path("\"fixtures/MLE.pdf\"");
        assert!(res_double.is_ok());

        let res_single = parse_pdf_path("'fixtures/MLE.pdf'");
        assert!(res_single.is_ok());
    }

    #[test]
    fn test_parse_file_uri() {
        if let Ok(abs) = std::fs::canonicalize("fixtures/MLE.pdf") {
            let uri = format!("file://{}", abs.display());
            let res = parse_pdf_path(&uri);
            assert!(res.is_ok());
        }
    }
}
