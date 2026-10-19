use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};

use crate::annotations::{DocumentAnnotations, EditingTextBox, UndoStack};
use crate::cache::{PageTextCache, TextureCache, TextureKey};
use crate::pdf::types::{
    DocId, OutlineItem, PageSize, RenderCommand, SearchMatch, TextLocation, TextSelection,
};
use crate::pdf::{start_worker, Command, Event, PageTransform};
use crate::settings::Settings;
use crate::ui::findbar::{render_find_bar, FindBarAction, FindState};
use crate::ui::icons::{render_adobe_button, render_adobe_checkbox};
use crate::ui::pathbar::{parse_pdf_path, PathBar, PathBarAction};
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
    pub editing_text_box: Option<EditingTextBox>,
    pub selected_text_box_id: Option<u64>,
    pub dragging_text_box: Option<(u64, egui::Pos2, [f32; 2])>,
    pub resizing_text_box: Option<(Option<u64>, egui::Pos2, f32)>,
    pub nav_history: Vec<(usize, f32)>,
    pub nav_future: Vec<(usize, f32)>,
    pub last_accessed: Instant,
    pub is_dormant: bool,
}

impl PdfTab {
    /// Marks the tab as dormant to free RAM, returning true if state changed.
    #[allow(dead_code)]
    pub fn deactivate(&mut self) -> bool {
        if !self.is_dormant && !self.path.as_os_str().is_empty() {
            self.is_dormant = true;
            true
        } else {
            false
        }
    }

    /// Wakes up a dormant tab and refreshes its activity timestamp.
    pub fn wake_up(&mut self) {
        self.is_dormant = false;
        self.last_accessed = Instant::now();
    }

    /// Records a discontinuous navigation jump (e.g. from page box, TOC, search, Home/End).
    pub fn record_nav_jump(&mut self) {
        let current_entry = (self.current_page, self.scroll_offset_y);
        // Avoid duplicate entries if jump is within the same page and nearly identical offset
        if let Some(&(last_page, last_offset)) = self.nav_history.last() {
            if last_page == current_entry.0 && (last_offset - current_entry.1).abs() < 50.0 {
                return;
            }
        }
        self.nav_history.push(current_entry);
        if self.nav_history.len() > 50 {
            self.nav_history.remove(0);
        }
        self.nav_future.clear();
    }

    /// Returns true if backwards navigation is available.
    pub fn can_go_back(&self) -> bool {
        !self.nav_history.is_empty()
    }

    /// Returns true if forward navigation is available.
    pub fn can_go_forward(&self) -> bool {
        !self.nav_future.is_empty()
    }

    /// Navigates back in history stack.
    pub fn go_back(&mut self) {
        if let Some((target_page, target_offset)) = self.nav_history.pop() {
            self.nav_future
                .push((self.current_page, self.scroll_offset_y));
            if self.nav_future.len() > 50 {
                self.nav_future.remove(0);
            }
            if target_offset > 0.0 {
                self.pending_set_scroll_offset_y = Some(target_offset);
                self.pending_scroll_to_page = None;
            } else {
                self.pending_scroll_to_page = Some(target_page);
                self.pending_set_scroll_offset_y = None;
            }
        }
    }

    /// Navigates forward in history stack.
    pub fn go_forward(&mut self) {
        if let Some((target_page, target_offset)) = self.nav_future.pop() {
            self.nav_history
                .push((self.current_page, self.scroll_offset_y));
            if self.nav_history.len() > 50 {
                self.nav_future.remove(0);
            }
            if target_offset > 0.0 {
                self.pending_set_scroll_offset_y = Some(target_offset);
                self.pending_scroll_to_page = None;
            } else {
                self.pending_scroll_to_page = Some(target_page);
                self.pending_set_scroll_offset_y = None;
            }
        }
    }
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

/// Target to close upon confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseTarget {
    Tab(usize),
    Window,
}

/// Confirmation state for closing documents with unsaved changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingCloseConfirmation {
    pub target: CloseTarget,
    pub tab_idx: usize,
    pub doc_title: String,
}

/// System metrics tracking memory and CPU usage for F3 debug stats.
#[derive(Debug, Clone)]
pub struct SystemMetrics {
    pub rss_mb: f32,
    pub cpu_percent: f32,
    last_sample_instant: Instant,
    last_cpu_ticks: u64,
}

impl Default for SystemMetrics {
    fn default() -> Self {
        Self {
            rss_mb: 0.0,
            cpu_percent: 0.0,
            last_sample_instant: Instant::now(),
            last_cpu_ticks: 0,
        }
    }
}

impl SystemMetrics {
    /// Samples RSS physical memory and CPU tick delta on Linux.
    pub fn update(&mut self) {
        let elapsed = self.last_sample_instant.elapsed().as_secs_f32();
        if elapsed < 0.35 {
            return;
        }

        #[cfg(target_os = "linux")]
        {
            // Read Resident Set Size (RSS) memory from /proc/self/status
            if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
                for line in status.lines() {
                    if let Some(rest) = line.strip_prefix("VmRSS:") {
                        let num_part = rest.split_whitespace().next().unwrap_or("0");
                        if let Ok(kb) = num_part.parse::<f32>() {
                            self.rss_mb = kb / 1024.0;
                            break;
                        }
                    }
                }
            }

            // Read CPU ticks from /proc/self/stat
            if let Ok(stat) = std::fs::read_to_string("/proc/self/stat") {
                if let Some(r_paren) = stat.rfind(')') {
                    let after_comm = &stat[r_paren + 1..];
                    let fields: Vec<&str> = after_comm.split_whitespace().collect();
                    if fields.len() > 13 {
                        let utime = fields[11].parse::<u64>().unwrap_or(0);
                        let stime = fields[12].parse::<u64>().unwrap_or(0);
                        let total_ticks = utime + stime;
                        if self.last_cpu_ticks > 0 && total_ticks >= self.last_cpu_ticks {
                            let delta_ticks = total_ticks - self.last_cpu_ticks;
                            // Linux CLK_TCK standard 100 Hz
                            let cpu = (delta_ticks as f32 / (elapsed * 100.0)) * 100.0;
                            self.cpu_percent = cpu.clamp(0.0, 100.0);
                        }
                        self.last_cpu_ticks = total_ticks;
                    }
                }
            }
        }

        self.last_sample_instant = Instant::now();
    }
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
    pub pending_close_confirm: Option<PendingCloseConfirmation>,

    // UI Chrome
    pub path_bar: PathBar,
    pub toolbar: Toolbar,
    pub is_fullscreen: bool,

    // Startup and file dialog
    pub pending_open_path: Option<PathBuf>,
    pub rx_file_chooser: Option<crossbeam_channel::Receiver<Option<PathBuf>>>,

    // Cache and active requests
    pub cache: TextureCache,
    pub text_cache: PageTextCache,
    pub pending_render_requests: HashSet<(usize, u32, u16)>,
    pub render_generation: u64,

    // Debugging (F3) and System Metrics
    pub debug_visible: bool,
    pub last_frame_time: Instant,
    pub frame_time_ms: f32,
    pub metrics: SystemMetrics,

    #[cfg(target_os = "linux")]
    pub rx_pinch_gesture:
        Option<crossbeam_channel::Receiver<crate::platform::wayland::WaylandPinchEvent>>,
    #[cfg(target_os = "linux")]
    pub wayland_gesture_init_attempted: bool,
    pub pinch_active: bool,
    pub pinch_last_active: Option<Instant>,
    pub last_scroll_unit: Option<egui::MouseWheelUnit>,
    pub last_window_title: String,
    pub settings: Settings,
    pub draft_settings: Settings,
    pub reading_history: crate::settings::ReadingHistory,
    pub last_session_save: Option<Instant>,
    pub session_restored: bool,
    pub settings_saved_toast: Option<Instant>,
    pub show_about_modal: bool,
    pub fullscreen_quickbar_open: bool,
    pub fullscreen_page_input: String,
    pub fullscreen_is_editing_page: bool,
    #[cfg(unix)]
    pub _ipc_server: Option<crate::platform::ipc::IpcServer>,
    #[cfg(unix)]
    pub rx_ipc: Option<crossbeam_channel::Receiver<crate::platform::ipc::IpcMessage>>,
}

impl PdfViewApp {
    pub fn new(cc: &eframe::CreationContext<'_>, initial_path: Option<String>) -> Self {
        crate::ui::theme::Theme::apply(&cc.egui_ctx);

        let (tx_cmd, rx_evt) = start_worker(cc.egui_ctx.clone());

        #[cfg(unix)]
        let (ipc_server, rx_ipc) = match crate::platform::ipc::IpcServer::start(cc.egui_ctx.clone())
        {
            Some((srv, rx)) => (Some(srv), Some(rx)),
            None => (None, None),
        };

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

        let loaded_settings = Settings::load();
        let reading_history = Settings::load_history();
        let mut texture_cache = TextureCache::default();
        texture_cache.set_max_bytes(loaded_settings.cache_size_mb * 1024 * 1024);

        let mut app = Self {
            tx_cmd,
            rx_evt,
            pdfium_status: PdfiumStatus::Initializing,
            tabs: Vec::new(),
            active_tab_idx: 0,
            next_doc_id: 1,
            pending_close_confirm: None,
            path_bar,
            toolbar: Toolbar::new(),
            is_fullscreen: false,
            pending_open_path,
            rx_file_chooser: None,
            cache: texture_cache,
            text_cache: PageTextCache::default(),
            pending_render_requests: HashSet::new(),
            render_generation: 0,
            debug_visible: false,
            last_frame_time: Instant::now(),
            frame_time_ms: 16.0,
            metrics: SystemMetrics::default(),
            #[cfg(target_os = "linux")]
            rx_pinch_gesture: None,
            #[cfg(target_os = "linux")]
            wayland_gesture_init_attempted: false,
            pinch_active: false,
            pinch_last_active: None,
            last_scroll_unit: None,
            last_window_title: "Verso Reader".to_string(),
            draft_settings: loaded_settings.clone(),
            settings: loaded_settings,
            reading_history,
            last_session_save: None,
            session_restored: false,
            settings_saved_toast: None,
            show_about_modal: false,
            fullscreen_quickbar_open: false,
            fullscreen_page_input: "1".to_string(),
            fullscreen_is_editing_page: false,
            #[cfg(unix)]
            _ipc_server: ipc_server,
            #[cfg(unix)]
            rx_ipc,
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
            editing_text_box: None,
            selected_text_box_id: None,
            dragging_text_box: None,
            resizing_text_box: None,
            nav_history: Vec::new(),
            nav_future: Vec::new(),
            last_accessed: Instant::now(),
            is_dormant: false,
        };

        self.tabs.push(Tab::Pdf(tab));
        self.active_tab_idx = self.tabs.len() - 1;
        self.pending_render_requests.clear();
        self.path_bar.set_text(String::new());
        self.path_bar.expand_and_focus();
    }

    /// Opens a PDF file with restored reading position, zoom, and rotation.
    pub fn open_pdf_tab_with_state(
        &mut self,
        path: PathBuf,
        page: usize,
        scroll_offset_y: f32,
        zoom: f32,
        rotation: u16,
    ) {
        self.path_bar.set_text(path.display().to_string());
        self.path_bar.set_error(None);
        self.path_bar.is_collapsed = true;

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

        let pending_scroll_to_page = if scroll_offset_y > 0.0 {
            None
        } else {
            Some(page.max(1))
        };
        let pending_set_scroll_offset_y = if scroll_offset_y > 0.0 {
            Some(scroll_offset_y)
        } else {
            None
        };

        if active_is_empty {
            if let Some(Tab::Pdf(tab)) = self.tabs.get_mut(self.active_tab_idx) {
                tab.path = path.clone();
                tab.title = title;
                tab.page_count = 0;
                tab.page_sizes.clear();
                tab.current_page = page.max(1);
                tab.zoom = zoom.clamp(0.1, 8.0);
                tab.rotation = rotation;
                tab.scroll_offset_y = scroll_offset_y;
                tab.pending_scroll_to_page = pending_scroll_to_page;
                tab.pending_scroll_delta_y = None;
                tab.pending_set_scroll_offset_y = pending_set_scroll_offset_y;
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
                tab.has_unsaved_changes = false;
                tab.freehand_stroke = None;
                tab.editing_text_box = None;
                tab.selected_text_box_id = None;
                tab.dragging_text_box = None;
                tab.resizing_text_box = None;
                tab.nav_history.clear();
                tab.nav_future.clear();
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
            current_page: page.max(1),
            zoom: zoom.clamp(0.1, 8.0),
            rotation,
            scroll_offset_y,
            pending_scroll_to_page,
            pending_scroll_delta_y: None,
            pending_set_scroll_offset_y,
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
            editing_text_box: None,
            selected_text_box_id: None,
            dragging_text_box: None,
            resizing_text_box: None,
            nav_history: Vec::new(),
            nav_future: Vec::new(),
            last_accessed: Instant::now(),
            is_dormant: false,
        };

        // If the current active tab is an empty placeholder tab (e.g. "New Tab" with no file),
        // replace it with this document instead of leaving an empty tab behind.
        let replace_active_empty = match self.tabs.get(self.active_tab_idx) {
            Some(Tab::Pdf(t)) => t.path.as_os_str().is_empty() && t.page_count == 0,
            _ => false,
        };

        if replace_active_empty {
            self.tabs[self.active_tab_idx] = Tab::Pdf(tab);
        } else {
            // If switching from an existing tab, drop its textures from cache
            if let Some(prev) = self.active_pdf_tab() {
                self.cache.clear_doc(prev.doc_id);
            }
            self.tabs.push(Tab::Pdf(tab));
            self.active_tab_idx = self.tabs.len() - 1;
        }

        self.pending_render_requests.clear();

        let _ = self.tx_cmd.send(Command::Open { doc_id, path });
    }

    /// Opens a PDF file. Restores reading position if available, or starts at page 1.
    pub fn open_pdf_tab(&mut self, path: PathBuf) {
        let (initial_page, initial_scroll, initial_zoom, initial_rotation) =
            if self.settings.remember_reading_position {
                let key = Settings::canonical_path_key(&path);
                if let Some(pos) = self.reading_history.entries.get(&key) {
                    (
                        pos.current_page.max(1),
                        pos.scroll_offset_y.max(0.0),
                        pos.zoom.clamp(0.1, 8.0),
                        pos.rotation,
                    )
                } else {
                    (1, 0.0, 1.0, 0)
                }
            } else {
                (1, 0.0, 1.0, 0)
            };

        self.open_pdf_tab_with_state(
            path,
            initial_page,
            initial_scroll,
            initial_zoom,
            initial_rotation,
        );
    }

    /// Restores previously open tabs from saved session state.
    pub fn restore_saved_session(&mut self) {
        let Some(session) = Settings::load_session() else {
            return;
        };
        let valid_tabs: Vec<_> = session
            .tabs
            .into_iter()
            .filter(|t| t.path.exists())
            .collect();
        if valid_tabs.is_empty() {
            return;
        }

        let target_active_idx = session.active_tab_index;
        for tab_state in valid_tabs {
            self.open_pdf_tab_with_state(
                tab_state.path,
                tab_state.current_page,
                tab_state.scroll_offset_y,
                tab_state.zoom,
                tab_state.rotation,
            );
        }

        if target_active_idx < self.tabs.len() {
            self.active_tab_idx = target_active_idx;
        } else if !self.tabs.is_empty() {
            self.active_tab_idx = self.tabs.len() - 1;
        }

        if let Some(curr) = self.active_pdf_tab_mut() {
            curr.pending_set_scroll_offset_y = Some(curr.scroll_offset_y);
            let path_str = if curr.path.as_os_str().is_empty() {
                String::new()
            } else {
                curr.path.display().to_string()
            };
            self.path_bar.set_text(path_str);
            self.path_bar.set_error(None);
        }
    }

    /// Records the reading position for the tab at index into history.
    pub fn update_reading_history_for_tab(&mut self, tab_idx: usize) {
        if let Some(Tab::Pdf(tab)) = self.tabs.get(tab_idx) {
            if !tab.path.as_os_str().is_empty() && tab.page_count > 0 {
                let key = Settings::canonical_path_key(&tab.path);
                let now_ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                self.reading_history.entries.insert(
                    key,
                    crate::settings::DocumentPosition {
                        scroll_offset_y: tab.scroll_offset_y,
                        current_page: tab.current_page,
                        zoom: tab.zoom,
                        rotation: tab.rotation,
                        updated_at: now_ts,
                    },
                );

                if self.reading_history.entries.len() > 100 {
                    if let Some(oldest_key) = self
                        .reading_history
                        .entries
                        .iter()
                        .min_by_key(|(_, pos)| pos.updated_at)
                        .map(|(k, _)| k.clone())
                    {
                        self.reading_history.entries.remove(&oldest_key);
                    }
                }
            }
        }
    }

    /// Saves session tabs to `session.json` and reading positions to `history.json`.
    pub fn save_session_and_history(&mut self) {
        if !self.session_restored {
            return;
        }
        for idx in 0..self.tabs.len() {
            self.update_reading_history_for_tab(idx);
        }
        if self.settings.remember_reading_position {
            Settings::save_history(&self.reading_history);
        }

        if self.settings.restore_session {
            let mut session_tabs = Vec::new();
            for tab_item in &self.tabs {
                if let Tab::Pdf(tab) = tab_item {
                    if !tab.path.as_os_str().is_empty() {
                        session_tabs.push(crate::settings::SessionTab {
                            path: tab.path.clone(),
                            scroll_offset_y: tab.scroll_offset_y,
                            current_page: tab.current_page,
                            zoom: tab.zoom,
                            rotation: tab.rotation,
                        });
                    }
                }
            }
            let session = crate::settings::SessionState {
                tabs: session_tabs,
                active_tab_index: self.active_tab_idx,
            };
            Settings::save_session(&session);
        } else {
            Settings::save_session(&crate::settings::SessionState::default());
        }
    }

    /// Requests closing the tab at index. If it has unsaved changes, presents a confirmation modal.
    pub fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }

        if let Tab::Pdf(ref tab) = self.tabs[index] {
            if tab.has_unsaved_changes {
                self.pending_close_confirm = Some(PendingCloseConfirmation {
                    target: CloseTarget::Tab(index),
                    tab_idx: index,
                    doc_title: tab.title.clone(),
                });
                return;
            }
        }

        self.force_close_tab(index);
    }

    /// Forcibly closes the tab at the given index, freeing its textures and sending Command::Close.
    pub fn force_close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }

        self.update_reading_history_for_tab(index);

        let closed = self.tabs.remove(index);
        if let Tab::Pdf(tab) = closed {
            let _ = self.tx_cmd.send(Command::Close { doc_id: tab.doc_id });
            self.cache.clear_doc(tab.doc_id);
            self.text_cache.clear_doc(tab.doc_id);
        }

        if self.tabs.is_empty() {
            self.create_empty_tab();
            self.save_session_and_history();
            return;
        }

        if self.active_tab_idx >= self.tabs.len() {
            self.active_tab_idx = self.tabs.len().saturating_sub(1);
        }

        self.render_generation += 1;
        self.pending_render_requests.clear();

        // If new active tab exists, restore its scroll offset and synchronize path bar
        let tab_path_info = if let Some(curr) = self.active_pdf_tab_mut() {
            curr.pending_set_scroll_offset_y = Some(curr.scroll_offset_y);
            let path_str = if curr.path.as_os_str().is_empty() {
                String::new()
            } else {
                curr.path.display().to_string()
            };
            let is_empty = curr.path.as_os_str().is_empty() && curr.page_count == 0;
            Some((path_str, is_empty))
        } else {
            None
        };

        if let Some((path_str, is_empty)) = tab_path_info {
            self.path_bar.set_text(path_str);
            self.path_bar.set_error(None);
            self.path_bar.is_collapsed = !is_empty;
        } else {
            self.path_bar.set_text(String::new());
            self.path_bar.set_error(None);
            self.path_bar.is_collapsed = false;
        }

        self.save_session_and_history();
        self.ensure_active_tab_awake();
    }

    /// Evaluates background tabs for dormancy: unloads textures, text cache, and Pdfium document handles
    /// for tabs that have been inactive for longer than the configured timeout, freeing RAM without losing state.
    pub fn check_dormant_tabs(&mut self) {
        if self.settings.inactivity_timeout_mins == 0 {
            return;
        }
        let timeout_secs = (self.settings.inactivity_timeout_mins as u64) * 60;
        let dormant_timeout = std::time::Duration::from_secs(timeout_secs);
        let active_doc_id = self.active_pdf_tab().map(|t| t.doc_id);
        let now = Instant::now();

        let mut to_deactivate = Vec::new();
        for tab_item in &mut self.tabs {
            if let Tab::Pdf(ref mut tab) = tab_item {
                if Some(tab.doc_id) != active_doc_id
                    && !tab.is_dormant
                    && !tab.path.as_os_str().is_empty()
                    && now.duration_since(tab.last_accessed) >= dormant_timeout
                {
                    tab.is_dormant = true;
                    to_deactivate.push(tab.doc_id);
                }
            }
        }

        for doc_id in to_deactivate {
            self.cache.clear_doc(doc_id);
            self.text_cache.clear_doc(doc_id);
            let _ = self.tx_cmd.send(Command::UnloadDoc { doc_id });
        }
    }

    /// Ensures the active tab is awakened from dormancy and reloaded from disk if needed.
    pub fn ensure_active_tab_awake(&mut self) {
        let wakeup_info = match self.tabs.get_mut(self.active_tab_idx) {
            Some(Tab::Pdf(curr)) if curr.is_dormant && !curr.path.as_os_str().is_empty() => {
                curr.wake_up();
                Some((
                    curr.doc_id,
                    curr.path.clone(),
                    curr.has_unsaved_changes,
                    curr.annotations.highlights.clone(),
                    curr.annotations.text_boxes.clone(),
                ))
            }
            Some(Tab::Pdf(curr)) => {
                curr.last_accessed = Instant::now();
                None
            }
            _ => None,
        };

        if let Some((doc_id, path, unsaved, hl, tb)) = wakeup_info {
            let _ = self.tx_cmd.send(Command::Open { doc_id, path });
            if unsaved {
                let _ = self.tx_cmd.send(Command::SyncAnnotations {
                    doc_id,
                    highlights: hl,
                    text_boxes: tb,
                });
            }
        }
    }

    /// Switches active tab to target_idx, dropping the previous tab's textures from cache.
    pub fn switch_to_tab(&mut self, target_idx: usize) {
        if target_idx == self.active_tab_idx || target_idx >= self.tabs.len() {
            return;
        }

        self.update_reading_history_for_tab(self.active_tab_idx);

        // Drop previous tab's cached textures
        if let Some(prev) = self.active_pdf_tab() {
            self.cache.clear_doc(prev.doc_id);
        }

        self.active_tab_idx = target_idx;
        self.render_generation += 1;
        self.pending_render_requests.clear();
        self.save_session_and_history();
        self.ensure_active_tab_awake();

        // Restore scroll offset for newly active tab and synchronize path bar
        let tab_path_info = if let Some(curr) = self.active_pdf_tab_mut() {
            curr.pending_set_scroll_offset_y = Some(curr.scroll_offset_y);
            let path_str = if curr.path.as_os_str().is_empty() {
                String::new()
            } else {
                curr.path.display().to_string()
            };
            let is_empty = curr.path.as_os_str().is_empty() && curr.page_count == 0;
            Some((path_str, is_empty))
        } else {
            None
        };

        if let Some((path_str, is_empty)) = tab_path_info {
            self.path_bar.set_text(path_str);
            self.path_bar.set_error(None);
            self.path_bar.is_collapsed = !is_empty;
        } else {
            self.path_bar.set_text(String::new());
            self.path_bar.set_error(None);
            self.path_bar.is_collapsed = false;
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

    /// Saves annotations directly into the specified PDF tab.
    pub fn save_pdf_tab(&mut self, index: usize) {
        if let Some(Tab::Pdf(tab)) = self.tabs.get_mut(index) {
            if !tab.is_loading && tab.page_count > 0 && !tab.path.as_os_str().is_empty() {
                // If a text box was actively being edited, commit it before saving
                if let Some(etb) = tab.editing_text_box.take() {
                    if let Some(tb) = etb.to_text_box() {
                        if let Some(existing) = tab
                            .annotations
                            .text_boxes
                            .iter_mut()
                            .find(|x| x.id == tb.id)
                        {
                            *existing = tb;
                        } else {
                            tab.annotations.text_boxes.push(tb);
                        }
                    }
                }
                // Ensure local sidecar JSON is up to date
                if !tab.file_key.is_empty() {
                    let _ = tab.annotations.save_for_key(&tab.file_key);
                    tab.annotations_dirty = false;
                }
                let _ = self.tx_cmd.send(Command::SaveDoc {
                    doc_id: tab.doc_id,
                    path: tab.path.clone(),
                    highlights: tab.annotations.highlights.clone(),
                    text_boxes: tab.annotations.text_boxes.clone(),
                });
            }
        }
    }

    /// Saves annotations directly into the active PDF file.
    pub fn save_active_pdf(&mut self) {
        self.save_pdf_tab(self.active_tab_idx);
    }

    /// Requests closing the window. If any tab has unsaved changes, asks for confirmation first.
    pub fn request_close_window(&mut self, ctx: &egui::Context) {
        if let Some((idx, title)) = self.tabs.iter().enumerate().find_map(|(i, t)| {
            if let Tab::Pdf(pdf) = t {
                if pdf.has_unsaved_changes {
                    return Some((i, pdf.title.clone()));
                }
            }
            None
        }) {
            self.pending_close_confirm = Some(PendingCloseConfirmation {
                target: CloseTarget::Window,
                tab_idx: idx,
                doc_title: title,
            });
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// Launches the native desktop file picker on a background thread.
    pub fn open_native_file_dialog(&mut self, ctx: &egui::Context) {
        if self.rx_file_chooser.is_some() {
            return;
        }

        let (tx, rx) = crossbeam_channel::bounded(1);
        self.rx_file_chooser = Some(rx);
        let ctx_clone = ctx.clone();

        std::thread::Builder::new()
            .name("pdfview-file-dialog".to_string())
            .spawn(move || {
                let chosen = crate::platform::dialog::pick_pdf_file_native();
                let _ = tx.send(chosen);
                ctx_clone.request_repaint();
            })
            .expect("failed to spawn native file chooser thread");
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

        // Measure frame time for F3 debug stats
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_frame_time).as_secs_f32() * 1000.0;
        self.last_frame_time = now;
        self.frame_time_ms = self.frame_time_ms * 0.9 + elapsed * 0.1;

        // Synchronize viewport title with active document
        let desired_title = if let Some(tab) = self.active_pdf_tab() {
            if tab.title.is_empty() {
                "Verso Reader".to_string()
            } else {
                format!("{} — Verso Reader", tab.title)
            }
        } else {
            "Verso Reader".to_string()
        };

        if self.last_window_title != desired_title {
            self.last_window_title = desired_title.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(desired_title));
        }

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

        // Window close requested by OS or window manager
        if ctx.input(|i| i.viewport().close_requested()) {
            if let Some((idx, title)) = self.tabs.iter().enumerate().find_map(|(i, t)| {
                if let Tab::Pdf(pdf) = t {
                    if pdf.has_unsaved_changes {
                        return Some((i, pdf.title.clone()));
                    }
                }
                None
            }) {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.pending_close_confirm = Some(PendingCloseConfirmation {
                    target: CloseTarget::Window,
                    tab_idx: idx,
                    doc_title: title,
                });
            } else {
                self.save_session_and_history();
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
                        if let Some(sel_id) = tab.selected_text_box_id {
                            if !tab.annotations.text_boxes.iter().any(|tb| tb.id == sel_id) {
                                tab.selected_text_box_id = None;
                            }
                        }
                        Some((
                            tab.doc_id,
                            tab.annotations.highlights.clone(),
                            tab.annotations.text_boxes.clone(),
                        ))
                    } else {
                        None
                    }
                });
                if let Some((doc_id, highlights, text_boxes)) = undo_info {
                    let _ = self.tx_cmd.send(Command::SyncAnnotations {
                        doc_id,
                        highlights,
                        text_boxes,
                    });
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
                        if let Some(sel_id) = tab.selected_text_box_id {
                            if !tab.annotations.text_boxes.iter().any(|tb| tb.id == sel_id) {
                                tab.selected_text_box_id = None;
                            }
                        }
                        Some((
                            tab.doc_id,
                            tab.annotations.highlights.clone(),
                            tab.annotations.text_boxes.clone(),
                        ))
                    } else {
                        None
                    }
                });
                if let Some((doc_id, highlights, text_boxes)) = redo_info {
                    let _ = self.tx_cmd.send(Command::SyncAnnotations {
                        doc_id,
                        highlights,
                        text_boxes,
                    });
                    self.render_generation += 1;
                    self.pending_render_requests.clear();
                    ctx.request_repaint();
                }
            }

            // Delete selected text box shortcut: Delete
            if ctx.input(|i| i.key_pressed(egui::Key::Delete)) {
                let delete_info = self.active_pdf_tab_mut().and_then(|tab| {
                    if tab.editing_text_box.is_none() {
                        if let Some(sel_id) = tab.selected_text_box_id.take() {
                            if let Some(pos) = tab
                                .annotations
                                .text_boxes
                                .iter()
                                .position(|tb| tb.id == sel_id)
                            {
                                let removed = tab.annotations.text_boxes.remove(pos);
                                tab.undo_stack.push_delete_text_box(removed);
                                tab.annotations_dirty = true;
                                tab.has_unsaved_changes = true;
                                tab.last_annotation_change = Some(Instant::now());
                                return Some((
                                    tab.doc_id,
                                    tab.annotations.highlights.clone(),
                                    tab.annotations.text_boxes.clone(),
                                ));
                            }
                        }
                    }
                    None
                });
                if let Some((doc_id, highlights, text_boxes)) = delete_info {
                    let _ = self.tx_cmd.send(Command::SyncAnnotations {
                        doc_id,
                        highlights,
                        text_boxes,
                    });
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
                if tab.editing_text_box.is_some() {
                    tab.editing_text_box = None;
                } else if tab.selected_text_box_id.is_some() {
                    tab.selected_text_box_id = None;
                } else if tab.context_menu.is_some() {
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

        // Periodic debounced save of session and reading history every 3 seconds (only once session is restored)
        if self.session_restored
            && self
                .last_session_save
                .is_none_or(|t| t.elapsed() > Duration::from_secs(3))
        {
            self.last_session_save = Some(Instant::now());
            self.save_session_and_history();
        }

        // Keyboard navigation when no text input has focus and not pinching
        let any_focus = ctx.memory(|m| m.focused().is_some());
        if !any_focus && !ctrl_pressed && !is_pinching {
            if let Some(tab) = self.active_pdf_tab_mut() {
                if ctx.input(|i| i.modifiers.alt && i.key_pressed(egui::Key::ArrowLeft)) {
                    tab.go_back();
                } else if ctx.input(|i| i.modifiers.alt && i.key_pressed(egui::Key::ArrowRight)) {
                    tab.go_forward();
                } else if ctx.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
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
                    tab.record_nav_jump();
                    tab.pending_scroll_to_page = Some(1);
                } else if ctx.input(|i| i.key_pressed(egui::Key::End)) {
                    tab.record_nav_jump();
                    tab.pending_scroll_to_page = Some(tab.page_count);
                }
            }
        }

        // Drain native file chooser response non-blockingly
        if let Some(ref rx) = self.rx_file_chooser {
            match rx.try_recv() {
                Ok(Some(path)) => {
                    self.path_bar.set_text(path.display().to_string());
                    self.path_bar.set_error(None);
                    if self.pdfium_status == PdfiumStatus::Ready {
                        self.open_pdf_tab(path);
                    } else if self.pdfium_status == PdfiumStatus::Initializing {
                        self.pending_open_path = Some(path);
                    }
                    self.rx_file_chooser = None;
                }
                Ok(None) | Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.rx_file_chooser = None;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {}
            }
        }

        // Drain single-instance IPC requests non-blockingly
        #[cfg(unix)]
        let ipc_messages: Vec<_> = if let Some(ref rx) = self.rx_ipc {
            let mut msgs = Vec::new();
            while let Ok(msg) = rx.try_recv() {
                msgs.push(msg);
            }
            msgs
        } else {
            Vec::new()
        };

        #[cfg(unix)]
        for msg in ipc_messages {
            match msg {
                crate::platform::ipc::IpcMessage::Open(path) => {
                    let mut found_tab = None;
                    for (idx, tab) in self.tabs.iter().enumerate() {
                        if let Tab::Pdf(pdf_tab) = tab {
                            if pdf_tab.path == path {
                                found_tab = Some(idx);
                                break;
                            }
                        }
                    }
                    if let Some(idx) = found_tab {
                        self.active_tab_idx = idx;
                    } else if self.pdfium_status == PdfiumStatus::Ready {
                        self.open_pdf_tab(path);
                    } else if self.pdfium_status == PdfiumStatus::Initializing {
                        self.pending_open_path = Some(path);
                    }
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                        egui::UserAttentionType::Critical,
                    ));
                }
                crate::platform::ipc::IpcMessage::Focus => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                        egui::UserAttentionType::Critical,
                    ));
                }
            }
        }

        self.check_dormant_tabs();

        // Drain worker events non-blockingly
        while let Ok(evt) = self.rx_evt.try_recv() {
            match evt {
                Event::PdfiumReady => {
                    self.pdfium_status = PdfiumStatus::Ready;
                    if let Some(path) = self.pending_open_path.take() {
                        self.open_pdf_tab(path);
                    } else if self.settings.restore_session {
                        self.restore_saved_session();
                    }
                    self.session_restored = true;
                    self.last_session_save = Some(Instant::now());
                }
                Event::PdfiumError(err) => {
                    self.pdfium_status = PdfiumStatus::Error(err);
                }
                Event::Opened {
                    doc_id,
                    page_count,
                    page_sizes,
                    annotations,
                    text_boxes,
                } => {
                    for tab_item in &mut self.tabs {
                        if let Tab::Pdf(ref mut tab) = tab_item {
                            if tab.doc_id == doc_id {
                                tab.page_count = page_count;
                                tab.page_sizes = page_sizes;
                                if tab.current_page > page_count && page_count > 0 {
                                    tab.current_page = page_count;
                                }
                                tab.is_loading = false;
                                tab.outline_loading = true;
                                let _ = self.tx_cmd.send(Command::Outline { doc_id });
                                if tab.has_unsaved_changes {
                                    // Preserve unsaved in-memory annotations across dormant reload
                                    let _ = self.tx_cmd.send(Command::SyncAnnotations {
                                        doc_id,
                                        highlights: tab.annotations.highlights.clone(),
                                        text_boxes: tab.annotations.text_boxes.clone(),
                                    });
                                } else if !tab.path.as_os_str().is_empty() {
                                    let file_size =
                                        std::fs::metadata(&tab.path).map(|m| m.len()).unwrap_or(0);
                                    let abs_path = std::fs::canonicalize(&tab.path)
                                        .unwrap_or_else(|_| tab.path.clone());
                                    tab.file_key =
                                        DocumentAnnotations::compute_file_key(&abs_path, file_size);
                                    tab.annotations_dirty = false;
                                    let sidecar = DocumentAnnotations::load_for_key(&tab.file_key);
                                    if !sidecar.highlights.is_empty()
                                        || !sidecar.text_boxes.is_empty()
                                    {
                                        tab.annotations = sidecar;
                                    } else {
                                        tab.annotations.highlights = annotations;
                                        tab.annotations.text_boxes = text_boxes;
                                    }
                                } else {
                                    tab.annotations.highlights = annotations;
                                    tab.annotations.text_boxes = text_boxes;
                                }
                                break;
                            }
                        }
                    }
                }
                Event::AnnotationsExtracted {
                    doc_id,
                    annotations,
                    text_boxes,
                } => {
                    for tab_item in &mut self.tabs {
                        if let Tab::Pdf(ref mut tab) = tab_item {
                            if tab.doc_id == doc_id {
                                let mut changed = false;
                                for hl in annotations {
                                    let exists = tab.annotations.highlights.iter().any(|e| {
                                        e.page == hl.page
                                            && e.rects == hl.rects
                                            && e.points == hl.points
                                    });
                                    if !exists {
                                        let next_id = tab
                                            .annotations
                                            .highlights
                                            .iter()
                                            .map(|h| h.id)
                                            .max()
                                            .unwrap_or(0)
                                            + 1;
                                        let mut new_hl = hl;
                                        new_hl.id = next_id;
                                        tab.annotations.highlights.push(new_hl);
                                        changed = true;
                                    }
                                }
                                for tb in text_boxes {
                                    let exists = tab.annotations.text_boxes.iter().any(|e| {
                                        e.page == tb.page && e.pos == tb.pos && e.text == tb.text
                                    });
                                    if !exists {
                                        let next_id = tab
                                            .annotations
                                            .text_boxes
                                            .iter()
                                            .map(|t| t.id)
                                            .max()
                                            .unwrap_or(0)
                                            + 1;
                                        let mut new_tb = tb;
                                        new_tb.id = next_id;
                                        tab.annotations.text_boxes.push(new_tb);
                                        changed = true;
                                    }
                                }
                                if changed {
                                    ctx.request_repaint();
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
                                is_settings: false,
                                is_dormant: pdf.is_dormant,
                            },
                            Tab::Settings => TabItem {
                                title: "Preferences",
                                tooltip: "Preferences",
                                is_active: idx == self.active_tab_idx,
                                is_loading: false,
                                is_dirty: false,
                                is_settings: true,
                                is_dormant: false,
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
                            TabBarAction::CloseWindow => self.request_close_window(ctx),
                            TabBarAction::OpenAbout => self.show_about_modal = true,
                        }
                    }
                });

            // 2. Path Bar (Collapsible, hidden on Settings tab)
            let is_settings_tab = matches!(self.tabs.get(self.active_tab_idx), Some(Tab::Settings));
            if !self.path_bar.is_collapsed && !is_settings_tab {
                egui::TopBottomPanel::top("top_pathbar")
                    .frame(
                        egui::Frame::side_top_panel(&ctx.style())
                            .inner_margin(egui::Margin::symmetric(10, 4)),
                    )
                    .show(ctx, |ui| {
                        if let Some(action) = self.path_bar.ui(ui) {
                            match action {
                                PathBarAction::OpenPath(path) => {
                                    if self.pdfium_status == PdfiumStatus::Ready {
                                        self.open_pdf_tab(path);
                                    } else if self.pdfium_status == PdfiumStatus::Initializing {
                                        self.pending_open_path = Some(path);
                                    }
                                }
                                PathBarAction::LaunchFileChooser => {
                                    let active_has_doc = if let Some(Tab::Pdf(t)) =
                                        self.tabs.get(self.active_tab_idx)
                                    {
                                        !t.path.as_os_str().is_empty() || t.page_count > 0
                                    } else {
                                        false
                                    };
                                    if active_has_doc {
                                        self.create_empty_tab();
                                    }
                                    self.open_native_file_dialog(ctx);
                                }
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
                        .exact_height(crate::ui::theme::Theme::TOOLBAR_HEIGHT)
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
                                    settings_open: matches!(
                                        self.tabs.get(self.active_tab_idx),
                                        Some(Tab::Settings)
                                    ),
                                    active_tool,
                                    highlighter_color: self.settings.highlighter_color,
                                    highlighter_thickness: self.settings.highlighter_thickness,
                                    text_font_size: self.settings.text_font_size,
                                    text_color: self.settings.text_color,
                                    can_save: self
                                        .active_pdf_tab()
                                        .map(|t| !t.is_loading && t.page_count > 0)
                                        .unwrap_or(false),
                                    has_unsaved_changes: self
                                        .active_pdf_tab()
                                        .map(|t| t.has_unsaved_changes)
                                        .unwrap_or(false),
                                    can_undo: self
                                        .active_pdf_tab()
                                        .map(|t| !t.undo_stack.undo.is_empty())
                                        .unwrap_or(false),
                                    can_redo: self
                                        .active_pdf_tab()
                                        .map(|t| !t.undo_stack.redo.is_empty())
                                        .unwrap_or(false),
                                    can_go_back: self
                                        .active_pdf_tab()
                                        .map(|t| t.can_go_back())
                                        .unwrap_or(false),
                                    can_go_forward: self
                                        .active_pdf_tab()
                                        .map(|t| t.can_go_forward())
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
                                    tab.record_nav_jump();
                                    tab.pending_scroll_to_page = Some(target);
                                }
                            }
                            ToolbarAction::GoBack => {
                                if let Some(tab) = self.active_pdf_tab_mut() {
                                    tab.go_back();
                                }
                            }
                            ToolbarAction::GoForward => {
                                if let Some(tab) = self.active_pdf_tab_mut() {
                                    tab.go_forward();
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
                            ToolbarAction::ToggleText => {
                                if let Some(tab) = self.active_pdf_tab_mut() {
                                    tab.active_tool = if tab.active_tool == ActiveTool::Text {
                                        ActiveTool::None
                                    } else {
                                        ActiveTool::Text
                                    };
                                }
                            }
                            ToolbarAction::Undo => {
                                if let Some(tab) = self.active_pdf_tab_mut() {
                                    if tab.undo_stack.undo(&mut tab.annotations) {
                                        tab.has_unsaved_changes = true;
                                        tab.annotations_dirty = true;
                                        ctx.request_repaint();
                                    }
                                }
                            }
                            ToolbarAction::Redo => {
                                if let Some(tab) = self.active_pdf_tab_mut() {
                                    if tab.undo_stack.redo(&mut tab.annotations) {
                                        tab.has_unsaved_changes = true;
                                        tab.annotations_dirty = true;
                                        ctx.request_repaint();
                                    }
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
                            ToolbarAction::SetTextFontSize(size) => {
                                self.settings.text_font_size = size;
                                let _ = self.settings.save();
                                if let Some(tab) = self.active_pdf_tab_mut() {
                                    if let Some(ref mut etb) = tab.editing_text_box {
                                        etb.font_size = size;
                                    }
                                    if let Some(sel_id) = tab.selected_text_box_id {
                                        if let Some(tb) = tab
                                            .annotations
                                            .text_boxes
                                            .iter_mut()
                                            .find(|tb| tb.id == sel_id)
                                        {
                                            if (tb.font_size - size).abs() > 0.1 {
                                                let old_tb = tb.clone();
                                                tb.font_size = size;
                                                tab.undo_stack
                                                    .push_modify_text_box(old_tb, tb.clone());
                                                tab.has_unsaved_changes = true;
                                                tab.annotations_dirty = true;
                                            }
                                        }
                                    }
                                }
                            }
                            ToolbarAction::SetTextColor(color) => {
                                self.settings.text_color = color;
                                let _ = self.settings.save();
                                if let Some(tab) = self.active_pdf_tab_mut() {
                                    if let Some(ref mut etb) = tab.editing_text_box {
                                        etb.color = color;
                                    }
                                    if let Some(sel_id) = tab.selected_text_box_id {
                                        if let Some(tb) = tab
                                            .annotations
                                            .text_boxes
                                            .iter_mut()
                                            .find(|tb| tb.id == sel_id)
                                        {
                                            if tb.color != color {
                                                let old_tb = tb.clone();
                                                tb.color = color;
                                                tab.undo_stack
                                                    .push_modify_text_box(old_tb, tb.clone());
                                                tab.has_unsaved_changes = true;
                                                tab.annotations_dirty = true;
                                            }
                                        }
                                    }
                                }
                            }
                            ToolbarAction::Save => {
                                self.save_active_pdf();
                            }
                            ToolbarAction::OpenSettings => {
                                let existing =
                                    self.tabs.iter().position(|t| matches!(t, Tab::Settings));
                                if let Some(idx) = existing {
                                    self.switch_to_tab(idx);
                                } else {
                                    self.draft_settings = self.settings.clone();
                                    self.tabs.push(Tab::Settings);
                                    self.switch_to_tab(self.tabs.len() - 1);
                                }
                            }
                        }
                    }
                }
            }

            // 4. Find Bar (Floating docked card in top right, directly below search icon)
            let find_open = self
                .active_pdf_tab()
                .map(|t| t.find_state.is_open && (t.page_count > 0 || t.is_loading))
                .unwrap_or(false);

            if find_open {
                let mut find_action = None;
                let mut avail_w = 0.0;
                let top_pos = crate::ui::theme::Theme::TAB_HEIGHT
                    + if self.path_bar.is_collapsed {
                        0.0
                    } else {
                        38.0
                    }
                    + crate::ui::theme::Theme::TOOLBAR_HEIGHT
                    + 4.0;

                egui::Area::new(egui::Id::new("floating_find_bar"))
                    .order(egui::Order::Foreground)
                    .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-14.0, top_pos))
                    .show(ctx, |ui| {
                        avail_w = ui.available_width();
                        egui::Frame::NONE
                            .fill(egui::Color32::from_rgb(28, 28, 30))
                            .corner_radius(crate::ui::theme::Theme::RADIUS)
                            .stroke(egui::Stroke::new(
                                1.0_f32,
                                egui::Color32::from_rgb(55, 55, 60),
                            ))
                            .inner_margin(egui::Margin::symmetric(12, 6))
                            .show(ui, |ui| {
                                ui.set_width(400.0);
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
                                    tab.record_nav_jump();
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
                                    tab.record_nav_jump();
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

                        self.metrics.update();
                        let cpu_disp = self.metrics.cpu_percent;
                        let rss_disp = self.metrics.rss_mb;

                        ui.horizontal(|ui| {
                            ui.colored_label(
                                egui::Color32::from_rgb(0, 255, 128),
                                format!(
                                    "F3 Debug | CPU: {:.1}% | RAM (RSS): {:.1} MB | Cache: {:.2} MB / {:.0} MB ({} cached) | Frame: {:.1} ms ({:.0} FPS) | Zoom: {:.0}% | Rot: {}° | Tabs: {}",
                                    cpu_disp,
                                    rss_disp,
                                    cache_mb,
                                    cap_mb,
                                    self.cache.len(),
                                    self.frame_time_ms,
                                    fps,
                                    zoom_disp,
                                    rot_disp,
                                    self.tabs.len()
                                ),
                            );
                        });
                    });
            }
        }

        // TOC Sidebar (Left panel in document body with smooth slide-in / slide-out animation)
        if !self.is_fullscreen {
            let (target_open, sidebar_width, outline, outline_loading, current_page) =
                if let Some(t) = self.active_pdf_tab() {
                    (
                        t.sidebar_open && (t.page_count > 0 || t.is_loading),
                        t.sidebar_width.clamp(180.0, 600.0),
                        t.outline.clone(),
                        t.outline_loading,
                        t.current_page,
                    )
                } else {
                    (false, 280.0, None, false, 1)
                };

            let anim_id = egui::Id::new("toc_sidebar_slide_anim");
            let anim_progress = ctx.animate_bool_with_time_and_easing(
                anim_id,
                target_open,
                0.22,
                egui::emath::easing::cubic_out,
            );

            if anim_progress > 0.001 {
                let current_width = (anim_progress * sidebar_width).max(1.0);
                let is_fully_open = anim_progress >= 0.999;
                let mut sidebar_out = None;

                let side_panel = egui::SidePanel::left("toc_sidebar")
                    .resizable(false)
                    .exact_width(current_width)
                    .show_separator_line(false)
                    .frame(egui::Frame::NONE);

                let _panel_res = side_panel.show(ctx, |ui| {
                    let panel_rect = ui.max_rect();
                    // Draw panel background strictly within panel_rect
                    ui.painter()
                        .rect_filled(panel_rect, 0.0, egui::Color32::from_rgb(26, 26, 28));

                    ui.set_clip_rect(panel_rect);

                    // Content slides in from left in lockstep with the panel opening.
                    // Keep the right border clear by 14px so the vertical scrollbar never collides with the resize handle.
                    let content_w = (sidebar_width - 14.0).max(100.0);
                    let content_left = panel_rect.right() - sidebar_width;
                    let inner_rect = egui::Rect::from_min_size(
                        egui::pos2(content_left + 10.0, panel_rect.top() + 8.0),
                        egui::vec2(content_w - 10.0, panel_rect.height() - 16.0),
                    );

                    let mut child_ui = ui.new_child(egui::UiBuilder::new().max_rect(inner_rect));
                    sidebar_out = Some(render_sidebar_toc(
                        &mut child_ui,
                        outline.as_deref(),
                        outline_loading,
                        current_page,
                    ));

                    // Interactive drag resize handle on right border (allocated AFTER child_ui on top)
                    if is_fully_open {
                        let border_x = panel_rect.right();
                        let handle_rect = egui::Rect::from_min_max(
                            egui::pos2(border_x - 5.0, panel_rect.top()),
                            egui::pos2(border_x + 5.0, panel_rect.bottom()),
                        );
                        let handle_id = ui.id().with("toc_sidebar_resize_handle");
                        let handle_resp = ui.interact(handle_rect, handle_id, egui::Sense::drag());
                        if handle_resp.hovered() || handle_resp.dragged() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                        }
                        let (border_col, border_w) = if handle_resp.dragged() {
                            (crate::ui::theme::Theme::TAB_INDICATOR, 2.0_f32)
                        } else if handle_resp.hovered() {
                            (egui::Color32::from_rgb(90, 160, 240), 2.0_f32)
                        } else {
                            (egui::Color32::from_rgb(45, 45, 48), 1.0_f32)
                        };
                        ui.painter().line_segment(
                            [panel_rect.right_top(), panel_rect.right_bottom()],
                            egui::Stroke::new(border_w, border_col),
                        );
                        if handle_resp.dragged() {
                            let delta_x = handle_resp.drag_delta().x;
                            if let Some(tab) = self.active_pdf_tab_mut() {
                                tab.sidebar_width =
                                    (tab.sidebar_width + delta_x).clamp(180.0, 600.0);
                            }
                            ui.ctx().request_repaint();
                        }
                    } else {
                        ui.painter().line_segment(
                            [panel_rect.right_top(), panel_rect.right_bottom()],
                            egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(45, 45, 48)),
                        );
                    }
                });

                if let Some(out) = sidebar_out {
                    if let Some(tab) = self.active_pdf_tab_mut() {
                        if out.close_requested {
                            tab.sidebar_open = false;
                        }
                        if let Some(target) = out.target_page {
                            tab.record_nav_jump();
                            tab.pending_scroll_to_page = Some(target);
                        }
                    }
                }
            }
        }

        // Fullscreen Floating Left-Edge Quick-Toolbar
        // Appears when mouse reaches middle portion of the left screen edge
        if self.is_fullscreen {
            let screen = ctx.screen_rect();
            let trigger_y_min = screen.top() + screen.height() * 0.25;
            let trigger_y_max = screen.bottom() - screen.height() * 0.25;
            let pointer_pos = ctx.input(|i| i.pointer.hover_pos());

            if let Some(pos) = pointer_pos {
                if pos.x <= 20.0 && pos.y >= trigger_y_min && pos.y <= trigger_y_max {
                    self.fullscreen_quickbar_open = true;
                }
            }

            if self.fullscreen_quickbar_open {
                if let Some(tab_info) = self
                    .active_pdf_tab()
                    .map(|t| (t.current_page, t.page_count, t.active_tool))
                {
                    let (current_page, total_pages, active_tool) = tab_info;
                    if !self.fullscreen_is_editing_page {
                        self.fullscreen_page_input = current_page.to_string();
                    }

                    let mut jump_target = None;
                    let mut toggle_hl = false;
                    let mut toggle_eraser = false;
                    let mut toggle_text = false;
                    let mut bar_rect_opt = None;

                    egui::Area::new(egui::Id::new("fullscreen_quickbar"))
                        .order(egui::Order::Foreground)
                        .anchor(egui::Align2::LEFT_CENTER, egui::vec2(12.0, 0.0))
                        .show(ctx, |ui| {
                            let frame_res = egui::Frame::NONE
                                .fill(egui::Color32::from_rgb(26, 26, 28))
                                .corner_radius(crate::ui::theme::Theme::RADIUS)
                                .stroke(egui::Stroke::new(
                                    1.0_f32,
                                    egui::Color32::from_rgb(55, 55, 60),
                                ))
                                .inner_margin(egui::Margin::symmetric(10, 12))
                                .show(ui, |ui| {
                                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 8.0);
                                    ui.vertical_centered(|ui| {
                                        // 1. Highlighter
                                        let hl_active = active_tool == ActiveTool::Highlighter;
                                        let hl_color = egui::Color32::from_rgb(
                                            self.settings.highlighter_color.r,
                                            self.settings.highlighter_color.g,
                                            self.settings.highlighter_color.b,
                                        );
                                        let hl_btn = crate::ui::icons::render_icon_button(
                                            ui,
                                            egui::vec2(32.0, 32.0),
                                            hl_active,
                                            true,
                                            "Highlighter",
                                            |p, r, c| {
                                                crate::ui::icons::paint_highlighter(
                                                    p, r, c, hl_color,
                                                )
                                            },
                                        );
                                        if hl_btn.clicked() {
                                            toggle_hl = true;
                                        }

                                        // 2. Eraser
                                        let eraser_active = active_tool == ActiveTool::Eraser;
                                        let eraser_btn = crate::ui::icons::render_icon_button(
                                            ui,
                                            egui::vec2(32.0, 32.0),
                                            eraser_active,
                                            true,
                                            "Eraser",
                                            |p, r, c| {
                                                crate::ui::icons::paint_eraser(
                                                    p,
                                                    r,
                                                    c,
                                                    eraser_active,
                                                )
                                            },
                                        );
                                        if eraser_btn.clicked() {
                                            toggle_eraser = true;
                                        }

                                        // 3. Text Tool
                                        let text_active = active_tool == ActiveTool::Text;
                                        let text_btn = crate::ui::icons::render_icon_button(
                                            ui,
                                            egui::vec2(32.0, 32.0),
                                            text_active,
                                            true,
                                            "Text tool",
                                            |p, r, c| {
                                                crate::ui::icons::paint_text_tool(
                                                    p,
                                                    r,
                                                    c,
                                                    text_active,
                                                )
                                            },
                                        );
                                        if text_btn.clicked() {
                                            toggle_text = true;
                                        }

                                        ui.add_space(4.0);
                                        ui.separator();
                                        ui.add_space(4.0);

                                        // 4. Page Jump Search Box
                                        let text_edit = egui::TextEdit::singleline(
                                            &mut self.fullscreen_page_input,
                                        )
                                        .desired_width(46.0)
                                        .margin(egui::Margin::symmetric(4, 4))
                                        .horizontal_align(egui::Align::Center);
                                        let response = ui.add(text_edit);

                                        if response.gained_focus() {
                                            self.fullscreen_is_editing_page = true;
                                            let mut state = egui::text_edit::TextEditState::load(
                                                ui.ctx(),
                                                response.id,
                                            )
                                            .unwrap_or_default();
                                            let len = self.fullscreen_page_input.chars().count();
                                            state.cursor.set_char_range(Some(
                                                egui::text_selection::CCursorRange::two(
                                                    egui::text::CCursor::new(0),
                                                    egui::text::CCursor::new(len),
                                                ),
                                            ));
                                            state.store(ui.ctx(), response.id);
                                        }

                                        let enter_pressed = (response.has_focus()
                                            || response.lost_focus())
                                            && ui.input(|i| i.key_pressed(egui::Key::Enter));

                                        if enter_pressed
                                            || (response.lost_focus()
                                                && self.fullscreen_is_editing_page)
                                        {
                                            if enter_pressed {
                                                response.surrender_focus();
                                            }
                                            if let Ok(num) =
                                                self.fullscreen_page_input.trim().parse::<usize>()
                                            {
                                                if num >= 1 && num <= total_pages {
                                                    jump_target = Some(num);
                                                }
                                            }
                                            self.fullscreen_is_editing_page = false;
                                            self.fullscreen_page_input = current_page.to_string();
                                        }

                                        ui.label(
                                            egui::RichText::new(format!("/ {total_pages}"))
                                                .size(11.5)
                                                .color(crate::ui::theme::Theme::TEXT_MUTED),
                                        );
                                    });
                                });
                            bar_rect_opt = Some(frame_res.response.rect);
                        });

                    if let Some(rect) = bar_rect_opt {
                        if let Some(pos) = pointer_pos {
                            if !rect.expand(30.0).contains(pos) && !self.fullscreen_is_editing_page
                            {
                                self.fullscreen_quickbar_open = false;
                            }
                        } else if !self.fullscreen_is_editing_page {
                            self.fullscreen_quickbar_open = false;
                        }
                    }

                    if toggle_hl {
                        if let Some(tab) = self.active_pdf_tab_mut() {
                            tab.active_tool = if tab.active_tool == ActiveTool::Highlighter {
                                ActiveTool::None
                            } else {
                                ActiveTool::Highlighter
                            };
                        }
                    }
                    if toggle_eraser {
                        if let Some(tab) = self.active_pdf_tab_mut() {
                            tab.active_tool = if tab.active_tool == ActiveTool::Eraser {
                                ActiveTool::None
                            } else {
                                ActiveTool::Eraser
                            };
                        }
                    }
                    if toggle_text {
                        if let Some(tab) = self.active_pdf_tab_mut() {
                            tab.active_tool = if tab.active_tool == ActiveTool::Text {
                                ActiveTool::None
                            } else {
                                ActiveTool::Text
                            };
                        }
                    }
                    if let Some(target) = jump_target {
                        if let Some(tab) = self.active_pdf_tab_mut() {
                            tab.record_nav_jump();
                            tab.pending_scroll_to_page = Some(target);
                        }
                    }
                } else {
                    self.fullscreen_quickbar_open = false;
                }
            }
        } else {
            self.fullscreen_quickbar_open = false;
        }

        // Main Central Panel
        egui::CentralPanel::default().show(ctx, |ui| {
            let central_rect = ui.max_rect();
            let pointer_clicked =
                ui.input(|i| i.pointer.primary_clicked() || i.pointer.secondary_clicked());
            if pointer_clicked {
                if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
                    if central_rect.contains(pos) {
                        if let Some(tab) = self.active_pdf_tab_mut() {
                            if tab.sidebar_open {
                                tab.sidebar_open = false;
                            }
                        }
                    }
                }
            }

            match &self.pdfium_status {
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
                        editing_text_box: Option<EditingTextBox>,
                        selected_text_box_id: Option<u64>,
                        dragging_text_box: Option<(u64, egui::Pos2, [f32; 2])>,
                        resizing_text_box: Option<(Option<u64>, egui::Pos2, f32)>,
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
                                editing_text_box: tab.editing_text_box.clone(),
                                selected_text_box_id: tab.selected_text_box_id,
                                dragging_text_box: tab.dragging_text_box,
                                resizing_text_box: tab.resizing_text_box,
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
                                    "Open a PDF document or press Ctrl+L to enter a file path.",
                                )
                                .color(egui::Color32::from_rgb(170, 170, 175))
                                .size(14.0),
                            );
                            ui.add_space(20.0);
                            if render_adobe_button(ui, "Open Document", false).clicked() {
                                self.open_native_file_dialog(ui.ctx());
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
                        mut editing_text_box,
                        mut selected_text_box_id,
                        mut dragging_text_box,
                        mut resizing_text_box,
                    } => {
                        let mut output = render_viewer(
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
                            &mut editing_text_box,
                            &mut selected_text_box_id,
                            &mut dragging_text_box,
                            &mut resizing_text_box,
                            self.settings.text_font_size,
                            self.settings.text_color,
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
                                ViewerAction::HighlightSelection => {
                                    if let Some(sel) = selection {
                                        if !sel.is_empty() {
                                            let (start, end) = sel.normalized();
                                            for p in start.page..=end.page {
                                                if let Some(text_data) = self.text_cache.get(doc_id, p) {
                                                    let p_start = if p == start.page { start.char_idx } else { 0 };
                                                    let p_end = if p == end.page { end.char_idx } else { text_data.chars.len() };
                                                    if p_start <= p_end {
                                                        let raw_rects = text_data.get_selection_rects(p_start, p_end);
                                                        let line_rects: Vec<crate::pdf::types::CanonicalRect> = raw_rects
                                                            .into_iter()
                                                            .map(|r| crate::annotations::adjust_rect_thickness(r, self.settings.highlighter_thickness))
                                                            .collect();
                                                        if !line_rects.is_empty() {
                                                            let id = crate::annotations::generate_highlight_id();
                                                            let hl = crate::annotations::Highlight {
                                                                id,
                                                                page: p,
                                                                rects: line_rects,
                                                                points: Vec::new(),
                                                                color: self.settings.highlighter_color,
                                                                alpha: 0.35,
                                                                thickness: self.settings.highlighter_thickness,
                                                            };
                                                            annotations.highlights.push(hl.clone());
                                                            undo_stack.push_add(hl);
                                                            output.annotations_changed = true;
                                                        }
                                                    }
                                                }
                                            }
                                            selection = None;
                                            drag_start = None;
                                        }
                                    }
                                }
                                ViewerAction::AddTextNote(page, can_pos) => {
                                    editing_text_box = Some(crate::annotations::EditingTextBox {
                                        id: None,
                                        page,
                                        pos: can_pos,
                                        text: String::new(),
                                        font_size: self.settings.text_font_size,
                                        color: self.settings.text_color,
                                        width: 180.0,
                                        is_new: true,
                                    });
                                    ctx.request_repaint();
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
                                ViewerAction::JumpToPage(p) => {
                                    if let Some(tab) = self.active_pdf_tab_mut() {
                                        tab.record_nav_jump();
                                        tab.pending_scroll_to_page = Some(p + 1);
                                        ctx.request_repaint();
                                    }
                                }
                                ViewerAction::OpenUrl(url) => {
                                    ctx.open_url(egui::OpenUrl::new_tab(url));
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
                            tab.editing_text_box = editing_text_box;
                            tab.selected_text_box_id = selected_text_box_id;
                            tab.dragging_text_box = dragging_text_box;
                            tab.resizing_text_box = resizing_text_box;
                            if output.annotations_changed {
                                tab.has_unsaved_changes = true;
                                tab.annotations_dirty = true;
                                tab.last_annotation_change = Some(Instant::now());
                                let _ = self.tx_cmd.send(Command::SyncAnnotations {
                                    doc_id,
                                    highlights: tab.annotations.highlights.clone(),
                                    text_boxes: tab.annotations.text_boxes.clone(),
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
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            ui.vertical_centered(|ui| {
                                ui.set_max_width(620.0);
                                ui.add_space(20.0);
                                ui.heading(
                                    egui::RichText::new("Preferences").size(22.0).strong(),
                                );
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new(
                                        "Customize scroll sensitivity, default annotations, and memory cache budget.",
                                    )
                                    .color(egui::Color32::from_rgb(160, 160, 165))
                                    .size(12.5),
                                );
                                ui.add_space(16.0);

                                // Customize visuals so the slider groove line/track behind the thumb is prominently visible
                                let mut slider_style = (*ui.ctx().style()).clone();
                                slider_style.visuals.widgets.inactive.bg_fill =
                                    egui::Color32::from_rgb(45, 45, 52);
                                slider_style.visuals.widgets.inactive.bg_stroke =
                                    egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(85, 85, 95));
                                slider_style.visuals.widgets.hovered.bg_fill =
                                    egui::Color32::from_rgb(55, 55, 64);
                                slider_style.visuals.widgets.hovered.bg_stroke =
                                    egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(115, 115, 130));
                                slider_style.visuals.widgets.active.bg_fill =
                                    egui::Color32::from_rgb(60, 60, 72);
                                slider_style.visuals.widgets.active.bg_stroke =
                                    egui::Stroke::new(1.5_f32, crate::ui::theme::Theme::TAB_INDICATOR);
                                ui.set_style(slider_style);

                                let card_w = 580.0;

                                // Section 1: Scrolling & Touchpad
                                egui::Frame::NONE
                                    .fill(egui::Color32::from_rgb(28, 28, 30))
                                    .stroke(egui::Stroke::new(
                                        1.0_f32,
                                        egui::Color32::from_rgb(48, 48, 52),
                                    ))
                                    .corner_radius(crate::ui::theme::Theme::RADIUS)
                                    .inner_margin(egui::Margin::symmetric(20, 16))
                                    .show(ui, |ui| {
                                        ui.set_width(card_w);
                                        ui.label(
                                            egui::RichText::new("Scrolling & Touchpad")
                                                .strong()
                                                .size(14.0),
                                        );
                                        ui.add_space(10.0);
                                        ui.horizontal(|ui| {
                                            ui.label("Touchpad scroll speed multiplier:");
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.add(
                                                        egui::Slider::new(
                                                            &mut self.draft_settings.touchpad_scroll_speed,
                                                            1.0..=8.0,
                                                        )
                                                        .text("x")
                                                        .step_by(0.5),
                                                    );
                                                },
                                            );
                                        });
                                        ui.add_space(8.0);
                                        ui.horizontal(|ui| {
                                            ui.label("Mouse wheel scroll speed:");
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.add(
                                                        egui::Slider::new(
                                                            &mut self.draft_settings.mouse_wheel_speed,
                                                            0.5..=4.0,
                                                        )
                                                        .text("x")
                                                        .step_by(0.25),
                                                    );
                                                },
                                            );
                                        });
                                    });

                                ui.add_space(14.0);

                                // Section 2: Default Annotations
                                egui::Frame::NONE
                                    .fill(egui::Color32::from_rgb(28, 28, 30))
                                    .stroke(egui::Stroke::new(
                                        1.0_f32,
                                        egui::Color32::from_rgb(48, 48, 52),
                                    ))
                                    .corner_radius(crate::ui::theme::Theme::RADIUS)
                                    .inner_margin(egui::Margin::symmetric(20, 16))
                                    .show(ui, |ui| {
                                        ui.set_width(card_w);
                                        ui.label(
                                            egui::RichText::new("Default Annotation Settings")
                                                .strong()
                                                .size(14.0),
                                        );
                                        ui.add_space(10.0);
                                        ui.horizontal(|ui| {
                                            ui.label("Highlighter thickness:");
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.add(
                                                        egui::Slider::new(
                                                            &mut self.draft_settings.highlighter_thickness,
                                                            0.25..=1.0,
                                                        )
                                                        .custom_formatter(|n, _| {
                                                            format!("{:.0}%", n * 100.0)
                                                        }),
                                                    );
                                                },
                                            );
                                        });
                                        ui.add_space(10.0);
                                        ui.horizontal(|ui| {
                                            ui.label("Highlighter color swatch:");
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    for &color in crate::annotations::PRESET_HIGHLIGHT_COLORS.iter().rev() {
                                                        let is_sel = self.draft_settings.highlighter_color == color;
                                                        let (rect, resp) = ui.allocate_exact_size(
                                                            egui::vec2(22.0, 22.0),
                                                            egui::Sense::click(),
                                                        );
                                                        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                                                        let fill = egui::Color32::from_rgb(color.r, color.g, color.b);
                                                        ui.painter().rect_filled(rect, egui::CornerRadius::same(4), fill);
                                                        if is_sel {
                                                            ui.painter().rect_stroke(
                                                                rect,
                                                                egui::CornerRadius::same(4),
                                                                egui::Stroke::new(2.0_f32, egui::Color32::WHITE),
                                                                egui::StrokeKind::Outside,
                                                            );
                                                        }
                                                        if resp.clicked() {
                                                            self.draft_settings.highlighter_color = color;
                                                        }
                                                    }
                                                },
                                            );
                                        });
                                        ui.add_space(10.0);
                                        ui.horizontal(|ui| {
                                            ui.label("Text tool font size:");
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.add(
                                                        egui::Slider::new(
                                                            &mut self.draft_settings.text_font_size,
                                                            10.0..=36.0,
                                                        )
                                                        .text("pt"),
                                                    );
                                                },
                                            );
                                        });
                                    });

                                ui.add_space(14.0);

                                // Section 3: Performance & Memory Cache
                                egui::Frame::NONE
                                    .fill(egui::Color32::from_rgb(28, 28, 30))
                                    .stroke(egui::Stroke::new(
                                        1.0_f32,
                                        egui::Color32::from_rgb(48, 48, 52),
                                    ))
                                    .corner_radius(crate::ui::theme::Theme::RADIUS)
                                    .inner_margin(egui::Margin::symmetric(20, 16))
                                    .show(ui, |ui| {
                                        ui.set_width(card_w);
                                        ui.label(
                                            egui::RichText::new("Performance & Memory")
                                                .strong()
                                                .size(14.0),
                                        );
                                        ui.add_space(10.0);
                                        ui.horizontal(|ui| {
                                            ui.label("Texture cache memory budget:");
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.add(
                                                        egui::Slider::new(
                                                            &mut self.draft_settings.cache_size_mb,
                                                            50..=500,
                                                        )
                                                        .text("MB")
                                                        .step_by(25.0),
                                                    );
                                                },
                                            );
                                        });
                                        ui.add_space(8.0);
                                        ui.horizontal(|ui| {
                                            ui.label("Background tab inactivity timeout:");
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.add(
                                                        egui::Slider::new(
                                                            &mut self
                                                                .draft_settings
                                                                .inactivity_timeout_mins,
                                                            0..=30,
                                                        )
                                                        .custom_formatter(|n, _| {
                                                            if n <= 0.0 {
                                                                "Never (disabled)".to_string()
                                                            } else if (n - 1.0).abs() < 0.1 {
                                                                "1 min".to_string()
                                                            } else {
                                                                format!("{:.0} mins", n)
                                                            }
                                                        })
                                                        .step_by(1.0),
                                                    );
                                                },
                                            );
                                        });
                                    });

                                ui.add_space(14.0);

                                // Section 4: Startup & Session
                                egui::Frame::NONE
                                    .fill(egui::Color32::from_rgb(28, 28, 30))
                                    .stroke(egui::Stroke::new(
                                        1.0_f32,
                                        egui::Color32::from_rgb(48, 48, 52),
                                    ))
                                    .corner_radius(crate::ui::theme::Theme::RADIUS)
                                    .inner_margin(egui::Margin::symmetric(20, 16))
                                    .show(ui, |ui| {
                                        ui.set_width(card_w);
                                        ui.label(
                                            egui::RichText::new("Startup & Session")
                                                .strong()
                                                .size(14.0),
                                        );
                                        ui.add_space(10.0);
                                        ui.horizontal(|ui| {
                                            let label_resp = ui
                                                .add(
                                                    egui::Label::new(
                                                        egui::RichText::new(
                                                            "Restore open tabs on startup",
                                                        )
                                                        .size(13.0),
                                                    )
                                                    .sense(egui::Sense::click()),
                                                )
                                                .on_hover_cursor(egui::CursorIcon::PointingHand);

                                            let _box_resp = ui
                                                .with_layout(
                                                    egui::Layout::right_to_left(egui::Align::Center),
                                                    |ui| {
                                                        render_adobe_checkbox(
                                                            ui,
                                                            &mut self.draft_settings.restore_session,
                                                        )
                                                    },
                                                )
                                                .inner;

                                            if label_resp.clicked() {
                                                self.draft_settings.restore_session =
                                                    !self.draft_settings.restore_session;
                                            }
                                        });
                                        ui.add_space(10.0);
                                        ui.horizontal(|ui| {
                                            let label_resp = ui
                                                .add(
                                                    egui::Label::new(
                                                        egui::RichText::new(
                                                            "Remember last read page and position per document",
                                                        )
                                                        .size(13.0),
                                                    )
                                                    .sense(egui::Sense::click()),
                                                )
                                                .on_hover_cursor(egui::CursorIcon::PointingHand);

                                            let _box_resp = ui
                                                .with_layout(
                                                    egui::Layout::right_to_left(egui::Align::Center),
                                                    |ui| {
                                                        render_adobe_checkbox(
                                                            ui,
                                                            &mut self
                                                                .draft_settings
                                                                .remember_reading_position,
                                                        )
                                                    },
                                                )
                                                .inner;

                                            if label_resp.clicked() {
                                                self.draft_settings.remember_reading_position =
                                                    !self.draft_settings.remember_reading_position;
                                            }
                                        });
                                    });

                                ui.add_space(14.0);

                                // Section 5: About Verso Reader
                                egui::Frame::NONE
                                    .fill(egui::Color32::from_rgb(28, 28, 30))
                                    .stroke(egui::Stroke::new(
                                        1.0_f32,
                                        egui::Color32::from_rgb(48, 48, 52),
                                    ))
                                    .corner_radius(crate::ui::theme::Theme::RADIUS)
                                    .inner_margin(egui::Margin::symmetric(20, 16))
                                    .show(ui, |ui| {
                                        ui.set_width(card_w);
                                        ui.label(
                                            egui::RichText::new("About Verso Reader")
                                                .strong()
                                                .size(14.0),
                                        );
                                        ui.add_space(8.0);
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new(
                                                    "Verso Reader 2.0.0 ",
                                                )
                                                .color(egui::Color32::from_rgb(170, 170, 175))
                                                .size(12.0),
                                            );
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    if render_adobe_button(ui, "About Info", false).clicked() {
                                                        self.show_about_modal = true;
                                                    }
                                                },
                                            );
                                        });
                                    });

                                ui.add_space(20.0);

                                // Action Buttons: Modern Adobe-styled Save and Cancel buttons
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing = egui::vec2(12.0, 0.0);

                                    if render_adobe_button(ui, "Save Preferences", true).clicked() {
                                        self.settings = self.draft_settings.clone();
                                        let _ = self.settings.save();
                                        self.cache.set_max_bytes(self.settings.cache_size_mb * 1024 * 1024);
                                        self.save_session_and_history();
                                        self.settings_saved_toast = Some(Instant::now());
                                        ctx.request_repaint();
                                    }

                                    if crate::ui::icons::render_adobe_button_variant(
                                        ui,
                                        "Cancel",
                                        crate::ui::icons::AdobeButtonVariant::Destructive,
                                    )
                                    .clicked()
                                    {
                                        self.draft_settings = self.settings.clone();
                                        if self.tabs.len() > 1 {
                                            let target = if self.active_tab_idx > 0 {
                                                self.active_tab_idx - 1
                                            } else {
                                                1
                                            };
                                            self.switch_to_tab(target);
                                        }
                                        ctx.request_repaint();
                                    }

                                    if let Some(t) = self.settings_saved_toast {
                                        if t.elapsed() < Duration::from_secs(3) {
                                            ui.add_space(8.0);
                                            ui.label(
                                                egui::RichText::new("Preferences saved successfully!")
                                                    .color(egui::Color32::from_rgb(0, 220, 130))
                                                    .size(13.0),
                                            );
                                        } else {
                                            self.settings_saved_toast = None;
                                        }
                                    }
                                });
                                ui.add_space(24.0);
                            });
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
        }
    });

        // Confirmation modal for closing documents with unsaved changes
        if let Some(confirm) = self.pending_close_confirm.clone() {
            let mut dismiss = false;
            let mut do_save = false;
            let mut do_discard = false;

            let modal_frame = egui::Frame::NONE
                .fill(egui::Color32::from_rgb(34, 34, 38))
                .corner_radius(egui::CornerRadius::same(10))
                .stroke(egui::Stroke::new(
                    1.0_f32,
                    egui::Color32::from_rgb(58, 58, 66),
                ))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 8],
                    blur: 24,
                    spread: 0,
                    color: egui::Color32::from_black_alpha(150),
                })
                .inner_margin(egui::Margin::symmetric(24, 20));

            egui::Modal::new(egui::Id::new("unsaved_changes_confirm_modal"))
                .backdrop_color(egui::Color32::from_black_alpha(175))
                .frame(modal_frame)
                .show(ctx, |ui| {
                    ui.set_width(450.0);
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 12.0);

                    ui.vertical(|ui| {
                        // 1. Header with Adobe warning badge and title
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(14.0, 0.0);

                            // Adobe alert icon badge (rounded square with warning exclamation)
                            let (badge_rect, _) = ui.allocate_exact_size(
                                egui::vec2(40.0, 40.0),
                                egui::Sense::hover(),
                            );
                            ui.painter().rect(
                                badge_rect,
                                egui::CornerRadius::same(8),
                                egui::Color32::from_rgb(46, 36, 18),
                                egui::Stroke::new(1.2_f32, egui::Color32::from_rgb(215, 140, 20)),
                                egui::StrokeKind::Inside,
                            );
                            ui.painter().text(
                                badge_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "!",
                                egui::FontId::proportional(22.0),
                                egui::Color32::from_rgb(255, 185, 55),
                            );

                            ui.vertical(|ui| {
                                ui.label(
                                    egui::RichText::new("Save changes?")
                                        .strong()
                                        .size(17.0)
                                        .color(egui::Color32::WHITE),
                                );
                                ui.add_space(2.0);
                                ui.label(
                                    egui::RichText::new(
                                        "Unsaved annotations and edits will be lost.",
                                    )
                                    .size(12.5)
                                    .color(egui::Color32::from_rgb(160, 160, 168)),
                                );
                            });
                        });

                        ui.add_space(6.0);

                        // 2. Document file callout container
                        let callout_frame = egui::Frame::NONE
                            .fill(egui::Color32::from_rgb(25, 25, 28))
                            .corner_radius(egui::CornerRadius::same(6))
                            .stroke(egui::Stroke::new(
                                1.0_f32,
                                egui::Color32::from_rgb(48, 48, 54),
                            ))
                            .inner_margin(egui::Margin::symmetric(14, 10));

                        callout_frame.show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing = egui::vec2(10.0, 0.0);

                                // Red "PDF" badge
                                let (pdf_badge_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(28.0, 18.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(
                                    pdf_badge_rect,
                                    egui::CornerRadius::same(3),
                                    egui::Color32::from_rgb(215, 45, 45),
                                );
                                ui.painter().text(
                                    pdf_badge_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "PDF",
                                    egui::FontId::proportional(9.5),
                                    egui::Color32::WHITE,
                                );

                                // File title (clean with truncation if needed)
                                let doc_title = if confirm.doc_title.chars().count() > 42 {
                                    let tr: String = confirm.doc_title.chars().take(39).collect();
                                    format!("{tr}...")
                                } else {
                                    confirm.doc_title.clone()
                                };

                                ui.label(
                                    egui::RichText::new(doc_title)
                                        .strong()
                                        .size(13.0)
                                        .color(egui::Color32::from_rgb(230, 230, 235)),
                                );
                            });
                        });

                        // 3. Descriptive prompt text
                        ui.label(
                            egui::RichText::new(
                                "Do you want to save the changes you made to this document before closing?",
                            )
                            .size(13.0)
                            .color(egui::Color32::from_rgb(180, 180, 188)),
                        );

                        ui.add_space(8.0);

                        // 4. Subtle separator line
                        let (sep_rect, _) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 1.0),
                            egui::Sense::hover(),
                        );
                        ui.painter()
                            .rect_filled(sep_rect, 0.0, egui::Color32::from_rgb(50, 50, 56));

                        ui.add_space(6.0);

                        // 5. Adobe-styled action buttons
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(12.0, 0.0);

                            // Save (Primary action)
                            if crate::ui::icons::render_adobe_button_variant(
                                ui,
                                "Save",
                                crate::ui::icons::AdobeButtonVariant::Primary,
                            )
                            .clicked()
                            {
                                do_save = true;
                            }

                            // Don't Save (Destructive action)
                            if crate::ui::icons::render_adobe_button_variant(
                                ui,
                                "Don't Save",
                                crate::ui::icons::AdobeButtonVariant::Destructive,
                            )
                            .clicked()
                            {
                                do_discard = true;
                            }

                            // Cancel (Secondary neutral action)
                            if crate::ui::icons::render_adobe_button_variant(
                                ui,
                                "Cancel",
                                crate::ui::icons::AdobeButtonVariant::Secondary,
                            )
                            .clicked()
                            {
                                dismiss = true;
                            }
                        });
                    });
                });

            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                dismiss = true;
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                do_save = true;
            }

            if dismiss {
                self.pending_close_confirm = None;
            } else if do_save {
                match confirm.target {
                    CloseTarget::Tab(idx) => {
                        self.save_pdf_tab(idx);
                        self.force_close_tab(idx);
                        self.pending_close_confirm = None;
                    }
                    CloseTarget::Window => {
                        self.save_pdf_tab(confirm.tab_idx);
                        if let Some((next_idx, next_title)) =
                            self.tabs.iter().enumerate().find_map(|(i, t)| {
                                if i != confirm.tab_idx {
                                    if let Tab::Pdf(pdf) = t {
                                        if pdf.has_unsaved_changes {
                                            return Some((i, pdf.title.clone()));
                                        }
                                    }
                                }
                                None
                            })
                        {
                            self.pending_close_confirm = Some(PendingCloseConfirmation {
                                target: CloseTarget::Window,
                                tab_idx: next_idx,
                                doc_title: next_title,
                            });
                        } else {
                            self.pending_close_confirm = None;
                            self.save_session_and_history();
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                }
            } else if do_discard {
                match confirm.target {
                    CloseTarget::Tab(idx) => {
                        self.force_close_tab(idx);
                        self.pending_close_confirm = None;
                    }
                    CloseTarget::Window => {
                        if let Some(Tab::Pdf(pdf)) = self.tabs.get_mut(confirm.tab_idx) {
                            pdf.has_unsaved_changes = false;
                        }
                        if let Some((next_idx, next_title)) =
                            self.tabs.iter().enumerate().find_map(|(i, t)| {
                                if let Tab::Pdf(pdf) = t {
                                    if pdf.has_unsaved_changes {
                                        return Some((i, pdf.title.clone()));
                                    }
                                }
                                None
                            })
                        {
                            self.pending_close_confirm = Some(PendingCloseConfirmation {
                                target: CloseTarget::Window,
                                tab_idx: next_idx,
                                doc_title: next_title,
                            });
                        } else {
                            self.pending_close_confirm = None;
                            self.save_session_and_history();
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                }
            }
        }

        // About Dialog Modal
        if self.show_about_modal {
            let mut close_about = false;
            egui::Modal::new(egui::Id::new("verso_about_modal"))
                .backdrop_color(egui::Color32::from_black_alpha(175))
                .show(ctx, |ui| {
                    ui.set_width(430.0);

                    egui::Frame::NONE
                        .fill(egui::Color32::from_rgb(26, 26, 28))
                        .stroke(egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(50, 50, 56)))
                        .corner_radius(crate::ui::theme::Theme::RADIUS)
                        .inner_margin(egui::Margin::symmetric(22, 18))
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(0.0, 10.0);

                            // Header row: Logo, Titles, and close button
                            ui.horizontal(|ui| {
                                let (icon_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(34.0, 34.0),
                                    egui::Sense::hover(),
                                );
                                crate::ui::icons::paint_verso_logo(ui.painter(), icon_rect);
                                ui.add_space(10.0);
                                ui.vertical(|ui| {
                                    ui.label(
                                        egui::RichText::new("Verso Reader")
                                            .strong()
                                            .size(19.0)
                                            .color(egui::Color32::WHITE),
                                    );
                                    ui.label(
                                        egui::RichText::new(
                                            "Version 2.0.0 • Fast, Minimalist PDF Reader",
                                        )
                                        .color(egui::Color32::from_rgb(160, 160, 168))
                                        .size(12.0),
                                    );
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let close_btn = crate::ui::icons::render_icon_button(
                                            ui,
                                            egui::vec2(26.0, 26.0),
                                            false,
                                            true,
                                            "Close (Esc)",
                                            crate::ui::icons::paint_close,
                                        );
                                        if close_btn.clicked() {
                                            close_about = true;
                                        }
                                    },
                                );
                            });

                            ui.add_space(2.0);
                            ui.separator();
                            ui.add_space(2.0);

                            ui.label(
                                egui::RichText::new(
                                    "Verso Reader is a high-performance desktop PDF reader for Ubuntu written in Rust, delivering an Edge-inspired experience with instantaneous rendering and low memory footprint.",
                                )
                                .size(12.5)
                                .color(egui::Color32::from_rgb(210, 210, 215)),
                            );

                            ui.add_space(6.0);

                            // Structured Info Card
                            egui::Frame::NONE
                                .fill(egui::Color32::from_rgb(33, 33, 36))
                                .stroke(egui::Stroke::new(
                                    1.0_f32,
                                    egui::Color32::from_rgb(46, 46, 52),
                                ))
                                .corner_radius(crate::ui::theme::Theme::RADIUS)
                                .inner_margin(egui::Margin::symmetric(14, 12))
                                .show(ui, |ui| {
                                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 7.0);

                                    let mut render_info_row = |key: &str, val: &str| {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new(key)
                                                    .strong()
                                                    .size(12.5)
                                                    .color(egui::Color32::from_rgb(150, 150, 155)),
                                            );
                                            ui.add_space(8.0);
                                            ui.label(
                                                egui::RichText::new(val)
                                                    .size(12.5)
                                                    .color(egui::Color32::from_rgb(230, 230, 235)),
                                            );
                                        });
                                    };

                                    render_info_row("Developer:", "Mohammed Alim Uddin");
                                    render_info_row(
                                        "Institution:",
                                        "BUET (Bangladesh University of Engineering and Technology)",
                                    );
                                    render_info_row("Framework:", "Rust • egui • eframe (Glow)");
                                    render_info_row("Engine:", "Google Pdfium (libpdfium.so)");
                                });

                            ui.add_space(8.0);

                            // Action button footer: modern Adobe-styled Close button
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if render_adobe_button(ui, "Close", true).clicked() {
                                        close_about = true;
                                    }
                                },
                            );
                        });
                });

            if close_about || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.show_about_modal = false;
            }
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let _ = self.settings.save();
        self.save_session_and_history();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_touchpad_scroll_multiplier_logic() {
        let mut settings = Settings::default();
        assert_eq!(settings.touchpad_scroll_speed, 4.5);
        assert_eq!(settings.mouse_wheel_speed, 1.0);

        // When touchpad (Point) scroll occurs, multiplier should be settings.touchpad_scroll_speed
        let delta = egui::vec2(0.0, 10.0);
        let speed = settings.touchpad_scroll_speed.clamp(0.5, 10.0);
        let scaled = delta * speed;
        assert_eq!(scaled.y, 45.0);

        // Custom setting
        settings.touchpad_scroll_speed = 6.0;
        let speed2 = settings.touchpad_scroll_speed.clamp(0.5, 10.0);
        let scaled2 = delta * speed2;
        assert_eq!(scaled2.y, 60.0);
    }

    #[test]
    fn test_close_tab_with_unsaved_changes_triggers_confirm() {
        let (tx_cmd, _) = crossbeam_channel::unbounded();
        let (_, rx_evt) = crossbeam_channel::unbounded();
        let mut app = PdfViewApp {
            tx_cmd,
            rx_evt,
            pdfium_status: PdfiumStatus::Ready,
            tabs: Vec::new(),
            active_tab_idx: 0,
            next_doc_id: 1,
            pending_close_confirm: None,
            path_bar: PathBar::new(),
            toolbar: Toolbar::new(),
            is_fullscreen: false,
            pending_open_path: None,
            rx_file_chooser: None,
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
            last_window_title: "Verso Reader".to_string(),
            draft_settings: Settings::default(),
            settings: Settings::default(),
            reading_history: crate::settings::ReadingHistory::default(),
            last_session_save: None,
            session_restored: true,
            settings_saved_toast: None,
            metrics: SystemMetrics::default(),
            show_about_modal: false,
            fullscreen_quickbar_open: false,
            fullscreen_page_input: "1".to_string(),
            fullscreen_is_editing_page: false,
            #[cfg(unix)]
            _ipc_server: None,
            #[cfg(unix)]
            rx_ipc: None,
        };

        let mut tab = PdfTab {
            doc_id: 1,
            path: PathBuf::from("/test.pdf"),
            title: "test.pdf".to_string(),
            page_count: 1,
            page_sizes: vec![PageSize {
                width: 595.0,
                height: 842.0,
            }],
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
            has_unsaved_changes: true,
            freehand_stroke: None,
            editing_text_box: None,
            selected_text_box_id: None,
            dragging_text_box: None,
            resizing_text_box: None,
            nav_history: Vec::new(),
            nav_future: Vec::new(),
            last_accessed: Instant::now(),
            is_dormant: false,
        };

        app.tabs.push(Tab::Pdf(tab.clone()));
        assert_eq!(app.tabs.len(), 1);

        // Attempting to close dirty tab must not remove it, but prompt confirmation
        app.close_tab(0);
        assert_eq!(app.tabs.len(), 1);
        assert!(app.pending_close_confirm.is_some());
        let confirm = app.pending_close_confirm.as_ref().unwrap();
        assert_eq!(confirm.target, CloseTarget::Tab(0));
        assert_eq!(confirm.doc_title, "test.pdf");

        // Force close directly closes it and restores empty tab
        app.force_close_tab(0);
        assert_eq!(app.tabs.len(), 1);
        if let Tab::Pdf(empty) = &app.tabs[0] {
            assert_eq!(empty.title, "New Tab");
        }

        // Clean tab closes immediately without confirmation modal
        app.tabs.clear();
        tab.has_unsaved_changes = false;
        app.pending_close_confirm = None;
        app.tabs.push(Tab::Pdf(tab));
        app.close_tab(0);
        assert!(app.pending_close_confirm.is_none());
        assert_eq!(app.tabs.len(), 1);
        if let Tab::Pdf(empty) = &app.tabs[0] {
            assert_eq!(empty.title, "New Tab");
        }
    }

    #[test]
    fn test_open_pdf_tab_allows_same_doc_multiple_tabs() {
        let (tx_cmd, _) = crossbeam_channel::unbounded();
        let (_, rx_evt) = crossbeam_channel::unbounded();
        let mut app = PdfViewApp {
            tx_cmd,
            rx_evt,
            pdfium_status: PdfiumStatus::Ready,
            tabs: Vec::new(),
            active_tab_idx: 0,
            next_doc_id: 1,
            pending_close_confirm: None,
            path_bar: PathBar::new(),
            toolbar: Toolbar::new(),
            is_fullscreen: false,
            pending_open_path: None,
            rx_file_chooser: None,
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
            last_window_title: "Verso Reader".to_string(),
            draft_settings: Settings::default(),
            settings: Settings::default(),
            reading_history: crate::settings::ReadingHistory::default(),
            last_session_save: None,
            session_restored: true,
            settings_saved_toast: None,
            metrics: SystemMetrics::default(),
            show_about_modal: false,
            fullscreen_quickbar_open: false,
            fullscreen_page_input: "1".to_string(),
            fullscreen_is_editing_page: false,
            #[cfg(unix)]
            _ipc_server: None,
            #[cfg(unix)]
            rx_ipc: None,
        };

        let path1 = PathBuf::from("fixtures/MLE.pdf");
        app.open_pdf_tab(path1.clone());
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active_tab_idx, 0);

        // Open a different document
        let path2 =
            PathBuf::from("fixtures/Modern Operating Systems 4th Edition--Andrew Tanenbaum.pdf");
        app.open_pdf_tab(path2.clone());
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active_tab_idx, 1);

        // Re-opening first document must create a new tab (allows reading different chapters)
        app.open_pdf_tab(path1.clone());
        assert_eq!(
            app.tabs.len(),
            3,
            "Must allow opening same document in multiple tabs"
        );
        assert_eq!(app.active_tab_idx, 2, "Must activate the newly opened tab");
        assert_eq!(app.path_bar.text, path1.display().to_string());

        // Switching back to tab 1 must update path_bar text to path 2
        app.switch_to_tab(1);
        assert_eq!(app.active_tab_idx, 1);
        assert_eq!(app.path_bar.text, path2.display().to_string());

        // Switching to tab 0 must update path_bar text to path 1
        app.switch_to_tab(0);
        assert_eq!(app.active_tab_idx, 0);
        assert_eq!(app.path_bar.text, path1.display().to_string());
    }

    #[test]
    fn test_navigation_history_back_forward_stack() {
        let mut tab = PdfTab {
            doc_id: 1,
            path: PathBuf::from("/test.pdf"),
            title: "test.pdf".to_string(),
            page_count: 50,
            page_sizes: vec![PageSize {
                width: 595.0,
                height: 842.0,
            }],
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
            editing_text_box: None,
            selected_text_box_id: None,
            dragging_text_box: None,
            resizing_text_box: None,
            nav_history: Vec::new(),
            nav_future: Vec::new(),
            last_accessed: Instant::now(),
            is_dormant: false,
        };

        // Initially no history
        assert!(!tab.can_go_back());
        assert!(!tab.can_go_forward());

        // Jump 1: From Page 1 (offset 0.0) to Page 15 (offset 1500.0)
        tab.record_nav_jump();
        tab.current_page = 15;
        tab.scroll_offset_y = 1500.0;
        assert!(tab.can_go_back());
        assert!(!tab.can_go_forward());
        assert_eq!(tab.nav_history.len(), 1);
        assert_eq!(tab.nav_history[0], (1, 0.0));

        // Jump 2: From Page 15 (offset 1500.0) to Page 30 (offset 3000.0)
        tab.record_nav_jump();
        assert_eq!(tab.nav_history.len(), 2);
        assert_eq!(tab.nav_history[1], (15, 1500.0));

        // Deduplication: Calling record_nav_jump again from the same location must not duplicate entry
        tab.scroll_offset_y = 1510.0;
        tab.record_nav_jump();
        assert_eq!(
            tab.nav_history.len(),
            2,
            "Must deduplicate near identical jumps"
        );

        tab.current_page = 30;
        tab.scroll_offset_y = 3000.0;

        // Navigate Back -> Should jump to Page 15, offset 1500.0
        tab.go_back();
        assert_eq!(tab.pending_set_scroll_offset_y, Some(1500.0));
        assert!(tab.pending_scroll_to_page.is_none());
        assert!(tab.can_go_back());
        assert!(tab.can_go_forward());
        assert_eq!(tab.nav_future.len(), 1);
        assert_eq!(tab.nav_future[0], (30, 3000.0));

        // Simulate viewer reaching Page 15
        tab.current_page = 15;
        tab.scroll_offset_y = 1500.0;

        // Navigate Back again -> Should jump to Page 1, offset 0.0
        tab.go_back();
        assert_eq!(tab.pending_scroll_to_page, Some(1));
        assert!(!tab.can_go_back());
        assert!(tab.can_go_forward());
        assert_eq!(tab.nav_future.len(), 2);

        // Simulate viewer reaching Page 1
        tab.current_page = 1;
        tab.scroll_offset_y = 0.0;

        // Navigate Forward -> Should return to Page 15, offset 1500.0
        tab.go_forward();
        assert_eq!(tab.pending_set_scroll_offset_y, Some(1500.0));
        assert!(tab.can_go_back());
        assert!(tab.can_go_forward());

        // Simulate viewer reaching Page 15
        tab.current_page = 15;
        tab.scroll_offset_y = 1500.0;

        // Branching: If we now perform a new jump to Page 42, nav_future must be cleared
        tab.record_nav_jump();
        tab.current_page = 42;
        tab.scroll_offset_y = 4200.0;
        assert!(tab.can_go_back());
        assert!(
            !tab.can_go_forward(),
            "New jump must clear forward navigation stack"
        );
    }

    #[test]
    fn test_reading_history_tracking_and_pruning() {
        let (tx_cmd, _) = crossbeam_channel::unbounded();
        let (_, rx_evt) = crossbeam_channel::unbounded();
        let mut app = PdfViewApp {
            tx_cmd,
            rx_evt,
            pdfium_status: PdfiumStatus::Ready,
            tabs: Vec::new(),
            active_tab_idx: 0,
            next_doc_id: 1,
            pending_close_confirm: None,
            path_bar: PathBar::new(),
            toolbar: Toolbar::new(),
            is_fullscreen: false,
            pending_open_path: None,
            rx_file_chooser: None,
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
            last_window_title: "Verso Reader".to_string(),
            draft_settings: Settings::default(),
            settings: Settings::default(),
            reading_history: crate::settings::ReadingHistory::default(),
            last_session_save: None,
            session_restored: true,
            settings_saved_toast: None,
            metrics: SystemMetrics::default(),
            show_about_modal: false,
            fullscreen_quickbar_open: false,
            fullscreen_page_input: "1".to_string(),
            fullscreen_is_editing_page: false,
            #[cfg(unix)]
            _ipc_server: None,
            #[cfg(unix)]
            rx_ipc: None,
        };

        let dummy_tab = PdfTab {
            doc_id: 1,
            path: PathBuf::from("/tmp/doc1.pdf"),
            title: "doc1.pdf".to_string(),
            page_count: 10,
            page_sizes: Vec::new(),
            current_page: 7,
            zoom: 1.5,
            rotation: 90,
            scroll_offset_y: 1234.0,
            pending_scroll_to_page: None,
            pending_scroll_delta_y: None,
            pending_set_scroll_offset_y: None,
            last_viewport_height: 600.0,
            last_viewport_top: 0.0,
            last_scroll_offset_y: 1234.0,
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
            editing_text_box: None,
            selected_text_box_id: None,
            dragging_text_box: None,
            resizing_text_box: None,
            nav_history: Vec::new(),
            nav_future: Vec::new(),
            last_accessed: Instant::now(),
            is_dormant: false,
        };

        app.tabs.push(Tab::Pdf(dummy_tab));
        app.update_reading_history_for_tab(0);

        let key = Settings::canonical_path_key(&PathBuf::from("/tmp/doc1.pdf"));
        let pos = app
            .reading_history
            .entries
            .get(&key)
            .expect("entry must exist");
        assert_eq!(pos.current_page, 7);
        assert_eq!(pos.scroll_offset_y, 1234.0);
        assert_eq!(pos.zoom, 1.5);
        assert_eq!(pos.rotation, 90);
    }

    #[test]
    fn test_open_pdf_tab_replaces_active_empty_tab() {
        let (tx_cmd, _) = crossbeam_channel::unbounded();
        let (_, rx_evt) = crossbeam_channel::unbounded();
        let mut app = PdfViewApp {
            tx_cmd,
            rx_evt,
            pdfium_status: PdfiumStatus::Ready,
            tabs: Vec::new(),
            active_tab_idx: 0,
            next_doc_id: 1,
            pending_close_confirm: None,
            path_bar: PathBar::new(),
            toolbar: Toolbar::new(),
            is_fullscreen: false,
            pending_open_path: None,
            rx_file_chooser: None,
            cache: TextureCache::default(),
            text_cache: PageTextCache::default(),
            pending_render_requests: HashSet::new(),
            render_generation: 0,
            debug_visible: false,
            last_frame_time: Instant::now(),
            frame_time_ms: 16.0,
            metrics: SystemMetrics::default(),
            #[cfg(target_os = "linux")]
            rx_pinch_gesture: None,
            #[cfg(target_os = "linux")]
            wayland_gesture_init_attempted: false,
            pinch_active: false,
            pinch_last_active: None,
            last_scroll_unit: None,
            last_window_title: "Verso Reader".to_string(),
            draft_settings: Settings::default(),
            settings: Settings::default(),
            reading_history: crate::settings::ReadingHistory::default(),
            last_session_save: None,
            session_restored: true,
            settings_saved_toast: None,
            show_about_modal: false,
            fullscreen_quickbar_open: false,
            fullscreen_page_input: "1".to_string(),
            fullscreen_is_editing_page: false,
            #[cfg(unix)]
            _ipc_server: None,
            #[cfg(unix)]
            rx_ipc: None,
        };

        // App starts with an empty tab
        app.create_empty_tab();
        assert_eq!(app.tabs.len(), 1);
        if let Tab::Pdf(ref t) = app.tabs[0] {
            assert!(t.path.as_os_str().is_empty());
        }

        // Opening a document replaces the active empty tab
        let path1 = PathBuf::from("fixtures/MLE.pdf");
        app.open_pdf_tab(path1.clone());
        assert_eq!(app.tabs.len(), 1, "Must replace active empty tab slot");
        if let Tab::Pdf(ref t) = app.tabs[0] {
            assert_eq!(t.path, path1);
        }

        // Opening a new tab creates a second tab
        app.create_empty_tab();
        assert_eq!(app.tabs.len(), 2);
        assert_eq!(app.active_tab_idx, 1);

        // Opening document in the active empty tab replaces tab 1
        let path2 =
            PathBuf::from("fixtures/Modern Operating Systems 4th Edition--Andrew Tanenbaum.pdf");
        app.open_pdf_tab(path2.clone());
        assert_eq!(
            app.tabs.len(),
            2,
            "Must replace newly active empty tab slot"
        );
        if let Tab::Pdf(ref t) = app.tabs[1] {
            assert_eq!(t.path, path2);
        }
    }

    #[test]
    fn test_session_save_guarded_by_session_restored() {
        let (tx_cmd, _) = crossbeam_channel::unbounded();
        let (_, rx_evt) = crossbeam_channel::unbounded();
        let mut app = PdfViewApp {
            tx_cmd,
            rx_evt,
            pdfium_status: PdfiumStatus::Ready,
            tabs: Vec::new(),
            active_tab_idx: 0,
            next_doc_id: 1,
            pending_close_confirm: None,
            path_bar: PathBar::new(),
            toolbar: Toolbar::new(),
            is_fullscreen: false,
            pending_open_path: None,
            rx_file_chooser: None,
            cache: TextureCache::default(),
            text_cache: PageTextCache::default(),
            pending_render_requests: HashSet::new(),
            render_generation: 0,
            debug_visible: false,
            last_frame_time: Instant::now(),
            frame_time_ms: 16.0,
            metrics: SystemMetrics::default(),
            #[cfg(target_os = "linux")]
            rx_pinch_gesture: None,
            #[cfg(target_os = "linux")]
            wayland_gesture_init_attempted: false,
            pinch_active: false,
            pinch_last_active: None,
            last_scroll_unit: None,
            last_window_title: "Verso Reader".to_string(),
            draft_settings: Settings::default(),
            settings: Settings::default(),
            reading_history: crate::settings::ReadingHistory::default(),
            last_session_save: None,
            session_restored: false, // Session restoration in progress
            settings_saved_toast: None,
            show_about_modal: false,
            fullscreen_quickbar_open: false,
            fullscreen_page_input: "1".to_string(),
            fullscreen_is_editing_page: false,
            #[cfg(unix)]
            _ipc_server: None,
            #[cfg(unix)]
            rx_ipc: None,
        };

        app.create_empty_tab();
        // With session_restored = false, save_session_and_history must safely early-exit
        app.save_session_and_history();
        assert!(!app.session_restored);
    }

    #[test]
    fn test_dormant_tab_transition_and_wakeup() {
        let (tx_cmd, rx_cmd) = crossbeam_channel::unbounded();
        let (_, rx_evt) = crossbeam_channel::unbounded();
        let mut app = PdfViewApp {
            tx_cmd,
            rx_evt,
            pdfium_status: PdfiumStatus::Ready,
            tabs: Vec::new(),
            active_tab_idx: 0,
            next_doc_id: 1,
            pending_close_confirm: None,
            path_bar: PathBar::new(),
            toolbar: Toolbar::new(),
            is_fullscreen: false,
            pending_open_path: None,
            rx_file_chooser: None,
            cache: TextureCache::default(),
            text_cache: PageTextCache::default(),
            pending_render_requests: HashSet::new(),
            render_generation: 0,
            debug_visible: false,
            last_frame_time: Instant::now(),
            frame_time_ms: 16.0,
            metrics: SystemMetrics::default(),
            #[cfg(target_os = "linux")]
            rx_pinch_gesture: None,
            #[cfg(target_os = "linux")]
            wayland_gesture_init_attempted: false,
            pinch_active: false,
            pinch_last_active: None,
            last_scroll_unit: None,
            last_window_title: "Verso Reader".to_string(),
            draft_settings: Settings::default(),
            settings: Settings::default(),
            reading_history: crate::settings::ReadingHistory::default(),
            last_session_save: None,
            session_restored: true,
            settings_saved_toast: None,
            show_about_modal: false,
            fullscreen_quickbar_open: false,
            fullscreen_page_input: "1".to_string(),
            fullscreen_is_editing_page: false,
            #[cfg(unix)]
            _ipc_server: None,
            #[cfg(unix)]
            rx_ipc: None,
        };

        let tab0 = PdfTab {
            doc_id: 10,
            path: PathBuf::from("/tmp/doc0.pdf"),
            title: "doc0.pdf".to_string(),
            page_count: 10,
            page_sizes: vec![PageSize {
                width: 500.0,
                height: 700.0,
            }],
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
            editing_text_box: None,
            selected_text_box_id: None,
            dragging_text_box: None,
            resizing_text_box: None,
            nav_history: Vec::new(),
            nav_future: Vec::new(),
            last_accessed: Instant::now(),
            is_dormant: false,
        };

        let mut tab1 = tab0.clone();
        tab1.doc_id = 20;
        tab1.path = PathBuf::from("/tmp/doc1.pdf");
        tab1.title = "doc1.pdf".to_string();
        tab1.current_page = 5;
        tab1.scroll_offset_y = 1200.0;
        tab1.last_accessed = Instant::now() - std::time::Duration::from_secs(600);

        app.tabs.push(Tab::Pdf(tab0));
        app.tabs.push(Tab::Pdf(tab1));
        app.active_tab_idx = 0;

        app.check_dormant_tabs();

        if let Tab::Pdf(ref t1) = app.tabs[1] {
            assert!(
                t1.is_dormant,
                "Tab 1 must become dormant after 10m inactivity"
            );
        } else {
            panic!("Expected Tab::Pdf");
        }

        let mut unloaded = false;
        while let Ok(cmd) = rx_cmd.try_recv() {
            if let Command::UnloadDoc { doc_id } = cmd {
                if doc_id == 20 {
                    unloaded = true;
                }
            }
        }
        assert!(unloaded, "Worker must receive Command::UnloadDoc for tab 1");

        app.switch_to_tab(1);
        assert_eq!(app.active_tab_idx, 1);

        if let Tab::Pdf(ref t1) = app.tabs[1] {
            assert!(!t1.is_dormant, "Tab 1 must be awakened on switch");
            assert_eq!(t1.current_page, 5, "Reading page must be preserved");
            assert_eq!(
                t1.scroll_offset_y, 1200.0,
                "Scroll offset must be preserved"
            );
        } else {
            panic!("Expected Tab::Pdf");
        }

        let mut reopened = false;
        while let Ok(cmd) = rx_cmd.try_recv() {
            if let Command::Open { doc_id, path } = cmd {
                if doc_id == 20 && path == std::path::Path::new("/tmp/doc1.pdf") {
                    reopened = true;
                }
            }
        }
        assert!(
            reopened,
            "Worker must receive Command::Open on dormant tab wakeup"
        );
    }

    #[test]
    fn test_pathbar_collapse_and_uncollapse_on_tab_states() {
        let (tx_cmd, _rx_cmd) = crossbeam_channel::unbounded();
        let (_, rx_evt) = crossbeam_channel::unbounded();
        let mut app = PdfViewApp {
            tx_cmd,
            rx_evt,
            pdfium_status: PdfiumStatus::Ready,
            tabs: Vec::new(),
            active_tab_idx: 0,
            next_doc_id: 1,
            pending_close_confirm: None,
            path_bar: PathBar::new(),
            toolbar: Toolbar::new(),
            is_fullscreen: false,
            pending_open_path: None,
            rx_file_chooser: None,
            cache: TextureCache::default(),
            text_cache: PageTextCache::default(),
            pending_render_requests: HashSet::new(),
            render_generation: 0,
            debug_visible: false,
            last_frame_time: Instant::now(),
            frame_time_ms: 16.0,
            metrics: SystemMetrics::default(),
            #[cfg(target_os = "linux")]
            rx_pinch_gesture: None,
            #[cfg(target_os = "linux")]
            wayland_gesture_init_attempted: false,
            pinch_active: false,
            pinch_last_active: None,
            last_scroll_unit: None,
            last_window_title: "Verso Reader".to_string(),
            draft_settings: Settings::default(),
            settings: Settings::default(),
            reading_history: crate::settings::ReadingHistory::default(),
            last_session_save: None,
            session_restored: true,
            settings_saved_toast: None,
            show_about_modal: false,
            fullscreen_quickbar_open: false,
            fullscreen_page_input: "1".to_string(),
            fullscreen_is_editing_page: false,
            #[cfg(unix)]
            _ipc_server: None,
            #[cfg(unix)]
            rx_ipc: None,
        };

        // Open a PDF document: pathbar must collapse
        let path1 = PathBuf::from("fixtures/MLE.pdf");
        app.open_pdf_tab(path1);
        assert_eq!(app.tabs.len(), 1);
        assert!(
            app.path_bar.is_collapsed,
            "Path bar must collapse when PDF is opened"
        );

        // Create empty tab: pathbar must uncollapse
        app.create_empty_tab();
        assert_eq!(app.tabs.len(), 2);
        assert!(
            !app.path_bar.is_collapsed,
            "Path bar must be uncollapsed on empty tab"
        );

        // Switch back to PDF tab: pathbar must collapse
        app.switch_to_tab(0);
        assert!(
            app.path_bar.is_collapsed,
            "Path bar must re-collapse when switching to PDF tab"
        );

        // Switch to empty tab: pathbar must uncollapse
        app.switch_to_tab(1);
        assert!(
            !app.path_bar.is_collapsed,
            "Path bar must uncollapse when switching to empty tab"
        );
    }

    #[test]
    fn test_configurable_inactivity_timeout() {
        let (tx_cmd, _rx_cmd) = crossbeam_channel::unbounded();
        let (_, rx_evt) = crossbeam_channel::unbounded();
        let mut app = PdfViewApp {
            tx_cmd,
            rx_evt,
            pdfium_status: PdfiumStatus::Ready,
            tabs: Vec::new(),
            active_tab_idx: 0,
            next_doc_id: 1,
            pending_close_confirm: None,
            path_bar: PathBar::new(),
            toolbar: Toolbar::new(),
            is_fullscreen: false,
            pending_open_path: None,
            rx_file_chooser: None,
            cache: TextureCache::default(),
            text_cache: PageTextCache::default(),
            pending_render_requests: HashSet::new(),
            render_generation: 0,
            debug_visible: false,
            last_frame_time: Instant::now(),
            frame_time_ms: 16.0,
            metrics: SystemMetrics::default(),
            #[cfg(target_os = "linux")]
            rx_pinch_gesture: None,
            #[cfg(target_os = "linux")]
            wayland_gesture_init_attempted: false,
            pinch_active: false,
            pinch_last_active: None,
            last_scroll_unit: None,
            last_window_title: "Verso Reader".to_string(),
            draft_settings: Settings::default(),
            settings: Settings::default(),
            reading_history: crate::settings::ReadingHistory::default(),
            last_session_save: None,
            session_restored: true,
            settings_saved_toast: None,
            show_about_modal: false,
            fullscreen_quickbar_open: false,
            fullscreen_page_input: "1".to_string(),
            fullscreen_is_editing_page: false,
            #[cfg(unix)]
            _ipc_server: None,
            #[cfg(unix)]
            rx_ipc: None,
        };

        let tab0 = PdfTab {
            doc_id: 10,
            path: PathBuf::from("/tmp/doc0.pdf"),
            title: "doc0.pdf".to_string(),
            page_count: 10,
            page_sizes: vec![PageSize {
                width: 500.0,
                height: 700.0,
            }],
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
            editing_text_box: None,
            selected_text_box_id: None,
            dragging_text_box: None,
            resizing_text_box: None,
            nav_history: Vec::new(),
            nav_future: Vec::new(),
            last_accessed: Instant::now(),
            is_dormant: false,
        };

        let mut tab1 = tab0.clone();
        tab1.doc_id = 20;
        tab1.path = PathBuf::from("/tmp/doc1.pdf");
        tab1.last_accessed = Instant::now() - std::time::Duration::from_secs(120); // 2 mins inactive

        app.tabs.push(Tab::Pdf(tab0));
        app.tabs.push(Tab::Pdf(tab1));
        app.active_tab_idx = 0;

        // With timeout = 0 (disabled), tab 1 must not become dormant
        app.settings.inactivity_timeout_mins = 0;
        app.check_dormant_tabs();
        if let Tab::Pdf(ref t1) = app.tabs[1] {
            assert!(
                !t1.is_dormant,
                "Tab must not become dormant when timeout is disabled (0)"
            );
        }

        // With timeout = 1 (1 min), tab 1 (2 mins inactive) must become dormant
        app.settings.inactivity_timeout_mins = 1;
        app.check_dormant_tabs();
        if let Tab::Pdf(ref t1) = app.tabs[1] {
            assert!(
                t1.is_dormant,
                "Tab must become dormant when inactive > timeout"
            );
        }
    }

    #[test]
    fn test_bengali_font_fallback_registered() {
        let ctx = egui::Context::default();
        crate::setup_custom_fonts(&ctx);
    }
}
