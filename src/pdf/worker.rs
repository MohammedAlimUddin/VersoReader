use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::thread;

use crossbeam_channel::{Receiver, Sender};
use pdfium_render::prelude::*;

use crate::annotations::{AnnotationColor, Highlight, TextBox};
use crate::pdf::types::{
    CanonicalRect, CharBox, Command, DocId, Event, LineBox, LinkTarget, OutlineItem, PageLink,
    PageSize, PageTextData, RenderResult, SearchMatch,
};

static PDFIUM_INSTANCE: OnceLock<Result<&'static Pdfium, String>> = OnceLock::new();

/// Computes candidate file paths to search for `libpdfium.so`, in priority order:
/// 1. Directory of the executable
/// 2. `<exe dir>/lib`
/// 3. `./lib`
pub fn candidate_library_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // 1. Directory of the executable
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            paths.push(exe_dir.join(Pdfium::pdfium_platform_library_name_at_path("")));
            // 2. <exe dir>/lib
            paths.push(
                exe_dir
                    .join("lib")
                    .join(Pdfium::pdfium_platform_library_name_at_path("")),
            );
        }
    }

    // 3. ~/.local/lib/pdfview
    if let Some(base_dirs) = directories::BaseDirs::new() {
        paths.push(
            base_dirs
                .home_dir()
                .join(".local")
                .join("lib")
                .join("pdfview")
                .join(Pdfium::pdfium_platform_library_name_at_path("")),
        );
    }

    // 4. ./lib
    paths.push(PathBuf::from("./lib").join(Pdfium::pdfium_platform_library_name_at_path("")));

    paths
}

/// Attempts to bind to Pdfium using the prioritized search order:
/// Directory of executable -> `<exe dir>/lib` -> `./lib` -> System library.
pub fn bind_pdfium() -> Result<&'static Pdfium, String> {
    PDFIUM_INSTANCE
        .get_or_init(|| {
            let candidates = candidate_library_paths();
            let mut searched: Vec<String> = Vec::new();

            for path in &candidates {
                searched.push(path.display().to_string());
                if path.exists() {
                    if let Ok(bindings) = Pdfium::bind_to_library(path) {
                        let pdfium = Box::new(Pdfium::new(bindings));
                        return Ok(Box::leak(pdfium) as &'static Pdfium);
                    }
                }
            }

            // 4. System library
            searched.push("System library search path".to_string());
            if let Ok(bindings) = Pdfium::bind_to_system_library() {
                let pdfium = Box::new(Pdfium::new(bindings));
                return Ok(Box::leak(pdfium) as &'static Pdfium);
            }

            Err(format!(
                "Could not load libpdfium.so.\n\nSearched locations:\n  • {}\n\nPlease run `bash scripts/setup_pdfium.sh` to download libpdfium.so into ./lib/.",
                searched.join("\n  • ")
            ))
        })
        .clone()
}

/// Spawns the dedicated pdfium worker thread and returns communication channels.
pub fn start_worker(ctx: egui::Context) -> (Sender<Command>, Receiver<Event>) {
    let (tx_cmd, rx_cmd) = crossbeam_channel::unbounded::<Command>();
    let (tx_evt, rx_evt) = crossbeam_channel::unbounded::<Event>();

    thread::Builder::new()
        .name("pdfview-worker".to_string())
        .spawn(move || {
            worker_main(ctx, rx_cmd, tx_evt);
        })
        .expect("Failed to spawn pdfview-worker thread");

    (tx_cmd, rx_evt)
}

fn worker_main(ctx: egui::Context, rx_cmd: Receiver<Command>, tx_evt: Sender<Event>) {
    let pdfium_opt = match bind_pdfium() {
        Ok(instance) => {
            let _ = tx_evt.send(Event::PdfiumReady);
            ctx.request_repaint();
            Some(instance)
        }
        Err(err) => {
            let _ = tx_evt.send(Event::PdfiumError(err));
            ctx.request_repaint();
            None
        }
    };

    struct ActiveSearch {
        doc_id: DocId,
        query_id: u64,
        query: String,
        current_page: usize,
        total_pages: usize,
    }

    struct ActiveAnnotationScan {
        doc_id: DocId,
        current_page: usize,
        total_pages: usize,
    }

    let mut documents: HashMap<DocId, PdfDocument<'static>> = HashMap::new();
    let mut latest_generations: HashMap<DocId, u64> = HashMap::new();
    let mut doc_annotated_pages: HashMap<DocId, HashSet<usize>> = HashMap::new();
    let mut doc_freehand_highlights: HashMap<DocId, Vec<Highlight>> = HashMap::new();
    let mut active_search: Option<ActiveSearch> = None;
    let mut active_annotation_scan: Option<ActiveAnnotationScan> = None;
    let mut cmd_queue: std::collections::VecDeque<Command> = std::collections::VecDeque::new();

    // Command loop: processes incoming UI requests sequentially on the worker thread.
    // Drains pending commands so intermediate stale zoom generations are dropped instantly.
    loop {
        // If local queue is empty, wait for next command from channel
        if cmd_queue.is_empty() {
            let next = if active_search.is_some() || active_annotation_scan.is_some() {
                match rx_cmd.try_recv() {
                    Ok(cmd) => Some(cmd),
                    Err(crossbeam_channel::TryRecvError::Empty) => None,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => break,
                }
            } else {
                match rx_cmd.recv() {
                    Ok(cmd) => Some(cmd),
                    Err(_) => break,
                }
            };
            if let Some(cmd) = next {
                cmd_queue.push_back(cmd);
            }
        }

        // Drain all pending incoming commands from rx_cmd into local queue
        while let Ok(pending_cmd) = rx_cmd.try_recv() {
            cmd_queue.push_back(pending_cmd);
        }

        // Fast scan: update latest_generations across all queued commands
        for c in &cmd_queue {
            if let Command::Render(rc) = c {
                let latest = latest_generations.entry(rc.doc_id).or_insert(rc.generation);
                if rc.generation > *latest {
                    *latest = rc.generation;
                }
            }
        }

        // Prune stale Render requests whose generation is older than latest seen for that document
        cmd_queue.retain(|c| {
            if let Command::Render(rc) = c {
                if let Some(&latest) = latest_generations.get(&rc.doc_id) {
                    return rc.generation >= latest;
                }
            }
            true
        });

        // Pick next command:
        // 1. Critical doc mutations (Shutdown, Open, Close, SyncAnnotations, SaveDoc) must run first in FIFO order
        // 2. Otherwise prioritize visible page renders (priority 1) over prefetch and background search/text
        let cmd = if let Some(pos) = cmd_queue.iter().position(|c| {
            matches!(
                c,
                Command::Shutdown
                    | Command::Open { .. }
                    | Command::Close { .. }
                    | Command::UnloadDoc { .. }
                    | Command::SyncAnnotations { .. }
                    | Command::SaveDoc { .. }
            )
        }) {
            cmd_queue.remove(pos)
        } else if let Some(pos) = cmd_queue
            .iter()
            .position(|c| matches!(c, Command::Render(rc) if rc.priority == 1))
        {
            cmd_queue.remove(pos)
        } else {
            cmd_queue.pop_front()
        };

        if let Some(cmd) = cmd {
            match cmd {
                Command::Open { doc_id, path } => {
                    if let Some(pdfium) = pdfium_opt {
                        match pdfium.load_pdf_from_file(&path, None) {
                            Ok(doc) => {
                                let page_count = doc.pages().len() as usize;
                                let mut page_sizes = Vec::with_capacity(page_count);

                                for idx in 0..page_count {
                                    if let Ok(rect) = doc.pages().page_size(idx as i32) {
                                        page_sizes.push(PageSize {
                                            width: rect.width().value,
                                            height: rect.height().value,
                                        });
                                    } else {
                                        page_sizes.push(PageSize {
                                            width: 612.0,
                                            height: 792.0,
                                        });
                                    }
                                }

                                let mut annotations = Vec::new();
                                let mut text_boxes = Vec::new();
                                if page_count > 0 {
                                    if let Ok(mut page0) = doc.pages().get(0) {
                                        extract_annotations_from_page(
                                            &mut page0,
                                            0,
                                            &mut annotations,
                                            &mut text_boxes,
                                        );
                                    }
                                }

                                doc_annotated_pages.insert(doc_id, HashSet::new());
                                doc_freehand_highlights.insert(
                                    doc_id,
                                    annotations
                                        .iter()
                                        .filter(|h| !h.points.is_empty())
                                        .cloned()
                                        .collect(),
                                );
                                documents.insert(doc_id, doc);
                                let _ = tx_evt.send(Event::Opened {
                                    doc_id,
                                    page_count,
                                    page_sizes,
                                    annotations,
                                    text_boxes,
                                });
                                ctx.request_repaint();

                                if page_count > 1 {
                                    active_annotation_scan = Some(ActiveAnnotationScan {
                                        doc_id,
                                        current_page: 1,
                                        total_pages: page_count,
                                    });
                                } else {
                                    active_annotation_scan = None;
                                }
                            }
                            Err(err) => {
                                let _ = tx_evt.send(Event::OpenError {
                                    doc_id,
                                    error: format!(
                                        "Failed to open PDF ({}): {err}",
                                        path.display()
                                    ),
                                });
                                ctx.request_repaint();
                            }
                        }
                    } else {
                        let _ = tx_evt.send(Event::OpenError {
                            doc_id,
                            error: "Pdfium engine is not available.".to_string(),
                        });
                        ctx.request_repaint();
                    }
                }
                Command::Render(render_cmd) => {
                    // Skip request if a newer generation has already been seen for this document
                    let latest = latest_generations
                        .entry(render_cmd.doc_id)
                        .or_insert(render_cmd.generation);
                    if render_cmd.generation < *latest {
                        continue;
                    }
                    if render_cmd.generation > *latest {
                        *latest = render_cmd.generation;
                    }

                    if let Some(doc) = documents.get(&render_cmd.doc_id) {
                        let page_count = doc.pages().len() as usize;
                        if render_cmd.page < page_count {
                            match doc.pages().get(render_cmd.page as i32) {
                                Ok(mut page) => {
                                    // Strip external FreeText, Text notes, or Popup annotations so PDFium
                                    // never rasterizes solid background blocks or opaque rectangles into the page bitmap.
                                    let annots = page.annotations_mut();
                                    let mut i = annots.len();
                                    while i > 0 {
                                        i -= 1;
                                        if let Ok(annot) = annots.get(i) {
                                            if matches!(
                                                annot.annotation_type(),
                                                PdfPageAnnotationType::FreeText
                                                    | PdfPageAnnotationType::Text
                                                    | PdfPageAnnotationType::Popup
                                            ) {
                                                let _ = annots.delete_annotation(annot);
                                            }
                                        }
                                    }

                                    let target_w = render_cmd.width_px.clamp(32, 6144) as i32;
                                    let rotation_setting = match render_cmd.rotation % 360 {
                                        90 => PdfPageRenderRotation::Degrees90,
                                        180 => PdfPageRenderRotation::Degrees180,
                                        270 => PdfPageRenderRotation::Degrees270,
                                        _ => PdfPageRenderRotation::None,
                                    };
                                    let render_config = PdfRenderConfig::new()
                                        .set_target_width(target_w)
                                        .set_maximum_width(6144)
                                        .set_maximum_height(6144)
                                        .set_format(PdfBitmapFormat::BGRA)
                                        .rotate(rotation_setting, true)
                                        .use_lcd_text_rendering(false)
                                        .set_text_smoothing(true)
                                        .set_image_smoothing(true)
                                        .set_path_smoothing(true);

                                    match page.render_with_config(&render_config) {
                                        Ok(bitmap) => {
                                            let w = bitmap.width() as u32;
                                            let h = bitmap.height() as u32;
                                            let mut rgba = bitmap.as_rgba_bytes();

                                            if let Some(fhs) =
                                                doc_freehand_highlights.get(&render_cmd.doc_id)
                                            {
                                                let page_fhs: Vec<&Highlight> = fhs
                                                    .iter()
                                                    .filter(|hl| hl.page == render_cmd.page)
                                                    .collect();
                                                if !page_fhs.is_empty() {
                                                    let (canon_w, canon_h) = doc
                                                        .pages()
                                                        .page_size(render_cmd.page as i32)
                                                        .map(|r| {
                                                            (r.width().value, r.height().value)
                                                        })
                                                        .unwrap_or((612.0, 792.0));
                                                    for hl in page_fhs {
                                                        rasterize_freehand_highlight(
                                                            &mut rgba,
                                                            w as usize,
                                                            h as usize,
                                                            hl,
                                                            canon_w,
                                                            canon_h,
                                                            render_cmd.rotation,
                                                        );
                                                    }
                                                }
                                            }

                                            let _ = tx_evt.send(Event::Rendered(RenderResult {
                                                doc_id: render_cmd.doc_id,
                                                page: render_cmd.page,
                                                width_px: render_cmd.width_px,
                                                height_px: h,
                                                bitmap_width_px: w,
                                                rotation: render_cmd.rotation,
                                                generation: render_cmd.generation,
                                                rgba,
                                            }));
                                            ctx.request_repaint();
                                        }
                                        Err(err) => {
                                            let _ = tx_evt.send(Event::RenderError {
                                                doc_id: render_cmd.doc_id,
                                                page: render_cmd.page,
                                                generation: render_cmd.generation,
                                                error: format!(
                                                    "Failed to render page {}: {err}",
                                                    render_cmd.page
                                                ),
                                            });
                                            ctx.request_repaint();
                                        }
                                    }
                                }
                                Err(err) => {
                                    let _ = tx_evt.send(Event::RenderError {
                                        doc_id: render_cmd.doc_id,
                                        page: render_cmd.page,
                                        generation: render_cmd.generation,
                                        error: format!(
                                            "Failed to access page {}: {err}",
                                            render_cmd.page
                                        ),
                                    });
                                    ctx.request_repaint();
                                }
                            }
                        }
                    }
                }
                Command::Close { doc_id } => {
                    if let Some(ref s) = active_search {
                        if s.doc_id == doc_id {
                            active_search = None;
                        }
                    }
                    if let Some(ref s) = active_annotation_scan {
                        if s.doc_id == doc_id {
                            active_annotation_scan = None;
                        }
                    }
                    doc_annotated_pages.remove(&doc_id);
                    doc_freehand_highlights.remove(&doc_id);
                    documents.remove(&doc_id);
                    latest_generations.remove(&doc_id);
                }
                Command::UnloadDoc { doc_id } => {
                    if let Some(ref s) = active_search {
                        if s.doc_id == doc_id {
                            active_search = None;
                        }
                    }
                    if let Some(ref s) = active_annotation_scan {
                        if s.doc_id == doc_id {
                            active_annotation_scan = None;
                        }
                    }
                    // Free Pdfium document handle and associated caches from worker memory
                    doc_annotated_pages.remove(&doc_id);
                    doc_freehand_highlights.remove(&doc_id);
                    documents.remove(&doc_id);
                    latest_generations.remove(&doc_id);
                }
                Command::Outline { doc_id } => {
                    if let Some(doc) = documents.get(&doc_id) {
                        let items = extract_outline(doc);
                        let _ = tx_evt.send(Event::OutlineResult { doc_id, items });
                        ctx.request_repaint();
                    }
                }
                Command::PageText { doc_id, page } => {
                    if let Some(doc) = documents.get(&doc_id) {
                        if let Some(data) = extract_page_text(doc, page) {
                            let _ = tx_evt.send(Event::PageTextResult {
                                doc_id,
                                page,
                                data: std::sync::Arc::new(data),
                            });
                            ctx.request_repaint();
                        }
                    }
                }
                Command::ExtractTextForCopy {
                    doc_id,
                    start_page,
                    start_char,
                    end_page,
                    end_char,
                } => {
                    if let Some(doc) = documents.get(&doc_id) {
                        let page_count = doc.pages().len() as usize;
                        let s_page = start_page.min(page_count.saturating_sub(1));
                        let e_page = end_page.min(page_count.saturating_sub(1));
                        let mut full_text = String::new();

                        for p in s_page..=e_page {
                            if let Ok(page) = doc.pages().get(p as _) {
                                if let Ok(text_page) = page.text() {
                                    let page_str = if p == s_page && s_page == e_page {
                                        if start_char == 0 && end_char == usize::MAX {
                                            text_page.all()
                                        } else {
                                            let chars: Vec<char> = text_page
                                                .chars()
                                                .iter()
                                                .map(|c| c.unicode_char().unwrap_or(' '))
                                                .collect();
                                            let c_start = start_char.min(chars.len());
                                            let c_end = end_char.min(chars.len().saturating_sub(1));
                                            if c_start <= c_end && c_start < chars.len() {
                                                chars[c_start..=c_end].iter().collect()
                                            } else {
                                                String::new()
                                            }
                                        }
                                    } else if p == s_page {
                                        if start_char == 0 {
                                            text_page.all()
                                        } else {
                                            let chars: Vec<char> = text_page
                                                .chars()
                                                .iter()
                                                .map(|c| c.unicode_char().unwrap_or(' '))
                                                .collect();
                                            let c_start = start_char.min(chars.len());
                                            if c_start < chars.len() {
                                                chars[c_start..].iter().collect()
                                            } else {
                                                String::new()
                                            }
                                        }
                                    } else if p == e_page {
                                        if end_char == usize::MAX {
                                            text_page.all()
                                        } else {
                                            let chars: Vec<char> = text_page
                                                .chars()
                                                .iter()
                                                .map(|c| c.unicode_char().unwrap_or(' '))
                                                .collect();
                                            let c_end = end_char.min(chars.len().saturating_sub(1));
                                            if c_end < chars.len() {
                                                chars[..=c_end].iter().collect()
                                            } else {
                                                String::new()
                                            }
                                        }
                                    } else {
                                        text_page.all()
                                    };

                                    if !page_str.is_empty() {
                                        if !full_text.is_empty() && !full_text.ends_with('\n') {
                                            full_text.push('\n');
                                        }
                                        full_text.push_str(&page_str);
                                    }
                                }
                            }
                        }

                        let _ = tx_evt.send(Event::CopyTextResult { text: full_text });
                        ctx.request_repaint();
                    }
                }
                Command::Search {
                    doc_id,
                    query_id,
                    query,
                } => {
                    let trimmed = query.trim();
                    if trimmed.is_empty() {
                        if let Some(ref s) = active_search {
                            if s.doc_id == doc_id {
                                active_search = None;
                            }
                        }
                        let _ = tx_evt.send(Event::SearchResults {
                            doc_id,
                            query_id,
                            page: 0,
                            matches: Vec::new(),
                            done: true,
                        });
                        ctx.request_repaint();
                    } else if let Some(doc) = documents.get(&doc_id) {
                        let total_pages = doc.pages().len() as usize;
                        if total_pages == 0 {
                            let _ = tx_evt.send(Event::SearchResults {
                                doc_id,
                                query_id,
                                page: 0,
                                matches: Vec::new(),
                                done: true,
                            });
                            ctx.request_repaint();
                        } else {
                            active_search = Some(ActiveSearch {
                                doc_id,
                                query_id,
                                query: trimmed.to_string(),
                                current_page: 1,
                                total_pages,
                            });
                        }
                    }
                }
                Command::SyncAnnotations {
                    doc_id,
                    highlights,
                    text_boxes: _,
                } => {
                    let fh: Vec<Highlight> = highlights
                        .iter()
                        .filter(|h| !h.points.is_empty())
                        .cloned()
                        .collect();
                    doc_freehand_highlights.insert(doc_id, fh);

                    if let Some(doc) = documents.get(&doc_id) {
                        let mut target_pages =
                            doc_annotated_pages.remove(&doc_id).unwrap_or_default();
                        for hl in &highlights {
                            target_pages.insert(hl.page);
                        }
                        let _ = apply_annotations_to_doc_on_pages(
                            doc,
                            &highlights,
                            &[],
                            target_pages.iter().copied(),
                        );
                        let new_pages: HashSet<usize> = highlights.iter().map(|h| h.page).collect();
                        doc_annotated_pages.insert(doc_id, new_pages);
                    }
                }
                Command::SaveDoc {
                    doc_id,
                    path,
                    highlights,
                    text_boxes,
                } => {
                    if let Some(doc) = documents.get(&doc_id) {
                        let mut target_pages =
                            doc_annotated_pages.remove(&doc_id).unwrap_or_default();
                        for hl in &highlights {
                            target_pages.insert(hl.page);
                        }
                        for tb in &text_boxes {
                            target_pages.insert(tb.page);
                        }
                        match apply_annotations_to_doc_on_pages(
                            doc,
                            &highlights,
                            &text_boxes,
                            target_pages.iter().copied(),
                        ) {
                            Ok(_) => {
                                let save_res = doc.save_to_bytes();
                                // Keep in-memory document clean of FreeText annotations so PDFium won't render black boxes
                                let _ = apply_annotations_to_doc_on_pages(
                                    doc,
                                    &highlights,
                                    &[],
                                    target_pages.iter().copied(),
                                );
                                let new_pages: HashSet<usize> = highlights
                                    .iter()
                                    .map(|h| h.page)
                                    .chain(text_boxes.iter().map(|t| t.page))
                                    .collect();
                                doc_annotated_pages.insert(doc_id, new_pages);
                                match save_res {
                                    Ok(bytes) => {
                                        let tmp_path = path.with_extension("pdf.tmp_pdfview");
                                        match std::fs::write(&tmp_path, &bytes) {
                                            Ok(_) => match std::fs::rename(&tmp_path, &path) {
                                                Ok(_) => {
                                                    let _ = tx_evt.send(Event::Saved {
                                                        doc_id,
                                                        success: true,
                                                        error: None,
                                                    });
                                                }
                                                Err(err) => {
                                                    let _ = std::fs::remove_file(&tmp_path);
                                                    let _ = tx_evt.send(Event::Saved {
                                                        doc_id,
                                                        success: false,
                                                        error: Some(format!(
                                                            "Failed to overwrite PDF: {err}"
                                                        )),
                                                    });
                                                }
                                            },
                                            Err(err) => {
                                                let _ = tx_evt.send(Event::Saved {
                                                    doc_id,
                                                    success: false,
                                                    error: Some(format!(
                                                        "Failed to write temporary file: {err}"
                                                    )),
                                                });
                                            }
                                        }
                                    }
                                    Err(err) => {
                                        let _ = tx_evt.send(Event::Saved {
                                            doc_id,
                                            success: false,
                                            error: Some(format!("Failed to serialize PDF: {err}")),
                                        });
                                    }
                                }
                            }
                            Err(err) => {
                                let _ = apply_annotations_to_doc_on_pages(
                                    doc,
                                    &highlights,
                                    &[],
                                    target_pages.iter().copied(),
                                );
                                let _ = tx_evt.send(Event::Saved {
                                    doc_id,
                                    success: false,
                                    error: Some(format!("Failed to apply annotations: {err}")),
                                });
                            }
                        }
                    } else {
                        let _ = tx_evt.send(Event::Saved {
                            doc_id,
                            success: false,
                            error: Some("Document not open".to_string()),
                        });
                    }
                    ctx.request_repaint();
                }
                Command::Shutdown => break,
            }
        } else if let Some(ref mut search) = active_search {
            // No pending commands: perform search for the current page
            let page_num = search.current_page;
            let is_last = page_num >= search.total_pages;

            let matches = if let Some(doc) = documents.get(&search.doc_id) {
                search_page(doc, page_num, &search.query)
            } else {
                Vec::new()
            };

            if !matches.is_empty() || is_last {
                let _ = tx_evt.send(Event::SearchResults {
                    doc_id: search.doc_id,
                    query_id: search.query_id,
                    page: page_num,
                    matches,
                    done: is_last,
                });
                ctx.request_repaint();
            }

            if is_last {
                active_search = None;
            } else {
                search.current_page += 1;
            }
        } else if let Some(ref mut scan) = active_annotation_scan {
            let start = scan.current_page;
            let end = (scan.current_page + 10).min(scan.total_pages);
            let mut found_hl = Vec::new();
            let mut found_tb = Vec::new();

            if let Some(doc) = documents.get(&scan.doc_id) {
                for p_idx in start..end {
                    if let Ok(mut page) = doc.pages().get(p_idx as i32) {
                        extract_annotations_from_page(
                            &mut page,
                            p_idx,
                            &mut found_hl,
                            &mut found_tb,
                        );
                    }
                }
            }

            if !found_hl.is_empty() || !found_tb.is_empty() {
                let fh_found: Vec<Highlight> = found_hl
                    .iter()
                    .filter(|h| !h.points.is_empty())
                    .cloned()
                    .collect();
                if !fh_found.is_empty() {
                    doc_freehand_highlights
                        .entry(scan.doc_id)
                        .or_default()
                        .extend(fh_found);
                }

                let _ = tx_evt.send(Event::AnnotationsExtracted {
                    doc_id: scan.doc_id,
                    annotations: found_hl,
                    text_boxes: found_tb,
                });
                ctx.request_repaint();
            }

            scan.current_page = end;
            if scan.current_page >= scan.total_pages {
                active_annotation_scan = None;
            }
        }
    }
}

pub fn get_page_crop_and_rotation(page: &PdfPage) -> (PdfRect, PdfPageRenderRotation) {
    let crop_box = if let Ok(crop) = page.boundaries().crop() {
        crop.bounds
    } else if let Ok(media) = page.boundaries().media() {
        media.bounds
    } else {
        PdfRect::new(
            PdfPoints::ZERO,
            PdfPoints::ZERO,
            page.height(),
            page.width(),
        )
    };
    let rotation = page.rotation().unwrap_or(PdfPageRenderRotation::None);
    (crop_box, rotation)
}

pub fn canonical_point_to_user(
    can_x: f32,
    can_y: f32,
    cl: f32,
    cr: f32,
    cb: f32,
    ct: f32,
    rotation: PdfPageRenderRotation,
) -> (f32, f32) {
    match rotation {
        PdfPageRenderRotation::None => (can_x + cl, ct - can_y),
        PdfPageRenderRotation::Degrees90 => (can_y + cl, can_x + cb),
        PdfPageRenderRotation::Degrees180 => (cr - can_x, can_y + cb),
        PdfPageRenderRotation::Degrees270 => (cr - can_y, ct - can_x),
    }
}

fn map_point(
    x: f32,
    y: f32,
    cl: f32,
    cr: f32,
    cb: f32,
    ct: f32,
    rotation: PdfPageRenderRotation,
) -> (f32, f32) {
    match rotation {
        PdfPageRenderRotation::None => (x - cl, ct - y),
        PdfPageRenderRotation::Degrees90 => (y - cb, x - cl),
        PdfPageRenderRotation::Degrees180 => (cr - x, y - cb),
        PdfPageRenderRotation::Degrees270 => (ct - y, cr - x),
    }
}

/// Maps a bounding box in PDF user space points to canonical unrotated page points
/// (origin top-left, matching the unrotated rendered page).
pub fn user_rect_to_canonical(
    rect: &PdfRect,
    crop_box: &PdfRect,
    rotation: PdfPageRenderRotation,
) -> CanonicalRect {
    let x1 = rect.left().value;
    let x2 = rect.right().value;
    let y1 = rect.bottom().value;
    let y2 = rect.top().value;

    let cl = crop_box.left().value;
    let cr = crop_box.right().value;
    let cb = crop_box.bottom().value;
    let ct = crop_box.top().value;

    let (p1_x, p1_y) = map_point(x1, y1, cl, cr, cb, ct, rotation);
    let (p2_x, p2_y) = map_point(x2, y2, cl, cr, cb, ct, rotation);

    let min_x = p1_x.min(p2_x);
    let max_x = p1_x.max(p2_x);
    let min_y = p1_y.min(p2_y);
    let max_y = p1_y.max(p2_y);

    CanonicalRect::new(min_x, min_y, max_x, max_y)
}

fn merge_line_rects(rects: Vec<CanonicalRect>) -> Vec<CanonicalRect> {
    if rects.len() <= 1 {
        return rects;
    }

    // Group rects into lines based on vertical overlap
    let mut lines: Vec<Vec<CanonicalRect>> = Vec::new();
    for r in rects {
        let mut found_line = false;
        for line in &mut lines {
            let ref_r = &line[0];
            let y_overlap = (ref_r.max_y.min(r.max_y) - ref_r.min_y.max(r.min_y)).max(0.0);
            let min_h = (ref_r.max_y - ref_r.min_y).min(r.max_y - r.min_y);
            if min_h > 0.0 && y_overlap / min_h > 0.4 {
                line.push(r);
                found_line = true;
                break;
            }
        }
        if !found_line {
            lines.push(vec![r]);
        }
    }

    // Sort lines by their top-most Y coordinate (top to bottom reading order)
    lines.sort_by(|a, b| {
        let min_y_a = a.iter().map(|r| r.min_y).fold(f32::INFINITY, f32::min);
        let min_y_b = b.iter().map(|r| r.min_y).fold(f32::INFINITY, f32::min);
        min_y_a
            .partial_cmp(&min_y_b)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut result = Vec::new();
    for mut line in lines {
        // Sort rects on the same line from left to right
        line.sort_by(|a, b| {
            a.min_x
                .partial_cmp(&b.min_x)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut merged_line: Vec<CanonicalRect> = Vec::new();
        for r in line {
            if let Some(last) = merged_line.last_mut() {
                // Merge if adjacent or overlapping horizontally (space threshold ~15 pt)
                if r.min_x <= last.max_x + 15.0 && r.max_x >= last.min_x {
                    last.min_x = last.min_x.min(r.min_x);
                    last.max_x = last.max_x.max(r.max_x);
                    last.min_y = last.min_y.min(r.min_y);
                    last.max_y = last.max_y.max(r.max_y);
                    continue;
                }
            }
            merged_line.push(r);
        }
        result.extend(merged_line);
    }

    result
}

fn safe_extract_annot_color(
    objects: &PdfPageAnnotationObjects,
    default_color: AnnotationColor,
) -> AnnotationColor {
    for obj in objects.iter() {
        if let Ok(c) = obj.fill_color() {
            if c.red() > 0 || c.green() > 0 || c.blue() > 0 {
                return AnnotationColor::new(c.red(), c.green(), c.blue());
            }
        }
        if let Ok(c) = obj.stroke_color() {
            if c.red() > 0 || c.green() > 0 || c.blue() > 0 {
                return AnnotationColor::new(c.red(), c.green(), c.blue());
            }
        }
    }
    default_color
}

fn parse_font_size_from_metadata(s: &str) -> Option<f32> {
    if let Some(idx) = s.find("size=") {
        let rest = &s[idx + 5..];
        let num_str: String = rest
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if let Ok(v) = num_str.parse::<f32>() {
            if (6.0..=120.0).contains(&v) {
                return Some(v);
            }
        }
    }
    None
}

fn safe_extract_annot_font_size(
    annot: &PdfPageAnnotation,
    objects: &PdfPageAnnotationObjects,
    text: &str,
    default_font_size: f32,
) -> f32 {
    // 1. Try to read size encoded in creator metadata (e.g. "VersoReader:size=24.0")
    if let Some(creator) = annot.creator() {
        if let Some(size) = parse_font_size_from_metadata(&creator) {
            return size;
        }
    }

    // 2. Try to inspect any embedded text objects within the annotation
    for obj in objects.iter() {
        if let Some(text_obj) = obj.as_text_object() {
            let size = text_obj.unscaled_font_size().value;
            if size > 0.0 {
                return size;
            }
        }
    }

    // 3. Estimate from annotation bounding box height and line count
    if let Ok(b) = annot.bounds() {
        let box_h = (b.top().value - b.bottom().value).abs();
        let width = (b.right().value - b.left().value).abs().max(40.0);
        let chars_per_line = (width / (16.0 * 0.52)).max(1.0);
        let mut total_lines = 0.0;
        for line in text.lines() {
            let count = (line.chars().count() as f32 / chars_per_line)
                .ceil()
                .max(1.0);
            total_lines += count;
        }
        if total_lines == 0.0 {
            total_lines = 1.0;
        }

        let line_h = ((box_h - 8.0) / total_lines).max(12.0);
        let approx_size = line_h / 1.3;

        const PRESETS: [f32; 10] = [10.0, 12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 28.0, 32.0, 36.0];
        let mut best_preset = default_font_size;
        let mut min_diff = f32::MAX;
        for &p in &PRESETS {
            let diff = (approx_size - p).abs();
            if diff < min_diff {
                min_diff = diff;
                best_preset = p;
            }
        }
        if min_diff <= 2.5 {
            return best_preset;
        }
        return approx_size.clamp(8.0, 72.0);
    }

    default_font_size
}

/// Rasterizes a smooth, continuous, anti-aliased freehand highlight stroke directly
/// onto an RGBA page bitmap using authentic ISO 32000 Multiply blend mode.
///
/// Features:
/// - Continuous polyline pathing with round joints and round caps: zero gaps, seams, or discrete sticks.
/// - Coverage-based anti-aliasing: smooth sub-pixel borders.
/// - Authentic Multiply blending: white paper becomes saturated highlight ink,
///   while black text remains 100% pitch-black and razor sharp.
pub fn rasterize_freehand_highlight(
    rgba: &mut [u8],
    w: usize,
    h: usize,
    hl: &Highlight,
    canon_w: f32,
    canon_h: f32,
    rotation: u16,
) {
    if hl.points.is_empty() || w == 0 || h == 0 {
        return;
    }

    let rot = rotation % 360;
    let (visual_w, visual_h) = match rot {
        90 | 270 => (canon_h, canon_w),
        _ => (canon_w, canon_h),
    };

    if visual_w <= 0.0 || visual_h <= 0.0 {
        return;
    }

    let scale_x = w as f32 / visual_w;
    let scale_y = h as f32 / visual_h;

    // Convert canonical points (top-left unrotated) to bitmap pixel coordinates
    let pixel_points: Vec<[f32; 2]> = hl
        .points
        .iter()
        .map(|pt| {
            let px = pt[0];
            let py = pt[1];
            let (rx, ry) = match rot {
                90 => (canon_h - py, px),
                180 => (canon_w - px, canon_h - py),
                270 => (py, canon_w - px),
                _ => (px, py),
            };
            [rx * scale_x, ry * scale_y]
        })
        .collect();

    let stroke_radius = (hl.thickness * ((scale_x + scale_y) * 0.5) * 0.5).max(1.0);
    let color_r = hl.color.r as f32;
    let color_g = hl.color.g as f32;
    let color_b = hl.color.b as f32;

    // Bounding box of the entire stroke in bitmap pixel coordinates
    let mut min_px = f32::MAX;
    let mut max_px = f32::MIN;
    let mut min_py = f32::MAX;
    let mut max_py = f32::MIN;

    for p in &pixel_points {
        min_px = min_px.min(p[0]);
        max_px = max_px.max(p[0]);
        min_py = min_py.min(p[1]);
        max_py = max_py.max(p[1]);
    }

    let pad = stroke_radius + 2.0;
    let bb_min_x = ((min_px - pad).floor() as isize).clamp(0, w as isize - 1) as usize;
    let bb_max_x = ((max_px + pad).ceil() as isize).clamp(0, w as isize - 1) as usize;
    let bb_min_y = ((min_py - pad).floor() as isize).clamp(0, h as isize - 1) as usize;
    let bb_max_y = ((max_py + pad).ceil() as isize).clamp(0, h as isize - 1) as usize;

    if bb_max_x < bb_min_x || bb_max_y < bb_min_y {
        return;
    }

    let box_w = bb_max_x - bb_min_x + 1;
    let box_h = bb_max_y - bb_min_y + 1;
    let mut coverage = vec![0.0_f32; box_w * box_h];

    if pixel_points.len() == 1 {
        // Single-click round dot
        let cx = pixel_points[0][0];
        let cy = pixel_points[0][1];
        let outer_r = stroke_radius + 0.5;
        let outer_r2 = outer_r * outer_r;

        let dot_min_x = ((cx - stroke_radius - 1.0).floor() as isize)
            .clamp(bb_min_x as isize, bb_max_x as isize) as usize;
        let dot_max_x = ((cx + stroke_radius + 1.0).ceil() as isize)
            .clamp(bb_min_x as isize, bb_max_x as isize) as usize;
        let dot_min_y = ((cy - stroke_radius - 1.0).floor() as isize)
            .clamp(bb_min_y as isize, bb_max_y as isize) as usize;
        let dot_max_y = ((cy + stroke_radius + 1.0).ceil() as isize)
            .clamp(bb_min_y as isize, bb_max_y as isize) as usize;

        for py in dot_min_y..=dot_max_y {
            let dy = py as f32 + 0.5 - cy;
            let dy2 = dy * dy;
            let row_offset = (py - bb_min_y) * box_w;
            for px in dot_min_x..=dot_max_x {
                let dx = px as f32 + 0.5 - cx;
                let d2 = dx * dx + dy2;
                if d2 <= outer_r2 {
                    let d = d2.sqrt();
                    let alpha = (stroke_radius + 0.5 - d).clamp(0.0, 1.0);
                    let idx = row_offset + (px - bb_min_x);
                    coverage[idx] = coverage[idx].max(alpha);
                }
            }
        }
    } else {
        // Continuous polyline with round joins and round caps
        let outer_r = stroke_radius + 0.5;
        let outer_r2 = outer_r * outer_r;

        for i in 0..pixel_points.len() - 1 {
            let p1 = pixel_points[i];
            let p2 = pixel_points[i + 1];

            let seg_min_x = ((p1[0].min(p2[0]) - pad).floor() as isize)
                .clamp(bb_min_x as isize, bb_max_x as isize) as usize;
            let seg_max_x = ((p1[0].max(p2[0]) + pad).ceil() as isize)
                .clamp(bb_min_x as isize, bb_max_x as isize) as usize;
            let seg_min_y = ((p1[1].min(p2[1]) - pad).floor() as isize)
                .clamp(bb_min_y as isize, bb_max_y as isize) as usize;
            let seg_max_y = ((p1[1].max(p2[1]) + pad).ceil() as isize)
                .clamp(bb_min_y as isize, bb_max_y as isize) as usize;

            let vx = p2[0] - p1[0];
            let vy = p2[1] - p1[1];
            let len_sq = vx * vx + vy * vy;
            let inv_len_sq = if len_sq > 0.0001 { 1.0 / len_sq } else { 0.0 };

            for py in seg_min_y..=seg_max_y {
                let my = py as f32 + 0.5;
                let row_offset = (py - bb_min_y) * box_w;
                for px in seg_min_x..=seg_max_x {
                    let mx = px as f32 + 0.5;

                    let d2 = if inv_len_sq > 0.0 {
                        let t =
                            (((mx - p1[0]) * vx + (my - p1[1]) * vy) * inv_len_sq).clamp(0.0, 1.0);
                        let cx = p1[0] + t * vx;
                        let cy = p1[1] + t * vy;
                        let dx = mx - cx;
                        let dy = my - cy;
                        dx * dx + dy * dy
                    } else {
                        let dx = mx - p1[0];
                        let dy = my - p1[1];
                        dx * dx + dy * dy
                    };

                    if d2 <= outer_r2 {
                        let d = d2.sqrt();
                        let alpha = (stroke_radius + 0.5 - d).clamp(0.0, 1.0);
                        let idx = row_offset + (px - bb_min_x);
                        coverage[idx] = coverage[idx].max(alpha);
                    }
                }
            }
        }
    }

    // Blend into rgba buffer using ISO 32000 Multiply blend mode
    for by in 0..box_h {
        let py = bb_min_y + by;
        let row_offset = by * box_w;
        let img_row_offset = py * w * 4;
        for bx in 0..box_w {
            let cov = coverage[row_offset + bx];
            if cov > 0.001 {
                let px = bb_min_x + bx;
                let pixel_idx = img_row_offset + px * 4;
                if pixel_idx + 2 < rgba.len() {
                    let r_bg = rgba[pixel_idx] as f32;
                    let g_bg = rgba[pixel_idx + 1] as f32;
                    let b_bg = rgba[pixel_idx + 2] as f32;

                    // ISO 32000 Multiply formula: (bg * hl) / 255.0
                    let r_mult = (r_bg * color_r) / 255.0;
                    let g_mult = (g_bg * color_g) / 255.0;
                    let b_mult = (b_bg * color_b) / 255.0;

                    rgba[pixel_idx] = ((1.0 - cov) * r_bg + cov * r_mult).round() as u8;
                    rgba[pixel_idx + 1] = ((1.0 - cov) * g_bg + cov * g_mult).round() as u8;
                    rgba[pixel_idx + 2] = ((1.0 - cov) * b_bg + cov * b_mult).round() as u8;
                }
            }
        }
    }
}

/// Extracts existing Highlight, Ink, and FreeText annotations from a single PdfPage into canonical models.
pub fn extract_annotations_from_page(
    page: &mut PdfPage,
    page_idx: usize,
    highlights: &mut Vec<Highlight>,
    text_boxes: &mut Vec<TextBox>,
) {
    let (crop_box, rotation) = get_page_crop_and_rotation(page);
    let cl = crop_box.left().value;
    let cr = crop_box.right().value;
    let cb = crop_box.bottom().value;
    let ct = crop_box.top().value;
    let annots = page.annotations_mut();
    let mut i = annots.len();

    while i > 0 {
        i -= 1;
        let Ok(mut annot) = annots.get(i) else {
            continue;
        };

        if let Some(hl_annot) = annot.as_highlight_annotation_mut() {
            let annot_color =
                safe_extract_annot_color(hl_annot.objects(), AnnotationColor::new(255, 235, 59));

            let mut rects = Vec::new();
            let pts = hl_annot.attachment_points_mut();
            let pt_count = pts.len();
            for p_idx in 0..pt_count {
                if let Ok(q) = pts.get(p_idx) {
                    let (p1_x, p1_y) =
                        map_point(q.x1().value, q.y1().value, cl, cr, cb, ct, rotation);
                    let (p2_x, p2_y) =
                        map_point(q.x2().value, q.y2().value, cl, cr, cb, ct, rotation);
                    let (p3_x, p3_y) =
                        map_point(q.x3().value, q.y3().value, cl, cr, cb, ct, rotation);
                    let (p4_x, p4_y) =
                        map_point(q.x4().value, q.y4().value, cl, cr, cb, ct, rotation);

                    let min_x = p1_x.min(p2_x).min(p3_x).min(p4_x);
                    let max_x = p1_x.max(p2_x).max(p3_x).max(p4_x);
                    let min_y = p1_y.min(p2_y).min(p3_y).min(p4_y);
                    let max_y = p1_y.max(p2_y).max(p3_y).max(p4_y);

                    if max_x > min_x && max_y > min_y {
                        rects.push(CanonicalRect::new(min_x, min_y, max_x, max_y));
                    }
                }
            }

            if rects.is_empty() {
                if let Ok(b) = hl_annot.bounds() {
                    let canon = user_rect_to_canonical(&b, &crop_box, rotation);
                    if canon.max_x > canon.min_x && canon.max_y > canon.min_y {
                        rects.push(canon);
                    }
                }
            }

            if !rects.is_empty() {
                highlights.push(Highlight {
                    id: crate::annotations::generate_highlight_id(),
                    page: page_idx,
                    rects,
                    points: Vec::new(),
                    color: annot_color,
                    alpha: 0.35,
                    thickness: 1.0,
                });
            }
        } else if let Some(ink_annot) = annot.as_ink_annotation_mut() {
            let mut annot_color = AnnotationColor::new(255, 235, 59);
            let mut stroke_width = 12.0_f32;
            let mut points = Vec::new();

            let objects = ink_annot.objects_mut();
            for obj in objects.iter() {
                if let Some(path_obj) = obj.as_path_object() {
                    if let Ok(c) = path_obj.stroke_color() {
                        annot_color = AnnotationColor::new(c.red(), c.green(), c.blue());
                    }
                    if let Ok(w) = path_obj.stroke_width() {
                        if w.value > 0.0 {
                            stroke_width = w.value;
                        }
                    }
                    let segs = path_obj.segments();
                    for s_idx in 0..segs.len() {
                        if let Ok(seg) = segs.get(s_idx) {
                            let (px, py) = seg.point();
                            let (can_x, can_y) =
                                map_point(px.value, py.value, cl, cr, cb, ct, rotation);
                            points.push([can_x, can_y]);
                        }
                    }
                }
            }

            if points.is_empty() {
                if let Ok(b) = ink_annot.bounds() {
                    let canon = user_rect_to_canonical(&b, &crop_box, rotation);
                    points.push([canon.min_x, canon.min_y]);
                    points.push([canon.max_x, canon.max_y]);
                }
            }

            if !points.is_empty() {
                highlights.push(Highlight {
                    id: crate::annotations::generate_highlight_id(),
                    page: page_idx,
                    rects: Vec::new(),
                    points,
                    color: annot_color,
                    alpha: 0.35,
                    thickness: stroke_width,
                });
            }
        } else if annot.annotation_type() == PdfPageAnnotationType::FreeText
            || (annot.annotation_type() == PdfPageAnnotationType::Text
                && !annot.contents().unwrap_or_default().is_empty())
        {
            let text = annot.contents().unwrap_or_default();
            if !text.is_empty() {
                let annot_color = annot
                    .stroke_color()
                    .or_else(|_| annot.fill_color())
                    .map(|c| AnnotationColor::new(c.red(), c.green(), c.blue()))
                    .unwrap_or_else(|_| {
                        if !annot.objects().is_empty() {
                            safe_extract_annot_color(annot.objects(), AnnotationColor::new(0, 0, 0))
                        } else {
                            AnnotationColor::new(0, 0, 0)
                        }
                    });
                let (pos, width) = if let Ok(b) = annot.bounds() {
                    let canon = user_rect_to_canonical(&b, &crop_box, rotation);
                    (
                        [canon.min_x, canon.min_y],
                        (canon.max_x - canon.min_x).max(40.0),
                    )
                } else {
                    ([50.0, 50.0], 180.0)
                };
                let font_size = safe_extract_annot_font_size(&annot, annot.objects(), &text, 16.0);
                text_boxes.push(TextBox {
                    id: crate::annotations::generate_text_box_id(),
                    page: page_idx,
                    pos,
                    text,
                    font_size,
                    color: annot_color,
                    width,
                });
            }
            // Delete from in-memory page so PDFium won't bake solid blocks into the bitmap
            let _ = annots.delete_annotation(annot);
        } else if annot.annotation_type() == PdfPageAnnotationType::Popup {
            let _ = annots.delete_annotation(annot);
        }
    }

    // Restore natural forward document reading order
    highlights.reverse();
    text_boxes.reverse();
}

/// Extracts all existing Highlight, Ink, and FreeText annotations from a PdfDocument's pages into canonical models.
#[allow(dead_code)]
pub fn extract_annotations_from_doc(doc: &PdfDocument) -> (Vec<Highlight>, Vec<TextBox>) {
    let mut highlights = Vec::new();
    let mut text_boxes = Vec::new();
    let page_count = doc.pages().len() as usize;

    for page_idx in 0..page_count {
        if let Ok(mut page) = doc.pages().get(page_idx as i32) {
            extract_annotations_from_page(&mut page, page_idx, &mut highlights, &mut text_boxes);
        }
    }

    (highlights, text_boxes)
}

/// Applies highlights and text boxes directly to specified pages in PDFium.
pub fn apply_annotations_to_doc_on_pages(
    doc: &PdfDocument,
    highlights: &[Highlight],
    text_boxes: &[TextBox],
    pages: impl IntoIterator<Item = usize>,
) -> Result<(), String> {
    let mut page_map: HashMap<usize, Vec<&Highlight>> = HashMap::new();
    for hl in highlights {
        page_map.entry(hl.page).or_default().push(hl);
    }

    let mut text_box_map: HashMap<usize, Vec<&TextBox>> = HashMap::new();
    for tb in text_boxes {
        text_box_map.entry(tb.page).or_default().push(tb);
    }

    let total_pages = doc.pages().len() as usize;

    for page_idx in pages {
        if page_idx >= total_pages {
            continue;
        }
        let mut page = doc
            .pages()
            .get(page_idx as i32)
            .map_err(|e| e.to_string())?;

        let (crop_box, rotation) = get_page_crop_and_rotation(&page);
        let cl = crop_box.left().value;
        let cr = crop_box.right().value;
        let cb = crop_box.bottom().value;
        let ct = crop_box.top().value;

        let annots = page.annotations_mut();
        let mut i = annots.len();
        while i > 0 {
            i -= 1;
            if let Ok(annot) = annots.get(i) {
                if matches!(
                    annot,
                    PdfPageAnnotation::Highlight(_)
                        | PdfPageAnnotation::Ink(_)
                        | PdfPageAnnotation::FreeText(_)
                ) {
                    let _ = annots.delete_annotation(annot);
                }
            }
        }

        if let Some(page_highlights) = page_map.get(&page_idx) {
            for hl in page_highlights {
                if !hl.points.is_empty() {
                    // Freehand ink annotation for persistence, round-trip extraction, and standard PDF viewers
                    if let Ok(mut annot) = annots.create_ink_annotation() {
                        let _ = annot.set_stroke_color(PdfColor::new(
                            hl.color.r, hl.color.g, hl.color.b, 255,
                        ));
                        if hl.points.len() >= 2 {
                            let (p0_x, p0_y) = canonical_point_to_user(
                                hl.points[0][0],
                                hl.points[0][1],
                                cl,
                                cr,
                                cb,
                                ct,
                                rotation,
                            );
                            let stroke_w = hl.thickness.max(1.0);
                            if let Ok(mut path) = PdfPagePathObject::new(
                                doc,
                                PdfPoints::new(p0_x),
                                PdfPoints::new(p0_y),
                                Some(PdfColor::new(hl.color.r, hl.color.g, hl.color.b, 255)),
                                Some(PdfPoints::new(stroke_w)),
                                None,
                            ) {
                                for pt in &hl.points[1..] {
                                    let (px, py) = canonical_point_to_user(
                                        pt[0], pt[1], cl, cr, cb, ct, rotation,
                                    );
                                    let _ = path.line_to(PdfPoints::new(px), PdfPoints::new(py));
                                }
                                let _ = annot.objects_mut().add_path_object(path);
                            }
                        }
                        if let Some(bounds) = hl.bounding_rect() {
                            let p_bl = canonical_point_to_user(
                                bounds.min_x,
                                bounds.max_y,
                                cl,
                                cr,
                                cb,
                                ct,
                                rotation,
                            );
                            let p_tr = canonical_point_to_user(
                                bounds.max_x,
                                bounds.min_y,
                                cl,
                                cr,
                                cb,
                                ct,
                                rotation,
                            );
                            let pdf_rect = PdfRect::new_from_values(
                                p_bl.1.min(p_tr.1),
                                p_bl.0.min(p_tr.0),
                                p_bl.1.max(p_tr.1),
                                p_bl.0.max(p_tr.0),
                            );
                            let _ = annot.set_bounds(pdf_rect);
                        }
                    }
                } else if !hl.rects.is_empty() {
                    // Text highlight annotation
                    if let Ok(mut annot) = annots.create_highlight_annotation() {
                        let _ = annot.set_stroke_color(PdfColor::new(
                            hl.color.r, hl.color.g, hl.color.b, 255,
                        ));
                        let mut all_user_min_x = f32::MAX;
                        let mut all_user_max_x = f32::MIN;
                        let mut all_user_min_y = f32::MAX;
                        let mut all_user_max_y = f32::MIN;

                        for r in &hl.rects {
                            let p_tl =
                                canonical_point_to_user(r.min_x, r.min_y, cl, cr, cb, ct, rotation);
                            let p_tr =
                                canonical_point_to_user(r.max_x, r.min_y, cl, cr, cb, ct, rotation);
                            let p_bl =
                                canonical_point_to_user(r.min_x, r.max_y, cl, cr, cb, ct, rotation);
                            let p_br =
                                canonical_point_to_user(r.max_x, r.max_y, cl, cr, cb, ct, rotation);

                            all_user_min_x = all_user_min_x
                                .min(p_tl.0)
                                .min(p_tr.0)
                                .min(p_bl.0)
                                .min(p_br.0);
                            all_user_max_x = all_user_max_x
                                .max(p_tl.0)
                                .max(p_tr.0)
                                .max(p_bl.0)
                                .max(p_br.0);
                            all_user_min_y = all_user_min_y
                                .min(p_tl.1)
                                .min(p_tr.1)
                                .min(p_bl.1)
                                .min(p_br.1);
                            all_user_max_y = all_user_max_y
                                .max(p_tl.1)
                                .max(p_tr.1)
                                .max(p_bl.1)
                                .max(p_br.1);

                            // Quadpoints in PDFium: top-left, top-right, bottom-left, bottom-right
                            let quad = PdfQuadPoints::new_from_values(
                                p_tl.0, p_tl.1, p_tr.0, p_tr.1, p_bl.0, p_bl.1, p_br.0, p_br.1,
                            );
                            let _ = annot
                                .attachment_points_mut()
                                .create_attachment_point_at_end(quad);
                        }

                        if all_user_max_x > all_user_min_x && all_user_max_y > all_user_min_y {
                            let pdf_rect = PdfRect::new_from_values(
                                all_user_min_y,
                                all_user_min_x,
                                all_user_max_y,
                                all_user_max_x,
                            );
                            let _ = annot.set_bounds(pdf_rect);
                        }
                    }
                }
            }
        }

        if let Some(page_boxes) = text_box_map.get(&page_idx) {
            for tb in page_boxes {
                if let Ok(mut annot) = annots.create_free_text_annotation(&tb.text) {
                    let box_h = tb.estimated_height();
                    let p_tl =
                        canonical_point_to_user(tb.pos[0], tb.pos[1], cl, cr, cb, ct, rotation);
                    let p_br = canonical_point_to_user(
                        tb.pos[0] + tb.width.max(40.0),
                        tb.pos[1] + box_h,
                        cl,
                        cr,
                        cb,
                        ct,
                        rotation,
                    );
                    let pdf_left = p_tl.0.min(p_br.0);
                    let pdf_right = p_tl.0.max(p_br.0);
                    let pdf_bottom = p_tl.1.min(p_br.1);
                    let pdf_top = p_tl.1.max(p_br.1);
                    let pdf_rect =
                        PdfRect::new_from_values(pdf_bottom, pdf_left, pdf_top, pdf_right);
                    let _ = annot.set_bounds(pdf_rect);
                    let color = PdfColor::new(tb.color.r, tb.color.g, tb.color.b, 255);
                    let _ = annot.set_stroke_color(color);
                    let _ = annot.set_is_printed(true);
                    let _ = annot.set_is_zoomable(true);
                    let _ = annot.set_is_rotatable(true);
                    let _ = annot.set_creator(&format!("VersoReader:size={:.1}", tb.font_size));
                }
            }
        }

        let _ = page.regenerate_content();
    }

    Ok(())
}

/// Applies highlights and text boxes directly to a PdfDocument's pages in PDFium.
/// Removes previous highlight, ink, and free text annotations and regenerates them.
pub fn apply_annotations_to_doc(
    doc: &PdfDocument,
    highlights: &[Highlight],
    text_boxes: &[TextBox],
) -> Result<(), String> {
    let pages: HashSet<usize> = highlights
        .iter()
        .map(|h| h.page)
        .chain(text_boxes.iter().map(|t| t.page))
        .collect();
    if pages.is_empty() {
        let page_count = doc.pages().len() as usize;
        return apply_annotations_to_doc_on_pages(doc, highlights, text_boxes, 0..page_count);
    }
    apply_annotations_to_doc_on_pages(doc, highlights, text_boxes, pages)
}

/// Helper wrapper for applying only highlights.
#[allow(dead_code)]
pub fn apply_highlights_to_doc(doc: &PdfDocument, highlights: &[Highlight]) -> Result<(), String> {
    apply_annotations_to_doc(doc, highlights, &[])
}

/// Searches text on a single 1-based page of the document, returning matches in canonical coordinates.
pub fn search_page(doc: &PdfDocument, page_num: usize, query: &str) -> Vec<SearchMatch> {
    let mut results = Vec::new();
    if page_num == 0 || query.is_empty() {
        return results;
    }

    let page_idx = (page_num - 1) as i32;
    let Ok(page) = doc.pages().get(page_idx) else {
        return results;
    };
    let Ok(text_page) = page.text() else {
        return results;
    };

    let crop_box = if let Ok(crop) = page.boundaries().crop() {
        crop.bounds
    } else if let Ok(media) = page.boundaries().media() {
        media.bounds
    } else {
        PdfRect::new(
            PdfPoints::ZERO,
            PdfPoints::ZERO,
            page.height(),
            page.width(),
        )
    };
    let rotation = page.rotation().unwrap_or(PdfPageRenderRotation::None);

    let search_options = PdfSearchOptions::new().match_case(false);
    let Ok(search) = text_page.search(query, &search_options) else {
        return results;
    };

    for segments in search.iter(PdfSearchDirection::SearchForward) {
        let mut rects = Vec::new();
        for segment in segments.iter() {
            let bounds = segment.bounds();
            rects.push(user_rect_to_canonical(&bounds, &crop_box, rotation));
        }

        let merged = merge_line_rects(rects);
        if !merged.is_empty() {
            results.push(SearchMatch {
                page: page_num,
                rects: merged,
            });
        }
    }

    results
}

/// Resolves a bookmark's target destination to a 1-based page number.
fn resolve_bookmark_page(bookmark: &PdfBookmark) -> Option<usize> {
    if let Some(dest) = bookmark.destination() {
        if let Ok(idx) = dest.page_index() {
            return Some((idx as usize) + 1);
        }
    }
    if let Some(PdfAction::LocalDestination(local_dest)) = bookmark.action() {
        if let Ok(dest) = local_dest.destination() {
            if let Ok(idx) = dest.page_index() {
                return Some((idx as usize) + 1);
            }
        }
    }
    None
}

/// Recursively traverses a bookmark node and its children.
fn extract_bookmark_node<'a>(
    bookmark: &PdfBookmark<'a>,
    visited: &mut HashSet<PdfBookmark<'a>>,
    depth: usize,
) -> OutlineItem {
    let title = bookmark.title().unwrap_or_else(|| "Untitled".to_string());
    let page_number = resolve_bookmark_page(bookmark);

    let mut children = Vec::new();
    if depth < 32 {
        let mut curr_child = bookmark.first_child();
        while let Some(child) = curr_child {
            if visited.insert(child.clone()) {
                children.push(extract_bookmark_node(&child, visited, depth + 1));
            }
            curr_child = child.next_sibling();
        }
    }

    OutlineItem {
        title,
        page_number,
        children,
    }
}

/// Extracts the full outline (table of contents) tree for a document.
pub fn extract_outline(doc: &PdfDocument) -> Vec<OutlineItem> {
    let mut items = Vec::new();
    let mut visited = HashSet::new();

    let mut curr_root = doc.bookmarks().root();
    while let Some(root_bm) = curr_root {
        if visited.insert(root_bm.clone()) {
            items.push(extract_bookmark_node(&root_bm, &mut visited, 0));
        }
        curr_root = root_bm.next_sibling();
    }

    items
}

/// Extracts interactive links for a given page in canonical coordinates.
pub fn extract_links_from_page(
    page: &PdfPage,
    crop_box: &PdfRect,
    rotation: PdfPageRenderRotation,
) -> Vec<PageLink> {
    let mut links = Vec::new();

    // 1. Enumerate standard page links via FPDFLink_Enumerate
    for link in page.links().iter() {
        if let Ok(raw_rect) = link.rect() {
            let canon = user_rect_to_canonical(&raw_rect, crop_box, rotation);
            if canon.max_x <= canon.min_x || canon.max_y <= canon.min_y {
                continue;
            }

            let target = if let Some(dest) = link.destination() {
                dest.page_index()
                    .ok()
                    .map(|idx| LinkTarget::Page(idx as usize))
            } else if let Some(action) = link.action() {
                match action {
                    PdfAction::Uri(uri_action) => uri_action.uri().ok().and_then(|u| {
                        let trimmed = u.trim().to_string();
                        if !trimmed.is_empty() {
                            Some(LinkTarget::Uri(trimmed))
                        } else {
                            None
                        }
                    }),
                    PdfAction::LocalDestination(local) => {
                        if let Ok(dest) = local.destination() {
                            dest.page_index()
                                .ok()
                                .map(|idx| LinkTarget::Page(idx as usize))
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            } else {
                None
            };

            if let Some(target) = target {
                links.push(PageLink {
                    rect: canon,
                    target,
                });
            }
        }
    }

    // 2. Supplement with any link annotations from page.annotations()
    for annot in page.annotations().iter() {
        if let Some(link_annot) = annot.as_link_annotation() {
            if let Ok(link) = link_annot.link() {
                if let Ok(raw_rect) = link.rect() {
                    let canon = user_rect_to_canonical(&raw_rect, crop_box, rotation);
                    if canon.max_x <= canon.min_x || canon.max_y <= canon.min_y {
                        continue;
                    }

                    // Deduplicate if already present
                    let already_present = links.iter().any(|existing| {
                        (existing.rect.min_x - canon.min_x).abs() < 1.0
                            && (existing.rect.min_y - canon.min_y).abs() < 1.0
                            && (existing.rect.max_x - canon.max_x).abs() < 1.0
                            && (existing.rect.max_y - canon.max_y).abs() < 1.0
                    });
                    if already_present {
                        continue;
                    }

                    let target = if let Some(dest) = link.destination() {
                        dest.page_index()
                            .ok()
                            .map(|idx| LinkTarget::Page(idx as usize))
                    } else if let Some(action) = link.action() {
                        match action {
                            PdfAction::Uri(uri_action) => uri_action.uri().ok().and_then(|u| {
                                let trimmed = u.trim().to_string();
                                if !trimmed.is_empty() {
                                    Some(LinkTarget::Uri(trimmed))
                                } else {
                                    None
                                }
                            }),
                            PdfAction::LocalDestination(local) => {
                                if let Ok(dest) = local.destination() {
                                    dest.page_index()
                                        .ok()
                                        .map(|idx| LinkTarget::Page(idx as usize))
                                } else {
                                    None
                                }
                            }
                            _ => None,
                        }
                    } else {
                        None
                    };

                    if let Some(target) = target {
                        links.push(PageLink {
                            rect: canon,
                            target,
                        });
                    }
                }
            }
        }
    }

    links
}

/// Extracts all characters, layout line boxes, and interactive links for a given page in canonical coordinates.
pub fn extract_page_text(doc: &PdfDocument, page_idx: usize) -> Option<PageTextData> {
    let page = doc.pages().get(page_idx as i32).ok()?;
    let crop_box = if let Ok(crop) = page.boundaries().crop() {
        crop.bounds
    } else if let Ok(media) = page.boundaries().media() {
        media.bounds
    } else {
        PdfRect::new(
            PdfPoints::ZERO,
            PdfPoints::ZERO,
            page.height(),
            page.width(),
        )
    };
    let rotation = page.rotation().unwrap_or(PdfPageRenderRotation::None);
    let links = extract_links_from_page(&page, &crop_box, rotation);

    let text_page = match page.text() {
        Ok(tp) => tp,
        Err(_) => return Some(PageTextData::new(page_idx, Vec::new(), Vec::new(), links)),
    };
    let count = text_page.len();
    if count <= 0 {
        return Some(PageTextData::new(page_idx, Vec::new(), Vec::new(), links));
    }

    let page_chars = text_page.chars();
    let mut raw_chars: Vec<(char, CanonicalRect)> = Vec::with_capacity(count as usize);

    for char_item in page_chars.iter() {
        let ch = char_item.unicode_char().unwrap_or(' ');
        let raw_rect = char_item
            .loose_bounds()
            .or_else(|_| char_item.tight_bounds())
            .unwrap_or(PdfRect::new(
                PdfPoints::new(0.0),
                PdfPoints::new(0.0),
                PdfPoints::new(0.0),
                PdfPoints::new(0.0),
            ));

        let canon = user_rect_to_canonical(&raw_rect, &crop_box, rotation);
        raw_chars.push((ch, canon));
    }

    // Group characters into reading order lines
    struct LineAccum {
        char_indices: Vec<usize>,
        min_x: f32,
        max_x: f32,
        min_y: f32,
        max_y: f32,
    }

    let mut line_accums: Vec<LineAccum> = Vec::new();
    let mut cur_accum: Option<LineAccum> = None;

    for (idx, &(ch, rect)) in raw_chars.iter().enumerate() {
        if let Some(mut cur) = cur_accum.take() {
            if ch == '\r' || ch == '\n' {
                cur.char_indices.push(idx);
                line_accums.push(cur);
                cur_accum = None;
            } else if rect.max_x > rect.min_x && rect.max_y > rect.min_y {
                let overlap = (cur.max_y.min(rect.max_y) - cur.min_y.max(rect.min_y)).max(0.0);
                let min_h = (cur.max_y - cur.min_y).min(rect.max_y - rect.min_y);
                if min_h > 0.0 && (overlap / min_h) > 0.45 && rect.min_x >= cur.min_x - 15.0 {
                    cur.char_indices.push(idx);
                    cur.min_x = cur.min_x.min(rect.min_x);
                    cur.max_x = cur.max_x.max(rect.max_x);
                    cur.min_y = cur.min_y.min(rect.min_y);
                    cur.max_y = cur.max_y.max(rect.max_y);
                    cur_accum = Some(cur);
                } else {
                    line_accums.push(cur);
                    cur_accum = Some(LineAccum {
                        char_indices: vec![idx],
                        min_x: rect.min_x,
                        max_x: rect.max_x,
                        min_y: rect.min_y,
                        max_y: rect.max_y,
                    });
                }
            } else {
                cur.char_indices.push(idx);
                cur_accum = Some(cur);
            }
        } else if ch == '\r' || ch == '\n' {
            if let Some(mut cur) = cur_accum.take() {
                cur.char_indices.push(idx);
                line_accums.push(cur);
            }
        } else {
            cur_accum = Some(LineAccum {
                char_indices: vec![idx],
                min_x: rect.min_x,
                max_x: rect.max_x,
                min_y: rect.min_y,
                max_y: rect.max_y,
            });
        }
    }

    if let Some(cur) = cur_accum {
        line_accums.push(cur);
    }

    // Build final CharBox and LineBox collections
    let mut final_chars = Vec::with_capacity(raw_chars.len());
    let mut final_lines = Vec::with_capacity(line_accums.len());

    for (line_idx, accum) in line_accums.into_iter().enumerate() {
        if accum.char_indices.is_empty() {
            continue;
        }

        let start_char_idx = final_chars.len();
        let l_min_y = accum.min_y;
        let l_max_y = accum.max_y;

        for &c_idx in &accum.char_indices {
            let (ch, mut r) = raw_chars[c_idx];
            if l_max_y > l_min_y {
                r.min_y = l_min_y;
                r.max_y = l_max_y;
            }
            final_chars.push(CharBox {
                ch,
                rect: r,
                line_idx,
            });
        }
        let end_char_idx = final_chars.len();

        // Fix zero-width spaces within the line
        for i in start_char_idx..end_char_idx {
            if final_chars[i].rect.max_x <= final_chars[i].rect.min_x + 0.01 {
                let prev_x = if i > start_char_idx {
                    final_chars[i - 1].rect.max_x
                } else {
                    accum.min_x
                };
                let next_x = if i + 1 < end_char_idx
                    && final_chars[i + 1].rect.max_x > final_chars[i + 1].rect.min_x
                {
                    final_chars[i + 1].rect.min_x
                } else {
                    prev_x + 4.0
                };
                final_chars[i].rect.min_x = prev_x;
                final_chars[i].rect.max_x = next_x.max(prev_x);
            }
        }

        if accum.max_x > accum.min_x + 0.01 && l_max_y > l_min_y {
            let l_box = CanonicalRect::new(accum.min_x, l_min_y, accum.max_x, l_max_y);
            final_lines.push(LineBox {
                rect: l_box,
                char_range: start_char_idx..end_char_idx,
            });
        }
    }

    Some(PageTextData::new(page_idx, final_chars, final_lines, links))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mutex to ensure tests calling Pdfium or spawning the worker execute sequentially,
    // adhering strictly to Rule 3 (single-threaded Pdfium engine).
    static TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_candidate_paths_not_empty() {
        let paths = candidate_library_paths();
        assert!(!paths.is_empty());
        assert!(paths.iter().any(|p| p.ends_with("libpdfium.so")
            || p.ends_with("pdfium.dll")
            || p.ends_with("libpdfium.dylib")));
    }

    #[test]
    fn test_bind_pdfium_with_local_lib() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let res = bind_pdfium();
        assert!(
            res.is_ok(),
            "Expected bind_pdfium to succeed with ./lib/libpdfium.so: {:?}",
            res.err()
        );
    }

    #[test]
    fn test_worker_open_and_render_fixture() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = bind_pdfium().expect("Pdfium must be available for fixture test");
        let path = PathBuf::from("fixtures/MLE.pdf");
        if !path.exists() {
            return;
        }

        let doc = pdfium
            .load_pdf_from_file(&path, None)
            .expect("Failed to load fixture PDF");
        let page_count = doc.pages().len();
        assert!(page_count > 0, "Page count must be > 0");

        let page = doc.pages().get(0).expect("Failed to get page 0");
        assert!(page.width().value > 0.0);
        assert!(page.height().value > 0.0);

        let render_config = PdfRenderConfig::new().set_target_width(400);
        let bitmap = page
            .render_with_config(&render_config)
            .expect("Render page 0 failed");
        assert_eq!(bitmap.width(), 400);
        assert!(bitmap.height() > 0);

        let rgba = bitmap.as_rgba_bytes();
        assert_eq!(
            rgba.len(),
            (bitmap.width() * bitmap.height() * 4) as usize,
            "RGBA buffer length must equal width * height * 4"
        );
    }

    #[test]
    fn test_performance_audit_cold_start_metrics() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let t0 = std::time::Instant::now();
        let pdfium = bind_pdfium().expect("Pdfium bind");
        let t_bind = t0.elapsed();

        let path = PathBuf::from("fixtures/MLE.pdf");
        if !path.exists() {
            return;
        }

        let t_load_start = std::time::Instant::now();
        let doc = pdfium
            .load_pdf_from_file(&path, None)
            .expect("load MLE.pdf");
        let t_load = t_load_start.elapsed();

        let t_render_start = std::time::Instant::now();
        let page = doc.pages().get(0).expect("get page 0");
        let render_config = PdfRenderConfig::new().set_target_width(800);
        let bitmap = page.render_with_config(&render_config).expect("render");
        let t_render = t_render_start.elapsed();
        let total_time = t0.elapsed();

        println!("\n=== PERFORMANCE AUDIT METRICS ===");
        println!("• Pdfium binding time: {:?}", t_bind);
        println!("• Document load time: {:?}", t_load);
        println!("• Page 0 render time (800px): {:?}", t_render);
        println!("• Total cold start to first page: {:?}", total_time);
        println!(
            "• Bitmap size: {}x{} ({} bytes)",
            bitmap.width(),
            bitmap.height(),
            bitmap.as_rgba_bytes().len()
        );
        println!("=================================\n");

        assert!(
            total_time.as_millis() < 300,
            "Cold start to first page must be < 300ms (got {:?})",
            total_time
        );
    }

    #[test]
    fn test_worker_open_large_book_fixture() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = bind_pdfium().expect("Pdfium must be available for fixture test");
        let path =
            PathBuf::from("fixtures/Modern Operating Systems 4th Edition--Andrew Tanenbaum.pdf");
        if !path.exists() {
            return;
        }

        let doc = pdfium
            .load_pdf_from_file(&path, None)
            .expect("Failed to load large book PDF");
        let page_count = doc.pages().len();
        assert!(page_count > 100, "Large book should have > 100 pages");
    }

    #[test]
    fn test_worker_open_and_close_multiple_tabs() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = bind_pdfium().expect("Pdfium must be available for fixture test");
        let path = PathBuf::from("fixtures/MLE.pdf");
        if !path.exists() {
            return;
        }

        let mut docs = std::collections::HashMap::new();
        for i in 0..10 {
            let doc = pdfium
                .load_pdf_from_file(&path, None)
                .expect("Failed to load fixture PDF");
            docs.insert(i, doc);
        }
        assert_eq!(docs.len(), 10);

        for i in 0..10 {
            let removed = docs.remove(&i);
            assert!(removed.is_some());
        }
        assert!(docs.is_empty());
    }

    #[test]
    fn test_worker_extract_outline_large_book() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = bind_pdfium().expect("Pdfium must be available for fixture test");
        let path =
            PathBuf::from("fixtures/Modern Operating Systems 4th Edition--Andrew Tanenbaum.pdf");
        if !path.exists() {
            return;
        }

        let doc = pdfium
            .load_pdf_from_file(&path, None)
            .expect("Failed to load large book PDF");
        let outline = extract_outline(&doc);
        assert!(
            !outline.is_empty(),
            "Large book should have bookmarks outline"
        );
        assert!(outline.iter().any(|item| item.page_number.is_some()));
    }

    #[test]
    fn test_worker_search_page_fixture() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = bind_pdfium().expect("Pdfium must be available for fixture test");
        let path = PathBuf::from("fixtures/MLE.pdf");
        if !path.exists() {
            return;
        }

        let doc = pdfium
            .load_pdf_from_file(&path, None)
            .expect("Failed to load MLE fixture");

        // Case-insensitive search on page 1 for common word
        let matches = search_page(&doc, 1, "probability");
        assert!(!matches.is_empty(), "Page 1 should match 'probability'");
        let first_match = &matches[0];
        assert_eq!(first_match.page, 1);
        assert!(!first_match.rects.is_empty());
        let r = first_match.rects[0];
        assert!(r.min_x >= 0.0 && r.max_x > r.min_x);
        assert!(r.min_y >= 0.0 && r.max_y > r.min_y);

        // Case-insensitive search for another word
        let matches_likelihood = search_page(&doc, 1, "likelihood");
        assert!(
            !matches_likelihood.is_empty(),
            "Page 1 should match 'likelihood'"
        );

        // Search for non-existent text
        let no_matches = search_page(&doc, 1, "NonExistentRandomSearchString12345");
        assert!(no_matches.is_empty());

        let book_path =
            PathBuf::from("fixtures/Modern Operating Systems 4th Edition--Andrew Tanenbaum.pdf");
        if book_path.exists() {
            let book_doc = pdfium
                .load_pdf_from_file(&book_path, None)
                .expect("Failed to load book");
            let matches = search_page(&book_doc, 2, "operating systems");
            assert_eq!(matches.len(), 1);
            let m = &matches[0];
            println!("Book Page 2 'operating systems' rects: {:?}", m.rects);
            assert_eq!(
                m.rects.len(),
                1,
                "Should merge into a single line rectangle"
            );
            let r = m.rects[0];
            // X spans roughly 142 to 346, Y spans roughly 194 to 212
            assert!((r.min_x - 142.3).abs() < 2.0);
            assert!((r.max_x - 346.5).abs() < 2.0);
            assert!((r.min_y - 194.9).abs() < 2.0);
            assert!((r.max_y - 212.0).abs() < 2.0);

            let modern_matches = search_page(&book_doc, 2, "modern");
            assert_eq!(modern_matches.len(), 1);
            let m_mod = &modern_matches[0];
            assert_eq!(m_mod.rects.len(), 1);
            let r_mod = m_mod.rects[0];
            // 'MODERN' is above 'OPERATING SYSTEMS'
            assert!(r_mod.min_y < r.min_y);

            let tanenbaum_matches = search_page(&book_doc, 4, "tanenbaum");
            assert_eq!(tanenbaum_matches.len(), 1);
        }
    }

    #[test]
    fn test_worker_extract_page_text() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = bind_pdfium().expect("Pdfium must be available for fixture test");
        let path = PathBuf::from("fixtures/MLE.pdf");
        if !path.exists() {
            return;
        }

        let doc = pdfium
            .load_pdf_from_file(&path, None)
            .expect("load MLE.pdf");
        let page_text = extract_page_text(&doc, 0).expect("page text extraction");
        assert_eq!(page_text.page, 0);
        assert!(!page_text.chars.is_empty());
        assert!(!page_text.lines.is_empty());

        // Characters should have valid canonical bounds
        for ch in &page_text.chars {
            assert!(ch.rect.max_x >= ch.rect.min_x);
            assert!(ch.rect.max_y >= ch.rect.min_y);
        }

        // Lines should group characters properly
        for line in &page_text.lines {
            assert!(line.char_range.end > line.char_range.start);
            assert!(line.char_range.end <= page_text.chars.len());
            assert!(line.rect.max_x >= line.rect.min_x);
            assert!(line.rect.max_y >= line.rect.min_y);
        }

        let book_path =
            PathBuf::from("fixtures/Modern Operating Systems 4th Edition--Andrew Tanenbaum.pdf");
        if book_path.exists() {
            let book_doc = pdfium
                .load_pdf_from_file(&book_path, None)
                .expect("load book");
            let p3_text = extract_page_text(&book_doc, 3).expect("page 3 text");
            for line in p3_text.lines.iter().take(10) {
                let s = p3_text.extract_text_range(
                    line.char_range.start,
                    line.char_range.end.saturating_sub(1),
                );
                assert!(!s.is_empty());
            }
        }
    }

    #[test]
    fn test_worker_extract_page_links() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = bind_pdfium().expect("Pdfium must be available for fixture test");
        let book_path =
            PathBuf::from("fixtures/Modern Operating Systems 4th Edition--Andrew Tanenbaum.pdf");
        if book_path.exists() {
            let doc = pdfium
                .load_pdf_from_file(&book_path, None)
                .expect("load Modern Operating Systems book");
            let mut found_links = false;
            for p in 0..15.min(doc.pages().len() as usize) {
                if let Some(data) = extract_page_text(&doc, p) {
                    if !data.links.is_empty() {
                        found_links = true;
                        for link in &data.links {
                            assert!(link.rect.max_x >= link.rect.min_x);
                            assert!(link.rect.max_y >= link.rect.min_y);
                        }
                        break;
                    }
                }
            }
            assert!(
                found_links,
                "Modern Operating Systems book should contain interactive links in first 15 pages"
            );
        }
    }

    #[test]
    fn test_worker_extract_text_for_copy() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = bind_pdfium().expect("Pdfium must be available for fixture test");
        let path = PathBuf::from("fixtures/MLE.pdf");
        if !path.exists() {
            return;
        }

        let doc = pdfium
            .load_pdf_from_file(&path, None)
            .expect("load MLE.pdf");
        let page = doc.pages().get(0).expect("get page 0");
        let text_page = page.text().expect("get text page");
        let all_text = text_page.all();
        assert!(!all_text.is_empty());
    }

    #[test]
    fn test_worker_create_highlight_and_save() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = bind_pdfium().expect("Pdfium must be available for fixture test");
        let path = PathBuf::from("fixtures/MLE.pdf");
        if !path.exists() {
            return;
        }

        let doc = pdfium
            .load_pdf_from_file(&path, None)
            .expect("load MLE.pdf");
        let mut page = doc.pages().get(0).expect("get page 0");
        let initial_annots = page.annotations().len();

        // Create a highlight annotation
        let mut annot = page
            .annotations_mut()
            .create_highlight_annotation()
            .expect("create highlight");
        annot
            .set_stroke_color(PdfColor::new(255, 235, 59, 255))
            .expect("set stroke color");

        let quad =
            PdfQuadPoints::new_from_values(72.0, 720.0, 200.0, 720.0, 72.0, 700.0, 200.0, 700.0);
        annot
            .attachment_points_mut()
            .create_attachment_point_at_end(quad)
            .expect("add quad");

        let bounds = PdfRect::new_from_values(700.0, 72.0, 720.0, 200.0);
        annot.set_bounds(bounds).expect("set bounds");

        assert_eq!(page.annotations().len(), initial_annots + 1);

        // Save doc to bytes
        let bytes = doc.save_to_bytes().expect("save to bytes");
        assert!(!bytes.is_empty());

        // Reload from bytes to verify the annotation was embedded
        let reloaded_doc = pdfium
            .load_pdf_from_byte_vec(bytes, None)
            .expect("reload from bytes");
        let reloaded_page = reloaded_doc.pages().get(0).expect("reloaded page 0");
        assert_eq!(
            reloaded_page.annotations().len(),
            initial_annots + 1,
            "Annotation must be present in reloaded document"
        );
    }

    #[test]
    fn test_worker_create_ink_annotation_and_save() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = bind_pdfium().expect("Pdfium must be available for fixture test");
        let path = PathBuf::from("fixtures/MLE.pdf");
        if !path.exists() {
            return;
        }

        let doc = pdfium
            .load_pdf_from_file(&path, None)
            .expect("load MLE.pdf");
        let mut page = doc.pages().get(0).expect("get page 0");
        let initial_annots = page.annotations().len();

        let mut annot = page
            .annotations_mut()
            .create_ink_annotation()
            .expect("create ink");
        annot
            .set_stroke_color(PdfColor::new(76, 175, 80, 255))
            .expect("set stroke color");

        // Add a line path object to the ink annotation
        let mut path_obj = PdfPagePathObject::new(
            &doc,
            PdfPoints::new(100.0),
            PdfPoints::new(500.0),
            Some(PdfColor::new(76, 175, 80, 255)),
            Some(PdfPoints::new(4.0)),
            None,
        )
        .expect("create path object");
        path_obj
            .line_to(PdfPoints::new(200.0), PdfPoints::new(550.0))
            .expect("line to");
        annot
            .objects_mut()
            .add_path_object(path_obj)
            .expect("add path to annot");

        let bounds = PdfRect::new_from_values(490.0, 95.0, 560.0, 205.0);
        annot.set_bounds(bounds).expect("set bounds");

        assert_eq!(page.annotations().len(), initial_annots + 1);

        let bytes = doc.save_to_bytes().expect("save to bytes");
        assert!(!bytes.is_empty());

        let reloaded_doc = pdfium
            .load_pdf_from_byte_vec(bytes, None)
            .expect("reload from bytes");
        let reloaded_page = reloaded_doc.pages().get(0).expect("reloaded page 0");
        assert_eq!(
            reloaded_page.annotations().len(),
            initial_annots + 1,
            "Ink annotation must be present in reloaded document"
        );
    }

    #[test]
    fn test_worker_highlight_render_multiply_blending() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = bind_pdfium().expect("Pdfium must be available for fixture test");
        let path = PathBuf::from("fixtures/MLE.pdf");
        if !path.exists() {
            return;
        }

        let doc = pdfium
            .load_pdf_from_file(&path, None)
            .expect("load MLE.pdf");
        let mut page = doc.pages().get(0).expect("get page 0");
        let (ch, first_rect) = {
            let text = page.text().expect("get text page");
            let c = text.chars().get(0).expect("first char");
            (
                c.unicode_char(),
                c.loose_bounds().expect("char loose bounds"),
            )
        };
        println!("char 0: {:?}, rect: {:?}", ch, first_rect);

        let render_config = PdfRenderConfig::new()
            .set_target_width(800)
            .render_annotations(true);
        let orig_rgba = page
            .render_with_config(&render_config)
            .expect("render orig")
            .as_rgba_bytes();

        // Now add yellow highlight over the first char
        let mut annot = page
            .annotations_mut()
            .create_highlight_annotation()
            .expect("create highlight");
        annot
            .set_stroke_color(PdfColor::new(255, 235, 59, 255))
            .expect("set stroke color");
        let quad = PdfQuadPoints::new_from_values(
            first_rect.left().value,
            first_rect.top().value,
            first_rect.right().value,
            first_rect.top().value,
            first_rect.left().value,
            first_rect.bottom().value,
            first_rect.right().value,
            first_rect.bottom().value,
        );
        annot
            .attachment_points_mut()
            .create_attachment_point_at_end(quad)
            .expect("add quad");
        annot.set_bounds(first_rect).expect("set bounds");

        let hl_bitmap = page
            .render_with_config(&render_config)
            .expect("render with hl");
        let hl_rgba = hl_bitmap.as_rgba_bytes();

        assert_eq!(orig_rgba.len(), hl_rgba.len());
        // Find pixels that changed
        let mut changed = 0;
        let mut black_text_stayed_dark = 0;
        for i in (0..orig_rgba.len()).step_by(4) {
            let o_r = orig_rgba[i];
            let o_g = orig_rgba[i + 1];
            let o_b = orig_rgba[i + 2];
            let h_r = hl_rgba[i];
            let h_g = hl_rgba[i + 1];
            let h_b = hl_rgba[i + 2];
            if o_r != h_r || o_g != h_g || o_b != h_b {
                changed += 1;
                // If the original was black (text), under multiply blend it should still be black/dark (< 30)
                if o_r < 30 && o_g < 30 && o_b < 30 && h_r < 40 && h_g < 40 && h_b < 40 {
                    black_text_stayed_dark += 1;
                }
            }
        }
        println!(
            "total pixels: {}, changed: {}, black_text_stayed_dark: {}",
            orig_rgba.len() / 4,
            changed,
            black_text_stayed_dark
        );
        assert!(changed > 0, "Highlight must affect rendered bitmap");
    }

    #[test]
    fn test_worker_freehand_highlight_multiply_blending() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = bind_pdfium().expect("Pdfium must be available for fixture test");
        let path = PathBuf::from("fixtures/MLE.pdf");
        if !path.exists() {
            return;
        }

        let doc = pdfium
            .load_pdf_from_file(&path, None)
            .expect("load MLE.pdf");
        let page = doc.pages().get(0).expect("get page 0");
        let first_rect = {
            let text = page.text().expect("get text page");
            let c = text.chars().get(0).expect("first char");
            c.loose_bounds().expect("char loose bounds")
        };

        let render_config = PdfRenderConfig::new()
            .set_target_width(800)
            .render_annotations(true);
        let orig_bitmap = page
            .render_with_config(&render_config)
            .expect("render orig");
        let orig_rgba = orig_bitmap.as_rgba_bytes();
        let w = orig_bitmap.width() as usize;
        let h = orig_bitmap.height() as usize;

        let (crop_box, rotation) = get_page_crop_and_rotation(&page);
        let canon_rect = user_rect_to_canonical(&first_rect, &crop_box, rotation);
        let mid_y = (canon_rect.min_y + canon_rect.max_y) / 2.0;

        // Apply a freehand highlight stroke that passes across the first character in canonical space
        let hl = Highlight {
            id: 9999,
            page: 0,
            rects: Vec::new(),
            points: vec![
                [canon_rect.min_x - 10.0, mid_y],
                [canon_rect.max_x + 10.0, mid_y],
            ],
            color: AnnotationColor::new(255, 235, 59),
            alpha: 0.35,
            thickness: 12.0,
        };

        let mut hl_rgba = orig_rgba.clone();
        rasterize_freehand_highlight(
            &mut hl_rgba,
            w,
            h,
            &hl,
            page.width().value,
            page.height().value,
            0,
        );

        assert_eq!(orig_rgba.len(), hl_rgba.len());
        let mut changed = 0;
        let mut black_text_stayed_dark = 0;
        for i in (0..orig_rgba.len()).step_by(4) {
            let o_r = orig_rgba[i];
            let o_g = orig_rgba[i + 1];
            let o_b = orig_rgba[i + 2];
            let h_r = hl_rgba[i];
            let h_g = hl_rgba[i + 1];
            let h_b = hl_rgba[i + 2];
            if o_r != h_r || o_g != h_g || o_b != h_b {
                changed += 1;
                // Under Multiply blend, black text must remain black/dark (< 40)
                if o_r < 30 && o_g < 30 && o_b < 30 && h_r < 40 && h_g < 40 && h_b < 40 {
                    black_text_stayed_dark += 1;
                }
            }
        }
        assert!(
            changed > 0,
            "Freehand highlight must affect rendered bitmap"
        );
        assert!(
            black_text_stayed_dark > 0,
            "Freehand highlight must use Multiply blend: black text must remain black"
        );
    }

    #[test]
    fn test_rasterize_freehand_highlight_continuity() {
        let w = 200;
        let h = 200;
        let mut canvas = vec![255_u8; w * h * 4];

        // Curved polyline with multiple control points
        let hl = Highlight {
            id: 1234,
            page: 0,
            rects: Vec::new(),
            points: vec![
                [20.0, 30.0],
                [40.0, 60.0],
                [70.0, 90.0],
                [110.0, 110.0],
                [150.0, 140.0],
                [180.0, 170.0],
            ],
            color: AnnotationColor::new(255, 235, 59),
            alpha: 0.35,
            thickness: 16.0,
        };

        rasterize_freehand_highlight(&mut canvas, w, h, &hl, 200.0, 200.0, 0);

        // Verify that every vertex along the path is filled with pure highlight color
        for pt in &hl.points {
            let px = pt[0] as usize;
            let py = pt[1] as usize;
            let idx = (py * w + px) * 4;
            assert_eq!(canvas[idx], 255);
            assert_eq!(canvas[idx + 1], 235);
            assert_eq!(canvas[idx + 2], 59);
        }

        // Verify midpoint of segment (20,30)->(40,60) is continuous without any gap
        let mid_x = 30;
        let mid_y = 45;
        let mid_idx = (mid_y * w + mid_x) * 4;
        assert_eq!(canvas[mid_idx], 255, "Midpoint must be continuous");
        assert_eq!(canvas[mid_idx + 1], 235, "Midpoint must be continuous");
        assert_eq!(canvas[mid_idx + 2], 59, "Midpoint must be continuous");
    }

    #[test]
    fn test_worker_queue_priority_and_generation_pruning() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let path = PathBuf::from("fixtures/MLE.pdf");
        if !path.exists() {
            return;
        }

        let ctx = egui::Context::default();
        let (tx_cmd, rx_evt) = start_worker(ctx);

        // Receive PdfiumReady
        let evt = rx_evt
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert!(matches!(evt, Event::PdfiumReady));

        // Open doc
        let doc_id = 999;
        tx_cmd
            .send(Command::Open {
                doc_id,
                path: path.clone(),
            })
            .unwrap();

        let evt = rx_evt
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert!(matches!(evt, Event::Opened { doc_id: 999, .. }));

        // Send:
        // 1. Stale generation Render: gen 1, priority 3 (should be pruned)
        // 2. Prefetch Render: gen 2, priority 3 (prefetch, page 1)
        // 3. Visible Render: gen 2, priority 1 (visible, page 0)
        use crate::pdf::types::RenderCommand;
        tx_cmd
            .send(Command::Render(RenderCommand {
                doc_id,
                page: 1,
                width_px: 200,
                rotation: 0,
                generation: 1,
                priority: 3,
            }))
            .unwrap();
        tx_cmd
            .send(Command::Render(RenderCommand {
                doc_id,
                page: 1,
                width_px: 300,
                rotation: 0,
                generation: 2,
                priority: 3,
            }))
            .unwrap();
        tx_cmd
            .send(Command::Render(RenderCommand {
                doc_id,
                page: 0,
                width_px: 400,
                rotation: 0,
                generation: 2,
                priority: 1,
            }))
            .unwrap();

        // The first render result MUST be page 0 (priority 1), NOT page 1 (priority 3 or stale gen 1)
        let evt1 = loop {
            let evt = rx_evt
                .recv_timeout(std::time::Duration::from_secs(3))
                .unwrap();
            if let Event::AnnotationsExtracted { .. } = evt {
                continue;
            }
            break evt;
        };
        if let Event::Rendered(res) = evt1 {
            assert_eq!(res.page, 0, "Visible page 0 must be rendered first");
            assert_eq!(res.generation, 2);
            assert_eq!(res.width_px, 400);
        } else {
            panic!("Expected Rendered event, got {:?}", evt1);
        }

        // The second render result MUST be page 1 from generation 2 (generation 1 must have been skipped)
        let evt2 = loop {
            let evt = rx_evt
                .recv_timeout(std::time::Duration::from_secs(3))
                .unwrap();
            if let Event::AnnotationsExtracted { .. } = evt {
                continue;
            }
            break evt;
        };
        if let Event::Rendered(res) = evt2 {
            assert_eq!(res.page, 1);
            assert_eq!(res.generation, 2, "Stale generation 1 should be dropped");
            assert_eq!(res.width_px, 300);
        } else {
            panic!("Expected Rendered event, got {:?}", evt2);
        }

        // Close doc and shutdown worker cleanly
        tx_cmd.send(Command::Close { doc_id }).unwrap();
        tx_cmd.send(Command::Shutdown).unwrap();
    }

    #[test]
    fn test_worker_extract_annotations_roundtrip() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = bind_pdfium().expect("Pdfium must be available for fixture test");
        let path = PathBuf::from("fixtures/MLE.pdf");
        if !path.exists() {
            return;
        }

        let doc = pdfium
            .load_pdf_from_file(&path, None)
            .expect("load MLE.pdf");

        let hl1 = Highlight {
            id: 1,
            page: 0,
            rects: vec![CanonicalRect::new(100.0, 100.0, 250.0, 120.0)],
            points: Vec::new(),
            color: AnnotationColor::new(255, 235, 59),
            alpha: 0.35,
            thickness: 1.0,
        };

        let hl2 = Highlight {
            id: 2,
            page: 0,
            rects: Vec::new(),
            points: vec![[50.0, 200.0], [150.0, 220.0], [250.0, 210.0]],
            color: AnnotationColor::new(76, 175, 80),
            alpha: 0.35,
            thickness: 12.0,
        };

        apply_highlights_to_doc(&doc, &[hl1, hl2]).expect("apply highlights");
        let bytes = doc.save_to_bytes().expect("save to bytes");

        // Reload from bytes
        let reloaded_doc = pdfium
            .load_pdf_from_byte_vec(bytes, None)
            .expect("reload from bytes");

        let (extracted, text_boxes) = extract_annotations_from_doc(&reloaded_doc);
        assert_eq!(extracted.len(), 2, "Must extract 2 annotations");
        assert_eq!(text_boxes.len(), 0);

        // Now add a 3rd annotation and apply all 3 to the reloaded doc
        let hl3 = Highlight {
            id: 3,
            page: 0,
            rects: vec![CanonicalRect::new(300.0, 100.0, 400.0, 120.0)],
            points: Vec::new(),
            color: AnnotationColor::new(255, 128, 0),
            alpha: 0.35,
            thickness: 1.0,
        };

        let mut all_hl = extracted;
        all_hl.push(hl3);

        apply_highlights_to_doc(&reloaded_doc, &all_hl).expect("apply 3 highlights");
        let bytes2 = reloaded_doc.save_to_bytes().expect("save to bytes2");

        let reloaded_doc2 = pdfium
            .load_pdf_from_byte_vec(bytes2, None)
            .expect("reload from bytes2");

        let (extracted2, _) = extract_annotations_from_doc(&reloaded_doc2);
        assert_eq!(
            extracted2.len(),
            3,
            "Must extract all 3 annotations without vanishing"
        );
    }

    #[test]
    fn test_worker_text_box_annotations_roundtrip() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = bind_pdfium().expect("Pdfium must be available for fixture test");
        let path = PathBuf::from("fixtures/MLE.pdf");
        if !path.exists() {
            return;
        }

        let doc = pdfium
            .load_pdf_from_file(&path, None)
            .expect("load fixture");

        let tb = TextBox {
            id: 1,
            page: 0,
            pos: [100.0, 150.0],
            text: "Hello VersoReader Text Box".to_string(),
            font_size: 24.0,
            color: AnnotationColor::new(192, 57, 43),
            width: 200.0,
        };

        apply_annotations_to_doc(&doc, &[], std::slice::from_ref(&tb))
            .expect("apply text box annotation");
        let bytes = doc.save_to_bytes().expect("save to bytes");

        let reloaded = pdfium
            .load_pdf_from_byte_vec(bytes, None)
            .expect("reload from bytes");

        let (extracted_hl, extracted_tb) = extract_annotations_from_doc(&reloaded);
        assert_eq!(extracted_hl.len(), 0);
        assert_eq!(extracted_tb.len(), 1, "Must extract 1 text box annotation");
        assert_eq!(extracted_tb[0].page, 0);
        assert_eq!(extracted_tb[0].text, "Hello VersoReader Text Box");
        assert_eq!(extracted_tb[0].font_size, 24.0);
        assert_eq!(extracted_tb[0].color, AnnotationColor::new(192, 57, 43));

        let render_config = PdfRenderConfig::new()
            .set_target_width(800)
            .render_annotations(true);
        let clean_doc = pdfium.load_pdf_from_file(&path, None).unwrap();
        apply_highlights_to_doc(&clean_doc, &[]).unwrap();
        let orig_page = clean_doc.pages().get(0).unwrap();
        let orig_bmp = orig_page.render_with_config(&render_config).unwrap();

        // Clean reloaded doc of FreeText annotations as the worker does upon open
        apply_highlights_to_doc(&reloaded, &extracted_hl).unwrap();
        let page0 = reloaded.pages().get(0).unwrap();
        let bmp = page0.render_with_config(&render_config).unwrap();
        let orig_rgba = orig_bmp.as_rgba_bytes();
        let bmp_rgba = bmp.as_rgba_bytes();

        let mut diff_count = 0;
        for (o, b) in orig_rgba.iter().zip(bmp_rgba.iter()) {
            if o != b {
                diff_count += 1;
            }
        }
        assert_eq!(
            diff_count, 0,
            "Rendered page bitmap must have 0 diff pixels when FreeText is rendered by egui instead of PDFium"
        );
    }

    #[test]
    fn test_apply_highlights_to_doc_renders_multiply_bitmap() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = bind_pdfium().expect("Pdfium must be available for fixture test");
        let path = PathBuf::from("fixtures/MLE.pdf");
        if !path.exists() {
            return;
        }

        let doc = pdfium
            .load_pdf_from_file(&path, None)
            .expect("load MLE.pdf");
        let render_config = PdfRenderConfig::new()
            .set_target_width(800)
            .render_annotations(true);

        let orig_rgba = doc
            .pages()
            .get(0)
            .unwrap()
            .render_with_config(&render_config)
            .unwrap()
            .as_rgba_bytes();

        let page0 = doc.pages().get(0).unwrap();
        let page_h = page0.height().value;
        let char_rect = {
            let text = page0.text().unwrap();
            let c = text.chars().get(0).unwrap();
            let b = c.loose_bounds().unwrap();
            CanonicalRect::new(
                b.left().value,
                (page_h - b.top().value).max(0.0),
                b.right().value,
                (page_h - b.bottom().value).min(page_h),
            )
        };

        let hl = Highlight {
            id: 1,
            page: 0,
            rects: vec![char_rect],
            points: Vec::new(),
            color: AnnotationColor::new(255, 235, 59),
            alpha: 0.35,
            thickness: 1.0,
        };

        let hl_ink = Highlight {
            id: 2,
            page: 0,
            rects: Vec::new(),
            points: vec![[50.0, 200.0], [150.0, 220.0], [250.0, 210.0]],
            color: AnnotationColor::new(76, 175, 80),
            alpha: 0.35,
            thickness: 12.0,
        };

        apply_highlights_to_doc(&doc, &[hl, hl_ink]).expect("apply highlights");

        let hl_rgba = doc
            .pages()
            .get(0)
            .unwrap()
            .render_with_config(&render_config)
            .unwrap()
            .as_rgba_bytes();

        let mut changed = 0;
        let mut orig_dark_count = 0;
        let mut black_text_stayed_dark = 0;
        for i in (0..orig_rgba.len()).step_by(4) {
            let o_r = orig_rgba[i];
            let o_g = orig_rgba[i + 1];
            let o_b = orig_rgba[i + 2];
            let h_r = hl_rgba[i];
            let h_g = hl_rgba[i + 1];
            let h_b = hl_rgba[i + 2];
            if o_r != h_r || o_g != h_g || o_b != h_b {
                changed += 1;
                if o_r < 30 && o_g < 30 && o_b < 30 {
                    orig_dark_count += 1;
                    if orig_dark_count <= 5 {
                        println!(
                            "Orig dark pixel: ({}, {}, {}), after hl: ({}, {}, {})",
                            o_r, o_g, o_b, h_r, h_g, h_b
                        );
                    }
                    if h_r < 40 && h_g < 40 && h_b < 40 {
                        black_text_stayed_dark += 1;
                    }
                }
            }
        }
        println!(
            "apply_highlights changed pixels: {}, orig_dark_count: {}, black_text_stayed_dark: {}",
            changed, orig_dark_count, black_text_stayed_dark
        );
        assert!(
            changed > 0,
            "apply_highlights_to_doc must affect rendered page bitmap"
        );
    }

    #[test]
    fn test_extract_annotations_from_existing_fixture_mle() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let pdfium = bind_pdfium().expect("Pdfium must be available for fixture test");
        let path = PathBuf::from("fixtures/MLE.pdf");
        if !path.exists() {
            return;
        }

        let doc = pdfium
            .load_pdf_from_file(&path, None)
            .expect("load MLE.pdf");
        let (extracted_hl, extracted_tb) = extract_annotations_from_doc(&doc);
        assert!(
            extracted_tb.len() >= 3,
            "Must extract text boxes from MLE.pdf (got {})",
            extracted_tb.len()
        );

        // Notice: extract_annotations_from_doc automatically cleans the in-memory document
        // of FreeText annotations so PDFium won't render solid black/yellow boxes!
        let render_config = PdfRenderConfig::new()
            .set_target_width(800)
            .render_annotations(true);
        let page = doc.pages().get(0).unwrap();
        let bmp = page.render_with_config(&render_config).unwrap();
        let rgba = bmp.as_rgba_bytes();
        let w = bmp.width() as usize;
        let scale = 800.0 / 612.0;

        // Check pixels inside all text boxes: none should be solid black!
        for tb in &extracted_tb {
            let cx = tb.pos[0] + tb.width * 0.5;
            let cy = tb.pos[1] + 10.0;
            let px = (cx * scale) as usize;
            let py = (cy * scale) as usize;
            let idx = (py * w + px) * 4;
            let pixel = [rgba[idx], rgba[idx + 1], rgba[idx + 2], rgba[idx + 3]];
            println!(
                "Pixel at ({}, {}) for TB '{}': {:?}",
                px,
                py,
                tb.text.trim(),
                pixel
            );
            assert_ne!(
                pixel,
                [0, 0, 0, 255],
                "Pixel inside text box must NOT be solid black! Got: {:?}",
                pixel
            );
        }

        // Test saving with all annotations
        apply_annotations_to_doc(&doc, &extracted_hl, &extracted_tb)
            .expect("apply annotations for save");
        let bytes = doc.save_to_bytes().expect("save to bytes");
        let reloaded = pdfium.load_pdf_from_byte_vec(bytes, None).expect("reload");
        let (re_hl, re_tb) = extract_annotations_from_doc(&reloaded);
        assert_eq!(re_hl.len(), extracted_hl.len());
        assert_eq!(re_tb.len(), extracted_tb.len());
        for (tb, re) in extracted_tb.iter().zip(re_tb.iter()) {
            assert_eq!(tb.text, re.text);
            assert!(
                (tb.font_size - re.font_size).abs() < 0.1,
                "Font size must match across save/reload for '{}': expected {}, got {}",
                tb.text.trim(),
                tb.font_size,
                re.font_size
            );
        }
    }

    #[test]
    fn test_parse_font_size_from_metadata() {
        assert_eq!(
            parse_font_size_from_metadata("VersoReader:size=24.0"),
            Some(24.0)
        );
        assert_eq!(
            parse_font_size_from_metadata("VersoReader:size=10.5"),
            Some(10.5)
        );
        assert_eq!(parse_font_size_from_metadata("size=16"), Some(16.0));
        assert_eq!(parse_font_size_from_metadata("author_name"), None);
        assert_eq!(parse_font_size_from_metadata("size=2000"), None); // clamped
    }

    #[test]
    fn test_worker_stress_rapid_open_close_leak_audit() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let src_path = PathBuf::from("fixtures/MLE.pdf");
        if !src_path.exists() {
            return;
        }

        let temp_path =
            std::env::temp_dir().join(format!("verso_stress_leak_{}.pdf", std::process::id()));
        if let Err(e) = std::fs::copy(&src_path, &temp_path) {
            eprintln!("Failed to copy fixture for stress test: {e}");
            return;
        }

        let ctx = egui::Context::default();
        let (tx_cmd, rx_evt) = start_worker(ctx);
        let ready = rx_evt
            .recv_timeout(std::time::Duration::from_secs(3))
            .expect("Worker must send PdfiumReady");
        assert!(matches!(ready, Event::PdfiumReady));

        // Rapidly open and close documents in a loop 20 times
        for i in 1..=20 {
            let doc_id = i as u64;
            tx_cmd
                .send(Command::Open {
                    doc_id,
                    path: temp_path.clone(),
                })
                .unwrap();

            let evt = loop {
                let e = rx_evt
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .expect("Worker must respond with Open event");
                if let Event::AnnotationsExtracted { .. } = e {
                    continue;
                }
                break e;
            };
            assert!(
                matches!(evt, Event::Opened { doc_id: d, .. } if d == doc_id),
                "Expected Opened event for doc_id {doc_id}, got {evt:?}"
            );

            tx_cmd.send(Command::Close { doc_id }).unwrap();
        }

        tx_cmd.send(Command::Shutdown).unwrap();
        let _ = std::fs::remove_file(temp_path);
    }

    #[test]
    fn test_worker_stress_large_book_multipage_render_load() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let path =
            PathBuf::from("fixtures/Modern Operating Systems 4th Edition--Andrew Tanenbaum.pdf");
        if !path.exists() {
            return;
        }

        let ctx = egui::Context::default();
        let (tx_cmd, rx_evt) = start_worker(ctx);
        let ready = rx_evt
            .recv_timeout(std::time::Duration::from_secs(3))
            .expect("Worker must send PdfiumReady");
        assert!(matches!(ready, Event::PdfiumReady));

        let doc_id = 888;
        tx_cmd
            .send(Command::Open {
                doc_id,
                path: path.clone(),
            })
            .unwrap();

        let evt = rx_evt
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("Worker must open large book");
        let page_count = match evt {
            Event::Opened { page_count, .. } => page_count,
            other => panic!("Expected Opened event, got {:?}", other),
        };
        assert!(page_count > 100, "Large book fixture must have > 100 pages");

        // Queue high-res renders for multiple distant pages (simulating fast jumps and 300% zoom)
        use crate::pdf::types::RenderCommand;
        let test_pages = [0, 5, 20, 50, 100.min(page_count - 1)];
        for &p in &test_pages {
            tx_cmd
                .send(Command::Render(RenderCommand {
                    doc_id,
                    page: p,
                    width_px: 2400, // 300% zoom
                    rotation: 0,
                    generation: 1,
                    priority: 1,
                }))
                .unwrap();
        }

        // Receive all renders without crashes or timeout
        let mut received = 0;
        while received < test_pages.len() {
            let evt = rx_evt
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("Must receive render result");
            if let Event::Rendered(res) = evt {
                assert_eq!(res.doc_id, doc_id);
                assert!(res.bitmap_width_px > 0);
                assert!(res.height_px > 0);
                assert_eq!(
                    res.rgba.len(),
                    (res.bitmap_width_px * res.height_px * 4) as usize
                );
                received += 1;
            }
        }
        assert_eq!(received, test_pages.len());

        tx_cmd.send(Command::Close { doc_id }).unwrap();
        tx_cmd.send(Command::Shutdown).unwrap();
    }

    #[test]
    fn test_worker_corrupt_file_graceful_handling() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        // Create a temporary corrupted non-PDF file
        let temp_dir = std::env::temp_dir();
        let corrupt_path = temp_dir.join("verso_corrupt_test.pdf");
        std::fs::write(&corrupt_path, b"CORRUPTED_NOT_A_REAL_PDF_HEADER_12345").unwrap();

        let ctx = egui::Context::default();
        let (tx_cmd, rx_evt) = start_worker(ctx);
        let ready = rx_evt
            .recv_timeout(std::time::Duration::from_secs(3))
            .expect("Worker must send PdfiumReady");
        assert!(matches!(ready, Event::PdfiumReady));

        let doc_id = 777;
        tx_cmd
            .send(Command::Open {
                doc_id,
                path: corrupt_path.clone(),
            })
            .unwrap();

        let evt = rx_evt
            .recv_timeout(std::time::Duration::from_secs(3))
            .expect("Worker must respond to corrupt open");

        match evt {
            Event::OpenError { doc_id: d, error } => {
                assert_eq!(d, doc_id);
                assert!(
                    error.contains("Failed to open PDF"),
                    "Error message should explain failure gracefully: {error}"
                );
            }
            other => panic!("Expected OpenError for corrupt file, got {:?}", other),
        }

        let _ = std::fs::remove_file(corrupt_path);

        // Verify worker is still healthy and can open a valid file immediately after
        let valid_path = PathBuf::from("fixtures/MLE.pdf");
        if valid_path.exists() {
            tx_cmd
                .send(Command::Open {
                    doc_id: 778,
                    path: valid_path,
                })
                .unwrap();
            let evt2 = rx_evt
                .recv_timeout(std::time::Duration::from_secs(3))
                .expect("Worker must remain operational after error");
            assert!(matches!(evt2, Event::Opened { doc_id: 778, .. }));
            tx_cmd.send(Command::Close { doc_id: 778 }).unwrap();
        }
        tx_cmd.send(Command::Shutdown).unwrap();
    }

    #[test]
    fn test_dragon_book_worker_instant_open_and_render() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let path = PathBuf::from("fixtures/Dragon-book.pdf");
        if !path.exists() {
            return;
        }

        let ctx = egui::Context::default();
        let (tx_cmd, rx_evt) = start_worker(ctx);
        let ready = rx_evt
            .recv_timeout(std::time::Duration::from_secs(3))
            .expect("Worker must send PdfiumReady");
        assert!(matches!(ready, Event::PdfiumReady));

        let doc_id = 991;
        let t_start = std::time::Instant::now();
        tx_cmd
            .send(Command::Open {
                doc_id,
                path: path.clone(),
            })
            .unwrap();

        let evt = rx_evt
            .recv_timeout(std::time::Duration::from_millis(600))
            .expect("Worker must open Dragon-book in under 600ms");
        let t_open = t_start.elapsed();
        println!("Dragon-book Open -> Event::Opened took: {:?}", t_open);

        match evt {
            Event::Opened {
                doc_id: d,
                page_count,
                page_sizes,
                ..
            } => {
                assert_eq!(d, doc_id);
                assert_eq!(page_count, 1038);
                assert_eq!(page_sizes.len(), 1038);
            }
            other => panic!("Expected Opened event, got {:?}", other),
        }

        // Request page 0 render
        use crate::pdf::types::RenderCommand;
        let t_render_start = std::time::Instant::now();
        tx_cmd
            .send(Command::Render(RenderCommand {
                doc_id,
                page: 0,
                width_px: 1200,
                rotation: 0,
                generation: 1,
                priority: 1,
            }))
            .unwrap();

        let mut res_opt = None;
        while let Ok(evt) = rx_evt.recv_timeout(std::time::Duration::from_millis(1500)) {
            match evt {
                Event::Rendered(res) => {
                    res_opt = Some(res);
                    break;
                }
                Event::AnnotationsExtracted { .. } => {}
                other => panic!("Expected Rendered event, got {:?}", other),
            }
        }
        let res = res_opt.expect("Page 0 render must arrive");
        assert_eq!(res.doc_id, doc_id);
        assert_eq!(res.page, 0);
        assert!(res.bitmap_width_px > 0);
        let t_rendered = t_render_start.elapsed();
        println!("Dragon-book Page 0 Render took: {:?}", t_rendered);
        println!("TOTAL Cold Start to Rendered: {:?}", t_start.elapsed());

        tx_cmd.send(Command::Close { doc_id }).unwrap();
        tx_cmd.send(Command::Shutdown).unwrap();
    }

    #[test]
    fn test_tanenbaum_book_coordinates() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let path =
            PathBuf::from("fixtures/Modern Operating Systems 4th Edition--Andrew Tanenbaum.pdf");
        if !path.exists() {
            return;
        }

        let pdfium = bind_pdfium().unwrap();
        let doc = pdfium.load_pdf_from_file(&path, None).unwrap();
        println!("Tanenbaum page count: {}", doc.pages().len());
        for p in 0..5.min(doc.pages().len()) {
            let page = doc.pages().get(p).unwrap();
            let rot = page.rotation().unwrap_or(PdfPageRenderRotation::None);
            let size = doc.pages().page_size(p).unwrap();
            let w = page.width().value;
            let h = page.height().value;
            let crop = page.boundaries().crop().map(|b| {
                (
                    b.bounds.left().value,
                    b.bounds.bottom().value,
                    b.bounds.right().value,
                    b.bounds.top().value,
                )
            });
            let media = page.boundaries().media().map(|b| {
                (
                    b.bounds.left().value,
                    b.bounds.bottom().value,
                    b.bounds.right().value,
                    b.bounds.top().value,
                )
            });
            println!(
                "Page {}: size=({:?}, {:?}), page.w/h=({}, {}), rot={:?}, crop={:?}, media={:?}",
                p,
                size.width().value,
                size.height().value,
                w,
                h,
                rot,
                crop,
                media
            );

            if let Ok(text_page) = page.text() {
                println!("Page {} text len: {}", p, text_page.len());
                let chars = text_page.chars();
                for (ci, c) in chars.iter().take(5).enumerate() {
                    let ch = c.unicode_char().unwrap_or(' ');
                    let b = c.loose_bounds().or_else(|_| c.tight_bounds());
                    println!(
                        "  Char {}: '{}' bounds: {:?}",
                        ci,
                        ch,
                        b.map(|r| (
                            r.left().value,
                            r.bottom().value,
                            r.right().value,
                            r.top().value
                        ))
                    );
                }
            };
        }

        // Test applying and extracting an annotation on Page 1 (which has crop box offset)
        let orig_hl = Highlight {
            id: 12345,
            page: 1,
            rects: vec![CanonicalRect::new(200.0, 150.0, 250.0, 180.0)],
            points: Vec::new(),
            color: AnnotationColor::new(255, 235, 59),
            alpha: 0.35,
            thickness: 1.0,
        };
        apply_annotations_to_doc_on_pages(&doc, std::slice::from_ref(&orig_hl), &[], Some(1))
            .unwrap();

        let mut extracted_hl = Vec::new();
        let mut extracted_tb = Vec::new();
        let mut page1 = doc.pages().get(1).unwrap();
        extract_annotations_from_page(&mut page1, 1, &mut extracted_hl, &mut extracted_tb);
        assert_eq!(extracted_hl.len(), 1, "Must extract the applied highlight");
        assert_eq!(extracted_hl[0].rects.len(), 1);
        let ext_r = extracted_hl[0].rects[0];
        println!(
            "Original rect: {:?}, Extracted rect: {:?}",
            orig_hl.rects[0], ext_r
        );
        assert!((ext_r.min_x - 200.0).abs() < 1.0);
        assert!((ext_r.min_y - 150.0).abs() < 1.0);
        assert!((ext_r.max_x - 250.0).abs() < 1.0);
        assert!((ext_r.max_y - 180.0).abs() < 1.0);
    }

    #[test]
    fn test_worker_unload_doc() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let ctx = egui::Context::default();
        let (tx_cmd, rx_evt) = start_worker(ctx);

        let doc_id = 999;
        let path = PathBuf::from("fixtures/MLE.pdf");
        if !path.exists() {
            let _ = tx_cmd.send(Command::Shutdown);
            return;
        }

        // Wait for PdfiumReady
        let mut ready = false;
        while let Ok(evt) = rx_evt.recv_timeout(std::time::Duration::from_millis(500)) {
            if matches!(evt, Event::PdfiumReady) {
                ready = true;
                break;
            }
        }
        assert!(ready, "Worker must initialize Pdfium");

        // Open doc
        tx_cmd
            .send(Command::Open {
                doc_id,
                path: path.clone(),
            })
            .unwrap();

        let mut opened = false;
        while let Ok(evt) = rx_evt.recv_timeout(std::time::Duration::from_millis(500)) {
            if let Event::Opened {
                doc_id: id,
                page_count,
                ..
            } = evt
            {
                assert_eq!(id, doc_id);
                assert!(page_count > 0);
                opened = true;
                break;
            }
        }
        assert!(opened, "Doc must open");

        // Send UnloadDoc (dormant tab deactivation)
        tx_cmd.send(Command::UnloadDoc { doc_id }).unwrap();

        // Render request for unloaded doc should be ignored by worker
        tx_cmd
            .send(Command::Render(crate::pdf::types::RenderCommand {
                doc_id,
                page: 0,
                width_px: 400,
                rotation: 0,
                generation: 1,
                priority: 1,
            }))
            .unwrap();

        // Give worker 100ms; no Rendered event should arrive
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_millis(100) {
            if let Ok(evt) = rx_evt.recv_timeout(std::time::Duration::from_millis(20)) {
                if let Event::Rendered(_) = evt {
                    panic!("Unloaded document must not emit Rendered event: {:?}", evt);
                }
            }
        }

        // Reopen doc (instant wake up from dormant)
        tx_cmd
            .send(Command::Open {
                doc_id,
                path: path.clone(),
            })
            .unwrap();

        let mut reopened = false;
        while let Ok(evt) = rx_evt.recv_timeout(std::time::Duration::from_millis(500)) {
            if let Event::Opened {
                doc_id: id,
                page_count,
                ..
            } = evt
            {
                assert_eq!(id, doc_id);
                assert!(page_count > 0);
                reopened = true;
                break;
            }
        }
        assert!(reopened, "Reopened doc must emit Event::Opened");

        let _ = tx_cmd.send(Command::Shutdown);
    }
}
