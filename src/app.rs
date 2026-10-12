use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender};

use crate::cache::{TextureCache, TextureKey};
use crate::pdf::types::{DocId, PageSize, RenderCommand};
use crate::pdf::{start_worker, Command, Event};
use crate::ui::pathbar::{parse_pdf_path, PathBar};
use crate::ui::toolbar::Toolbar;
use crate::ui::viewer::{render_viewer, PageRenderRequest};

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
pub struct DocumentState {
    pub doc_id: DocId,
    #[allow(dead_code)]
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

    // Document state
    pub path_bar: PathBar,
    pub toolbar: Toolbar,
    pub current_doc_id: DocId,
    pub doc_state: Option<DocumentState>,
    pub render_generation: u64,
    pub is_loading: bool,
    pub loading_message: String,
    pub pending_open_path: Option<PathBuf>,

    // Texture cache and active requests
    pub cache: TextureCache,
    pub pending_render_requests: HashSet<(usize, u32)>,

    // Navigation and scrolling
    pub current_page: usize,
    pub pending_scroll_to_page: Option<usize>,
    pub pending_scroll_delta_y: Option<f32>,

    // Debugging (F3)
    pub debug_visible: bool,
    pub last_frame_time: Instant,
    pub frame_time_ms: f32,
}

impl PdfViewApp {
    pub fn new(cc: &eframe::CreationContext<'_>, initial_path: Option<String>) -> Self {
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
            toolbar: Toolbar::new(),
            current_doc_id: 0,
            doc_state: None,
            render_generation: 0,
            is_loading: false,
            loading_message: String::new(),
            pending_open_path,
            cache: TextureCache::default(),
            pending_render_requests: HashSet::new(),
            current_page: 1,
            pending_scroll_to_page: None,
            pending_scroll_delta_y: None,
            debug_visible: false,
            last_frame_time: Instant::now(),
            frame_time_ms: 16.0,
        }
    }

    /// Dispatches a command to open the specified PDF file.
    pub fn open_pdf(&mut self, path: PathBuf) {
        self.current_doc_id += 1;
        self.render_generation += 1;
        self.is_loading = true;
        self.loading_message = format!("Opening {}...", path.display());
        self.doc_state = None;
        self.current_page = 1;
        self.pending_scroll_to_page = Some(1);
        self.pending_render_requests.clear();

        let _ = self.tx_cmd.send(Command::Open {
            doc_id: self.current_doc_id,
            path,
        });
    }

    /// Dispatches a render request to the background worker.
    pub fn request_render(&mut self, req: PageRenderRequest) {
        if self
            .pending_render_requests
            .insert((req.page, req.width_px))
        {
            let _ = self.tx_cmd.send(Command::Render(RenderCommand {
                doc_id: self.current_doc_id,
                page: req.page,
                width_px: req.width_px,
                rotation: 0,
                generation: self.render_generation,
                priority: req.priority,
            }));
        }
    }
}

impl eframe::App for PdfViewApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Measure frame time for F3 debug stats
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_frame_time).as_secs_f32() * 1000.0;
        self.last_frame_time = now;
        self.frame_time_ms = self.frame_time_ms * 0.9 + elapsed * 0.1;

        // Toggle debug display with F3
        if ctx.input(|i| i.key_pressed(egui::Key::F3)) {
            self.debug_visible = !self.debug_visible;
        }

        // Handle keyboard scrolling when no text edit is focused
        let any_focus = ctx.memory(|m| m.focused().is_some());
        if !any_focus {
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                self.pending_scroll_delta_y = Some(-80.0);
            } else if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                self.pending_scroll_delta_y = Some(80.0);
            } else if ctx.input(|i| i.key_pressed(egui::Key::PageUp)) {
                self.pending_scroll_delta_y = Some(-500.0);
            } else if ctx
                .input(|i| i.key_pressed(egui::Key::PageDown) || i.key_pressed(egui::Key::Space))
            {
                self.pending_scroll_delta_y = Some(500.0);
            } else if ctx.input(|i| i.key_pressed(egui::Key::Home)) {
                self.pending_scroll_to_page = Some(1);
            } else if ctx.input(|i| i.key_pressed(egui::Key::End)) {
                if let Some(ref doc) = self.doc_state {
                    self.pending_scroll_to_page = Some(doc.page_count);
                }
            }
        }

        // Drain worker events non-blockingly
        while let Ok(evt) = self.rx_evt.try_recv() {
            match evt {
                Event::PdfiumReady => {
                    self.pdfium_status = PdfiumStatus::Ready;
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
                        self.is_loading = false;
                    }
                }
                Event::OpenError { doc_id, error } => {
                    if doc_id == self.current_doc_id {
                        self.is_loading = false;
                        self.path_bar.set_error(Some(error));
                    }
                }
                Event::Rendered(res) => {
                    // Drop results that arrive for a stale generation
                    if res.doc_id == self.current_doc_id && res.generation == self.render_generation
                    {
                        self.pending_render_requests
                            .remove(&(res.page, res.width_px));

                        let byte_size = (res.width_px * res.height_px * 4) as usize;
                        let color_image = egui::ColorImage::from_rgba_unmultiplied(
                            [res.width_px as usize, res.height_px as usize],
                            &res.rgba,
                        );

                        let texture_handle = ctx.load_texture(
                            format!("doc-{}-p{}-w{}", res.doc_id, res.page, res.width_px),
                            color_image,
                            egui::TextureOptions::LINEAR,
                        );

                        let key = TextureKey {
                            doc_id: res.doc_id,
                            page: res.page,
                            width_px: res.width_px,
                            rotation: 0,
                        };

                        self.cache.insert(
                            key,
                            texture_handle,
                            res.width_px,
                            res.height_px,
                            byte_size,
                        );
                    }
                }
                Event::RenderError {
                    doc_id,
                    page,
                    generation,
                    ..
                } => {
                    if doc_id == self.current_doc_id && generation == self.render_generation {
                        self.pending_render_requests.retain(|&(p, _)| p != page);
                    }
                }
            }
        }

        // Top bar: Path bar
        egui::TopBottomPanel::top("top_pathbar")
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(10, 6)),
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

        // Toolbar below path bar
        if let Some(ref doc) = self.doc_state {
            egui::TopBottomPanel::top("main_toolbar")
                .frame(
                    egui::Frame::side_top_panel(&ctx.style())
                        .inner_margin(egui::Margin::symmetric(10, 4)),
                )
                .show(ctx, |ui| {
                    if let Some(target_p) = self.toolbar.ui(ui, self.current_page, doc.page_count) {
                        self.pending_scroll_to_page = Some(target_p);
                    }
                });
        }

        // F3 Debug Bottom Bar
        if self.debug_visible {
            egui::TopBottomPanel::bottom("debug_panel")
                .frame(
                    egui::Frame::side_top_panel(&ctx.style())
                        .fill(egui::Color32::from_rgb(20, 20, 20))
                        .inner_margin(egui::Margin::symmetric(10, 4)),
                )
                .show(ctx, |ui| {
                    let cache_mb = self.cache.current_bytes() as f32 / (1024.0 * 1024.0);
                    let cap_mb = self.cache.max_bytes() as f32 / (1024.0 * 1024.0);
                    let fps = if self.frame_time_ms > 0.0 {
                        1000.0 / self.frame_time_ms
                    } else {
                        0.0
                    };

                    ui.horizontal(|ui| {
                        ui.colored_label(
                            egui::Color32::from_rgb(0, 255, 128),
                            format!(
                                "F3 Debug | Cache: {:.2} MB / {:.0} MB ({} cached pages) | Frame: {:.1} ms ({:.0} FPS)",
                                cache_mb,
                                cap_mb,
                                self.cache.len(),
                                self.frame_time_ms,
                                fps
                            ),
                        );
                    });
                });
        }

        // Main viewer central panel
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
                } else if let Some(doc) = self.doc_state.clone() {
                    let scroll_to_page = self.pending_scroll_to_page.take();
                    let scroll_delta = self.pending_scroll_delta_y.take();

                    let output = render_viewer(
                        ui,
                        doc.doc_id,
                        &doc.page_sizes,
                        &mut self.cache,
                        &self.pending_render_requests,
                        scroll_to_page,
                        scroll_delta,
                    );

                    self.current_page = output.current_page;

                    for req in output.requests {
                        self.request_render(req);
                    }
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
