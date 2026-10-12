/// Commands sent from the UI thread to the worker thread.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum Command {
    /// Terminate the worker thread.
    Shutdown,
}

/// Events sent from the worker thread to the UI thread.
#[derive(Debug, Clone)]
pub enum Event {
    /// Pdfium dynamic library was located and initialized successfully.
    PdfiumReady,
    /// Pdfium dynamic library could not be located or loaded.
    PdfiumError(String),
}
