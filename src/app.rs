use std::path::PathBuf;

use crossbeam_channel::{Receiver, Sender};

use crate::pdf::types::{DocId, PageSize, RenderCommand};
use crate::pdf::{start_worker, Command, Event};
use crate::ui::pathbar::{parse_pdf_path, PathBar};

/// Top-level tabs in the reader.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Tab {
    Pdf,
    Settings,
}

/// Status of the background Pdfium binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdfiumStatus {
    Initializing,
    Ready,
    Error(String),
}

/// State of the currently open document.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct DocumentState {
    pub doc_id: DocId,
    pub path: PathBuf,
    pub page_count: usize,
    pub page_sizes: Vec<PageSize>,
}

/// Main application state for pdfview.
pub struct PdfViewApp {
    pub tx_cmd: Sender<Command>,
    pub rx_evt: Receiver<Event>,
    pub pdfium_status: PdfiumStatus,
    #[allow(dead_code)]
    pub current_tab: Tab,

    // Document & Render state
    pub path_bar: PathBar,
    pub current_doc_id: DocId,
    pub doc_state: Option<DocumentState>,
    pub page_texture: Option<egui::TextureHandle>,
    pub page_texture_size: [usize; 2],
    pub render_generation: u64,
    pub is_loading: bool,
    pub loading_message: String,
    pub pending_open_path: Option<PathBuf>,
}

impl PdfViewApp {
    pub fn new(cc: &eframe::CreationContext<'_>, initial_path: Option<String>) -> Self {
        // Enforce dark UI visuals as specified in GEMINI.md
        cc.egui_ctx.set_visuals(egui::Visuals::dark());

        let (tx_cmd, rx_evt) = start_worker(cc.egui_ctx.clone());

        let mut path_bar = PathBar::new();
        let mut pending_open_path = None;

        if let Some(path_str) = initial_path {
            path_bar.set_text(path_str.clone());
            match parse_pdf_path(&path_str) {
                Ok(path) => {
                    pending_open_path = Some(path);
                }
                Err(err) => {
                    path_bar.set_error(Some(err));
                }
            }
        }

        Self {
            tx_cmd,
            rx_evt,
            pdfium_status: PdfiumStatus::Initializing,
            current_tab: Tab::Pdf,
            path_bar,
            current_doc_id: 0,
            doc_state: None,
            page_texture: None,
            page_texture_size: [0, 0],
            render_generation: 0,
            is_loading: false,
            loading_message: String::new(),
            pending_open_path,
        }
    }

    /// Dispatches a command to the worker to open the specified PDF file.
    pub fn open_pdf(&mut self, path: PathBuf) {
        self.current_doc_id += 1;
        self.render_generation += 1;
        self.is_loading = true;
        self.loading_message = format!("Opening {}...", path.display());
        self.page_texture = None;
        self.doc_state = None;

        let _ = self.tx_cmd.send(Command::Open {
            doc_id: self.current_doc_id,
            path,
        });
    }
}

impl eframe::App for PdfViewApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // UI thread must never block: drain worker events non-blockingly
        while let Ok(evt) = self.rx_evt.try_recv() {
            match evt {
                Event::PdfiumReady => {
                    self.pdfium_status = PdfiumStatus::Ready;
                    // If an initial file was queued before Pdfium finished initializing, open it now
                    if let Some(path) = self.pending_open_path.take() {
                        self.open_pdf(path);
                    }
                }
                Event::PdfiumError(err) => {
                    self.pdfium_status = PdfiumStatus::Error(err);
                    self.is_loading = false;
                }
                Event::Opened {
                    doc_id,
                    page_count,
                    page_sizes,
                } => {
                    if doc_id == self.current_doc_id {
                        let path = PathBuf::from(self.path_bar.text.clone());
                        self.doc_state = Some(DocumentState {
                            doc_id,
                            path,
                            page_count,
                            page_sizes,
                        });

                        // Request render of page 0 fit to available window width
                        self.render_generation += 1;
                        self.loading_message = "Rendering page 1...".to_string();

                        // Target width in physical pixels based on current screen width
                        let screen_width = ctx.screen_rect().width();
                        let target_width =
                            ((screen_width * ctx.pixels_per_point()).max(800.0) as u32).min(4096);

                        let _ = self.tx_cmd.send(Command::Render(RenderCommand {
                            doc_id,
                            page: 0,
                            width_px: target_width,
                            rotation: 0,
                            generation: self.render_generation,
                            priority: 1,
                        }));
                    }
                }
                Event::OpenError { doc_id, error } => {
                    if doc_id == self.current_doc_id {
                        self.is_loading = false;
                        self.path_bar.set_error(Some(error));
                    }
                }
                Event::Rendered(res) => {
                    if res.doc_id == self.current_doc_id && res.generation == self.render_generation
                    {
                        let color_image = egui::ColorImage::from_rgba_unmultiplied(
                            [res.width_px as usize, res.height_px as usize],
                            &res.rgba,
                        );
                        self.page_texture_size = [res.width_px as usize, res.height_px as usize];
                        self.page_texture = Some(ctx.load_texture(
                            format!("doc-{}-page-{}", res.doc_id, res.page),
                            color_image,
                            egui::TextureOptions::LINEAR,
                        ));
                        self.is_loading = false;
                    }
                }
                Event::RenderError {
                    doc_id,
                    generation,
                    error,
                    ..
                } => {
                    if doc_id == self.current_doc_id && generation == self.render_generation {
                        self.is_loading = false;
                        self.path_bar.set_error(Some(error));
                    }
                }
            }
        }

        // Top bar: Path bar
        egui::TopBottomPanel::top("top_pathbar")
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(10, 8)),
            )
            .show(ctx, |ui| {
                if let Some(path) = self.path_bar.ui(ui) {
                    if self.pdfium_status == PdfiumStatus::Ready {
                        self.open_pdf(path);
                    } else if self.pdfium_status == PdfiumStatus::Initializing {
                        self.pending_open_path = Some(path);
                    }
                }
            });

        // Main body
        egui::CentralPanel::default().show(ctx, |ui| match &self.pdfium_status {
            PdfiumStatus::Initializing => {
                ui.vertical_centered(|ui| {
                    ui.add_space(80.0);
                    ui.heading("Initializing Pdfium engine...");
                    ui.add_space(10.0);
                    ui.spinner();
                });
            }
            PdfiumStatus::Error(err) => {
                ui.vertical_centered(|ui| {
                    ui.add_space(60.0);
                    ui.heading(
                        egui::RichText::new("Pdfium Not Available").color(egui::Color32::LIGHT_RED),
                    );
                    ui.add_space(12.0);
                    egui::Frame::group(ui.style())
                        .fill(egui::Color32::from_rgb(30, 30, 30))
                        .show(ui, |ui| {
                            ui.set_max_width(650.0);
                            ui.label(egui::RichText::new(err).monospace());
                        });
                });
            }
            PdfiumStatus::Ready => {
                if self.is_loading {
                    ui.vertical_centered(|ui| {
                        ui.add_space(80.0);
                        ui.heading(&self.loading_message);
                        ui.add_space(10.0);
                        ui.spinner();
                    });
                } else if let Some(ref texture) = self.page_texture {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let available_width = ui.available_width().max(100.0);
                            let aspect = if self.page_texture_size[0] > 0 {
                                self.page_texture_size[1] as f32 / self.page_texture_size[0] as f32
                            } else {
                                1.414
                            };
                            let target_size = egui::vec2(available_width, available_width * aspect);

                            ui.vertical_centered(|ui| {
                                ui.add(egui::Image::new(texture).fit_to_exact_size(target_size));
                            });
                        });
                } else {
                    ui.vertical_centered(|ui| {
                        ui.add_space(100.0);
                        ui.heading("No document open");
                        ui.add_space(8.0);
                        ui.label("Enter a PDF file path in the bar above and press Open.");
                    });
                }
            }
        });
    }
}
