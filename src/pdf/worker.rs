use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::thread;

use crossbeam_channel::{Receiver, Sender};
use pdfium_render::prelude::*;

use crate::pdf::types::{
    CanonicalRect, CharBox, Command, DocId, Event, LineBox, OutlineItem, PageSize, PageTextData,
    RenderResult, SearchMatch,
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

    // 3. ./lib
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

    let mut documents: HashMap<DocId, PdfDocument<'static>> = HashMap::new();
    let mut latest_generations: HashMap<DocId, u64> = HashMap::new();
    let mut active_search: Option<ActiveSearch> = None;
    let mut cmd_queue: std::collections::VecDeque<Command> = std::collections::VecDeque::new();

    // Command loop: processes incoming UI requests sequentially on the worker thread.
    // Drains pending commands so intermediate stale zoom generations are dropped instantly.
    loop {
        // If local queue is empty, wait for next command from channel
        if cmd_queue.is_empty() {
            let next = if active_search.is_some() {
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

        // Pick next command: prioritize visible page renders (priority 1) over prefetch
        let cmd = if let Some(pos) = cmd_queue
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

                                for idx in 0..doc.pages().len() {
                                    if let Ok(page) = doc.pages().get(idx) {
                                        // Canonical points: origin top-left, unrotated page
                                        page_sizes.push(PageSize {
                                            width: page.width().value,
                                            height: page.height().value,
                                        });
                                    } else {
                                        page_sizes.push(PageSize {
                                            width: 612.0,
                                            height: 792.0,
                                        });
                                    }
                                }

                                documents.insert(doc_id, doc);
                                let _ = tx_evt.send(Event::Opened {
                                    doc_id,
                                    page_count,
                                    page_sizes,
                                });
                                ctx.request_repaint();
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
                                Ok(page) => {
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
                                            let rgba = bitmap.as_rgba_bytes();

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
        }
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

/// Extracts all characters and layout line boxes for a given page in canonical coordinates.
pub fn extract_page_text(doc: &PdfDocument, page_idx: usize) -> Option<PageTextData> {
    let page = doc.pages().get(page_idx as i32).ok()?;
    let text_page = page.text().ok()?;
    let count = text_page.len();
    if count <= 0 {
        return Some(PageTextData::new(page_idx, Vec::new(), Vec::new()));
    }

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

    Some(PageTextData::new(page_idx, final_chars, final_lines))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let res = bind_pdfium();
        assert!(
            res.is_ok(),
            "Expected bind_pdfium to succeed with ./lib/libpdfium.so: {:?}",
            res.err()
        );
    }

    #[test]
    fn test_worker_open_and_render_fixture() {
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
    fn test_worker_open_large_book_fixture() {
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
    fn test_worker_extract_text_for_copy() {
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
    fn test_worker_queue_priority_and_generation_pruning() {
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
        let evt1 = rx_evt
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        if let Event::Rendered(res) = evt1 {
            assert_eq!(res.page, 0, "Visible page 0 must be rendered first");
            assert_eq!(res.generation, 2);
            assert_eq!(res.width_px, 400);
        } else {
            panic!("Expected Rendered event, got {:?}", evt1);
        }

        // The second render result MUST be page 1 from generation 2 (generation 1 must have been skipped)
        let evt2 = rx_evt
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap();
        if let Event::Rendered(res) = evt2 {
            assert_eq!(res.page, 1);
            assert_eq!(res.generation, 2, "Stale generation 1 should be dropped");
            assert_eq!(res.width_px, 300);
        } else {
            panic!("Expected Rendered event, got {:?}", evt2);
        }

        // Close doc
        tx_cmd.send(Command::Close { doc_id }).unwrap();
    }
}
