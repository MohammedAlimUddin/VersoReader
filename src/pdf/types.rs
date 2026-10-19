use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub type DocId = u64;

/// Canonical page dimensions in points (origin top-left, unrotated).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageSize {
    pub width: f32,
    pub height: f32,
}

/// Parameters for rendering a page to bitmap.
#[derive(Debug, Clone)]
pub struct RenderCommand {
    pub doc_id: DocId,
    pub page: usize,
    pub width_px: u32,
    #[allow(dead_code)]
    pub rotation: u16,
    pub generation: u64,
    #[allow(dead_code)]
    pub priority: u8,
}

/// Rendered page bitmap payload sent to the UI thread.
#[derive(Debug, Clone)]
pub struct RenderResult {
    pub doc_id: DocId,
    pub page: usize,
    pub width_px: u32,
    pub height_px: u32,
    pub bitmap_width_px: u32,
    pub rotation: u16,
    pub generation: u64,
    pub rgba: Vec<u8>,
}

/// An item in the document table of contents / bookmarks outline tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineItem {
    pub title: String,
    /// 1-based destination page number, if resolvable.
    pub page_number: Option<usize>,
    pub children: Vec<OutlineItem>,
}

/// An axis-aligned rectangle in canonical page space (points, origin top-left, unrotated).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CanonicalRect {
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
}

impl CanonicalRect {
    pub fn new(min_x: f32, min_y: f32, max_x: f32, max_y: f32) -> Self {
        Self {
            min_x,
            min_y,
            max_x,
            max_y,
        }
    }

    pub fn to_egui_rect(self) -> egui::Rect {
        egui::Rect::from_min_max(
            egui::pos2(self.min_x, self.min_y),
            egui::pos2(self.max_x, self.max_y),
        )
    }
}

/// A search match occurrence, spanning one or more text rectangles in canonical points.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchMatch {
    /// 1-based page number.
    pub page: usize,
    /// Bounding rectangles in canonical space.
    pub rects: Vec<CanonicalRect>,
}

/// A single character on a page with its bounding box in canonical points.
#[derive(Debug, Clone, PartialEq)]
pub struct CharBox {
    pub ch: char,
    pub rect: CanonicalRect,
    pub line_idx: usize,
}

/// A line of characters on a page.
#[derive(Debug, Clone, PartialEq)]
pub struct LineBox {
    pub rect: CanonicalRect,
    pub char_range: std::ops::Range<usize>,
}

/// Target destination of an interactive PDF link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinkTarget {
    /// 0-based page index within the same document.
    Page(usize),
    /// External Web URI (e.g. https://... or mailto:...).
    Uri(String),
}

/// An interactive hyperlink on a page with its bounding box in canonical points.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageLink {
    pub rect: CanonicalRect,
    pub target: LinkTarget,
}

/// Extracted text data and layout information for a single page.
#[derive(Debug, Clone, PartialEq)]
pub struct PageTextData {
    pub page: usize,
    pub chars: Vec<CharBox>,
    pub lines: Vec<LineBox>,
    pub links: Vec<PageLink>,
}

impl PageTextData {
    pub fn new(
        page: usize,
        chars: Vec<CharBox>,
        lines: Vec<LineBox>,
        links: Vec<PageLink>,
    ) -> Self {
        Self {
            page,
            chars,
            lines,
            links,
        }
    }

    /// Finds the link containing the given canonical point, if any.
    pub fn hit_test_link(&self, pt: egui::Pos2) -> Option<&PageLink> {
        self.links.iter().find(|link| {
            pt.x >= link.rect.min_x
                && pt.x <= link.rect.max_x
                && pt.y >= link.rect.min_y
                && pt.y <= link.rect.max_y
        })
    }

    /// Finds the character index closest to or containing the given canonical point.
    pub fn hit_test(&self, pt: egui::Pos2) -> Option<usize> {
        if self.lines.is_empty() || self.chars.is_empty() {
            return None;
        }

        let best_line_idx = self.find_best_line(pt);
        let line = &self.lines[best_line_idx];
        if line.char_range.is_empty() {
            return None;
        }

        let mut closest_char = line.char_range.start;
        let mut min_dist_x = f32::MAX;

        for i in line.char_range.clone() {
            if i >= self.chars.len() {
                break;
            }
            let c = &self.chars[i];
            if pt.x >= c.rect.min_x && pt.x <= c.rect.max_x {
                return Some(i);
            }
            let dist = if pt.x < c.rect.min_x {
                c.rect.min_x - pt.x
            } else {
                pt.x - c.rect.max_x
            };
            if dist < min_dist_x {
                min_dist_x = dist;
                closest_char = i;
            }
        }

        Some(closest_char)
    }

    /// Returns true if the pointer is within hovering distance of any text line.
    pub fn is_hovering_text(&self, pt: egui::Pos2) -> bool {
        for line in &self.lines {
            if pt.y >= line.rect.min_y - 4.0
                && pt.y <= line.rect.max_y + 4.0
                && pt.x >= line.rect.min_x - 6.0
                && pt.x <= line.rect.max_x + 6.0
            {
                return true;
            }
        }
        false
    }

    /// Merges selected characters into exactly one rectangle per line.
    pub fn get_selection_rects(&self, start_char: usize, end_char: usize) -> Vec<CanonicalRect> {
        let mut rects = Vec::new();
        if start_char > end_char || self.chars.is_empty() {
            return rects;
        }

        for line in &self.lines {
            let line_start = line.char_range.start;
            let line_end = line.char_range.end;
            if line_start >= line_end {
                continue;
            }

            let sel_start = start_char.max(line_start);
            let sel_end = end_char.min(line_end.saturating_sub(1));

            if sel_start <= sel_end && sel_start < self.chars.len() && sel_end < self.chars.len() {
                let mut min_x = f32::MAX;
                let mut max_x = f32::MIN;
                for i in sel_start..=sel_end {
                    let r = &self.chars[i].rect;
                    if r.max_x > r.min_x {
                        min_x = min_x.min(r.min_x);
                        max_x = max_x.max(r.max_x);
                    }
                }
                let min_y = line.rect.min_y;
                let max_y = line.rect.max_y;
                if max_x > min_x && max_y > min_y {
                    rects.push(CanonicalRect::new(min_x, min_y, max_x, max_y));
                }
            }
        }
        rects
    }

    /// Finds the start and end character indices for the word containing `char_idx`.
    pub fn get_word_range(&self, char_idx: usize) -> (usize, usize) {
        if char_idx >= self.chars.len() {
            return (char_idx, char_idx);
        }

        let line_idx = self.chars[char_idx].line_idx;
        let is_word_char = |c: char| c.is_alphanumeric() || c == '_';
        let target_is_word = is_word_char(self.chars[char_idx].ch);

        let mut start = char_idx;
        while start > 0 && self.chars[start - 1].line_idx == line_idx {
            let prev = self.chars[start - 1].ch;
            if is_word_char(prev) == target_is_word && !prev.is_whitespace() {
                start -= 1;
            } else {
                break;
            }
        }

        let mut end = char_idx;
        while end + 1 < self.chars.len() && self.chars[end + 1].line_idx == line_idx {
            let next = self.chars[end + 1].ch;
            if is_word_char(next) == target_is_word && !next.is_whitespace() {
                end += 1;
            } else {
                break;
            }
        }

        (start, end)
    }

    /// Extracts selected text characters with sensible line breaks.
    pub fn extract_text_range(&self, start_char: usize, end_char: usize) -> String {
        if start_char > end_char || self.chars.is_empty() {
            return String::new();
        }
        let sel_start = start_char.min(self.chars.len().saturating_sub(1));
        let sel_end = end_char.min(self.chars.len().saturating_sub(1));
        let mut text = String::new();
        let mut last_line_idx = None;

        for i in sel_start..=sel_end {
            let c = &self.chars[i];
            if let Some(last_line) = last_line_idx {
                if c.line_idx != last_line && !text.ends_with('\n') {
                    text.push('\n');
                }
            }
            if c.ch != '\r' {
                text.push(c.ch);
            }
            last_line_idx = Some(c.line_idx);
        }
        text
    }

    fn find_best_line(&self, pt: egui::Pos2) -> usize {
        // 1. Check for lines that vertically cover pt.y
        let mut candidates = Vec::new();
        for (i, line) in self.lines.iter().enumerate() {
            if pt.y >= line.rect.min_y - 2.0 && pt.y <= line.rect.max_y + 2.0 {
                candidates.push(i);
            }
        }

        // If candidates found, pick the one closest to pt.x horizontally (column awareness)
        if !candidates.is_empty() {
            let mut best_c = candidates[0];
            let mut min_dist_x = f32::MAX;
            for &idx in &candidates {
                let line = &self.lines[idx];
                let dist_x = if pt.x >= line.rect.min_x && pt.x <= line.rect.max_x {
                    0.0
                } else if pt.x < line.rect.min_x {
                    line.rect.min_x - pt.x
                } else {
                    pt.x - line.rect.max_x
                };
                if dist_x < min_dist_x {
                    min_dist_x = dist_x;
                    best_c = idx;
                }
            }
            return best_c;
        }

        // 2. Fallback: find line with minimum weighted distance to center
        let mut best_idx = 0;
        let mut min_score = f32::MAX;
        for (i, line) in self.lines.iter().enumerate() {
            let center_y = (line.rect.min_y + line.rect.max_y) * 0.5;
            let dist_y = (center_y - pt.y).abs();
            let dist_x = if pt.x >= line.rect.min_x && pt.x <= line.rect.max_x {
                0.0
            } else if pt.x < line.rect.min_x {
                line.rect.min_x - pt.x
            } else {
                pt.x - line.rect.max_x
            };
            let score = dist_y * 3.0 + dist_x;
            if score < min_score {
                min_score = score;
                best_idx = i;
            }
        }
        best_idx
    }
}

/// A specific character position on a specific document page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextLocation {
    pub page: usize,
    pub char_idx: usize,
}

impl TextLocation {
    pub fn new(page: usize, char_idx: usize) -> Self {
        Self { page, char_idx }
    }
}

/// Active text selection range spanning from anchor to focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextSelection {
    pub anchor: TextLocation,
    pub focus: TextLocation,
}

impl TextSelection {
    pub fn new(anchor: TextLocation, focus: TextLocation) -> Self {
        Self { anchor, focus }
    }

    /// Normalizes selection into `(start, end)` in document reading order.
    pub fn normalized(&self) -> (TextLocation, TextLocation) {
        if self.anchor <= self.focus {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.focus
    }
}

/// Commands sent from the UI thread to the worker thread.
#[derive(Debug, Clone)]
pub enum Command {
    /// Open a PDF file from disk.
    Open { doc_id: DocId, path: PathBuf },
    /// Close an open document handle.
    #[allow(dead_code)]
    Close { doc_id: DocId },
    /// Unload an inactive document from worker memory (dormant tab deactivation).
    UnloadDoc { doc_id: DocId },
    /// Render a requested page.
    Render(RenderCommand),
    /// Request document outline / bookmarks tree.
    Outline { doc_id: DocId },
    /// Request extracted text and character bounding boxes for a page.
    PageText { doc_id: DocId, page: usize },
    /// Extract text directly for a page/char range for clipboard copy.
    ExtractTextForCopy {
        doc_id: DocId,
        start_page: usize,
        start_char: usize,
        end_page: usize,
        end_char: usize,
    },
    /// Search document text page-by-page.
    Search {
        doc_id: DocId,
        query_id: u64,
        query: String,
    },
    /// Synchronize all annotations on the document for native PDFium rendering.
    SyncAnnotations {
        doc_id: DocId,
        highlights: Vec<crate::annotations::Highlight>,
        #[allow(dead_code)]
        text_boxes: Vec<crate::annotations::TextBox>,
    },
    /// Save the document with embedded annotations directly to the PDF file on disk.
    SaveDoc {
        doc_id: DocId,
        path: PathBuf,
        highlights: Vec<crate::annotations::Highlight>,
        text_boxes: Vec<crate::annotations::TextBox>,
    },
    /// Terminate the worker thread.
    #[allow(dead_code)]
    Shutdown,
}

/// Events sent from the worker thread to the UI thread.
#[derive(Debug, Clone)]
pub enum Event {
    /// Pdfium dynamic library was located and initialized successfully.
    PdfiumReady,
    /// Pdfium dynamic library could not be located or loaded.
    PdfiumError(String),
    /// Document was opened successfully with its page count and canonical sizes.
    Opened {
        doc_id: DocId,
        page_count: usize,
        page_sizes: Vec<PageSize>,
        annotations: Vec<crate::annotations::Highlight>,
        text_boxes: Vec<crate::annotations::TextBox>,
    },
    /// Opening a document failed.
    OpenError { doc_id: DocId, error: String },
    /// Page rendering succeeded.
    Rendered(RenderResult),
    /// Page rendering failed.
    #[allow(dead_code)]
    RenderError {
        doc_id: DocId,
        page: usize,
        generation: u64,
        error: String,
    },
    /// Document outline / bookmarks result.
    OutlineResult {
        doc_id: DocId,
        items: Vec<OutlineItem>,
    },
    /// Background extracted annotations from document pages.
    AnnotationsExtracted {
        doc_id: DocId,
        annotations: Vec<crate::annotations::Highlight>,
        text_boxes: Vec<crate::annotations::TextBox>,
    },
    /// Search results streamed per page.
    SearchResults {
        doc_id: DocId,
        query_id: u64,
        #[allow(dead_code)]
        page: usize,
        matches: Vec<SearchMatch>,
        done: bool,
    },
    /// Extracted page text and character layout result.
    PageTextResult {
        doc_id: DocId,
        page: usize,
        data: std::sync::Arc<PageTextData>,
    },
    /// Result of an on-demand text extraction for clipboard copy.
    CopyTextResult { text: String },
    /// Document save finished.
    Saved {
        doc_id: DocId,
        success: bool,
        error: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_location_and_selection_ordering() {
        let loc1 = TextLocation::new(0, 10);
        let loc2 = TextLocation::new(0, 25);
        let loc3 = TextLocation::new(1, 2);

        assert!(loc1 < loc2);
        assert!(loc2 < loc3);
        assert!(loc1 < loc3);

        // Forward selection
        let sel_fwd = TextSelection::new(loc1, loc2);
        assert!(!sel_fwd.is_empty());
        let (start, end) = sel_fwd.normalized();
        assert_eq!(start, loc1);
        assert_eq!(end, loc2);

        // Reverse selection
        let sel_rev = TextSelection::new(loc3, loc1);
        assert!(!sel_rev.is_empty());
        let (start, end) = sel_rev.normalized();
        assert_eq!(start, loc1);
        assert_eq!(end, loc3);

        // Empty selection
        let sel_empty = TextSelection::new(loc1, loc1);
        assert!(sel_empty.is_empty());
    }

    #[test]
    fn test_page_text_hit_test_and_hover() {
        let chars = vec![
            CharBox {
                ch: 'H',
                rect: CanonicalRect::new(10.0, 10.0, 18.0, 24.0),
                line_idx: 0,
            },
            CharBox {
                ch: 'i',
                rect: CanonicalRect::new(18.0, 10.0, 24.0, 24.0),
                line_idx: 0,
            },
        ];
        let lines = vec![LineBox {
            rect: CanonicalRect::new(10.0, 10.0, 24.0, 24.0),
            char_range: 0..2,
        }];
        let data = PageTextData::new(0, chars, lines, Vec::new());

        // Hover
        assert!(data.is_hovering_text(egui::pos2(12.0, 15.0)));
        assert!(!data.is_hovering_text(egui::pos2(100.0, 100.0)));

        // Hit test
        assert_eq!(data.hit_test(egui::pos2(14.0, 15.0)), Some(0)); // 'H'
        assert_eq!(data.hit_test(egui::pos2(20.0, 15.0)), Some(1)); // 'i'
    }

    #[test]
    fn test_page_text_two_column_hit_test() {
        // Line 0 in Column 1 (x: 50..150, y: 100..120)
        // Line 1 in Column 2 (x: 300..400, y: 100..120)
        let chars = vec![
            CharBox {
                ch: 'A',
                rect: CanonicalRect::new(50.0, 100.0, 100.0, 120.0),
                line_idx: 0,
            },
            CharBox {
                ch: 'B',
                rect: CanonicalRect::new(100.0, 100.0, 150.0, 120.0),
                line_idx: 0,
            },
            CharBox {
                ch: 'X',
                rect: CanonicalRect::new(300.0, 100.0, 350.0, 120.0),
                line_idx: 1,
            },
            CharBox {
                ch: 'Y',
                rect: CanonicalRect::new(350.0, 100.0, 400.0, 120.0),
                line_idx: 1,
            },
        ];
        let lines = vec![
            LineBox {
                rect: CanonicalRect::new(50.0, 100.0, 150.0, 120.0),
                char_range: 0..2,
            },
            LineBox {
                rect: CanonicalRect::new(300.0, 100.0, 400.0, 120.0),
                char_range: 2..4,
            },
        ];
        let data = PageTextData::new(0, chars, lines, Vec::new());

        // Clicking in Column 1 at y = 110 selects Column 1 char
        assert_eq!(data.hit_test(egui::pos2(60.0, 110.0)), Some(0));
        assert_eq!(data.hit_test(egui::pos2(120.0, 110.0)), Some(1));

        // Clicking in Column 2 at same y = 110 selects Column 2 char
        assert_eq!(data.hit_test(egui::pos2(320.0, 110.0)), Some(2));
        assert_eq!(data.hit_test(egui::pos2(380.0, 110.0)), Some(3));
    }

    #[test]
    fn test_page_text_selection_rects_and_word_range() {
        let chars = vec![
            CharBox {
                ch: 'H',
                rect: CanonicalRect::new(10.0, 10.0, 20.0, 25.0),
                line_idx: 0,
            },
            CharBox {
                ch: 'e',
                rect: CanonicalRect::new(20.0, 10.0, 30.0, 25.0),
                line_idx: 0,
            },
            CharBox {
                ch: 'l',
                rect: CanonicalRect::new(30.0, 10.0, 40.0, 25.0),
                line_idx: 0,
            },
            CharBox {
                ch: 'l',
                rect: CanonicalRect::new(40.0, 10.0, 50.0, 25.0),
                line_idx: 0,
            },
            CharBox {
                ch: 'o',
                rect: CanonicalRect::new(50.0, 10.0, 60.0, 25.0),
                line_idx: 0,
            },
            CharBox {
                ch: ' ',
                rect: CanonicalRect::new(60.0, 10.0, 70.0, 25.0),
                line_idx: 0,
            },
            CharBox {
                ch: 'W',
                rect: CanonicalRect::new(70.0, 10.0, 80.0, 25.0),
                line_idx: 0,
            },
            CharBox {
                ch: 'b',
                rect: CanonicalRect::new(10.0, 30.0, 20.0, 45.0),
                line_idx: 1,
            },
            CharBox {
                ch: 'y',
                rect: CanonicalRect::new(20.0, 30.0, 30.0, 45.0),
                line_idx: 1,
            },
            CharBox {
                ch: 'e',
                rect: CanonicalRect::new(30.0, 30.0, 40.0, 45.0),
                line_idx: 1,
            },
        ];
        let lines = vec![
            LineBox {
                rect: CanonicalRect::new(10.0, 10.0, 80.0, 25.0),
                char_range: 0..7,
            },
            LineBox {
                rect: CanonicalRect::new(10.0, 30.0, 40.0, 45.0),
                char_range: 7..10,
            },
        ];
        let data = PageTextData::new(0, chars, lines, Vec::new());

        // Word range for 'Hello' (char index 2 is 'l')
        let (w_start, w_end) = data.get_word_range(2);
        assert_eq!((w_start, w_end), (0, 4));

        // Selection rects: select across lines (char 2 to char 8)
        let rects = data.get_selection_rects(2, 8);
        assert_eq!(rects.len(), 2); // One rectangle per line
        assert_eq!(rects[0], CanonicalRect::new(30.0, 10.0, 80.0, 25.0)); // line 0
        assert_eq!(rects[1], CanonicalRect::new(10.0, 30.0, 30.0, 45.0)); // line 1

        // Extract text range with sensible newline
        let text = data.extract_text_range(0, 9);
        assert_eq!(text, "Hello W\nbye");
    }

    #[test]
    fn test_page_link_hit_test() {
        let links = vec![
            PageLink {
                rect: CanonicalRect::new(50.0, 100.0, 150.0, 120.0),
                target: LinkTarget::Page(4),
            },
            PageLink {
                rect: CanonicalRect::new(200.0, 300.0, 350.0, 320.0),
                target: LinkTarget::Uri("https://example.com/doc.pdf".to_string()),
            },
        ];
        let data = PageTextData::new(0, Vec::new(), Vec::new(), links);

        // Internal page link hit test
        let hit1 = data.hit_test_link(egui::pos2(75.0, 110.0));
        assert!(hit1.is_some());
        assert_eq!(hit1.unwrap().target, LinkTarget::Page(4));

        // External URI link hit test
        let hit2 = data.hit_test_link(egui::pos2(250.0, 310.0));
        assert!(hit2.is_some());
        assert_eq!(
            hit2.unwrap().target,
            LinkTarget::Uri("https://example.com/doc.pdf".to_string())
        );

        // Miss
        assert!(data.hit_test_link(egui::pos2(10.0, 10.0)).is_none());
    }
}
