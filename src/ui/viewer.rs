use std::collections::HashSet;
use std::sync::Arc;

use egui::{pos2, vec2, Color32, Rect, Stroke, StrokeKind};

use crate::cache::{PageTextCache, TextureCache, TextureKey};
use crate::pdf::transform::PageTransform;
use crate::pdf::types::{DocId, PageSize, PageTextData, SearchMatch, TextLocation, TextSelection};

/// A render request emitted by the virtualized viewer.
#[derive(Debug, Clone)]
pub struct PageRenderRequest {
    pub page: usize,
    pub width_px: u32,
    pub rotation: u16,
    pub priority: u8,
}

/// Position and target page for the viewer right-click context menu.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextMenuState {
    pub pos: egui::Pos2,
    pub page: usize,
}

/// Actions dispatched from the viewer context menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerAction {
    Copy,
    SelectAllPage(usize),
    SelectAllDocument,
}

/// Results of a viewer pass.
pub struct ViewerOutput {
    pub current_page: usize,
    pub requests: Vec<PageRenderRequest>,
    pub text_requests: Vec<usize>,
    pub visible_viewport_height: f32,
    pub current_scroll_offset_y: f32,
    pub viewport_top: f32,
    pub action: Option<ViewerAction>,
}

/// Maximum texture side (in pixels) for page bitmaps.
/// Set to 6144 px so that single-page texture memory stays strictly under the 150 MB cache limit
/// (~116 MB max for US Letter) while providing 1:1 bit-perfect razor-sharp resolution up to ~580% zoom.
pub const MAX_TEXTURE_SIDE: f32 = 6144.0;

/// Computes sharp texture dimensions and device-pixel aligned screen rect for a page.
pub fn compute_sharp_page_geometry(
    transform: &PageTransform,
    ppp: f32,
    screen_origin: egui::Pos2,
) -> (u32, u32, Rect) {
    let raw_screen_rect = transform.screen_rect();
    let (visual_w, visual_h) = transform.visual_size_points();
    let max_w_from_h = if visual_h > 0.0 {
        (MAX_TEXTURE_SIDE * (visual_w / visual_h)).floor() as u32
    } else {
        MAX_TEXTURE_SIDE as u32
    };
    let max_w = (MAX_TEXTURE_SIDE as u32).min(max_w_from_h).max(32);
    let sharp_width = ((raw_screen_rect.width() * ppp).round() as u32).clamp(32, max_w);
    let bitmap_h_px = if visual_w > 0.0 {
        ((visual_h / visual_w) * sharp_width as f32).round()
    } else {
        sharp_width as f32
    };

    let is_clamped = (raw_screen_rect.width() * ppp).round() as u32 > max_w;
    let (screen_w, screen_h) = if is_clamped {
        (raw_screen_rect.width(), raw_screen_rect.height())
    } else {
        (sharp_width as f32 / ppp, bitmap_h_px / ppp)
    };
    let screen_rect = Rect::from_min_size(screen_origin, vec2(screen_w, screen_h));
    (sharp_width, bitmap_h_px as u32, screen_rect)
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
    text_cache: &mut PageTextCache,
    pending_requests: &HashSet<(usize, u32, u16)>,
    pending_text_requests: &HashSet<usize>,
    scroll_to_page: Option<usize>,
    scroll_delta_y: Option<f32>,
    set_scroll_offset_y: Option<f32>,
    search_matches: &[SearchMatch],
    active_match_idx: Option<usize>,
    is_pinching: bool,
    selection: &mut Option<TextSelection>,
    drag_start: &mut Option<TextLocation>,
    context_menu: &mut Option<ContextMenuState>,
) -> ViewerOutput {
    let mut action: Option<ViewerAction> = None;
    let mut render_requests = Vec::new();
    let mut text_requests = Vec::new();
    let gap_y = 16.0;
    let page_count = page_sizes.len();

    if page_count == 0 {
        return ViewerOutput {
            current_page: 1,
            requests: render_requests,
            text_requests,
            visible_viewport_height: 600.0,
            current_scroll_offset_y: 0.0,
            viewport_top: 0.0,
            action: None,
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
        .enable_scrolling(!is_pinching)
        .drag_to_scroll(false);

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

    struct VisiblePageTextInfo {
        page: usize,
        transform: PageTransform,
        screen_rect: Rect,
        text: Option<Arc<PageTextData>>,
    }

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
        let ppp = ui.ctx().pixels_per_point().max(1.0);
        let snap = |v: f32| (v * ppp).round() / ppp;

        // Collect visible page info and request text for pages in viewport
        let mut visible_info = Vec::new();
        for idx in start_page..=end_page {
            let rect = page_rects[idx];
            let screen_origin = pos2(
                snap(content_origin.x + rect.min.x),
                snap(content_origin.y + rect.min.y),
            );
            let transform = PageTransform::new(page_sizes[idx], zoom, rotation, screen_origin);
            let (_, _, screen_rect) = compute_sharp_page_geometry(&transform, ppp, screen_origin);

            let pt_data = text_cache.get(doc_id, idx);
            if pt_data.is_none() && !pending_text_requests.contains(&idx) {
                text_requests.push(idx);
            }

            visible_info.push(VisiblePageTextInfo {
                page: idx,
                transform,
                screen_rect,
                text: pt_data,
            });
        }

        // Pointer text selection & hover handling
        let pointer_pos = ui.input(|i| i.pointer.interact_pos());
        let is_primary_down = ui.input(|i| i.pointer.primary_down());
        let is_primary_pressed = ui.input(|i| i.pointer.primary_pressed());
        let is_primary_released = ui.input(|i| i.pointer.primary_released());
        let is_secondary_clicked = ui.input(|i| i.pointer.secondary_clicked());
        let is_double_click = ui.input(|i| {
            i.pointer
                .button_double_clicked(egui::PointerButton::Primary)
        });
        let mut autoscroll_delta: f32 = 0.0;

        // Render right-click context menu popup if open
        let mut menu_consumed_pointer = false;
        if let Some(menu) = *context_menu {
            let mut close_menu = false;
            let menu_response = egui::Area::new(egui::Id::new("viewer_context_menu"))
                .order(egui::Order::Foreground)
                .fixed_pos(menu.pos)
                .show(ui.ctx(), |ui| {
                    egui::Frame::menu(ui.style()).show(ui, |ui| {
                        ui.set_min_width(170.0);

                        let has_sel = selection.as_ref().map(|s| !s.is_empty()).unwrap_or(false);
                        let copy_btn = egui::Button::new("Copy\tCtrl+C");
                        if ui.add_enabled(has_sel, copy_btn).clicked() {
                            action = Some(ViewerAction::Copy);
                            close_menu = true;
                        }

                        ui.separator();

                        let page_label = format!("Select All on Page {}", menu.page + 1);
                        if ui.button(page_label).clicked() {
                            action = Some(ViewerAction::SelectAllPage(menu.page));
                            close_menu = true;
                        }

                        if ui.button("Select All in Document\tCtrl+A").clicked() {
                            action = Some(ViewerAction::SelectAllDocument);
                            close_menu = true;
                        }
                    });
                });

            let menu_rect = menu_response.response.rect;

            if let Some(pos) = pointer_pos {
                if menu_rect.contains(pos) {
                    menu_consumed_pointer = true;
                } else if is_primary_pressed || is_secondary_clicked {
                    close_menu = true;
                }
            }

            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                close_menu = true;
            }

            if close_menu {
                *context_menu = None;
            }
        }

        if let Some(pos) = pointer_pos {
            let in_viewer = ui.clip_rect().contains(pos);

            // Surrender focus from any text inputs if pointer interacts with viewer
            if in_viewer && (is_primary_pressed || is_secondary_clicked || is_primary_down) {
                ui.ctx().memory_mut(|m| {
                    if let Some(id) = m.focused() {
                        m.surrender_focus(id);
                    }
                });
            }

            // Right-click context menu trigger (when clicking outside existing menu)
            if in_viewer && is_secondary_clicked && !menu_consumed_pointer {
                let clicked_page = visible_info
                    .iter()
                    .find(|info| info.screen_rect.contains(pos))
                    .map(|info| info.page)
                    .unwrap_or(current_page.saturating_sub(1));
                *context_menu = Some(ContextMenuState {
                    pos,
                    page: clicked_page,
                });
            }

            // Only process text selection if context menu didn't consume this interaction
            if in_viewer && !menu_consumed_pointer {
                // 1. Hover cursor: set Text (I-beam) cursor when hovering over text
                if !is_primary_down {
                    for info in &visible_info {
                        if info.screen_rect.contains(pos) {
                            if let Some(ref text) = info.text {
                                let can_pt = info.transform.screen_to_canonical(pos);
                                if text.is_hovering_text(can_pt) {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
                                    break;
                                }
                            }
                        }
                    }
                }

                // 2. Double-click word selection: selects entire word
                if is_double_click {
                    *drag_start = None;
                    for info in &visible_info {
                        if info.screen_rect.contains(pos) {
                            if let Some(ref text) = info.text {
                                let can_pt = info.transform.screen_to_canonical(pos);
                                if let Some(char_idx) = text.hit_test(can_pt) {
                                    let (w_start, w_end) = text.get_word_range(char_idx);
                                    *selection = Some(TextSelection::new(
                                        TextLocation::new(info.page, w_start),
                                        TextLocation::new(info.page, w_end),
                                    ));
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
                                    break;
                                }
                            }
                        }
                    }
                }

                // 3. Primary press down (single click):
                // Clears selection and starts drag anchor if over text.
                // Does NOT select single character/letter!
                if is_primary_pressed && !is_double_click {
                    *selection = None;
                    *drag_start = None;

                    for info in &visible_info {
                        if info.screen_rect.contains(pos) {
                            if let Some(ref text) = info.text {
                                let can_pt = info.transform.screen_to_canonical(pos);
                                if text.is_hovering_text(can_pt) {
                                    if let Some(char_idx) = text.hit_test(can_pt) {
                                        *drag_start = Some(TextLocation::new(info.page, char_idx));
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }

                // 4. Dragging: only creates/extends selection when dragged to a different character
                if is_primary_down && !is_primary_pressed && !is_double_click {
                    if let Some(anchor) = *drag_start {
                        let target = visible_info
                            .iter()
                            .find(|info| info.screen_rect.contains(pos))
                            .or_else(|| {
                                visible_info.iter().min_by(|a, b| {
                                    let dist_a = (a.screen_rect.center().y - pos.y).abs();
                                    let dist_b = (b.screen_rect.center().y - pos.y).abs();
                                    dist_a
                                        .partial_cmp(&dist_b)
                                        .unwrap_or(std::cmp::Ordering::Equal)
                                })
                            });

                        if let Some(info) = target {
                            if let Some(ref text) = info.text {
                                let can_pt = info.transform.screen_to_canonical(pos);
                                if let Some(char_idx) = text.hit_test(can_pt) {
                                    let focus = TextLocation::new(info.page, char_idx);
                                    if focus != anchor {
                                        *selection = Some(TextSelection::new(anchor, focus));
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
                                    } else {
                                        *selection = None;
                                    }
                                }
                            }
                        }

                        // Autoscroll when dragging near top/bottom viewer boundaries
                        let clip = ui.clip_rect();
                        let margin = 35.0;
                        if pos.y < clip.min.y + margin {
                            let speed = ((clip.min.y + margin - pos.y) * 0.5).clamp(4.0, 24.0);
                            autoscroll_delta = -speed;
                            ui.ctx().request_repaint();
                        } else if pos.y > clip.max.y - margin {
                            let speed = ((pos.y - (clip.max.y - margin)) * 0.5).clamp(4.0, 24.0);
                            autoscroll_delta = speed;
                            ui.ctx().request_repaint();
                        }
                    }
                }

                // 5. Primary release: finish drag
                if is_primary_released && !is_double_click {
                    *drag_start = None;
                    if let Some(cur_sel) = *selection {
                        if cur_sel.is_empty() {
                            *selection = None;
                        }
                    }
                }
            }
        }

        // Render pages (start_page..=end_page)
        for idx in start_page..=end_page {
            let rect = page_rects[idx];
            let is_in_viewport = rect.max.y >= viewport.min.y && rect.min.y <= viewport.max.y;

            let screen_origin = pos2(
                snap(content_origin.x + rect.min.x),
                snap(content_origin.y + rect.min.y),
            );
            let transform = PageTransform::new(page_sizes[idx], zoom, rotation, screen_origin);
            let (sharp_width, bitmap_h_px, screen_rect) =
                compute_sharp_page_geometry(&transform, ppp, screen_origin);

            let sharp_key = TextureKey {
                doc_id,
                page: idx,
                width_px: sharp_width,
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

                    // Request sharp render immediately for instant crisp display
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

                // Draw text selection overlay on this page through PageTransform
                if let Some(sel) = *selection {
                    if !sel.is_empty() {
                        let (sel_start, sel_end) = sel.normalized();
                        if idx >= sel_start.page && idx <= sel_end.page {
                            if let Some(text_data) = text_cache.get(doc_id, idx) {
                                if !text_data.chars.is_empty() {
                                    let p_start = if idx == sel_start.page {
                                        sel_start.char_idx
                                    } else {
                                        0
                                    };
                                    let p_end = if idx == sel_end.page {
                                        sel_end
                                            .char_idx
                                            .min(text_data.chars.len().saturating_sub(1))
                                    } else {
                                        text_data.chars.len().saturating_sub(1)
                                    };
                                    if p_start <= p_end {
                                        let sel_rects =
                                            text_data.get_selection_rects(p_start, p_end);
                                        let sel_color =
                                            Color32::from_rgba_unmultiplied(245, 90, 40, 115);
                                        for r in sel_rects {
                                            let screen_r = transform
                                                .canonical_to_screen_rect(r.to_egui_rect());
                                            painter.rect_filled(screen_r, 0.0, sel_color);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            } else {
                // Prefetch page outside viewport: only prefetch if the texture size is reasonable (<= 45 MB)
                // to prevent cache thrashing at high zoom levels where prefetching would evict the visible page.
                let est_bytes = (sharp_width as usize) * (bitmap_h_px as usize) * 4;
                let should_prefetch = est_bytes <= 45 * 1024 * 1024;
                if should_prefetch
                    && !cache.contains(&sharp_key)
                    && !pending_requests.contains(&(idx, sharp_width, rotation))
                {
                    render_requests.push(PageRenderRequest {
                        page: idx,
                        width_px: sharp_width,
                        rotation,
                        priority: 3,
                    });
                }
            }
        }

        autoscroll_delta
    });

    // Ensure visible sharp pages (priority 1) are requested before low-res or prefetch
    render_requests.sort_by_key(|r| r.priority);

    // If user clicked or dragged inside the viewer area, surrender focus from text inputs so arrow navigation immediately controls document scrolling
    if ui.input(|i| i.pointer.primary_down() || i.pointer.secondary_clicked()) {
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
    let autoscroll_delta = scroll_output.inner;

    // Apply combined keyboard delta or drag autoscroll delta
    let combined_delta = scroll_delta_y.unwrap_or(0.0) + autoscroll_delta;
    if combined_delta.abs() > 0.001 && !is_pinching {
        let mut state = scroll_output.state;
        state.offset.y = (state.offset.y + combined_delta).clamp(0.0, total_height);
        state.store(ui.ctx(), scroll_output.id);
    }

    ViewerOutput {
        current_page,
        requests: render_requests,
        text_requests,
        visible_viewport_height,
        current_scroll_offset_y,
        viewport_top,
        action,
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

    #[test]
    fn test_viewer_action_and_context_menu_state() {
        let menu = ContextMenuState {
            pos: pos2(120.0, 340.0),
            page: 2,
        };
        assert_eq!(menu.page, 2);
        assert_eq!(menu.pos.x, 120.0);

        let action = ViewerAction::SelectAllPage(2);
        assert_eq!(action, ViewerAction::SelectAllPage(2));
        assert_ne!(action, ViewerAction::SelectAllDocument);
    }

    #[test]
    fn test_text_selection_empty_vs_non_empty() {
        let loc1 = TextLocation::new(0, 10);
        let loc2 = TextLocation::new(0, 10);
        let loc3 = TextLocation::new(0, 15);
        let loc4 = TextLocation::new(1, 0);

        // Same location: is_empty() must be true (single click should never select)
        let empty_sel = TextSelection::new(loc1, loc2);
        assert!(empty_sel.is_empty());

        // Different char on same page: is_empty() must be false
        let non_empty_same_page = TextSelection::new(loc1, loc3);
        assert!(!non_empty_same_page.is_empty());

        // Across pages: is_empty() must be false
        let non_empty_cross_page = TextSelection::new(loc1, loc4);
        assert!(!non_empty_cross_page.is_empty());
    }

    #[test]
    fn test_screen_rect_zoom_alignment_and_clamping() {
        let size = PageSize {
            width: 612.0,
            height: 792.0,
        };
        let ppp = 1.25_f32;

        let origin = pos2(100.0, 50.0);

        // 1. Moderate zoom (150%): unclamped -> bit-perfect exact device pixels
        let t1 = PageTransform::new(size, 1.5, 0, origin);
        let r1 = t1.screen_rect();
        let (sharp_w1, _, screen_rect1) = compute_sharp_page_geometry(&t1, ppp, origin);
        assert!((sharp_w1 as f32 / ppp - r1.width()).abs() < 1.0);
        assert_eq!(screen_rect1.min, origin);

        // 2. High zoom (400%): with MAX_TEXTURE_SIDE = 6144, 400% is also unclamped and 1:1 bit-perfect!
        let t2 = PageTransform::new(size, 4.0, 0, origin);
        let r2 = t2.screen_rect();
        let (sharp_w2, _, screen_rect2) = compute_sharp_page_geometry(&t2, ppp, origin);
        assert!((sharp_w2 as f32 / ppp - r2.width()).abs() < 1.0);
        assert_eq!(screen_rect2.min, origin);

        // 3. Extreme zoom (800%): clamped at max texture side -> screen_rect uses raw_screen_rect
        // so coordinates, selection, and hover I-beam never drift!
        let t3 = PageTransform::new(size, 8.0, 0, origin);
        let r3 = t3.screen_rect();
        let (_sharp_w3, _, screen_rect3) = compute_sharp_page_geometry(&t3, ppp, origin);
        assert_eq!(screen_rect3.width(), r3.width());
        assert_eq!(screen_rect3.height(), r3.height());
        assert_eq!(screen_rect3.min, origin);
    }
}
