use std::collections::HashMap;
use std::path::PathBuf;
use std::thread;

use crossbeam_channel::{Receiver, Sender};
use pdfium_render::prelude::*;

use crate::pdf::types::{Command, DocId, Event, PageSize, RenderResult};

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
    let candidates = candidate_library_paths();
    let mut searched: Vec<String> = Vec::new();

    for path in &candidates {
        searched.push(path.display().to_string());
        if path.exists() {
            if let Ok(bindings) = Pdfium::bind_to_library(path) {
                let pdfium = Box::new(Pdfium::new(bindings));
                return Ok(Box::leak(pdfium));
            }
        }
    }

    // 4. System library
    searched.push("System library search path".to_string());
    if let Ok(bindings) = Pdfium::bind_to_system_library() {
        let pdfium = Box::new(Pdfium::new(bindings));
        return Ok(Box::leak(pdfium));
    }

    Err(format!(
        "Could not load libpdfium.so.\n\nSearched locations:\n  • {}\n\nPlease run `bash scripts/setup_pdfium.sh` to download libpdfium.so into ./lib/.",
        searched.join("\n  • ")
    ))
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

    let mut documents: HashMap<DocId, PdfDocument<'static>> = HashMap::new();
    let mut latest_generations: HashMap<DocId, u64> = HashMap::new();

    // Command loop: processes incoming UI requests sequentially on the worker thread
    while let Ok(cmd) = rx_cmd.recv() {
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
                                error: format!("Failed to open PDF ({}): {err}", path.display()),
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
                                let target_w = render_cmd.width_px.clamp(32, 4096) as i32;
                                let render_config =
                                    PdfRenderConfig::new().set_target_width(target_w);

                                match page.render_with_config(&render_config) {
                                    Ok(bitmap) => {
                                        let w = bitmap.width() as u32;
                                        let h = bitmap.height() as u32;
                                        let rgba = bitmap.as_rgba_bytes();

                                        let _ = tx_evt.send(Event::Rendered(RenderResult {
                                            doc_id: render_cmd.doc_id,
                                            page: render_cmd.page,
                                            width_px: w,
                                            height_px: h,
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
                documents.remove(&doc_id);
                latest_generations.remove(&doc_id);
            }
            Command::Shutdown => break,
        }
    }
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
}
