//! Highlight and TextBox annotation models, hit-testing, and JSON persistence (Milestones 8-9).

use std::fs;
use std::path::{Path, PathBuf};

use directories::BaseDirs;
use egui::Pos2;
use serde::{Deserialize, Serialize};

use crate::pdf::types::CanonicalRect;

/// Color format for annotations (RGB).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct AnnotationColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl AnnotationColor {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// Six preset highlighter colors (imitating Microsoft Edge).
pub const PRESET_HIGHLIGHT_COLORS: [AnnotationColor; 6] = [
    AnnotationColor::new(255, 235, 59),  // 1. Classic Yellow
    AnnotationColor::new(105, 240, 174), // 2. Fresh Green
    AnnotationColor::new(68, 138, 255),  // 3. Sky Cyan/Blue
    AnnotationColor::new(255, 64, 129),  // 4. Vibrant Pink
    AnnotationColor::new(255, 152, 0),   // 5. Warm Orange
    AnnotationColor::new(171, 71, 188),  // 6. Lavender Purple
];

/// Preset text box colors.
pub const PRESET_TEXT_COLORS: [AnnotationColor; 6] = [
    AnnotationColor::new(0, 0, 0),      // 1. Classic Black
    AnnotationColor::new(80, 80, 80),   // 2. Dark Gray
    AnnotationColor::new(26, 82, 118),  // 3. Navy Blue
    AnnotationColor::new(192, 57, 43),  // 4. Crimson Red
    AnnotationColor::new(39, 174, 96),  // 5. Forest Green
    AnnotationColor::new(142, 68, 173), // 6. Purple
];

/// A highlight annotation over one or more text lines or a freehand ink stroke on a page.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Highlight {
    pub id: u64,
    /// 0-based page index
    pub page: usize,
    /// Canonical rectangles covering text lines (origin top-left, unrotated)
    #[serde(default)]
    pub rects: Vec<CanonicalRect>,
    /// Freehand stroke points in canonical coordinates, if freehand
    #[serde(default)]
    pub points: Vec<[f32; 2]>,
    pub color: AnnotationColor,
    pub alpha: f32,
    /// Thickness fraction of line height (0.25 to 1.0, default 1.0) or stroke width in points
    pub thickness: f32,
}

impl Highlight {
    /// True if this highlight represents a freehand stroke.
    #[allow(dead_code)]
    pub fn is_freehand(&self) -> bool {
        !self.points.is_empty()
    }

    /// Tests if a canonical point falls inside or near this highlight (for eraser hit-testing).
    pub fn contains_canonical_point(&self, pt: Pos2) -> bool {
        // 1. Text highlight hit-test: point inside any bounding rectangle
        if self
            .rects
            .iter()
            .any(|r| pt.x >= r.min_x && pt.x <= r.max_x && pt.y >= r.min_y && pt.y <= r.max_y)
        {
            return true;
        }

        // 2. Freehand stroke hit-test: distance from point to any line segment
        if self.points.len() >= 2 {
            let threshold = (self.thickness.max(4.0) * 0.5 + 4.0).max(6.0);
            for w in self.points.windows(2) {
                let ax = w[0][0];
                let ay = w[0][1];
                let bx = w[1][0];
                let by = w[1][1];
                if point_to_segment_dist(pt.x, pt.y, ax, ay, bx, by) <= threshold {
                    return true;
                }
            }
        } else if self.points.len() == 1 {
            let dx = pt.x - self.points[0][0];
            let dy = pt.y - self.points[0][1];
            if (dx * dx + dy * dy).sqrt() <= (self.thickness.max(4.0) * 0.5 + 4.0) {
                return true;
            }
        }

        false
    }

    /// Enclosing bounding rectangle in canonical points.
    #[allow(dead_code)]
    pub fn bounding_rect(&self) -> Option<CanonicalRect> {
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        let mut has_any = false;

        for r in &self.rects {
            min_x = min_x.min(r.min_x);
            min_y = min_y.min(r.min_y);
            max_x = max_x.max(r.max_x);
            max_y = max_y.max(r.max_y);
            has_any = true;
        }

        for p in &self.points {
            let pad = self.thickness.max(2.0) * 0.5;
            min_x = min_x.min(p[0] - pad);
            min_y = min_y.min(p[1] - pad);
            max_x = max_x.max(p[0] + pad);
            max_y = max_y.max(p[1] + pad);
            has_any = true;
        }

        if has_any {
            Some(CanonicalRect::new(min_x, min_y, max_x, max_y))
        } else {
            None
        }
    }
}

/// Computes the shortest distance from point (px, py) to line segment (ax, ay)-(bx, by).
pub fn point_to_segment_dist(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let dx = bx - ax;
    let dy = by - ay;
    let len_sq = dx * dx + dy * dy;
    if len_sq <= 1e-6 {
        let diff_x = px - ax;
        let diff_y = py - ay;
        return (diff_x * diff_x + diff_y * diff_y).sqrt();
    }
    let t = (((px - ax) * dx + (py - ay) * dy) / len_sq).clamp(0.0, 1.0);
    let proj_x = ax + t * dx;
    let proj_y = ay + t * dy;
    let diff_x = px - proj_x;
    let diff_y = py - proj_y;
    (diff_x * diff_x + diff_y * diff_y).sqrt()
}

static NEXT_HIGHLIGHT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Generates a globally unique identifier for a new highlight.
pub fn generate_highlight_id() -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    now.wrapping_mul(100_000)
        .wrapping_add(NEXT_HIGHLIGHT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

fn default_text_box_width() -> f32 {
    180.0
}

/// A text box annotation on a page (Milestone 9).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TextBox {
    pub id: u64,
    /// 0-based page index
    pub page: usize,
    /// Canonical position (top-left of text box in unrotated canonical points)
    pub pos: [f32; 2],
    pub text: String,
    pub font_size: f32,
    pub color: AnnotationColor,
    /// Width of text box in canonical points
    #[serde(default = "default_text_box_width")]
    pub width: f32,
}

impl TextBox {
    /// Estimates height based on font size, width, and wrapped line count.
    pub fn estimated_height(&self) -> f32 {
        let chars_per_line = (self.width / (self.font_size * 0.52)).max(1.0);
        let mut total_lines = 0.0;
        for line in self.text.lines() {
            let count = (line.chars().count() as f32 / chars_per_line)
                .ceil()
                .max(1.0);
            total_lines += count;
        }
        if total_lines == 0.0 {
            total_lines = 1.0;
        }
        let line_h = (self.font_size * 1.3).max(12.0);
        total_lines * line_h + 8.0
    }

    /// Canonical bounding box for hit-testing and rendering.
    pub fn bounding_rect(&self) -> CanonicalRect {
        let h = self.estimated_height();
        let w = self.width.max(40.0);
        CanonicalRect::new(self.pos[0], self.pos[1], self.pos[0] + w, self.pos[1] + h)
    }

    /// Tests if a canonical point falls inside the text box rectangle (with a 4pt tolerance).
    pub fn contains_canonical_point(&self, pt: Pos2) -> bool {
        let rect = self.bounding_rect();
        pt.x >= rect.min_x - 4.0
            && pt.x <= rect.max_x + 4.0
            && pt.y >= rect.min_y - 4.0
            && pt.y <= rect.max_y + 4.0
    }
}

static NEXT_TEXT_BOX_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(100_000);

/// Generates a globally unique identifier for a new text box.
pub fn generate_text_box_id() -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    now.wrapping_mul(100_000)
        .wrapping_add(NEXT_TEXT_BOX_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

/// Transient state when actively typing in an inline text box.
#[derive(Debug, Clone, PartialEq)]
pub struct EditingTextBox {
    pub id: Option<u64>,
    pub page: usize,
    pub pos: [f32; 2],
    pub text: String,
    pub font_size: f32,
    pub color: AnnotationColor,
    pub width: f32,
    pub is_new: bool,
}

impl EditingTextBox {
    /// Estimates height based on font size, width, and wrapped line count.
    pub fn estimated_height(&self) -> f32 {
        let chars_per_line = (self.width / (self.font_size * 0.52)).max(1.0);
        let mut total_lines = 0.0;
        for line in self.text.lines() {
            let count = (line.chars().count() as f32 / chars_per_line)
                .ceil()
                .max(1.0);
            total_lines += count;
        }
        if total_lines == 0.0 {
            total_lines = 1.0;
        }
        let line_h = (self.font_size * 1.3).max(12.0);
        total_lines * line_h + 8.0
    }

    pub fn new(page: usize, pos: [f32; 2], font_size: f32, color: AnnotationColor) -> Self {
        Self {
            id: None,
            page,
            pos,
            text: String::new(),
            font_size,
            color,
            width: 180.0,
            is_new: true,
        }
    }

    pub fn from_existing(tb: &TextBox) -> Self {
        Self {
            id: Some(tb.id),
            page: tb.page,
            pos: tb.pos,
            text: tb.text.clone(),
            font_size: tb.font_size,
            color: tb.color,
            width: tb.width,
            is_new: false,
        }
    }

    pub fn to_text_box(&self) -> Option<TextBox> {
        let trimmed = self.text.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(TextBox {
                id: self.id.unwrap_or_else(generate_text_box_id),
                page: self.page,
                pos: self.pos,
                text: self.text.clone(),
                font_size: self.font_size,
                color: self.color,
                width: self.width,
            })
        }
    }
}

/// Persisted document annotations schema (version 2 includes text_boxes).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DocumentAnnotations {
    pub version: u32,
    pub highlights: Vec<Highlight>,
    #[serde(default)]
    pub text_boxes: Vec<TextBox>,
}

impl Default for DocumentAnnotations {
    fn default() -> Self {
        Self {
            version: 2,
            highlights: Vec::new(),
            text_boxes: Vec::new(),
        }
    }
}

impl DocumentAnnotations {
    /// Returns the unique Blake3 hash for a file based on its canonical path and byte size.
    pub fn compute_file_key(path: &Path, file_size: u64) -> String {
        let mut hasher = blake3::Hasher::new();
        hasher.update(path.to_string_lossy().as_bytes());
        hasher.update(&file_size.to_le_bytes());
        hasher.finalize().to_hex().to_string()
    }

    /// Returns the local share path for storing annotations:
    /// `~/.local/share/pdfview/annotations/<file_key>.json`
    pub fn storage_path(file_key: &str) -> Option<PathBuf> {
        BaseDirs::new().map(|base| {
            base.data_local_dir()
                .join("pdfview")
                .join("annotations")
                .join(format!("{file_key}.json"))
        })
    }

    /// Loads annotations from disk for the given file key, returning empty annotations if not found.
    #[allow(dead_code)]
    pub fn load_for_key(file_key: &str) -> Self {
        if let Some(path) = Self::storage_path(file_key) {
            if path.exists() {
                if let Ok(content) = fs::read_to_string(&path) {
                    if let Ok(annotations) = serde_json::from_str::<DocumentAnnotations>(&content) {
                        return annotations;
                    }
                }
            }
        }
        Self::default()
    }

    /// Saves annotations to disk for the given file key.
    pub fn save_for_key(&self, file_key: &str) -> Result<(), std::io::Error> {
        if let Some(path) = Self::storage_path(file_key) {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
            fs::write(path, json)?;
        }
        Ok(())
    }
}

/// Adjusts a canonical text line rectangle vertically according to the specified thickness fraction (0.25 to 1.0).
/// The resulting band is vertically centered on the line.
pub fn adjust_rect_thickness(rect: CanonicalRect, thickness: f32) -> CanonicalRect {
    let t = thickness.clamp(0.25, 1.0);
    let h = rect.max_y - rect.min_y;
    let band_h = h * t;
    let mid_y = (rect.min_y + rect.max_y) / 2.0;
    CanonicalRect::new(
        rect.min_x,
        mid_y - band_h / 2.0,
        rect.max_x,
        mid_y + band_h / 2.0,
    )
}

/// Undoable action for annotation editing.
#[derive(Debug, Clone, PartialEq)]
pub enum AnnotationAction {
    AddHighlight(Highlight),
    DeleteHighlight(Highlight),
    AddTextBox(TextBox),
    DeleteTextBox(TextBox),
    ModifyTextBox { old: TextBox, new: TextBox },
}

/// Bounded undo/redo stack for annotations per document tab.
#[derive(Debug, Clone, Default)]
pub struct UndoStack {
    pub undo: Vec<AnnotationAction>,
    pub redo: Vec<AnnotationAction>,
}

impl UndoStack {
    const MAX_HISTORY: usize = 50;

    pub fn push_add(&mut self, highlight: Highlight) {
        self.undo.push(AnnotationAction::AddHighlight(highlight));
        if self.undo.len() > Self::MAX_HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn push_delete(&mut self, highlight: Highlight) {
        self.undo.push(AnnotationAction::DeleteHighlight(highlight));
        if self.undo.len() > Self::MAX_HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn push_add_text_box(&mut self, text_box: TextBox) {
        self.undo.push(AnnotationAction::AddTextBox(text_box));
        if self.undo.len() > Self::MAX_HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn push_delete_text_box(&mut self, text_box: TextBox) {
        self.undo.push(AnnotationAction::DeleteTextBox(text_box));
        if self.undo.len() > Self::MAX_HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn push_modify_text_box(&mut self, old: TextBox, new: TextBox) {
        self.undo.push(AnnotationAction::ModifyTextBox { old, new });
        if self.undo.len() > Self::MAX_HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn undo(&mut self, annotations: &mut DocumentAnnotations) -> bool {
        if let Some(action) = self.undo.pop() {
            match action {
                AnnotationAction::AddHighlight(hl) => {
                    // Undo an add -> remove it
                    if let Some(idx) = annotations.highlights.iter().position(|h| h.id == hl.id) {
                        let removed = annotations.highlights.remove(idx);
                        self.redo.push(AnnotationAction::AddHighlight(removed));
                        return true;
                    }
                }
                AnnotationAction::DeleteHighlight(hl) => {
                    // Undo a delete -> restore it
                    annotations.highlights.push(hl.clone());
                    self.redo.push(AnnotationAction::DeleteHighlight(hl));
                    return true;
                }
                AnnotationAction::AddTextBox(tb) => {
                    // Undo an add -> remove it
                    if let Some(idx) = annotations.text_boxes.iter().position(|t| t.id == tb.id) {
                        let removed = annotations.text_boxes.remove(idx);
                        self.redo.push(AnnotationAction::AddTextBox(removed));
                        return true;
                    }
                }
                AnnotationAction::DeleteTextBox(tb) => {
                    // Undo a delete -> restore it
                    annotations.text_boxes.push(tb.clone());
                    self.redo.push(AnnotationAction::DeleteTextBox(tb));
                    return true;
                }
                AnnotationAction::ModifyTextBox { old, new } => {
                    // Undo a modification -> restore old
                    if let Some(idx) = annotations.text_boxes.iter().position(|t| t.id == new.id) {
                        annotations.text_boxes[idx] = old.clone();
                        self.redo.push(AnnotationAction::ModifyTextBox { old, new });
                        return true;
                    }
                }
            }
        }
        false
    }

    pub fn redo(&mut self, annotations: &mut DocumentAnnotations) -> bool {
        if let Some(action) = self.redo.pop() {
            match action {
                AnnotationAction::AddHighlight(hl) => {
                    // Redo an add -> re-insert it
                    annotations.highlights.push(hl.clone());
                    self.undo.push(AnnotationAction::AddHighlight(hl));
                    return true;
                }
                AnnotationAction::DeleteHighlight(hl) => {
                    // Redo a delete -> remove it again
                    if let Some(idx) = annotations.highlights.iter().position(|h| h.id == hl.id) {
                        let removed = annotations.highlights.remove(idx);
                        self.undo.push(AnnotationAction::DeleteHighlight(removed));
                        return true;
                    }
                }
                AnnotationAction::AddTextBox(tb) => {
                    // Redo an add -> re-insert it
                    annotations.text_boxes.push(tb.clone());
                    self.undo.push(AnnotationAction::AddTextBox(tb));
                    return true;
                }
                AnnotationAction::DeleteTextBox(tb) => {
                    // Redo a delete -> remove it again
                    if let Some(idx) = annotations.text_boxes.iter().position(|t| t.id == tb.id) {
                        let removed = annotations.text_boxes.remove(idx);
                        self.undo.push(AnnotationAction::DeleteTextBox(removed));
                        return true;
                    }
                }
                AnnotationAction::ModifyTextBox { old, new } => {
                    // Redo a modification -> apply new
                    if let Some(idx) = annotations.text_boxes.iter().position(|t| t.id == old.id) {
                        annotations.text_boxes[idx] = new.clone();
                        self.undo.push(AnnotationAction::ModifyTextBox { old, new });
                        return true;
                    }
                }
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    #[test]
    fn test_thickness_rect_math() {
        let r = CanonicalRect::new(10.0, 100.0, 200.0, 120.0);
        // 100% thickness: unchanged
        let full = adjust_rect_thickness(r, 1.0);
        assert!((full.min_y - 100.0).abs() < 1e-4);
        assert!((full.max_y - 120.0).abs() < 1e-4);

        // 50% thickness: height is 10.0, centered on mid_y = 110.0 -> [105.0, 115.0]
        let half = adjust_rect_thickness(r, 0.5);
        assert!((half.min_y - 105.0).abs() < 1e-4);
        assert!((half.max_y - 115.0).abs() < 1e-4);
        assert_eq!(half.min_x, 10.0);
        assert_eq!(half.max_x, 200.0);

        // Clamped: <= 25% clamps to 25% (height 5.0 -> [107.5, 112.5])
        let quarter = adjust_rect_thickness(r, 0.1);
        assert!((quarter.min_y - 107.5).abs() < 1e-4);
        assert!((quarter.max_y - 112.5).abs() < 1e-4);
    }

    #[test]
    fn test_highlight_hit_testing() {
        let r1 = CanonicalRect::new(20.0, 50.0, 100.0, 70.0);
        let r2 = CanonicalRect::new(20.0, 75.0, 120.0, 95.0);
        let hl = Highlight {
            id: 1,
            page: 0,
            rects: vec![r1, r2],
            points: Vec::new(),
            color: PRESET_HIGHLIGHT_COLORS[0],
            alpha: 0.35,
            thickness: 1.0,
        };

        // Inside line 1
        assert!(hl.contains_canonical_point(pos2(50.0, 60.0)));
        // Inside line 2
        assert!(hl.contains_canonical_point(pos2(110.0, 85.0)));
        // Between lines
        assert!(!hl.contains_canonical_point(pos2(50.0, 72.0)));
        // Outside
        assert!(!hl.contains_canonical_point(pos2(150.0, 60.0)));
    }

    #[test]
    fn test_freehand_stroke_hit_testing() {
        let hl = Highlight {
            id: 2,
            page: 1,
            rects: Vec::new(),
            points: vec![[10.0, 10.0], [50.0, 50.0], [100.0, 50.0]],
            color: PRESET_HIGHLIGHT_COLORS[1],
            alpha: 0.35,
            thickness: 8.0,
        };

        assert!(hl.is_freehand());
        // Directly on the diagonal segment (30, 30)
        assert!(hl.contains_canonical_point(pos2(30.0, 30.0)));
        // Near the diagonal segment (32, 28)
        assert!(hl.contains_canonical_point(pos2(32.0, 28.0)));
        // Near the horizontal segment (75, 52)
        assert!(hl.contains_canonical_point(pos2(75.0, 52.0)));
        // Far away from the stroke
        assert!(!hl.contains_canonical_point(pos2(10.0, 50.0)));
    }

    #[test]
    fn test_annotations_json_serialization_roundtrip() {
        let r = CanonicalRect::new(10.0, 20.0, 100.0, 35.0);
        let hl = Highlight {
            id: 42,
            page: 3,
            rects: vec![r],
            points: Vec::new(),
            color: PRESET_HIGHLIGHT_COLORS[3],
            alpha: 0.35,
            thickness: 0.75,
        };
        let tb = TextBox {
            id: 88,
            page: 3,
            pos: [50.0, 100.0],
            text: "Hello Rust".to_string(),
            font_size: 16.0,
            color: PRESET_TEXT_COLORS[0],
            width: 180.0,
        };
        let doc_ann = DocumentAnnotations {
            version: 2,
            highlights: vec![hl],
            text_boxes: vec![tb],
        };

        let json = serde_json::to_string(&doc_ann).expect("Serialize to JSON");
        let decoded: DocumentAnnotations = serde_json::from_str(&json).expect("Deserialize JSON");
        assert_eq!(doc_ann, decoded);

        // Test backwards compatibility with v1 JSON without text_boxes
        let v1_json = r#"{"version":1,"highlights":[]}"#;
        let v1_decoded: DocumentAnnotations =
            serde_json::from_str(v1_json).expect("Deserialize v1 JSON");
        assert_eq!(v1_decoded.text_boxes, Vec::<TextBox>::new());
    }

    #[test]
    fn test_undo_redo_stack() {
        let mut ann = DocumentAnnotations::default();
        let mut undo_stack = UndoStack::default();

        let hl1 = Highlight {
            id: 101,
            page: 0,
            rects: vec![CanonicalRect::new(10.0, 10.0, 50.0, 20.0)],
            points: Vec::new(),
            color: PRESET_HIGHLIGHT_COLORS[0],
            alpha: 0.35,
            thickness: 1.0,
        };

        // 1. Add highlight
        ann.highlights.push(hl1.clone());
        undo_stack.push_add(hl1.clone());
        assert_eq!(ann.highlights.len(), 1);

        // 2. Undo add -> highlights becomes empty
        assert!(undo_stack.undo(&mut ann));
        assert_eq!(ann.highlights.len(), 0);

        // 3. Redo add -> highlights has 1
        assert!(undo_stack.redo(&mut ann));
        assert_eq!(ann.highlights.len(), 1);

        // 4. Delete highlight
        let removed = ann.highlights.pop().unwrap();
        undo_stack.push_delete(removed);
        assert_eq!(ann.highlights.len(), 0);

        // 5. Undo delete -> restored
        assert!(undo_stack.undo(&mut ann));
        assert_eq!(ann.highlights.len(), 1);
        assert_eq!(ann.highlights[0].id, 101);

        // 6. Redo delete -> removed again
        assert!(undo_stack.redo(&mut ann));
        assert_eq!(ann.highlights.len(), 0);
    }

    #[test]
    fn test_text_box_undo_redo() {
        let mut ann = DocumentAnnotations::default();
        let mut undo_stack = UndoStack::default();

        let tb = TextBox {
            id: 200,
            page: 1,
            pos: [30.0, 40.0],
            text: "Initial text".to_string(),
            font_size: 14.0,
            color: PRESET_TEXT_COLORS[0],
            width: 150.0,
        };

        // Add
        ann.text_boxes.push(tb.clone());
        undo_stack.push_add_text_box(tb.clone());
        assert_eq!(ann.text_boxes.len(), 1);

        // Undo Add
        assert!(undo_stack.undo(&mut ann));
        assert_eq!(ann.text_boxes.len(), 0);

        // Redo Add
        assert!(undo_stack.redo(&mut ann));
        assert_eq!(ann.text_boxes.len(), 1);

        // Modify (e.g. edit text or drag move)
        let mut modified = tb.clone();
        modified.pos = [60.0, 80.0];
        modified.text = "Updated text".to_string();
        ann.text_boxes[0] = modified.clone();
        undo_stack.push_modify_text_box(tb.clone(), modified.clone());

        // Undo Modify -> restored to original pos and text
        assert!(undo_stack.undo(&mut ann));
        assert_eq!(ann.text_boxes[0].pos, [30.0, 40.0]);
        assert_eq!(ann.text_boxes[0].text, "Initial text");

        // Redo Modify -> updated
        assert!(undo_stack.redo(&mut ann));
        assert_eq!(ann.text_boxes[0].pos, [60.0, 80.0]);
        assert_eq!(ann.text_boxes[0].text, "Updated text");

        // Delete
        let removed = ann.text_boxes.remove(0);
        undo_stack.push_delete_text_box(removed);
        assert_eq!(ann.text_boxes.len(), 0);

        // Undo Delete -> restored
        assert!(undo_stack.undo(&mut ann));
        assert_eq!(ann.text_boxes.len(), 1);
        assert_eq!(ann.text_boxes[0].id, 200);

        // Redo Delete -> empty again
        assert!(undo_stack.redo(&mut ann));
        assert_eq!(ann.text_boxes.len(), 0);
    }

    #[test]
    fn test_blake3_file_key_determinism() {
        let p = Path::new("/home/user/Documents/test.pdf");
        let key1 = DocumentAnnotations::compute_file_key(p, 123456);
        let key2 = DocumentAnnotations::compute_file_key(p, 123456);
        let key3 = DocumentAnnotations::compute_file_key(p, 654321);
        assert_eq!(key1, key2);
        assert_ne!(key1, key3);
        assert_eq!(key1.len(), 64);
    }

    #[test]
    fn test_text_box_resize_and_wrapped_height() {
        let tb_wide = TextBox {
            id: 201,
            page: 0,
            pos: [50.0, 50.0],
            text: "This is a long sentence that should reflow and wrap across multiple lines when width decreases".to_string(),
            font_size: 14.0,
            color: PRESET_TEXT_COLORS[0],
            width: 400.0,
        };

        let mut tb_narrow = tb_wide.clone();
        tb_narrow.width = 100.0;

        // Narrow box must have greater estimated height than wide box because of line wrapping
        let h_wide = tb_wide.estimated_height();
        let h_narrow = tb_narrow.estimated_height();
        assert!(
            h_narrow > h_wide,
            "Narrow box height ({h_narrow}) must be greater than wide box ({h_wide})"
        );

        // Width clamping
        let clamped_w = (tb_narrow.width - 80.0).max(40.0);
        assert_eq!(clamped_w, 40.0);

        // Undo/Redo resize
        let mut ann = DocumentAnnotations::default();
        let mut undo_stack = UndoStack::default();
        ann.text_boxes.push(tb_wide.clone());

        ann.text_boxes[0].width = 200.0;
        let tb_resized = ann.text_boxes[0].clone();
        undo_stack.push_modify_text_box(tb_wide.clone(), tb_resized.clone());

        assert_eq!(ann.text_boxes[0].width, 200.0);
        assert!(undo_stack.undo(&mut ann));
        assert_eq!(ann.text_boxes[0].width, 400.0);
        assert!(undo_stack.redo(&mut ann));
        assert_eq!(ann.text_boxes[0].width, 200.0);
    }
}
