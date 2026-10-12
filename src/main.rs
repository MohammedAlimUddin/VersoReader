mod annotations;
mod app;
mod cache;
mod export;
mod pdf;
mod platform;
mod settings;
mod ui;

use app::PdfViewApp;

fn main() -> eframe::Result<()> {
    // Accept optional CLI argument: "pdfview path/to/file.pdf" opens that file on start
    let initial_path = std::env::args().nth(1);

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("pdfview")
            .with_inner_size([960.0, 720.0])
            .with_min_inner_size([400.0, 300.0]),
        ..Default::default()
    };

    eframe::run_native(
        "pdfview",
        native_options,
        Box::new(move |cc| Ok(Box::new(PdfViewApp::new(cc, initial_path)))),
    )
}
