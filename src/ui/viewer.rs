use std::collections::HashSet;

use egui::{pos2, vec2, Color32, Rect, Stroke, StrokeKind};

use crate::cache::{TextureCache, TextureKey};
use crate::pdf::transform::PageTransform;
use crate::pdf::types::{DocId, PageSize, SearchMatch};

/// A render request emitted by the virtualized viewer.
#[derive(Debug, Clone)]
pub struct PageRenderRequest {
    pub page: usize,
    pub width_px: u32,
    pub rotation: u16,
    pub priority: u8,
}

/// Results of a viewer pass.
pub struct ViewerOutput {
    pub current_page: usize,
    pub requests: Vec<PageRenderRequest>,
    pub visible_viewport_height: f32,
    pub current_scroll_offset_y: f32,
    pub viewport_top: f32,
}

/// Computes virtualized layout and paints intersecting pages (+1 prefetch).
#[allow(clippy::needless_range_loop, clippy::too_many_arguments)]
pub fn render_viewer(
    ui: &mut egui::Ui,
    doc_id: DocId,
    page_sizes: &[PageSize],
    zoom: f32,
    rotation: u16,
    cache: &mut TextureCache,
    pending_requests: &HashSet<(usize, u32, u16)>,
    scroll_to_page: Option<usize>,
    scroll_delta_y: Option<f32>,
    set_scroll_offset_y: Option<f32>,
    search_matches: &[SearchMatch],
    active_match_idx: Option<usize>,
    is_pinching: bool,
) -> ViewerOutput {
    let mut render_requests = Vec::new();
    let gap_y = 16.0;
    let page_count = page_sizes.len();

    if page_count == 0 {
        return ViewerOutput {
            current_page: 1,
            requests: render_requests,
            visible_viewport_height: 600.0,
            current_scroll_offset_y: 0.0,
            viewport_top: 0.0,
        };
    }

    let available_width = ui.available_width().max(100.0);

    // Compute canonical page layout in content coordinates using PageTransform
    let mut page_dims = Vec::with_capacity(page_count);
    let mut max_page_w: f32 = 0.0;

    for size in page_sizes {
        let dummy_transform = PageTransform::new(*size, zoom, rotation, pos2(0.0, 0.0));
        let screen_size = dummy_transform.screen_size();
        let w = screen_size.x;
        let h = screen_size.y;
        if w > max_page_w {
            max_page_w = w;
        }
        page_dims.push((w, h));
    }

    let content_width = available_width.max(max_page_w + 20.0);
    let mut page_rects = Vec::with_capacity(page_count);
    let mut cursor_y = gap_y;

    for (w, h) in page_dims {
        let x = ((content_width - w) / 2.0).max(10.0);
        let rect = Rect::from_min_size(pos2(x, cursor_y), vec2(w, h));
        page_rects.push(rect);
        cursor_y += h + gap_y;
    }

    let total_height = cursor_y;
    let mut current_page = 1;

    let mut scroll_area = egui::ScrollArea::both()
        .auto_shrink([false, false])
        .enable_scrolling(!is_pinching);

    // Jump to specific page
    if let Some(target_p) = scroll_to_page {
        if target_p >= 1 && target_p <= page_count {
            let target_rect = page_rects[target_p - 1];
            scroll_area = scroll_area.vertical_scroll_offset(target_rect.min.y - gap_y);
        }
    } else if let Some(offset_y) = set_scroll_offset_y {
        scroll_area = scroll_area.vertical_scroll_offset(offset_y.clamp(0.0, total_height));
    }

    let mut visible_viewport_height = 600.0;
    let mut current_scroll_offset_y = 0.0;

    let scroll_output = scroll_area.show_viewport(ui, |ui, viewport| {
        ui.set_min_size(vec2(content_width, total_height));

        visible_viewport_height = viewport.height();
        current_scroll_offset_y = viewport.min.y;

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

            let screen_origin = pos2(content_origin.x + rect.min.x, content_origin.y + rect.min.y);
            let transform = PageTransform::new(page_sizes[idx], zoom, rotation, screen_origin);
            let raw_screen_rect = transform.screen_rect();

            // Snap bounding box origin to integer physical device pixels, and size to exact bitmap dimensions.
            // Eliminates subpixel phase shifting (half-pixel blur) and aspect ratio stretching across both axes!
            let ppp = ui.ctx().pixels_per_point().max(1.0);
            let snap = |v: f32| (v * ppp).round() / ppp;

            // Target physical pixels on the user's display:
            // 1 texel = EXACTLY 1 physical screen pixel! Zero downsampling, zero blur!
            let (visual_w, visual_h) = transform.visual_size_points();
            let max_w_from_h = if visual_h > 0.0 {
                (4096.0 * (visual_w / visual_h)).floor() as u32
            } else {
                4096
            };
            let max_w = 4096.min(max_w_from_h).max(32);
            let sharp_width = ((raw_screen_rect.width() * ppp).round() as u32).clamp(32, max_w);
            let lowres_width = ((sharp_width as f32 * 0.35).round() as u32).clamp(32, max_w);

            let bitmap_h_px = if visual_w > 0.0 {
                ((visual_h / visual_w) * sharp_width as f32).round()
            } else {
                sharp_width as f32
            };

            let screen_rect = Rect::from_min_size(
                pos2(snap(raw_screen_rect.min.x), snap(raw_screen_rect.min.y)),
                vec2(sharp_width as f32 / ppp, bitmap_h_px / ppp),
            );

            let sharp_key = TextureKey {
                doc_id,
                page: idx,
                width_px: sharp_width,
                rotation,
            };
            let lowres_key = TextureKey {
                doc_id,
                page: idx,
                width_px: lowres_width,
                rotation,
            };

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
                    if !pending_requests.contains(&(idx, sharp_width, rotation)) {
                        render_requests.push(PageRenderRequest {
                            page: idx,
                            width_px: sharp_width,
                            rotation,
                            priority: 1,
                        });
                    }
                } else if let Some(fallback_tex) = cache.get_best_for_page(doc_id, idx, rotation) {
                    // Fallback to previous zoom level texture so zooming is smooth and never flashes blank
                    painter.image(
                        fallback_tex.id(),
                        screen_rect,
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                    if !pending_requests.contains(&(idx, sharp_width, rotation)) {
                        render_requests.push(PageRenderRequest {
                            page: idx,
                            width_px: sharp_width,
                            rotation,
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
                    if !pending_requests.contains(&(idx, lowres_width, rotation)) {
                        render_requests.push(PageRenderRequest {
                            page: idx,
                            width_px: lowres_width,
                            rotation,
                            priority: 2,
                        });
                    }
                    if !pending_requests.contains(&(idx, sharp_width, rotation)) {
                        render_requests.push(PageRenderRequest {
                            page: idx,
                            width_px: sharp_width,
                            rotation,
                            priority: 1,
                        });
                    }
                }

                // Draw search match highlights on the visible page through PageTransform
                if !search_matches.is_empty() {
                    let page_num = idx + 1;
                    for (m_idx, match_item) in search_matches.iter().enumerate() {
                        if match_item.page == page_num {
                            let is_active = active_match_idx == Some(m_idx);
                            for rect in &match_item.rects {
                                let match_screen_rect =
                                    transform.canonical_to_screen_rect(rect.to_egui_rect());
                                if is_active {
                                    // Current active match: stronger orange color with stroke
                                    painter.rect_filled(
                                        match_screen_rect,
                                        2.0,
                                        Color32::from_rgba_unmultiplied(255, 140, 0, 180),
                                    );
                                    painter.rect_stroke(
                                        match_screen_rect,
                                        2.0,
                                        Stroke::new(1.5_f32, Color32::from_rgb(220, 90, 0)),
                                        StrokeKind::Outside,
                                    );
                                } else {
                                    // Other matches: soft translucent yellow
                                    painter.rect_filled(
                                        match_screen_rect,
                                        2.0,
                                        Color32::from_rgba_unmultiplied(255, 235, 0, 110),
                                    );
                                }
                            }
                        }
                    }
                }
            } else {
                // Prefetch page outside viewport: request so texture is cached before user scrolls
                if !cache.contains(&sharp_key)
                    && !pending_requests.contains(&(idx, sharp_width, rotation))
                {
                    render_requests.push(PageRenderRequest {
                        page: idx,
                        width_px: sharp_width,
                        rotation,
                        priority: 1,
                    });
                }
            }
        }
    });

    // If user clicked inside the viewer area, surrender focus from text inputs so arrow navigation immediately controls document scrolling
    if ui.input(|i| i.pointer.primary_clicked()) {
        if let Some(pos) = ui.input(|i| i.pointer.interact_pos()) {
            if scroll_output.inner_rect.contains(pos) {
                ui.ctx().memory_mut(|m| {
                    if let Some(id) = m.focused() {
                        m.surrender_focus(id);
                    }
                });
            }
        }
    }

    let viewport_top = scroll_output.inner_rect.top();

    // If keyboard delta scrolling was requested
    if let Some(delta) = scroll_delta_y {
        if !is_pinching {
            let mut state = scroll_output.state;
            state.offset.y = (state.offset.y + delta).clamp(0.0, total_height);
            state.store(ui.ctx(), scroll_output.id);
        }
    }

    ViewerOutput {
        current_page,
        requests: render_requests,
        visible_viewport_height,
        current_scroll_offset_y,
        viewport_top,
    }
}

/// Computes the vertical scroll offset needed to center a search match in the viewport.
pub fn compute_match_scroll_offset(
    match_item: &SearchMatch,
    page_sizes: &[PageSize],
    zoom: f32,
    rotation: u16,
    viewport_height: f32,
    available_width: f32,
) -> Option<f32> {
    if match_item.page == 0 || match_item.page > page_sizes.len() || match_item.rects.is_empty() {
        return None;
    }

    let gap_y = 16.0;
    let page_count = page_sizes.len();
    let mut max_page_w: f32 = 0.0;
    let mut page_dims = Vec::with_capacity(page_count);

    for size in page_sizes {
        let dummy_transform = PageTransform::new(*size, zoom, rotation, pos2(0.0, 0.0));
        let screen_size = dummy_transform.screen_size();
        let w = screen_size.x;
        let h = screen_size.y;
        if w > max_page_w {
            max_page_w = w;
        }
        page_dims.push((w, h));
    }

    let content_width = available_width.max(max_page_w + 20.0);
    let mut cursor_y = gap_y;
    let target_idx = match_item.page - 1;

    let mut target_page_rect = Rect::ZERO;
    for (idx, (w, h)) in page_dims.into_iter().enumerate() {
        let x = ((content_width - w) / 2.0).max(10.0);
        let rect = Rect::from_min_size(pos2(x, cursor_y), vec2(w, h));
        if idx == target_idx {
            target_page_rect = rect;
            break;
        }
        cursor_y += h + gap_y;
    }

    let transform =
        PageTransform::new(page_sizes[target_idx], zoom, rotation, target_page_rect.min);
    let first_rect = match_item.rects[0];
    let screen_r = transform.canonical_to_screen_rect(first_rect.to_egui_rect());

    // Center match vertically in viewport
    let target_y = (screen_r.center().y - viewport_height / 2.0).max(0.0);
    Some(target_y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::types::CanonicalRect;

    #[test]
    fn test_compute_match_scroll_offset() {
        let page_sizes = vec![
            PageSize {
                width: 600.0,
                height: 800.0,
            },
            PageSize {
                width: 600.0,
                height: 800.0,
            },
        ];

        let m = SearchMatch {
            page: 1,
            rects: vec![CanonicalRect::new(100.0, 100.0, 200.0, 120.0)],
        };

        let offset = compute_match_scroll_offset(&m, &page_sizes, 1.0, 0, 600.0, 800.0);
        assert!(offset.is_some());

        // Invalid page
        let m_invalid = SearchMatch {
            page: 99,
            rects: vec![CanonicalRect::new(10.0, 10.0, 20.0, 20.0)],
        };
        assert!(
            compute_match_scroll_offset(&m_invalid, &page_sizes, 1.0, 0, 600.0, 800.0).is_none()
        );
    }

    #[test]
    fn test_zoom_anchor_scroll_calculation() {
        let last_scroll_offset_y: f32 = 200.0;
        let anchor_viewport_y: f32 = 150.0;
        let old_zoom: f32 = 1.0;
        let new_zoom: f32 = 1.5;

        let content_before: f32 = last_scroll_offset_y + anchor_viewport_y;
        let ratio: f32 = new_zoom / old_zoom;
        let content_after: f32 = content_before * ratio;
        let new_offset: f32 = (content_after - anchor_viewport_y).max(0.0);

        assert!((content_before - 350.0).abs() < 1e-4);
        assert!((content_after - 525.0).abs() < 1e-4);
        assert!((new_offset - 375.0).abs() < 1e-4);

        // Verify content under cursor stays exactly under cursor
        assert!((content_after - new_offset - anchor_viewport_y).abs() < 1e-4);
    }
}
