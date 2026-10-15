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
    // Accept optional CLI argument: "verso-reader path/to/file.pdf" opens that file on start
    let initial_path = std::env::args().nth(1);

    let icon_data = egui::IconData {
        rgba: include_bytes!("../assets/icon-256.rgba").to_vec(),
        width: 256,
        height: 256,
    };

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Verso Reader")
            .with_app_id("verso-reader")
            .with_icon(icon_data)
            .with_inner_size([1040.0, 780.0])
            .with_min_inner_size([480.0, 320.0])
            .with_decorations(false),
        ..Default::default()
    };

    eframe::run_native(
        "Verso Reader",
        native_options,
        Box::new(move |cc| Ok(Box::new(PdfViewApp::new(cc, initial_path)))),
    )
}
