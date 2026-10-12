use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use crossbeam_channel::{Receiver, Sender};

use crate::cache::{TextureCache, TextureKey};
use crate::pdf::types::{DocId, PageSize, RenderCommand};
use crate::pdf::{start_worker, Command, Event, PageTransform};
use crate::ui::pathbar::{parse_pdf_path, PathBar};
use crate::ui::toolbar::{Toolbar, ToolbarAction};
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

    // View projections
    pub zoom: f32,
    pub rotation: u16,
    pub is_fullscreen: bool,

    // Texture cache and active requests
    pub cache: TextureCache,
    pub pending_render_requests: HashSet<(usize, u32, u16)>,

    // Navigation and scrolling
    pub current_page: usize,
    pub pending_scroll_to_page: Option<usize>,
    pub pending_scroll_delta_y: Option<f32>,
    pub pending_set_scroll_offset_y: Option<f32>,
    pub last_viewport_height: f32,
    pub last_scroll_offset_y: f32,

    // Debugging (F3)
    pub debug_visible: bool,
    pub last_frame_time: Instant,
    pub frame_time_ms: f32,

    #[cfg(target_os = "linux")]
    pub rx_pinch_gesture: Option<crossbeam_channel::Receiver<f64>>,
    #[cfg(target_os = "linux")]
    pub wayland_gesture_init_attempted: bool,
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
            zoom: 1.0,
            rotation: 0,
            is_fullscreen: false,
            cache: TextureCache::default(),
            pending_render_requests: HashSet::new(),
            current_page: 1,
            pending_scroll_to_page: None,
            pending_scroll_delta_y: None,
            pending_set_scroll_offset_y: None,
            last_viewport_height: 600.0,
            last_scroll_offset_y: 0.0,
            debug_visible: false,
            last_frame_time: Instant::now(),
            frame_time_ms: 16.0,
            #[cfg(target_os = "linux")]
            rx_pinch_gesture: None,
            #[cfg(target_os = "linux")]
            wayland_gesture_init_attempted: false,
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

    /// Changes the zoom factor, scaling around a specified screen anchor point.
    pub fn apply_zoom(&mut self, new_zoom: f32, anchor_viewport_y: Option<f32>) {
        let clamped = new_zoom.clamp(0.10, 8.0);
        if (clamped - self.zoom).abs() < 1e-4 {
            return;
        }

        let old_zoom = self.zoom;
        self.zoom = clamped;
        self.render_generation += 1;
        self.pending_render_requests.clear();

        // Calculate anchor zoom offset
        let anchor_y = anchor_viewport_y.unwrap_or(self.last_viewport_height * 0.5);
        let content_before = self.last_scroll_offset_y + anchor_y;
        let ratio = self.zoom / old_zoom;
        let content_after = content_before * ratio;
        let new_offset = (content_after - anchor_y).max(0.0);
        self.pending_set_scroll_offset_y = Some(new_offset);
    }

    /// Rotates all pages 90° clockwise, maintaining the current active page in view.
    pub fn rotate_clockwise(&mut self) {
        self.rotation = (self.rotation + 90) % 360;
        self.render_generation += 1;
        self.pending_render_requests.clear();
        self.pending_scroll_to_page = Some(self.current_page);
    }

    /// Adjusts zoom so the current page width fits the available window width.
    pub fn fit_width(&mut self, available_width: f32) {
        if let Some(ref doc) = self.doc_state {
            if let Some(page) = doc.page_sizes.get(self.current_page.saturating_sub(1)) {
                let dummy = PageTransform::new(*page, 1.0, self.rotation, egui::pos2(0.0, 0.0));
                let (pw, _) = dummy.visual_size_points();
                if pw > 0.0 {
                    let target_zoom = ((available_width - 32.0).max(100.0) / pw).clamp(0.10, 8.0);
                    self.apply_zoom(target_zoom, None);
                }
            }
        }
    }

    /// Adjusts zoom so the current page fits completely inside the window.
    pub fn fit_page(&mut self, available_width: f32, available_height: f32) {
        if let Some(ref doc) = self.doc_state {
            if let Some(page) = doc.page_sizes.get(self.current_page.saturating_sub(1)) {
                let dummy = PageTransform::new(*page, 1.0, self.rotation, egui::pos2(0.0, 0.0));
                let (pw, ph) = dummy.visual_size_points();
                if pw > 0.0 && ph > 0.0 {
                    let z_w = (available_width - 32.0).max(100.0) / pw;
                    let z_h = (available_height - 32.0).max(100.0) / ph;
                    let target_zoom = z_w.min(z_h).clamp(0.10, 8.0);
                    self.apply_zoom(target_zoom, None);
                    self.pending_scroll_to_page = Some(self.current_page);
                }
            }
        }
    }

    /// Toggles fullscreen mode and updates window chrome visibility.
    pub fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        self.is_fullscreen = !self.is_fullscreen;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.is_fullscreen));
    }

    /// Dispatches a render request to the background worker.
    pub fn request_render(&mut self, req: PageRenderRequest) {
        if self
            .pending_render_requests
            .insert((req.page, req.width_px, req.rotation))
        {
            let _ = self.tx_cmd.send(Command::Render(RenderCommand {
                doc_id: self.current_doc_id,
                page: req.page,
                width_px: req.width_px,
                rotation: req.rotation,
                generation: self.render_generation,
                priority: req.priority,
            }));
        }
    }
}

impl eframe::App for PdfViewApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        #[cfg(target_os = "linux")]
        if !self.wayland_gesture_init_attempted {
            self.wayland_gesture_init_attempted = true;
            use raw_window_handle::HasDisplayHandle;
            if let Ok(display_handle) = frame.display_handle() {
                if let raw_window_handle::RawDisplayHandle::Wayland(w_display) =
                    display_handle.as_raw()
                {
                    let display_ptr = w_display.display.as_ptr();
                    self.rx_pinch_gesture = crate::platform::wayland::init_wayland_pinch_listener(
                        display_ptr,
                        ctx.clone(),
                    );
                }
            }
        }

        // Native Wayland touchpad pinch gestures (no Ctrl key required)
        #[cfg(target_os = "linux")]
        {
            let mut wayland_scales = Vec::new();
            if let Some(ref rx) = self.rx_pinch_gesture {
                while let Ok(scale_factor) = rx.try_recv() {
                    wayland_scales.push(scale_factor);
                }
            }
            for scale_factor in wayland_scales {
                if (scale_factor - 1.0).abs() > 0.0001 {
                    let hover_pos = ctx.input(|i| i.pointer.hover_pos());
                    let anchor_y = hover_pos.map(|p| p.y - 70.0);
                    self.apply_zoom(self.zoom * scale_factor as f32, anchor_y);
                }
            }
        }

        // Measure frame time for F3 debug stats
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_frame_time).as_secs_f32() * 1000.0;
        self.last_frame_time = now;
        self.frame_time_ms = self.frame_time_ms * 0.9 + elapsed * 0.1;

        // Toggle debug display with F3
        if ctx.input(|i| i.key_pressed(egui::Key::F3)) {
            self.debug_visible = !self.debug_visible;
        }

        // Fullscreen shortcuts: F11 toggles, Esc exits
        if ctx.input(|i| i.key_pressed(egui::Key::F11)) {
            self.toggle_fullscreen(ctx);
        }
        if self.is_fullscreen && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.toggle_fullscreen(ctx);
        }

        // 1. Touchpad pinch-to-zoom (smooth continuous gesture)
        let pinch_delta = ctx.input(|i| i.zoom_delta());
        let ctrl_pressed = ctx.input(|i| i.modifiers.ctrl);

        if (pinch_delta - 1.0).abs() > 0.001 {
            let anchor_y = ctx.input(|i| {
                i.multi_touch()
                    .map(|t| t.center_pos.y - 70.0)
                    .or_else(|| i.pointer.hover_pos().map(|p| p.y - 70.0))
            });
            self.apply_zoom(self.zoom * pinch_delta, anchor_y);
        } else if ctrl_pressed {
            // Zoom shortcuts: Ctrl + scroll, Ctrl + =, Ctrl + -, Ctrl + 0
            let scroll_delta = ctx.input(|i| i.raw_scroll_delta.y);
            if scroll_delta.abs() > 0.0 {
                // Smooth exponential scaling: proportional to trackpad scroll distance (or wheel notch)
                let factor = (scroll_delta * 0.003).clamp(-0.25, 0.25).exp();
                let hover_pos = ctx.input(|i| i.pointer.hover_pos());
                let anchor_y = hover_pos.map(|p| p.y - 70.0);
                self.apply_zoom(self.zoom * factor, anchor_y);
            }

            if ctx.input(|i| i.key_pressed(egui::Key::Equals)) {
                self.apply_zoom(self.zoom * 1.2, None);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Minus)) {
                self.apply_zoom(self.zoom / 1.2, None);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Num0)) {
                let avail_w = ctx.screen_rect().width();
                self.fit_width(avail_w);
            }
        }

        // Keyboard navigation when no text input has focus
        let any_focus = ctx.memory(|m| m.focused().is_some());
        if !any_focus && !ctrl_pressed {
            if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                self.pending_scroll_delta_y = Some(-80.0);
            } else if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                self.pending_scroll_delta_y = Some(80.0);
            } else if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
                // Edge navigation: Left arrow navigates to previous page
                if self.current_page > 1 {
                    self.pending_scroll_to_page = Some(self.current_page - 1);
                }
            } else if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
                // Edge navigation: Right arrow navigates to next page
                if let Some(ref doc) = self.doc_state {
                    if self.current_page < doc.page_count {
                        self.pending_scroll_to_page = Some(self.current_page + 1);
                    }
                }
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
                        self.pending_render_requests.remove(&(
                            res.page,
                            res.width_px,
                            res.rotation,
                        ));

                        let byte_size = (res.bitmap_width_px * res.height_px * 4) as usize;
                        let color_image = egui::ColorImage::from_rgba_unmultiplied(
                            [res.bitmap_width_px as usize, res.height_px as usize],
                            &res.rgba,
                        );

                        let texture_options = egui::TextureOptions {
                            magnification: egui::TextureFilter::Linear,
                            minification: egui::TextureFilter::Linear,
                            wrap_mode: egui::TextureWrapMode::ClampToEdge,
                            mipmap_mode: None,
                        };

                        let texture_handle = ctx.load_texture(
                            format!(
                                "doc-{}-p{}-w{}-r{}",
                                res.doc_id, res.page, res.width_px, res.rotation
                            ),
                            color_image,
                            texture_options,
                        );

                        let key = TextureKey {
                            doc_id: res.doc_id,
                            page: res.page,
                            width_px: res.width_px,
                            rotation: res.rotation,
                        };

                        self.cache.insert(
                            key,
                            texture_handle,
                            res.bitmap_width_px,
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
                        self.pending_render_requests.retain(|&(p, _, _)| p != page);
                    }
                }
            }
        }

        // In fullscreen mode, all window chrome is hidden
        if !self.is_fullscreen {
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
            let total_pages_opt = self.doc_state.as_ref().map(|d| d.page_count);
            if let Some(total_pages) = total_pages_opt {
                let mut toolbar_action = None;
                let current_p = self.current_page;
                let zoom = self.zoom;
                let is_fullscreen = self.is_fullscreen;
                let mut avail_w = 0.0;
                let mut avail_h = 0.0;

                egui::TopBottomPanel::top("main_toolbar")
                    .frame(
                        egui::Frame::side_top_panel(&ctx.style())
                            .inner_margin(egui::Margin::symmetric(10, 4)),
                    )
                    .show(ctx, |ui| {
                        avail_w = ui.available_width();
                        avail_h = ctx.screen_rect().height();
                        toolbar_action =
                            self.toolbar
                                .ui(ui, current_p, total_pages, zoom, is_fullscreen);
                    });

                if let Some(action) = toolbar_action {
                    match action {
                        ToolbarAction::JumpToPage(target) => {
                            self.pending_scroll_to_page = Some(target);
                        }
                        ToolbarAction::ZoomIn => {
                            self.apply_zoom(self.zoom * 1.2, None);
                        }
                        ToolbarAction::ZoomOut => {
                            self.apply_zoom(self.zoom / 1.2, None);
                        }
                        ToolbarAction::FitWidth => {
                            self.fit_width(avail_w);
                        }
                        ToolbarAction::FitPage => {
                            self.fit_page(avail_w, avail_h);
                        }
                        ToolbarAction::RotateClockwise => {
                            self.rotate_clockwise();
                        }
                        ToolbarAction::ToggleFullscreen => {
                            self.toggle_fullscreen(ctx);
                        }
                    }
                }
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
                                    "F3 Debug | Zoom: {:.0}% | Rot: {}° | Cache: {:.2} MB / {:.0} MB ({} cached) | Frame: {:.1} ms ({:.0} FPS)",
                                    self.zoom * 100.0,
                                    self.rotation,
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
                    let set_scroll = self.pending_set_scroll_offset_y.take();

                    let output = render_viewer(
                        ui,
                        doc.doc_id,
                        &doc.page_sizes,
                        self.zoom,
                        self.rotation,
                        &mut self.cache,
                        &self.pending_render_requests,
                        scroll_to_page,
                        scroll_delta,
                        set_scroll,
                    );

                    self.current_page = output.current_page;
                    self.last_viewport_height = output.visible_viewport_height;
                    self.last_scroll_offset_y = output.current_scroll_offset_y;

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
