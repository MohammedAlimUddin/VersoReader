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
}
