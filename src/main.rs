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

    #[cfg(unix)]
    {
        if let Some(ref arg) = initial_path {
            let path_to_open =
                ui::pathbar::parse_pdf_path(arg).unwrap_or_else(|_| std::path::PathBuf::from(arg));
            if platform::ipc::try_send_open(&path_to_open) {
                return Ok(());
            }
        } else if platform::ipc::try_send_focus() {
            return Ok(());
        }
    }

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
        Box::new(move |cc| {
            setup_custom_fonts(&cc.egui_ctx);
            Ok(Box::new(PdfViewApp::new(cc, initial_path)))
        }),
    )
}

/// Discovers available system fonts with Unicode and Bengali / Indic glyph coverage,
/// appending them as fallbacks to egui's default proportional and monospace font families.
pub fn setup_custom_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    let candidate_paths = [
        "/usr/share/fonts/truetype/noto/NotoSansBengali-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSansBengaliUI-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSerifBengali-Regular.ttf",
        "/usr/share/fonts/noto/NotoSansBengali-Regular.ttf",
        "/usr/share/fonts/truetype/freefont/FreeSans.ttf",
        "/usr/share/fonts/truetype/freefont/FreeSerif.ttf",
        "/usr/share/fonts/truetype/lohit-bengali/Lohit-Bengali.ttf",
    ];

    for path in candidate_paths {
        if let Ok(bytes) = std::fs::read(path) {
            fonts.font_data.insert(
                "unicode_bengali".to_string(),
                std::sync::Arc::new(egui::FontData::from_owned(bytes)),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .push("unicode_bengali".to_string());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("unicode_bengali".to_string());
            break;
        }
    }

    ctx.set_fonts(fonts);
}
