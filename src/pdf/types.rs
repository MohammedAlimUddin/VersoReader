use std::path::PathBuf;

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
#[derive(Debug, Clone, Copy, PartialEq)]
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

/// Commands sent from the UI thread to the worker thread.
#[derive(Debug, Clone)]
pub enum Command {
    /// Open a PDF file from disk.
    Open { doc_id: DocId, path: PathBuf },
    /// Close an open document handle.
    #[allow(dead_code)]
    Close { doc_id: DocId },
    /// Render a requested page.
    Render(RenderCommand),
    /// Request document outline / bookmarks tree.
    Outline { doc_id: DocId },
    /// Search document text page-by-page.
    Search {
        doc_id: DocId,
        query_id: u64,
        query: String,
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
    /// Search results streamed per page.
    SearchResults {
        doc_id: DocId,
        query_id: u64,
        #[allow(dead_code)]
        page: usize,
        matches: Vec<SearchMatch>,
        done: bool,
    },
}
