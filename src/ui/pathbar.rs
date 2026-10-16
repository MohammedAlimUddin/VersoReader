use std::path::PathBuf;

use egui::{Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

use crate::ui::theme::Theme;

/// Helper to percent-decode a URI string without external dependencies.
fn percent_decode_str(input: &str) -> String {
    let mut bytes = Vec::with_capacity(input.len());
    let input_bytes = input.as_bytes();
    let mut i = 0;
    while i < input_bytes.len() {
        if input_bytes[i] == b'%' && i + 2 < input_bytes.len() {
            if let Ok(val) = u8::from_str_radix(
                std::str::from_utf8(&input_bytes[i + 1..=i + 2]).unwrap_or(""),
                16,
            ) {
                bytes.push(val);
                i += 3;
                continue;
            }
        }
        bytes.push(input_bytes[i]);
        i += 1;
    }
    String::from_utf8(bytes).unwrap_or_else(|_| input.to_string())
}

/// Helper to strip leading and trailing quotes (including Unicode smart / curly quotes).
fn strip_quotes(s: &str) -> &str {
    let mut res = s.trim();
    let quote_pairs: &[(&str, &str)] = &[
        ("\"", "\""),
        ("'", "'"),
        ("“", "”"),
        ("”", "”"),
        ("‘", "’"),
        ("’", "’"),
    ];
    for (start, end) in quote_pairs {
        if res.starts_with(start) && res.ends_with(end) && res.len() >= start.len() + end.len() {
            res = &res[start.len()..res.len() - end.len()];
            return res.trim();
        }
    }
    res
}

/// Helper to unescape bash-escaped characters (e.g. `\ `, `\(`, `\)`, `\'`, `\"`, `\&`, `\[`, `\]`).
fn unescape_bash_str(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&next_c) = chars.peek() {
                if " ()[]{}'\"&;!$#*?`".contains(next_c) {
                    chars.next();
                    out.push(next_c);
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

/// Parses and validates a path entered by the user.
///
/// Accepts:
/// - Standard quotes (`"..."`, `'...'`) and smart/curly quotes (`“...”`, `‘...’`)
/// - Escaped spaces (`\ `) and bash shell escaped characters
/// - `file://` and `file://localhost` URI prefixes
/// - Percent-encoded URIs (`%20` for spaces, etc.)
/// - `~` (home directory) expansion
/// - Surrounding whitespace trimming
///
/// Returns `Ok(PathBuf)` if file exists and is a readable PDF, or `Err(String)` with an inline explanation.
pub fn parse_pdf_path(input: &str) -> Result<PathBuf, String> {
    let mut cleaned = input.trim().to_string();
    if cleaned.is_empty() {
        return Err("Please enter a file path.".to_string());
    }

    // Strip smart quotes or standard quotes
    cleaned = strip_quotes(&cleaned).to_string();

    // Strip file://localhost or file:// prefix
    if let Some(stripped) = cleaned.strip_prefix("file://localhost") {
        cleaned = stripped.to_string();
    } else if let Some(stripped) = cleaned.strip_prefix("file://") {
        cleaned = stripped.to_string();
    }

    // Strip quotes again if they were inside file://
    cleaned = strip_quotes(&cleaned).to_string();

    // Decode URL percent-encoding (e.g. %20 -> space)
    if cleaned.contains('%') {
        cleaned = percent_decode_str(&cleaned);
    }

    // Unescape bash-escaped characters (`\ ` -> ` `, etc.)
    cleaned = unescape_bash_str(&cleaned);

    let final_str = strip_quotes(&cleaned);

    // Expand ~
    let path_buf = if final_str == "~" {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "Could not determine home directory".to_string())?
    } else if let Some(rest) = final_str.strip_prefix("~/") {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "Could not determine home directory".to_string())?;
        home.join(rest)
    } else {
        PathBuf::from(final_str)
    };

    if !path_buf.exists() {
        return Err(format!("File not found: {}", path_buf.display()));
    }

    if !path_buf.is_file() {
        return Err(format!(
            "Path is a directory or not a regular file: {}",
            path_buf.display()
        ));
    }

    // Verify accessibility / readability permissions
    if let Err(e) = std::fs::File::open(&path_buf) {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            return Err(format!(
                "Permission denied: cannot read file: {}",
                path_buf.display()
            ));
        }
        return Err(format!("Cannot open file ({}): {}", e, path_buf.display()));
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

/// Action emitted by user interaction with the PathBar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathBarAction {
    OpenPath(PathBuf),
    LaunchFileChooser,
}

/// Draws a crisp vector folder/location badge in the path bar.
fn paint_path_badge(ui: &mut egui::Ui, rect: Rect) {
    let p = ui.painter();
    let center = rect.center();
    let box_rect = Rect::from_center_size(center, Vec2::new(15.0, 12.0));
    let stroke = Stroke::new(1.1_f32, Theme::TEXT_SECONDARY);

    // Folder tab
    let tab_rect = Rect::from_min_size(
        Pos2::new(box_rect.left(), box_rect.top() - 2.5),
        Vec2::new(6.0, 3.5),
    );
    p.rect_filled(tab_rect, CornerRadius::same(1), Theme::BG_PANEL_HOVER);
    p.rect_stroke(tab_rect, CornerRadius::same(1), stroke, StrokeKind::Inside);

    // Folder body
    p.rect_filled(box_rect, CornerRadius::same(2), Theme::BG_PANEL);
    p.rect_stroke(box_rect, CornerRadius::same(2), stroke, StrokeKind::Inside);
}

/// UI component for the document path bar.
pub struct PathBar {
    pub text: String,
    pub inline_error: Option<String>,
    pub is_collapsed: bool,
    pub request_focus: bool,
}

impl PathBar {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            inline_error: None,
            is_collapsed: false,
            request_focus: false,
        }
    }

    pub fn set_text(&mut self, text: String) {
        self.text = text;
    }

    pub fn set_error(&mut self, err: Option<String>) {
        if err.is_some() {
            self.is_collapsed = false;
        }
        self.inline_error = err;
    }

    pub fn expand_and_focus(&mut self) {
        self.is_collapsed = false;
        self.request_focus = true;
    }

    /// Renders the path bar. Returns `Some(PathBarAction)` if user triggered an open action.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> Option<PathBarAction> {
        if self.is_collapsed {
            return None;
        }

        let mut action = None;

        ui.set_height(28.0);
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 0.0);

            // Document location icon (clickable to launch file chooser)
            let (icon_rect, icon_resp) =
                ui.allocate_exact_size(Vec2::new(20.0, 20.0), Sense::click());
            let icon_resp = icon_resp
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text("Browse files...");
            paint_path_badge(ui, icon_rect);
            if icon_resp.clicked() {
                action = Some(PathBarAction::LaunchFileChooser);
            }

            // Calculate width for textedit: total available minus Open button (~80px) and optional clear button
            let open_btn_width = 80.0;
            let clear_btn_width = if !self.text.is_empty() { 24.0 } else { 0.0 };
            let edit_width =
                (ui.available_width() - open_btn_width - clear_btn_width - 8.0).max(120.0);

            let text_edit = egui::TextEdit::singleline(&mut self.text)
                .desired_width(edit_width)
                .margin(egui::Margin::symmetric(8, 4))
                .hint_text("Enter or paste PDF path (e.g. ~/document.pdf)...");
            let response = ui.add(text_edit);

            if response.changed() && self.inline_error.is_some() {
                self.inline_error = None;
            }

            if self.inline_error.is_some() {
                ui.painter().rect_stroke(
                    response.rect,
                    Theme::RADIUS,
                    Stroke::new(1.5_f32, Color32::from_rgb(232, 17, 35)),
                    StrokeKind::Outside,
                );
            }

            if self.request_focus {
                response.request_focus();
                self.request_focus = false;
            }

            // Quick clear button (vector cross)
            if !self.text.is_empty() {
                let (c_rect, c_resp) =
                    ui.allocate_exact_size(Vec2::new(20.0, 20.0), Sense::click());
                let c_resp = c_resp
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("Clear path");
                if c_resp.hovered() {
                    ui.painter()
                        .rect_filled(c_rect, Theme::RADIUS, Theme::BG_PANEL_HOVER);
                }
                let cross_col = if c_resp.hovered() {
                    Color32::WHITE
                } else {
                    Theme::TEXT_MUTED
                };
                crate::ui::icons::paint_close(ui.painter(), c_rect, cross_col);

                if c_resp.clicked() {
                    self.text.clear();
                    self.inline_error = None;
                    ui.ctx().request_repaint();
                }
            }

            // Open button with vector folder icon: opens entered path if present, or opens file chooser if empty
            let (open_rect, open_resp) =
                ui.allocate_exact_size(Vec2::new(76.0, 26.0), Sense::click());
            let open_tooltip = if self.text.trim().is_empty() {
                "Open file chooser (browse for PDF files)"
            } else {
                "Open entered PDF file"
            };
            let open_resp = open_resp
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(open_tooltip);
            let open_bg = if open_resp.hovered() {
                Theme::ACCENT_HOVER
            } else {
                Theme::ACCENT_PRIMARY
            };
            let p = ui.painter();
            p.rect_filled(open_rect, Theme::RADIUS, open_bg);
            let icon_rect = Rect::from_center_size(
                Pos2::new(open_rect.left() + 16.0, open_rect.center().y),
                Vec2::new(18.0, 18.0),
            );
            crate::ui::icons::paint_folder_open(p, icon_rect, Color32::WHITE);
            p.text(
                Pos2::new(open_rect.left() + 32.0, open_rect.center().y),
                egui::Align2::LEFT_CENTER,
                "Open",
                egui::FontId::proportional(12.5),
                Color32::WHITE,
            );

            if open_resp.clicked() {
                if self.text.trim().is_empty() {
                    action = Some(PathBarAction::LaunchFileChooser);
                } else {
                    match parse_pdf_path(&self.text) {
                        Ok(path) => {
                            self.inline_error = None;
                            action = Some(PathBarAction::OpenPath(path));
                            ui.ctx().request_repaint();
                        }
                        Err(err) => {
                            self.inline_error = Some(err);
                            ui.ctx().request_repaint();
                        }
                    }
                }
            }

            let enter_pressed = (response.has_focus() || response.lost_focus())
                && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if enter_pressed {
                match parse_pdf_path(&self.text) {
                    Ok(path) => {
                        self.inline_error = None;
                        response.surrender_focus();
                        action = Some(PathBarAction::OpenPath(path));
                        ui.ctx().request_repaint();
                    }
                    Err(err) => {
                        self.inline_error = Some(err);
                        response.request_focus();
                        ui.ctx().request_repaint();
                    }
                }
            }
        });

        if let Some(ref err) = self.inline_error {
            ui.add_space(3.0);
            ui.horizontal(|ui| {
                ui.add_space(26.0);
                ui.label(
                    egui::RichText::new(format!("⚠ {err}"))
                        .size(12.0)
                        .color(Color32::from_rgb(255, 110, 110))
                        .strong(),
                );
            });
            ui.add_space(2.0);
        }

        action
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
    fn test_parse_directory_fails() {
        let res = parse_pdf_path("fixtures");
        assert!(res.is_err());
        assert!(res.unwrap_err().contains("directory or not a regular file"));
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
    fn test_parse_path_with_spaces() {
        let res =
            parse_pdf_path("fixtures/Modern Operating Systems 4th Edition--Andrew Tanenbaum.pdf");
        assert!(res.is_ok(), "Must parse path containing spaces: {res:?}");
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

    #[test]
    fn test_pathbar_collapse_expand() {
        let mut pb = PathBar::new();
        assert!(!pb.is_collapsed);
        assert!(!pb.request_focus);

        pb.is_collapsed = true;
        assert!(pb.is_collapsed);

        pb.expand_and_focus();
        assert!(!pb.is_collapsed);
        assert!(pb.request_focus);
    }

    #[test]
    fn test_parse_escaped_spaces() {
        let res = parse_pdf_path(
            "fixtures/Modern\\ Operating\\ Systems\\ 4th\\ Edition--Andrew\\ Tanenbaum.pdf",
        );
        assert!(
            res.is_ok(),
            "Must parse path with bash-escaped spaces: {res:?}"
        );
    }

    #[test]
    fn test_parse_percent_encoded_uri() {
        if let Ok(abs) = std::fs::canonicalize(
            "fixtures/Modern Operating Systems 4th Edition--Andrew Tanenbaum.pdf",
        ) {
            let uri = format!("file://{}", abs.display()).replace(' ', "%20");
            let res = parse_pdf_path(&uri);
            assert!(res.is_ok(), "Must parse percent-encoded URI: {res:?}");
        }
    }

    #[test]
    fn test_parse_smart_curly_quotes() {
        let res = parse_pdf_path("“fixtures/MLE.pdf”");
        assert!(res.is_ok(), "Must strip smart quotes: {res:?}");
    }

    #[test]
    fn test_unescape_bash_str() {
        assert_eq!(
            unescape_bash_str(r"file\ with\ spaces.pdf"),
            "file with spaces.pdf"
        );
        assert_eq!(unescape_bash_str(r"doc\(1\)\[v2\].pdf"), "doc(1)[v2].pdf");
        assert_eq!(
            unescape_bash_str(r"folder\&files/test\'s.pdf"),
            "folder&files/test's.pdf"
        );
    }
}
