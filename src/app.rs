use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};

use crate::annotations::{DocumentAnnotations, UndoStack};
use crate::cache::{PageTextCache, TextureCache, TextureKey};
use crate::pdf::types::{
    DocId, OutlineItem, PageSize, RenderCommand, SearchMatch, TextLocation, TextSelection,
};
use crate::pdf::{start_worker, Command, Event, PageTransform};
use crate::settings::Settings;
use crate::ui::findbar::{render_find_bar, FindBarAction, FindState};
use crate::ui::pathbar::{parse_pdf_path, PathBar};
use crate::ui::sidebar_toc::render_sidebar_toc;
use crate::ui::tabbar::{render_tab_bar, TabBarAction, TabItem};
use crate::ui::toolbar::{ActiveTool, Toolbar, ToolbarAction, ToolbarProps};
use crate::ui::viewer::{
    compute_match_scroll_offset, render_viewer, ContextMenuState, PageRenderRequest, ViewerAction,
};

/// State of an open PDF document tab.
#[derive(Debug, Clone)]
pub struct PdfTab {
    pub doc_id: DocId,
    pub path: PathBuf,
    pub title: String,
    pub page_count: usize,
    pub page_sizes: Vec<PageSize>,
    pub current_page: usize,
    pub zoom: f32,
    pub rotation: u16,
    pub scroll_offset_y: f32,
    pub pending_scroll_to_page: Option<usize>,
    pub pending_scroll_delta_y: Option<f32>,
    pub pending_set_scroll_offset_y: Option<f32>,
    pub last_viewport_height: f32,
    pub last_viewport_top: f32,
    pub last_scroll_offset_y: f32,
    pub is_loading: bool,
    pub loading_message: String,
    pub outline: Option<Vec<OutlineItem>>,
    pub outline_loading: bool,
    pub sidebar_open: bool,
    pub sidebar_width: f32,
    pub find_state: FindState,
    pub selection: Option<TextSelection>,
    pub drag_start: Option<TextLocation>,
    pub context_menu: Option<ContextMenuState>,
    pub pending_text_requests: HashSet<usize>,
    pub active_tool: ActiveTool,
    pub annotations: DocumentAnnotations,
    pub file_key: String,
    pub annotations_dirty: bool,
    pub last_annotation_change: Option<Instant>,
    pub undo_stack: UndoStack,
    pub has_unsaved_changes: bool,
    pub freehand_stroke: Option<(usize, Vec<[f32; 2]>)>,
}

/// Top-level tabs in the reader.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
pub enum Tab {
    Pdf(PdfTab),
    #[allow(dead_code)]
    Settings,
}

/// Status of the background Pdfium binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdfiumStatus {
    Initializing,
    Ready,
    Error(String),
}

/// Main application state for pdfview.
pub struct PdfViewApp {
    pub tx_cmd: Sender<Command>,
    pub rx_evt: Receiver<Event>,
    pub pdfium_status: PdfiumStatus,

    // Tabs
    pub tabs: Vec<Tab>,
    pub active_tab_idx: usize,
    pub next_doc_id: DocId,

    // UI Chrome
    pub path_bar: PathBar,
    pub toolbar: Toolbar,
    pub is_fullscreen: bool,

    // Startup
    pub pending_open_path: Option<PathBuf>,

    // Cache and active requests
    pub cache: TextureCache,
    pub text_cache: PageTextCache,
    pub pending_render_requests: HashSet<(usize, u32, u16)>,
    pub render_generation: u64,

    // Debugging (F3)
    pub debug_visible: bool,
    pub last_frame_time: Instant,
    pub frame_time_ms: f32,

    #[cfg(target_os = "linux")]
    pub rx_pinch_gesture:
        Option<crossbeam_channel::Receiver<crate::platform::wayland::WaylandPinchEvent>>,
    #[cfg(target_os = "linux")]
    pub wayland_gesture_init_attempted: bool,
    pub pinch_active: bool,
    pub pinch_last_active: Option<Instant>,
    pub last_scroll_unit: Option<egui::MouseWheelUnit>,
    pub settings: Settings,
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

        let mut app = Self {
            tx_cmd,
            rx_evt,
            pdfium_status: PdfiumStatus::Initializing,
            tabs: Vec::new(),
            active_tab_idx: 0,
            next_doc_id: 1,
            path_bar,
            toolbar: Toolbar::new(),
            is_fullscreen: false,
            pending_open_path,
            cache: TextureCache::default(),
            text_cache: PageTextCache::default(),
            pending_render_requests: HashSet::new(),
            render_generation: 0,
            debug_visible: false,
            last_frame_time: Instant::now(),
            frame_time_ms: 16.0,
            #[cfg(target_os = "linux")]
            rx_pinch_gesture: None,
            #[cfg(target_os = "linux")]
            wayland_gesture_init_attempted: false,
            pinch_active: false,
            pinch_last_active: None,
            last_scroll_unit: None,
            settings: Settings::load(),
        };

        if app.pending_open_path.is_none() {
            app.create_empty_tab();
        }

        app
    }

    /// Returns a reference to the active PDF tab, if one is currently active.
    pub fn active_pdf_tab(&self) -> Option<&PdfTab> {
        match self.tabs.get(self.active_tab_idx) {
            Some(Tab::Pdf(tab)) => Some(tab),
            _ => None,
        }
    }

    /// Returns a mutable reference to the active PDF tab, if one is currently active.
    pub fn active_pdf_tab_mut(&mut self) -> Option<&mut PdfTab> {
        match self.tabs.get_mut(self.active_tab_idx) {
            Some(Tab::Pdf(tab)) => Some(tab),
            _ => None,
        }
    }

    /// Returns the DocId of the currently active tab.
    pub fn active_doc_id(&self) -> Option<DocId> {
        self.active_pdf_tab().map(|t| t.doc_id)
    }

    /// Creates a new empty tab immediately, expanding and focusing the path bar.
    pub fn create_empty_tab(&mut self) {
        let doc_id = self.next_doc_id;
        self.next_doc_id += 1;
        self.render_generation += 1;

        if let Some(prev) = self.active_pdf_tab() {
            self.cache.clear_doc(prev.doc_id);
        }

        let tab = PdfTab {
            doc_id,
            path: PathBuf::new(),
            title: "New Tab".to_string(),
            page_count: 0,
            page_sizes: Vec::new(),
            current_page: 1,
            zoom: 1.0,
            rotation: 0,
            scroll_offset_y: 0.0,
            pending_scroll_to_page: None,
            pending_scroll_delta_y: None,
            pending_set_scroll_offset_y: None,
            last_viewport_height: 600.0,
            last_viewport_top: 0.0,
            last_scroll_offset_y: 0.0,
            is_loading: false,
            loading_message: String::new(),
            outline: None,
            outline_loading: false,
            sidebar_open: false,
            sidebar_width: 280.0,
            find_state: FindState::default(),
            selection: None,
            drag_start: None,
            context_menu: None,
            pending_text_requests: HashSet::new(),
            active_tool: ActiveTool::None,
            annotations: DocumentAnnotations::default(),
            file_key: String::new(),
            annotations_dirty: false,
            last_annotation_change: None,
            undo_stack: UndoStack::default(),
            has_unsaved_changes: false,
            freehand_stroke: None,
        };

        self.tabs.push(Tab::Pdf(tab));
        self.active_tab_idx = self.tabs.len() - 1;
        self.pending_render_requests.clear();
        self.path_bar.set_text(String::new());
        self.path_bar.expand_and_focus();
    }

    /// Opens a PDF file. Reuses the active tab if it is an empty tab, or creates a new tab otherwise.
    pub fn open_pdf_tab(&mut self, path: PathBuf) {
        let title = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Untitled")
            .to_string();

        let active_is_empty = if let Some(Tab::Pdf(tab)) = self.tabs.get(self.active_tab_idx) {
            tab.page_count == 0 && !tab.is_loading && tab.path.as_os_str().is_empty()
        } else {
            false
        };

        if active_is_empty {
            if let Some(Tab::Pdf(tab)) = self.tabs.get_mut(self.active_tab_idx) {
                tab.path = path.clone();
                tab.title = title;
                tab.page_count = 0;
                tab.page_sizes.clear();
                tab.current_page = 1;
                tab.scroll_offset_y = 0.0;
                tab.pending_scroll_to_page = Some(1);
                tab.pending_scroll_delta_y = None;
                tab.pending_set_scroll_offset_y = None;
                tab.is_loading = true;
                tab.loading_message = format!("Opening {}...", path.display());
                tab.outline = None;
                tab.outline_loading = false;
                tab.find_state = FindState::default();
                tab.selection = None;
                tab.drag_start = None;
                tab.pending_text_requests.clear();
                tab.active_tool = ActiveTool::None;
                tab.annotations = DocumentAnnotations::default();
                tab.file_key.clear();
                tab.annotations_dirty = false;
                tab.last_annotation_change = None;
                tab.undo_stack = UndoStack::default();
                let doc_id = tab.doc_id;
                self.render_generation += 1;
                self.pending_render_requests.clear();
                let _ = self.tx_cmd.send(Command::Open { doc_id, path });
                return;
            }
        }

        let doc_id = self.next_doc_id;
        self.next_doc_id += 1;
        self.render_generation += 1;

        let tab = PdfTab {
            doc_id,
            path: path.clone(),
            title,
            page_count: 0,
            page_sizes: Vec::new(),
            current_page: 1,
            zoom: 1.0,
            rotation: 0,
            scroll_offset_y: 0.0,
            pending_scroll_to_page: Some(1),
            pending_scroll_delta_y: None,
            pending_set_scroll_offset_y: None,
            last_viewport_height: 600.0,
            last_viewport_top: 0.0,
            last_scroll_offset_y: 0.0,
            is_loading: true,
            loading_message: format!("Opening {}...", path.display()),
            outline: None,
            outline_loading: false,
            sidebar_open: false,
            sidebar_width: 280.0,
            find_state: FindState::default(),
            selection: None,
            drag_start: None,
            context_menu: None,
            pending_text_requests: HashSet::new(),
            active_tool: ActiveTool::None,
            annotations: DocumentAnnotations::default(),
            file_key: String::new(),
            annotations_dirty: false,
            last_annotation_change: None,
            undo_stack: UndoStack::default(),
            has_unsaved_changes: false,
            freehand_stroke: None,
        };

        // If switching from an existing tab, drop its textures from cache
        if let Some(prev) = self.active_pdf_tab() {
            self.cache.clear_doc(prev.doc_id);
        }

        self.tabs.push(Tab::Pdf(tab));
        self.active_tab_idx = self.tabs.len() - 1;
        self.pending_render_requests.clear();

        let _ = self.tx_cmd.send(Command::Open { doc_id, path });
    }

    /// Closes the tab at the given index, freeing its textures and sending Command::Close.
    pub fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }

        let closed = self.tabs.remove(index);
        if let Tab::Pdf(tab) = closed {
            if tab.annotations_dirty && !tab.file_key.is_empty() {
                let _ = tab.annotations.save_for_key(&tab.file_key);
            }
            let _ = self.tx_cmd.send(Command::Close { doc_id: tab.doc_id });
            self.cache.clear_doc(tab.doc_id);
            self.text_cache.clear_doc(tab.doc_id);
        }

        if self.tabs.is_empty() {
            self.create_empty_tab();
            return;
        }

        if self.active_tab_idx >= self.tabs.len() {
            self.active_tab_idx = self.tabs.len().saturating_sub(1);
        }

        self.render_generation += 1;
        self.pending_render_requests.clear();

        // If new active tab exists, restore its scroll offset
        if let Some(curr) = self.active_pdf_tab_mut() {
            curr.pending_set_scroll_offset_y = Some(curr.scroll_offset_y);
        }
    }

    /// Switches active tab to target_idx, dropping the previous tab's textures from cache.
    pub fn switch_to_tab(&mut self, target_idx: usize) {
        if target_idx == self.active_tab_idx || target_idx >= self.tabs.len() {
            return;
        }

        // Drop previous tab's cached textures
        if let Some(prev) = self.active_pdf_tab() {
            self.cache.clear_doc(prev.doc_id);
        }

        self.active_tab_idx = target_idx;
        self.render_generation += 1;
        self.pending_render_requests.clear();

        // Restore scroll offset for newly active tab
        if let Some(curr) = self.active_pdf_tab_mut() {
            curr.pending_set_scroll_offset_y = Some(curr.scroll_offset_y);
        }
    }

    /// Changes the zoom factor on the active tab, scaling around a specified screen anchor point.
    pub fn apply_zoom(&mut self, new_zoom: f32, anchor_viewport_y: Option<f32>) {
        let active_idx = self.active_tab_idx;
        if let Some(Tab::Pdf(tab)) = self.tabs.get_mut(active_idx) {
            let clamped = new_zoom.clamp(0.10, 8.0);
            if (clamped - tab.zoom).abs() < 1e-4 {
                return;
            }

            let old_zoom = tab.zoom;
            tab.zoom = clamped;

            let anchor_y = anchor_viewport_y.unwrap_or(tab.last_viewport_height * 0.5);
            let content_before = tab.last_scroll_offset_y + anchor_y;
            let ratio = tab.zoom / old_zoom;
            let content_after = content_before * ratio;
            let new_offset = (content_after - anchor_y).max(0.0);
            tab.last_scroll_offset_y = new_offset;
            tab.scroll_offset_y = new_offset;
            tab.pending_set_scroll_offset_y = Some(new_offset);

            self.render_generation += 1;
            self.pending_render_requests.clear();
        }
    }

    /// Rotates all pages of the active tab 90° clockwise.
    pub fn rotate_clockwise(&mut self) {
        let active_idx = self.active_tab_idx;
        if let Some(Tab::Pdf(tab)) = self.tabs.get_mut(active_idx) {
            tab.rotation = (tab.rotation + 90) % 360;
            self.render_generation += 1;
            self.pending_render_requests.clear();
            tab.pending_scroll_to_page = Some(tab.current_page);
        }
    }

    /// Adjusts zoom on the active tab so page width fits the available window width.
    pub fn fit_width(&mut self, available_width: f32) {
        let active_idx = self.active_tab_idx;
        if let Some(Tab::Pdf(tab)) = self.tabs.get_mut(active_idx) {
            let current_p = tab.current_page;
            let rot = tab.rotation;
            if let Some(page) = tab.page_sizes.get(current_p.saturating_sub(1)) {
                let dummy = PageTransform::new(*page, 1.0, rot, egui::pos2(0.0, 0.0));
                let (pw, _) = dummy.visual_size_points();
                if pw > 0.0 {
                    let target_zoom = ((available_width - 32.0).max(100.0) / pw).clamp(0.10, 8.0);
                    tab.zoom = target_zoom;
                    self.render_generation += 1;
                    self.pending_render_requests.clear();
                }
            }
        }
    }

    /// Adjusts zoom on the active tab so the current page fits completely inside the window.
    pub fn fit_page(&mut self, available_width: f32, available_height: f32) {
        let active_idx = self.active_tab_idx;
        if let Some(Tab::Pdf(tab)) = self.tabs.get_mut(active_idx) {
            let current_p = tab.current_page;
            let rot = tab.rotation;
            if let Some(page) = tab.page_sizes.get(current_p.saturating_sub(1)) {
                let dummy = PageTransform::new(*page, 1.0, rot, egui::pos2(0.0, 0.0));
                let (pw, ph) = dummy.visual_size_points();
                if pw > 0.0 && ph > 0.0 {
                    let z_w = (available_width - 32.0).max(100.0) / pw;
                    let z_h = (available_height - 32.0).max(100.0) / ph;
                    let target_zoom = z_w.min(z_h).clamp(0.10, 8.0);
                    tab.zoom = target_zoom;
                    self.render_generation += 1;
                    self.pending_render_requests.clear();
                    tab.pending_scroll_to_page = Some(current_p);
                }
            }
        }
    }

    /// Toggles fullscreen mode and updates window chrome visibility.
    pub fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        self.is_fullscreen = !self.is_fullscreen;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.is_fullscreen));
    }

    /// Saves annotations directly into the active PDF file.
    pub fn save_active_pdf(&mut self) {
        let active_idx = self.active_tab_idx;
        if let Some(Tab::Pdf(tab)) = self.tabs.get_mut(active_idx) {
            if !tab.is_loading && tab.page_count > 0 && !tab.path.as_os_str().is_empty() {
                // Ensure local sidecar JSON is up to date
                if !tab.file_key.is_empty() {
                    let _ = tab.annotations.save_for_key(&tab.file_key);
                    tab.annotations_dirty = false;
                }
                let _ = self.tx_cmd.send(Command::SaveDoc {
                    doc_id: tab.doc_id,
                    path: tab.path.clone(),
                    highlights: tab.annotations.highlights.clone(),
                });
            }
        }
    }

    /// Copies active text selection to system clipboard. If all selected pages are cached,
    /// performs instantaneous extraction on UI thread; otherwise requests on-demand worker
    /// extraction to prevent loading all pages into memory or freezing UI.
    pub fn copy_selection(&mut self, ctx: &egui::Context) {
        let active_idx = self.active_tab_idx;
        if let Some(Tab::Pdf(tab)) = self.tabs.get(active_idx) {
            if let Some(sel) = tab.selection {
                if sel.is_empty() {
                    return;
                }
                let (start, end) = sel.normalized();
                let all_cached =
                    (start.page..=end.page).all(|p| self.text_cache.peek(tab.doc_id, p).is_some());

                if all_cached {
                    let mut full_text = String::new();
                    for p in start.page..=end.page {
                        if let Some(pt_data) = self.text_cache.peek(tab.doc_id, p) {
                            let c_start = if p == start.page { start.char_idx } else { 0 };
                            let c_end = if p == end.page {
                                end.char_idx.min(pt_data.chars.len().saturating_sub(1))
                            } else {
                                pt_data.chars.len().saturating_sub(1)
                            };
                            let page_str = pt_data.extract_text_range(c_start, c_end);
                            if !page_str.is_empty() {
                                if !full_text.is_empty() && !full_text.ends_with('\n') {
                                    full_text.push('\n');
                                }
                                full_text.push_str(&page_str);
                            }
                        }
                    }
                    if !full_text.is_empty() {
                        ctx.copy_text(full_text);
                    }
                } else {
                    let _ = self.tx_cmd.send(Command::ExtractTextForCopy {
                        doc_id: tab.doc_id,
                        start_page: start.page,
                        start_char: start.char_idx,
                        end_page: end.page,
                        end_char: end.char_idx,
                    });
                }
            }
        }
    }

    /// Dispatches a render request to the background worker thread.
    fn request_render(&mut self, req: PageRenderRequest) {
        let active_id = match self.active_doc_id() {
            Some(id) => id,
            None => return,
        };

        let key = (req.page, req.width_px, req.rotation);
        if self.pending_render_requests.insert(key) {
            let _ = self.tx_cmd.send(Command::Render(RenderCommand {
                doc_id: active_id,
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
            let mut wayland_events = Vec::new();
            if let Some(ref rx) = self.rx_pinch_gesture {
                while let Ok(evt) = rx.try_recv() {
                    wayland_events.push(evt);
                }
            }
            for evt in wayland_events {
                match evt {
                    crate::platform::wayland::WaylandPinchEvent::Begin => {
                        self.pinch_active = true;
                        self.pinch_last_active = Some(Instant::now());
                    }
                    crate::platform::wayland::WaylandPinchEvent::Update(scale_factor) => {
                        self.pinch_active = true;
                        self.pinch_last_active = Some(Instant::now());
                        if (scale_factor - 1.0).abs() > 0.0001 {
                            let anchor_y = self.active_pdf_tab().and_then(|tab| {
                                ctx.input(|i| i.pointer.hover_pos()).map(|p| {
                                    if p.y >= tab.last_viewport_top
                                        && p.y <= tab.last_viewport_top + tab.last_viewport_height
                                    {
                                        p.y - tab.last_viewport_top
                                    } else {
                                        tab.last_viewport_height * 0.5
                                    }
                                })
                            });
                            let current_zoom = self.active_pdf_tab().map(|t| t.zoom).unwrap_or(1.0);
                            self.apply_zoom(current_zoom * scale_factor as f32, anchor_y);
                        }
                    }
                    crate::platform::wayland::WaylandPinchEvent::End => {
                        self.pinch_active = false;
                        self.pinch_last_active = Some(Instant::now());
                    }
                }
            }
        }

        // Multitouch trackpad / touchscreen pinch gesture fallback
        let egui_pinch_delta = ctx.input(|i| i.zoom_delta());
        let has_multitouch = ctx.input(|i| i.multi_touch().is_some());
        if (egui_pinch_delta - 1.0).abs() > 0.001 || has_multitouch {
            self.pinch_active = true;
            self.pinch_last_active = Some(Instant::now());
            if (egui_pinch_delta - 1.0).abs() > 0.001 {
                let anchor_y = self.active_pdf_tab().and_then(|tab| {
                    ctx.input(|i| {
                        i.multi_touch()
                            .map(|t| t.center_pos.y)
                            .or_else(|| i.pointer.hover_pos().map(|p| p.y))
                    })
                    .map(|py| {
                        if py >= tab.last_viewport_top
                            && py <= tab.last_viewport_top + tab.last_viewport_height
                        {
                            py - tab.last_viewport_top
                        } else {
                            tab.last_viewport_height * 0.5
                        }
                    })
                });
                let current_zoom = self.active_pdf_tab().map(|t| t.zoom).unwrap_or(1.0);
                self.apply_zoom(current_zoom * egui_pinch_delta, anchor_y);
            }
        }

        // Determine if pinch zoom is actively occurring or in cooldown
        let is_pinching = self.pinch_active
            || self
                .pinch_last_active
                .is_some_and(|t| t.elapsed() < Duration::from_millis(150));

        if is_pinching {
            // Suppress accidental scroll deltas generated by touchpad pinch movements
            ctx.input_mut(|i| {
                i.raw_scroll_delta = egui::Vec2::ZERO;
                i.smooth_scroll_delta = egui::Vec2::ZERO;
            });
            ctx.request_repaint();
        }

        // Debounced annotation autosave (~500 ms)
        for tab_item in &mut self.tabs {
            if let Tab::Pdf(ref mut tab) = tab_item {
                if tab.annotations_dirty {
                    if let Some(last_change) = tab.last_annotation_change {
                        if last_change.elapsed() >= Duration::from_millis(500) {
                            if !tab.file_key.is_empty() {
                                let _ = tab.annotations.save_for_key(&tab.file_key);
                                tab.annotations_dirty = false;
                                let _ = self.tx_cmd.send(Command::SyncAnnotations {
                                    doc_id: tab.doc_id,
                                    highlights: tab.annotations.highlights.clone(),
                                });
                            }
                        } else {
                            let remaining =
                                Duration::from_millis(500).saturating_sub(last_change.elapsed());
                            ctx.request_repaint_after(remaining);
                        }
                    }
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

        // Borderless window edge resize support
        if !self.is_fullscreen {
            let is_max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
            if !is_max {
                let screen_rect = ctx.screen_rect();
                if let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) {
                    let margin = 5.0;
                    let left = pos.x <= screen_rect.min.x + margin;
                    let right = pos.x >= screen_rect.max.x - margin;
                    let top = pos.y <= screen_rect.min.y + margin;
                    let bottom = pos.y >= screen_rect.max.y - margin;

                    let resize_dir = match (top, bottom, left, right) {
                        (true, _, true, _) => Some(egui::viewport::ResizeDirection::NorthWest),
                        (true, _, _, true) => Some(egui::viewport::ResizeDirection::NorthEast),
                        (_, true, true, _) => Some(egui::viewport::ResizeDirection::SouthWest),
                        (_, true, _, true) => Some(egui::viewport::ResizeDirection::SouthEast),
                        (true, _, _, _) => Some(egui::viewport::ResizeDirection::North),
                        (_, true, _, _) => Some(egui::viewport::ResizeDirection::South),
                        (_, _, true, _) => Some(egui::viewport::ResizeDirection::West),
                        (_, _, _, true) => Some(egui::viewport::ResizeDirection::East),
                        _ => None,
                    };

                    if let Some(dir) = resize_dir {
                        let cursor_icon = match dir {
                            egui::viewport::ResizeDirection::North => egui::CursorIcon::ResizeNorth,
                            egui::viewport::ResizeDirection::South => egui::CursorIcon::ResizeSouth,
                            egui::viewport::ResizeDirection::West => egui::CursorIcon::ResizeWest,
                            egui::viewport::ResizeDirection::East => egui::CursorIcon::ResizeEast,
                            egui::viewport::ResizeDirection::NorthWest => {
                                egui::CursorIcon::ResizeNorthWest
                            }
                            egui::viewport::ResizeDirection::NorthEast => {
                                egui::CursorIcon::ResizeNorthEast
                            }
                            egui::viewport::ResizeDirection::SouthWest => {
                                egui::CursorIcon::ResizeSouthWest
                            }
                            egui::viewport::ResizeDirection::SouthEast => {
                                egui::CursorIcon::ResizeSouthEast
                            }
                        };
                        ctx.set_cursor_icon(cursor_icon);

                        if ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary)) {
                            ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
                        }
                    }
                }
            }
        }

        // Path bar shortcut: Ctrl+L expands and focuses
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::L)) {
            self.path_bar.expand_and_focus();
        }

        // New Tab shortcut: Ctrl+T
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::T)) {
            self.create_empty_tab();
        }

        // Tab shortcuts: Ctrl+W closes active tab, Ctrl+Tab / Ctrl+Shift+Tab cycles
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::W)) && !self.tabs.is_empty() {
            self.close_tab(self.active_tab_idx);
        }
        if ctx.input(|i| i.modifiers.ctrl && !i.modifiers.shift && i.key_pressed(egui::Key::Tab))
            && !self.tabs.is_empty()
        {
            let next_idx = (self.active_tab_idx + 1) % self.tabs.len();
            self.switch_to_tab(next_idx);
        }
        if ctx.input(|i| i.modifiers.ctrl && i.modifiers.shift && i.key_pressed(egui::Key::Tab))
            && !self.tabs.is_empty()
        {
            let prev_idx = (self.active_tab_idx + self.tabs.len() - 1) % self.tabs.len();
            self.switch_to_tab(prev_idx);
        }

        // Find in document shortcut: Ctrl+F opens and focuses Find bar
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::F)) {
            if let Some(tab) = self.active_pdf_tab_mut() {
                tab.find_state.is_open = true;
                tab.find_state.focus_requested = true;
            }
        }

        // Save document shortcut: Ctrl+S / Cmd+S
        if ctx.input(|i| (i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::S)) {
            self.save_active_pdf();
        }

        // Copy selected text shortcut: Ctrl+C / Cmd+C or egui::Event::Copy
        let copy_requested = ctx.input(|i| {
            i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                || ((i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::C))
        });
        if copy_requested && !ctx.wants_keyboard_input() {
            let has_selection = self
                .active_pdf_tab()
                .and_then(|t| t.selection)
                .map(|s| !s.is_empty())
                .unwrap_or(false);

            if has_selection {
                self.copy_selection(ctx);
            }
        }

        // Undo / Redo shortcuts (when not editing text): Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z
        if !ctx.wants_keyboard_input() {
            let ctrl = ctx.input(|i| i.modifiers.ctrl || i.modifiers.command);
            let shift = ctx.input(|i| i.modifiers.shift);
            let z_pressed = ctx.input(|i| i.key_pressed(egui::Key::Z));
            let y_pressed = ctx.input(|i| i.key_pressed(egui::Key::Y));

            if ctrl && !shift && z_pressed {
                let undo_info = self.active_pdf_tab_mut().and_then(|tab| {
                    if tab.undo_stack.undo(&mut tab.annotations) {
                        tab.annotations_dirty = true;
                        tab.has_unsaved_changes = true;
                        tab.last_annotation_change = Some(Instant::now());
                        Some((tab.doc_id, tab.annotations.highlights.clone()))
                    } else {
                        None
                    }
                });
                if let Some((doc_id, highlights)) = undo_info {
                    let _ = self
                        .tx_cmd
                        .send(Command::SyncAnnotations { doc_id, highlights });
                    self.render_generation += 1;
                    self.pending_render_requests.clear();
                    ctx.request_repaint();
                }
            } else if (ctrl && y_pressed) || (ctrl && shift && z_pressed) {
                let redo_info = self.active_pdf_tab_mut().and_then(|tab| {
                    if tab.undo_stack.redo(&mut tab.annotations) {
                        tab.annotations_dirty = true;
                        tab.has_unsaved_changes = true;
                        tab.last_annotation_change = Some(Instant::now());
                        Some((tab.doc_id, tab.annotations.highlights.clone()))
                    } else {
                        None
                    }
                });
                if let Some((doc_id, highlights)) = redo_info {
                    let _ = self
                        .tx_cmd
                        .send(Command::SyncAnnotations { doc_id, highlights });
                    self.render_generation += 1;
                    self.pending_render_requests.clear();
                    ctx.request_repaint();
                }
            }
        }

        // Select all text in document shortcut: Ctrl+A / Cmd+A
        if ctx.input(|i| (i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::A)) {
            let text_input_active = ctx.wants_keyboard_input();
            if !text_input_active {
                if let Some(tab) = self.active_pdf_tab_mut() {
                    let last_page = tab.page_count.saturating_sub(1);
                    tab.selection = Some(TextSelection::new(
                        TextLocation::new(0, 0),
                        TextLocation::new(last_page, usize::MAX),
                    ));
                    tab.drag_start = None;
                }
            }
        }

        // Close context menu / Find bar / clear selection shortcut: Esc (when not in fullscreen)
        if !self.is_fullscreen && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if let Some(tab) = self.active_pdf_tab_mut() {
                if tab.context_menu.is_some() {
                    tab.context_menu = None;
                } else if tab.selection.is_some() {
                    tab.selection = None;
                    tab.drag_start = None;
                } else if tab.find_state.is_open {
                    tab.find_state.is_open = false;
                    tab.find_state.clear_matches();
                    let doc_id = tab.doc_id;
                    let query_id = tab.find_state.active_query_id;
                    let _ = self.tx_cmd.send(Command::Search {
                        doc_id,
                        query_id,
                        query: String::new(),
                    });
                    ctx.memory_mut(|m| {
                        if let Some(id) = m.focused() {
                            m.surrender_focus(id);
                        }
                    });
                }
            }
        }

        let ctrl_pressed = ctx.input(|i| i.modifiers.ctrl);
        if ctrl_pressed {
            // Zoom shortcuts: Ctrl + scroll, Ctrl + =, Ctrl + -, Ctrl + 0
            let scroll_delta = ctx.input(|i| i.raw_scroll_delta.y);
            if scroll_delta.abs() > 0.0 {
                // Clear scroll delta so ScrollArea does not scroll vertically while zooming
                ctx.input_mut(|i| {
                    i.raw_scroll_delta = egui::Vec2::ZERO;
                    i.smooth_scroll_delta = egui::Vec2::ZERO;
                });
                let factor = (scroll_delta * 0.003).clamp(-0.25, 0.25).exp();
                let anchor_y = self.active_pdf_tab().and_then(|tab| {
                    ctx.input(|i| i.pointer.hover_pos()).map(|p| {
                        if p.y >= tab.last_viewport_top
                            && p.y <= tab.last_viewport_top + tab.last_viewport_height
                        {
                            p.y - tab.last_viewport_top
                        } else {
                            tab.last_viewport_height * 0.5
                        }
                    })
                });
                let current_zoom = self.active_pdf_tab().map(|t| t.zoom).unwrap_or(1.0);
                self.apply_zoom(current_zoom * factor, anchor_y);
            }

            if ctx.input(|i| i.key_pressed(egui::Key::Equals)) {
                let current_zoom = self.active_pdf_tab().map(|t| t.zoom).unwrap_or(1.0);
                self.apply_zoom(current_zoom * 1.2, None);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Minus)) {
                let current_zoom = self.active_pdf_tab().map(|t| t.zoom).unwrap_or(1.0);
                self.apply_zoom(current_zoom / 1.2, None);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Num0)) {
                let avail_w = ctx.screen_rect().width();
                self.fit_width(avail_w);
            }
        } else {
            // Natural touchpad scrolling speed adjustment (platform-safe, matches Edge/Chrome/system responsiveness)
            let raw_scroll = ctx.input(|i| i.raw_scroll_delta);
            let smooth_scroll = ctx.input(|i| i.smooth_scroll_delta);

            if raw_scroll != egui::Vec2::ZERO || smooth_scroll != egui::Vec2::ZERO {
                let wheel_unit = ctx.input(|i| {
                    i.raw.events.iter().find_map(|e| match e {
                        egui::Event::MouseWheel { unit, .. } => Some(*unit),
                        _ => None,
                    })
                });

                if let Some(unit) = wheel_unit {
                    self.last_scroll_unit = Some(unit);
                }

                let is_touchpad = match self.last_scroll_unit {
                    Some(egui::MouseWheelUnit::Point) => true,
                    Some(egui::MouseWheelUnit::Line | egui::MouseWheelUnit::Page) => false,
                    None => true,
                };

                let multiplier = if is_touchpad {
                    self.settings.touchpad_scroll_speed.clamp(0.5, 10.0)
                } else {
                    self.settings.mouse_wheel_speed.clamp(0.5, 5.0)
                };

                if (multiplier - 1.0).abs() > 0.001 {
                    ctx.input_mut(|i| {
                        i.smooth_scroll_delta *= multiplier;
                        i.raw_scroll_delta *= multiplier;
                    });
                }
            } else {
                self.last_scroll_unit = None;
            }
        }

        // Keyboard navigation when no text input has focus and not pinching
        let any_focus = ctx.memory(|m| m.focused().is_some());
        if !any_focus && !ctrl_pressed && !is_pinching {
            if let Some(tab) = self.active_pdf_tab_mut() {
                if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                    tab.pending_scroll_delta_y = Some(-80.0);
                } else if ctx.input(|i| i.key_pressed(egui::Key::ArrowDown)) {
                    tab.pending_scroll_delta_y = Some(80.0);
                } else if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
                    if tab.current_page > 1 {
                        tab.pending_scroll_to_page = Some(tab.current_page - 1);
                    }
                } else if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
                    if tab.current_page < tab.page_count {
                        tab.pending_scroll_to_page = Some(tab.current_page + 1);
                    }
                } else if ctx.input(|i| i.key_pressed(egui::Key::PageUp)) {
                    tab.pending_scroll_delta_y = Some(-500.0);
                } else if ctx.input(|i| {
                    i.key_pressed(egui::Key::PageDown) || i.key_pressed(egui::Key::Space)
                }) {
                    tab.pending_scroll_delta_y = Some(500.0);
                } else if ctx.input(|i| i.key_pressed(egui::Key::Home)) {
                    tab.pending_scroll_to_page = Some(1);
                } else if ctx.input(|i| i.key_pressed(egui::Key::End)) {
                    tab.pending_scroll_to_page = Some(tab.page_count);
                }
            }
        }

        // Drain worker events non-blockingly
        while let Ok(evt) = self.rx_evt.try_recv() {
            match evt {
                Event::PdfiumReady => {
                    self.pdfium_status = PdfiumStatus::Ready;
                    if let Some(path) = self.pending_open_path.take() {
                        self.open_pdf_tab(path);
                    }
                }
                Event::PdfiumError(err) => {
                    self.pdfium_status = PdfiumStatus::Error(err);
                }
                Event::Opened {
                    doc_id,
                    page_count,
                    page_sizes,
                    annotations,
                } => {
                    for tab_item in &mut self.tabs {
                        if let Tab::Pdf(ref mut tab) = tab_item {
                            if tab.doc_id == doc_id {
                                tab.page_count = page_count;
                                tab.page_sizes = page_sizes;
                                tab.is_loading = false;
                                tab.outline_loading = true;
                                let _ = self.tx_cmd.send(Command::Outline { doc_id });
                                tab.annotations.highlights = annotations;
                                if !tab.path.as_os_str().is_empty() {
                                    let file_size =
                                        std::fs::metadata(&tab.path).map(|m| m.len()).unwrap_or(0);
                                    let abs_path = std::fs::canonicalize(&tab.path)
                                        .unwrap_or_else(|_| tab.path.clone());
                                    tab.file_key =
                                        DocumentAnnotations::compute_file_key(&abs_path, file_size);
                                    let sidecar = DocumentAnnotations::load_for_key(&tab.file_key);
                                    for sc_hl in sidecar.highlights {
                                        if !tab
                                            .annotations
                                            .highlights
                                            .iter()
                                            .any(|h| h.id == sc_hl.id)
                                        {
                                            tab.annotations.highlights.push(sc_hl);
                                        }
                                    }
                                    tab.annotations_dirty = false;
                                }
                                break;
                            }
                        }
                    }
                }
                Event::OpenError { doc_id, error } => {
                    for tab_item in &mut self.tabs {
                        if let Tab::Pdf(ref mut tab) = tab_item {
                            if tab.doc_id == doc_id {
                                tab.is_loading = false;
                                tab.loading_message = error.clone();
                                break;
                            }
                        }
                    }
                    self.path_bar.set_error(Some(error));
                }
                Event::Rendered(res) => {
                    // Only cache renders for the active document and current generation
                    if Some(res.doc_id) == self.active_doc_id()
                        && res.generation == self.render_generation
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
                            res.generation,
                        );
                    }
                }
                Event::RenderError {
                    doc_id,
                    page,
                    generation,
                    ..
                } => {
                    if Some(doc_id) == self.active_doc_id() && generation == self.render_generation
                    {
                        self.pending_render_requests.retain(|&(p, _, _)| p != page);
                    }
                }
                Event::OutlineResult { doc_id, items } => {
                    for tab_item in &mut self.tabs {
                        if let Tab::Pdf(ref mut tab) = tab_item {
                            if tab.doc_id == doc_id {
                                tab.outline = Some(items);
                                tab.outline_loading = false;
                                break;
                            }
                        }
                    }
                }
                Event::SearchResults {
                    doc_id,
                    query_id,
                    page: _,
                    matches,
                    done,
                } => {
                    for tab_item in &mut self.tabs {
                        if let Tab::Pdf(ref mut tab) = tab_item {
                            if tab.doc_id == doc_id && tab.find_state.active_query_id == query_id {
                                let was_empty = tab.find_state.matches.is_empty();
                                tab.find_state.matches.extend(matches);

                                if was_empty && !tab.find_state.matches.is_empty() {
                                    tab.find_state.current_match_idx = Some(0);
                                    let first_match = &tab.find_state.matches[0];
                                    let offset = compute_match_scroll_offset(
                                        first_match,
                                        &tab.page_sizes,
                                        tab.zoom,
                                        tab.rotation,
                                        tab.last_viewport_height,
                                        800.0,
                                    );
                                    tab.pending_set_scroll_offset_y = offset;
                                }

                                if done {
                                    tab.find_state.is_searching = false;
                                }
                                break;
                            }
                        }
                    }
                }
                Event::PageTextResult { doc_id, page, data } => {
                    self.text_cache.insert(doc_id, page, data);
                    for tab_item in &mut self.tabs {
                        if let Tab::Pdf(ref mut tab) = tab_item {
                            if tab.doc_id == doc_id {
                                tab.pending_text_requests.remove(&page);
                                break;
                            }
                        }
                    }
                }
                Event::CopyTextResult { text } => {
                    if !text.is_empty() {
                        ctx.copy_text(text);
                    }
                }
                Event::Saved {
                    doc_id,
                    success,
                    error,
                } => {
                    for tab_item in &mut self.tabs {
                        if let Tab::Pdf(ref mut tab) = tab_item {
                            if tab.doc_id == doc_id {
                                if success {
                                    tab.has_unsaved_changes = false;
                                    self.render_generation += 1;
                                    ctx.request_repaint();
                                } else if let Some(err) = error {
                                    eprintln!("Failed to save PDF {}: {}", tab.path.display(), err);
                                }
                                break;
                            }
                        }
                    }
                }
            }
        }

        // Debounce typing (~150 ms) for active tab's find query
        let search_dispatch = if let Some(tab) = self.active_pdf_tab_mut() {
            if let Some(last_time) = tab.find_state.last_edit_time {
                if last_time.elapsed().as_millis() >= 150 {
                    tab.find_state.last_edit_time = None;
                    tab.find_state.active_query_id += 1;
                    let query_id = tab.find_state.active_query_id;
                    let query = tab.find_state.query.clone();
                    let doc_id = tab.doc_id;
                    tab.find_state.clear_matches();
                    let is_non_empty = !query.trim().is_empty();
                    if is_non_empty {
                        tab.find_state.is_searching = true;
                    }
                    Some((doc_id, query_id, query))
                } else {
                    ctx.request_repaint_after(std::time::Duration::from_millis(25));
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        if let Some((doc_id, query_id, query)) = search_dispatch {
            let _ = self.tx_cmd.send(Command::Search {
                doc_id,
                query_id,
                query,
            });
        }

        // In fullscreen mode, all window chrome is hidden
        if !self.is_fullscreen {
            // 1. Tab Bar
            egui::TopBottomPanel::top("top_tabbar")
                .frame(
                    egui::Frame::side_top_panel(&ctx.style())
                        .fill(egui::Color32::from_rgb(20, 20, 22))
                        .inner_margin(egui::Margin::symmetric(6, 2)),
                )
                .show(ctx, |ui| {
                    let tab_items: Vec<TabItem> = self
                        .tabs
                        .iter()
                        .enumerate()
                        .map(|(idx, t)| match t {
                            Tab::Pdf(pdf) => TabItem {
                                title: &pdf.title,
                                tooltip: pdf.path.to_str().unwrap_or(&pdf.title),
                                is_active: idx == self.active_tab_idx,
                                is_loading: pdf.is_loading,
                                is_dirty: pdf.has_unsaved_changes,
                            },
                            Tab::Settings => TabItem {
                                title: "Settings",
                                tooltip: "Settings",
                                is_active: idx == self.active_tab_idx,
                                is_loading: false,
                                is_dirty: false,
                            },
                        })
                        .collect();

                    let (action_opt, toggle_path) =
                        render_tab_bar(ui, &tab_items, self.path_bar.is_collapsed);

                    if toggle_path {
                        self.path_bar.is_collapsed = !self.path_bar.is_collapsed;
                    }

                    if let Some(action) = action_opt {
                        match action {
                            TabBarAction::Select(idx) => self.switch_to_tab(idx),
                            TabBarAction::Close(idx) => self.close_tab(idx),
                            TabBarAction::NewTab => self.create_empty_tab(),
                        }
                    }
                });

            // 2. Path Bar (Collapsible)
            if !self.path_bar.is_collapsed {
                egui::TopBottomPanel::top("top_pathbar")
                    .frame(
                        egui::Frame::side_top_panel(&ctx.style())
                            .inner_margin(egui::Margin::symmetric(10, 6)),
                    )
                    .show(ctx, |ui| {
                        if let Some(path) = self.path_bar.ui(ui) {
                            if self.pdfium_status == PdfiumStatus::Ready {
                                self.open_pdf_tab(path);
                            } else if self.pdfium_status == PdfiumStatus::Initializing {
                                self.pending_open_path = Some(path);
                            }
                        }
                    });
            }

            // 3. Toolbar
            let active_tab_info = self.active_pdf_tab().map(|t| {
                (
                    t.current_page,
                    t.page_count,
                    t.zoom,
                    t.is_loading,
                    t.page_count > 0,
                    t.sidebar_open,
                    t.find_state.is_open,
                    t.active_tool,
                )
            });

            if let Some((
                current_p,
                total_pages,
                zoom,
                is_loading,
                has_pages,
                sidebar_open,
                find_open,
                active_tool,
            )) = active_tab_info
            {
                if has_pages || is_loading {
                    let mut toolbar_action = None;
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
                            toolbar_action = self.toolbar.ui(
                                ui,
                                ToolbarProps {
                                    current_page: current_p,
                                    total_pages,
                                    zoom,
                                    is_fullscreen,
                                    sidebar_open,
                                    find_open,
                                    active_tool,
                                    highlighter_color: self.settings.highlighter_color,
                                    highlighter_thickness: self.settings.highlighter_thickness,
                                    can_save: self
                                        .active_pdf_tab()
                                        .map(|t| !t.is_loading && t.page_count > 0)
                                        .unwrap_or(false),
                                    has_unsaved_changes: self
                                        .active_pdf_tab()
                                        .map(|t| t.has_unsaved_changes)
                                        .unwrap_or(false),
                                },
                            );
                        });

                    if let Some(action) = toolbar_action {
                        match action {
                            ToolbarAction::ToggleOutlineSidebar => {
                                if let Some(tab) = self.active_pdf_tab_mut() {
                                    tab.sidebar_open = !tab.sidebar_open;
                                }
                            }
                            ToolbarAction::ToggleFind => {
                                if let Some(tab) = self.active_pdf_tab_mut() {
                                    tab.find_state.is_open = !tab.find_state.is_open;
                                    if tab.find_state.is_open {
                                        tab.find_state.focus_requested = true;
                                    } else {
                                        tab.find_state.clear_matches();
                                        ctx.memory_mut(|m| {
                                            if let Some(id) = m.focused() {
                                                m.surrender_focus(id);
                                            }
                                        });
                                    }
                                }
                            }
                            ToolbarAction::JumpToPage(target) => {
                                if let Some(tab) = self.active_pdf_tab_mut() {
                                    tab.pending_scroll_to_page = Some(target);
                                }
                            }
                            ToolbarAction::ZoomIn => {
                                let curr_zoom =
                                    self.active_pdf_tab().map(|t| t.zoom).unwrap_or(1.0);
                                self.apply_zoom(curr_zoom * 1.2, None);
                            }
                            ToolbarAction::ZoomOut => {
                                let curr_zoom =
                                    self.active_pdf_tab().map(|t| t.zoom).unwrap_or(1.0);
                                self.apply_zoom(curr_zoom / 1.2, None);
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
                            ToolbarAction::ToggleHighlighter => {
                                if let Some(tab) = self.active_pdf_tab_mut() {
                                    tab.active_tool = if tab.active_tool == ActiveTool::Highlighter
                                    {
                                        ActiveTool::None
                                    } else {
                                        ActiveTool::Highlighter
                                    };
                                }
                            }
                            ToolbarAction::ToggleEraser => {
                                if let Some(tab) = self.active_pdf_tab_mut() {
                                    tab.active_tool = if tab.active_tool == ActiveTool::Eraser {
                                        ActiveTool::None
                                    } else {
                                        ActiveTool::Eraser
                                    };
                                }
                            }
                            ToolbarAction::SetHighlighterColor(color) => {
                                self.settings.highlighter_color = color;
                                let _ = self.settings.save();
                            }
                            ToolbarAction::SetHighlighterThickness(thickness) => {
                                self.settings.highlighter_thickness = thickness;
                                let _ = self.settings.save();
                            }
                            ToolbarAction::Save => {
                                self.save_active_pdf();
                            }
                        }
                    }
                }
            }

            // 4. Find Bar (Below toolbar)
            let find_open = self
                .active_pdf_tab()
                .map(|t| t.find_state.is_open && (t.page_count > 0 || t.is_loading))
                .unwrap_or(false);

            if find_open {
                let mut find_action = None;
                let mut avail_w = 0.0;
                egui::TopBottomPanel::top("main_findbar")
                    .frame(
                        egui::Frame::side_top_panel(&ctx.style())
                            .fill(egui::Color32::from_rgb(24, 24, 26))
                            .inner_margin(egui::Margin::symmetric(10, 4)),
                    )
                    .show(ctx, |ui| {
                        avail_w = ui.available_width();
                        if let Some(tab) = self.active_pdf_tab_mut() {
                            let match_count = tab.find_state.matches.len();
                            let current_match_idx = tab.find_state.current_match_idx;
                            let is_searching = tab.find_state.is_searching;
                            find_action = render_find_bar(
                                ui,
                                &mut tab.find_state.query,
                                match_count,
                                current_match_idx,
                                is_searching,
                                &mut tab.find_state.focus_requested,
                            );
                        }
                    });

                if let Some(action) = find_action {
                    match action {
                        FindBarAction::QueryChanged(new_query) => {
                            if let Some(tab) = self.active_pdf_tab_mut() {
                                tab.find_state.query = new_query.clone();
                                if new_query.trim().is_empty() {
                                    tab.find_state.clear_matches();
                                    tab.find_state.active_query_id += 1;
                                    let doc_id = tab.doc_id;
                                    let query_id = tab.find_state.active_query_id;
                                    let _ = self.tx_cmd.send(Command::Search {
                                        doc_id,
                                        query_id,
                                        query: String::new(),
                                    });
                                } else {
                                    tab.find_state.last_edit_time = Some(Instant::now());
                                }
                            }
                        }
                        FindBarAction::NextMatch => {
                            if let Some(tab) = self.active_pdf_tab_mut() {
                                if let Some(idx) = tab.find_state.next_match() {
                                    if let Some(m) = tab.find_state.matches.get(idx) {
                                        let offset = compute_match_scroll_offset(
                                            m,
                                            &tab.page_sizes,
                                            tab.zoom,
                                            tab.rotation,
                                            tab.last_viewport_height,
                                            avail_w,
                                        );
                                        tab.pending_set_scroll_offset_y = offset;
                                    }
                                }
                            }
                        }
                        FindBarAction::PrevMatch => {
                            if let Some(tab) = self.active_pdf_tab_mut() {
                                if let Some(idx) = tab.find_state.prev_match() {
                                    if let Some(m) = tab.find_state.matches.get(idx) {
                                        let offset = compute_match_scroll_offset(
                                            m,
                                            &tab.page_sizes,
                                            tab.zoom,
                                            tab.rotation,
                                            tab.last_viewport_height,
                                            avail_w,
                                        );
                                        tab.pending_set_scroll_offset_y = offset;
                                    }
                                }
                            }
                        }
                        FindBarAction::Close => {
                            if let Some(tab) = self.active_pdf_tab_mut() {
                                tab.find_state.is_open = false;
                                tab.find_state.clear_matches();
                                let doc_id = tab.doc_id;
                                let query_id = tab.find_state.active_query_id;
                                let _ = self.tx_cmd.send(Command::Search {
                                    doc_id,
                                    query_id,
                                    query: String::new(),
                                });
                                ctx.memory_mut(|m| {
                                    if let Some(id) = m.focused() {
                                        m.surrender_focus(id);
                                    }
                                });
                            }
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

                        let (zoom_disp, rot_disp) = self
                            .active_pdf_tab()
                            .map(|t| (t.zoom * 100.0, t.rotation))
                            .unwrap_or((100.0, 0));

                        ui.horizontal(|ui| {
                            ui.colored_label(
                                egui::Color32::from_rgb(0, 255, 128),
                                format!(
                                    "F3 Debug | Tabs: {} | Zoom: {:.0}% | Rot: {}° | Cache: {:.2} MB / {:.0} MB ({} cached) | Frame: {:.1} ms ({:.0} FPS)",
                                    self.tabs.len(),
                                    zoom_disp,
                                    rot_disp,
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

        // TOC Sidebar (Left panel in document body)
        if !self.is_fullscreen {
            let sidebar_state = self.active_pdf_tab().and_then(|t| {
                if t.sidebar_open && (t.page_count > 0 || t.is_loading) {
                    Some((
                        t.sidebar_width,
                        t.outline.clone(),
                        t.outline_loading,
                        t.current_page,
                    ))
                } else {
                    None
                }
            });

            if let Some((sidebar_width, outline, outline_loading, current_page)) = sidebar_state {
                let mut sidebar_out = None;
                let panel_res = egui::SidePanel::left("toc_sidebar")
                    .resizable(true)
                    .default_width(sidebar_width)
                    .min_width(180.0)
                    .max_width(600.0)
                    .frame(
                        egui::Frame::side_top_panel(&ctx.style())
                            .fill(egui::Color32::from_rgb(26, 26, 28))
                            .inner_margin(egui::Margin::symmetric(10, 8)),
                    )
                    .show(ctx, |ui| {
                        sidebar_out = Some(render_sidebar_toc(
                            ui,
                            outline.as_deref(),
                            outline_loading,
                            current_page,
                        ));
                    });

                let actual_width = panel_res.response.rect.width();
                if let Some(tab) = self.active_pdf_tab_mut() {
                    tab.sidebar_width = actual_width;
                    if let Some(out) = sidebar_out {
                        if out.close_requested {
                            tab.sidebar_open = false;
                        }
                        if let Some(target) = out.target_page {
                            tab.pending_scroll_to_page = Some(target);
                        }
                    }
                }
            }
        }

        // Main Central Panel
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
                #[allow(clippy::large_enum_variant)]
                enum ActiveTabState {
                    Loading(String),
                    EmptyTab,
                    Ready {
                        doc_id: DocId,
                        page_sizes: Vec<PageSize>,
                        zoom: f32,
                        rotation: u16,
                        scroll_to_page: Option<usize>,
                        scroll_delta: Option<f32>,
                        set_scroll: Option<f32>,
                        search_matches: Vec<SearchMatch>,
                        active_match_idx: Option<usize>,
                        selection: Option<TextSelection>,
                        drag_start: Option<TextLocation>,
                        context_menu: Option<ContextMenuState>,
                        pending_text_requests: HashSet<usize>,
                        active_tool: ActiveTool,
                        annotations: DocumentAnnotations,
                        undo_stack: UndoStack,
                        annotations_dirty: bool,
                        freehand_stroke: Option<(usize, Vec<[f32; 2]>)>,
                    },
                    Error(String),
                    Settings,
                    None,
                }

                let active_state = match self.tabs.get_mut(self.active_tab_idx) {
                    Some(Tab::Pdf(tab)) => {
                        if tab.is_loading {
                            ActiveTabState::Loading(tab.loading_message.clone())
                        } else if tab.page_count == 0 {
                            if tab.path.as_os_str().is_empty() {
                                ActiveTabState::EmptyTab
                            } else {
                                ActiveTabState::Error(tab.loading_message.clone())
                            }
                        } else {
                            ActiveTabState::Ready {
                                doc_id: tab.doc_id,
                                page_sizes: tab.page_sizes.clone(),
                                zoom: tab.zoom,
                                rotation: tab.rotation,
                                scroll_to_page: tab.pending_scroll_to_page.take(),
                                scroll_delta: tab.pending_scroll_delta_y.take(),
                                set_scroll: tab.pending_set_scroll_offset_y.take(),
                                search_matches: tab.find_state.matches.clone(),
                                active_match_idx: tab.find_state.current_match_idx,
                                selection: tab.selection,
                                drag_start: tab.drag_start,
                                context_menu: tab.context_menu,
                                pending_text_requests: tab.pending_text_requests.clone(),
                                active_tool: tab.active_tool,
                                annotations: std::mem::take(&mut tab.annotations),
                                undo_stack: std::mem::take(&mut tab.undo_stack),
                                annotations_dirty: tab.annotations_dirty,
                                freehand_stroke: tab.freehand_stroke.clone(),
                            }
                        }
                    }
                    Some(Tab::Settings) => ActiveTabState::Settings,
                    None => ActiveTabState::None,
                };

                match active_state {
                    ActiveTabState::Loading(msg) => {
                        ui.vertical_centered(|ui| {
                            ui.add_space(80.0);
                            ui.heading(&msg);
                            ui.add_space(10.0);
                            ui.spinner();
                        });
                    }
                    ActiveTabState::EmptyTab => {
                        ui.vertical_centered(|ui| {
                            ui.add_space(100.0);
                            ui.heading(egui::RichText::new("New Tab").size(24.0).strong());
                            ui.add_space(12.0);
                            ui.label(
                                egui::RichText::new(
                                    "Enter a PDF file path in the bar above or press Ctrl+L to open a document.",
                                )
                                .color(egui::Color32::from_rgb(170, 170, 175))
                                .size(14.0),
                            );
                            ui.add_space(20.0);
                            if ui
                                .button(egui::RichText::new("Open Document (Ctrl+L)").size(13.0))
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                            {
                                self.path_bar.expand_and_focus();
                            }
                        });
                    }
                    ActiveTabState::Ready {
                        doc_id,
                        page_sizes,
                        zoom,
                        rotation,
                        scroll_to_page,
                        scroll_delta,
                        set_scroll,
                        search_matches,
                        active_match_idx,
                        mut selection,
                        mut drag_start,
                        mut context_menu,
                        pending_text_requests,
                        active_tool,
                        mut annotations,
                        mut undo_stack,
                        mut annotations_dirty,
                        mut freehand_stroke,
                    } => {
                        let output = render_viewer(
                            ui,
                            doc_id,
                            &page_sizes,
                            zoom,
                            rotation,
                            self.render_generation,
                            &mut self.cache,
                            &mut self.text_cache,
                            &self.pending_render_requests,
                            &pending_text_requests,
                            scroll_to_page,
                            scroll_delta,
                            set_scroll,
                            &search_matches,
                            active_match_idx,
                            is_pinching,
                            &mut selection,
                            &mut drag_start,
                            &mut context_menu,
                            active_tool,
                            self.settings.highlighter_color,
                            self.settings.highlighter_thickness,
                            &mut annotations,
                            &mut undo_stack,
                            &mut annotations_dirty,
                            &mut freehand_stroke,
                        );

                        if let Some(action) = output.action {
                            match action {
                                ViewerAction::Copy => {
                                    self.copy_selection(ui.ctx());
                                }
                                ViewerAction::SelectAllPage(p) => {
                                    if let Some(Tab::Pdf(tab)) =
                                        self.tabs.get_mut(self.active_tab_idx)
                                    {
                                        tab.selection = Some(TextSelection::new(
                                             TextLocation::new(p, 0),
                                             TextLocation::new(p, usize::MAX),
                                        ));
                                        tab.drag_start = None;
                                        selection = tab.selection;
                                        drag_start = None;
                                    }
                                }
                                ViewerAction::SelectAllDocument => {
                                    if let Some(Tab::Pdf(tab)) =
                                        self.tabs.get_mut(self.active_tab_idx)
                                    {
                                        let last_p = tab.page_count.saturating_sub(1);
                                        tab.selection = Some(TextSelection::new(
                                             TextLocation::new(0, 0),
                                             TextLocation::new(last_p, usize::MAX),
                                        ));
                                        tab.drag_start = None;
                                        selection = tab.selection;
                                        drag_start = None;
                                    }
                                }
                            }
                        }

                        if let Some(Tab::Pdf(tab)) = self.tabs.get_mut(self.active_tab_idx) {
                            tab.current_page = output.current_page;
                            tab.last_viewport_height = output.visible_viewport_height;
                            tab.last_viewport_top = output.viewport_top;
                            tab.last_scroll_offset_y = output.current_scroll_offset_y;
                            tab.scroll_offset_y = output.current_scroll_offset_y;
                            tab.selection = selection;
                            tab.drag_start = drag_start;
                            tab.context_menu = context_menu;
                            tab.annotations = annotations;
                            tab.undo_stack = undo_stack;
                            tab.freehand_stroke = freehand_stroke;
                            if output.annotations_changed {
                                tab.has_unsaved_changes = true;
                                tab.annotations_dirty = true;
                                tab.last_annotation_change = Some(Instant::now());
                                let _ = self.tx_cmd.send(Command::SyncAnnotations {
                                    doc_id,
                                    highlights: tab.annotations.highlights.clone(),
                                });
                                self.render_generation += 1;
                                self.pending_render_requests.clear();
                                ctx.request_repaint();
                            }
                            for req_page in output.text_requests {
                                if tab.pending_text_requests.insert(req_page) {
                                    let _ = self.tx_cmd.send(Command::PageText {
                                        doc_id,
                                        page: req_page,
                                    });
                                }
                            }
                        }

                        for req in output.requests {
                            self.request_render(req);
                        }
                    }
                    ActiveTabState::Error(err) => {
                        ui.vertical_centered(|ui| {
                            ui.add_space(80.0);
                            ui.heading("Unable to display document");
                            ui.add_space(8.0);
                            ui.label(&err);
                        });
                    }
                    ActiveTabState::Settings => {
                        ui.vertical_centered(|ui| {
                            ui.add_space(80.0);
                            ui.heading("Settings");
                            ui.add_space(8.0);
                            ui.label("Settings options will be available in a later milestone.");
                        });
                    }
                    ActiveTabState::None => {
                        ui.vertical_centered(|ui| {
                            ui.add_space(100.0);
                            ui.heading("No document open");
                            ui.add_space(8.0);
                            ui.label(
                                "Enter a PDF file path above or press Ctrl+L to open a document.",
                            );
                            ui.add_space(16.0);
                            if ui
                                .button("Open Document (Ctrl+L)")
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .clicked()
                            {
                                self.path_bar.expand_and_focus();
                            }
                        });
                    }
                }
            }
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        for tab_item in &mut self.tabs {
            if let Tab::Pdf(ref mut tab) = tab_item {
                if tab.annotations_dirty && !tab.file_key.is_empty() {
                    let _ = tab.annotations.save_for_key(&tab.file_key);
                    tab.annotations_dirty = false;
                }
            }
        }
        let _ = self.settings.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_touchpad_scroll_multiplier_logic() {
        let mut settings = Settings::default();
        assert_eq!(settings.touchpad_scroll_speed, 2.5);
        assert_eq!(settings.mouse_wheel_speed, 1.0);

        // When touchpad (Point) scroll occurs, multiplier should be settings.touchpad_scroll_speed
        let delta = egui::vec2(0.0, 10.0);
        let speed = settings.touchpad_scroll_speed.clamp(0.5, 10.0);
        let scaled = delta * speed;
        assert_eq!(scaled.y, 25.0);

        // Custom setting
        settings.touchpad_scroll_speed = 3.5;
        let speed2 = settings.touchpad_scroll_speed.clamp(0.5, 10.0);
        let scaled2 = delta * speed2;
        assert_eq!(scaled2.y, 35.0);
    }
}
