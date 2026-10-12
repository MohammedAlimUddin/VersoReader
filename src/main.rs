mod annotations;
mod app;
mod cache;
mod export;
mod pdf;
mod settings;
mod ui;

use app::PdfViewApp;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("pdfview")
            .with_inner_size([900.0, 650.0])
            .with_min_inner_size([400.0, 300.0]),
        ..Default::default()
    };

    eframe::run_native(
        "pdfview",
        native_options,
        Box::new(|cc| Ok(Box::new(PdfViewApp::new(cc)))),
    )
}
