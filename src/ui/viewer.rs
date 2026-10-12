use std::collections::HashSet;

use egui::{pos2, vec2, Color32, Rect, Stroke, StrokeKind};

use crate::cache::{TextureCache, TextureKey};
use crate::pdf::types::{DocId, PageSize};

/// A render request emitted by the virtualized viewer.
#[derive(Debug, Clone)]
pub struct PageRenderRequest {
    pub page: usize,
    pub width_px: u32,
    pub priority: u8,
}

/// Results of a viewer pass.
pub struct ViewerOutput {
    pub current_page: usize,
    pub requests: Vec<PageRenderRequest>,
}

/// Computes virtualized layout and paints intersecting pages (+1 prefetch).
#[allow(clippy::needless_range_loop)]
pub fn render_viewer(
    ui: &mut egui::Ui,
    doc_id: DocId,
    page_sizes: &[PageSize],
    cache: &mut TextureCache,
    pending_requests: &HashSet<(usize, u32)>,
    scroll_to_page: Option<usize>,
    scroll_delta_y: Option<f32>,
) -> ViewerOutput {
    let mut render_requests = Vec::new();
    let gap_y = 16.0;
    let page_count = page_sizes.len();

    if page_count == 0 {
        return ViewerOutput {
            current_page: 1,
            requests: render_requests,
        };
    }

    let available_width = ui.available_width().max(100.0);

    // Compute canonical page layout in content coordinates
    let mut page_rects = Vec::with_capacity(page_count);
    let mut cursor_y = gap_y;

    for size in page_sizes {
        let w = size.width;
        let h = size.height;
        let x = ((available_width - w) / 2.0).max(10.0);
        let rect = Rect::from_min_size(pos2(x, cursor_y), vec2(w, h));
        page_rects.push(rect);
        cursor_y += h + gap_y;
    }

    let total_height = cursor_y;
    let mut current_page = 1;

    let mut scroll_area = egui::ScrollArea::vertical().auto_shrink([false, false]);

    // Jump to specific page
    if let Some(target_p) = scroll_to_page {
        if target_p >= 1 && target_p <= page_count {
            let target_rect = page_rects[target_p - 1];
            scroll_area = scroll_area.vertical_scroll_offset(target_rect.min.y - gap_y);
        }
    }

    let scroll_output = scroll_area.show_viewport(ui, |ui, viewport| {
        ui.set_min_size(vec2(available_width, total_height));

        // 1. Identify which pages intersect the viewport
        let mut first_visible = None;
        let mut last_visible = None;

        for (idx, rect) in page_rects.iter().enumerate() {
            if rect.max.y >= viewport.min.y && rect.min.y <= viewport.max.y {
                if first_visible.is_none() {
                    first_visible = Some(idx);
                }
                last_visible = Some(idx);
            }
        }

        let first_visible = first_visible.unwrap_or(0);
        let last_visible = last_visible.unwrap_or(0);

        // Determine current page (closest to viewport center)
        let viewport_center_y = viewport.center().y;
        let mut closest_page = first_visible;
        let mut min_dist = f32::MAX;
        for idx in first_visible..=last_visible {
            let dist = (page_rects[idx].center().y - viewport_center_y).abs();
            if dist < min_dist {
                min_dist = dist;
                closest_page = idx;
            }
        }
        current_page = closest_page + 1;

        // Prefetch range: +1 page on each side
        let start_page = first_visible.saturating_sub(1);
        let end_page = (last_visible + 1).min(page_count - 1);

        let painter = ui.painter();
        let content_origin = ui.min_rect().min.to_vec2();

        for idx in start_page..=end_page {
            let rect = page_rects[idx];
            let is_in_viewport = rect.max.y >= viewport.min.y && rect.min.y <= viewport.max.y;

            let sharp_width =
                ((rect.width() * ui.ctx().pixels_per_point()).round() as u32).clamp(32, 4096);
            let lowres_width =
                ((sharp_width as f32 * 0.35).round() as u32).clamp(32, 4096);

            let sharp_key = TextureKey {
                doc_id,
                page: idx,
                width_px: sharp_width,
                rotation: 0,
            };
            let lowres_key = TextureKey {
                doc_id,
                page: idx,
                width_px: lowres_width,
                rotation: 0,
            };

            let screen_rect = rect.translate(content_origin);

            if is_in_viewport {
                // Sharp render available -> paint sharp
                if let Some(sharp_tex) = cache.get(&sharp_key) {
                    painter.image(
                        sharp_tex.id(),
                        screen_rect,
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                } else if let Some(lowres_tex) = cache.get(&lowres_key) {
                    // Low-res render available -> paint low-res and request sharp
                    painter.image(
                        lowres_tex.id(),
                        screen_rect,
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                    if !pending_requests.contains(&(idx, sharp_width)) {
                        render_requests.push(PageRenderRequest {
                            page: idx,
                            width_px: sharp_width,
                            priority: 1,
                        });
                    }
                } else {
                    // Placeholder: light-gray page rectangle with crisp border
                    painter.rect_filled(screen_rect, 0.0, Color32::from_rgb(240, 240, 240));
                    painter.rect_stroke(
                        screen_rect,
                        0.0,
                        Stroke::new(1.0_f32, Color32::from_rgb(180, 180, 180)),
                        StrokeKind::Outside,
                    );

                    // Progressive: request low-res (~35%) first, then sharp
                    if !pending_requests.contains(&(idx, lowres_width)) {
                        render_requests.push(PageRenderRequest {
                            page: idx,
                            width_px: lowres_width,
                            priority: 2,
                        });
                    }
                    if !pending_requests.contains(&(idx, sharp_width)) {
                        render_requests.push(PageRenderRequest {
                            page: idx,
                            width_px: sharp_width,
                            priority: 1,
                        });
                    }
                }
            } else {
                // Prefetch page outside viewport: request so texture is cached before user scrolls
                if !cache.contains(&sharp_key) && !pending_requests.contains(&(idx, sharp_width)) {
                    render_requests.push(PageRenderRequest {
                        page: idx,
                        width_px: sharp_width,
                        priority: 1,
                    });
                }
            }
        }
    });

    // If keyboard delta scrolling was requested
    if let Some(delta) = scroll_delta_y {
        let mut state = scroll_output.state;
        state.offset.y = (state.offset.y + delta).clamp(0.0, total_height);
        state.store(ui.ctx(), scroll_output.id);
    }

    ViewerOutput {
        current_page,
        requests: render_requests,
    }
}
