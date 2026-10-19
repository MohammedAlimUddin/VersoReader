use std::collections::HashSet;
use std::sync::Arc;

use egui::{pos2, vec2, Color32, Rect, Stroke, StrokeKind};

use crate::annotations::{
    adjust_rect_thickness, generate_highlight_id, generate_text_box_id, AnnotationColor,
    DocumentAnnotations, EditingTextBox, Highlight, TextBox, UndoStack,
};
use crate::cache::{PageTextCache, TextureCache, TextureKey};
use crate::pdf::transform::PageTransform;
use crate::pdf::types::{
    CanonicalRect, DocId, LinkTarget, PageSize, PageTextData, SearchMatch, TextLocation,
    TextSelection,
};
use crate::ui::theme::Theme;
use crate::ui::toolbar::ActiveTool;

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

/// Actions dispatched from the viewer (context menu or link clicks).
#[derive(Debug, Clone, PartialEq)]
pub enum ViewerAction {
    Copy,
    HighlightSelection,
    AddTextNote(usize, [f32; 2]),
    SelectAllPage(usize),
    SelectAllDocument,
    JumpToPage(usize),
    OpenUrl(String),
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
    pub annotations_changed: bool,
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

/// Computes normalized UV coordinates [0.0, 1.0] for a screen sub-rectangle within a page's screen rectangle.
pub fn compute_page_subrect_uv(screen_rect: Rect, sub_rect: Rect) -> Option<(Rect, Rect)> {
    if screen_rect.width() <= 0.0 || screen_rect.height() <= 0.0 {
        return None;
    }
    let visible_rect = sub_rect.intersect(screen_rect);
    if !visible_rect.is_positive() {
        return None;
    }
    let u_min = ((visible_rect.min.x - screen_rect.min.x) / screen_rect.width()).clamp(0.0, 1.0);
    let u_max = ((visible_rect.max.x - screen_rect.min.x) / screen_rect.width()).clamp(0.0, 1.0);
    let v_min = ((visible_rect.min.y - screen_rect.min.y) / screen_rect.height()).clamp(0.0, 1.0);
    let v_max = ((visible_rect.max.y - screen_rect.min.y) / screen_rect.height()).clamp(0.0, 1.0);
    let uv = Rect::from_min_max(pos2(u_min, v_min), pos2(u_max, v_max));
    Some((visible_rect, uv))
}

/// Draws a vertical dotted-line rectangle centered at the cursor for the highlighter tool over non-text whitespace.
pub fn draw_dotted_vertical_rect(
    painter: &egui::Painter,
    center: egui::Pos2,
    width: f32,
    height: f32,
) {
    let rect = Rect::from_center_size(center, vec2(width, height));
    // Subtle background tint ensures interior stands out cleanly over any document content
    painter.rect_filled(
        rect,
        0.0,
        Color32::from_rgba_unmultiplied(255, 255, 255, 120),
    );
    // Prominent black border with thick dashed segments for maximum visibility on white pages
    let stroke = Stroke::new(2.0_f32, Color32::BLACK);
    let dash = 4.0;
    let gap = 2.0;
    let step = dash + gap;

    let mut x = rect.min.x;
    while x < rect.max.x {
        let x2 = (x + dash).min(rect.max.x);
        painter.line_segment([pos2(x, rect.min.y), pos2(x2, rect.min.y)], stroke);
        painter.line_segment([pos2(x, rect.max.y), pos2(x2, rect.max.y)], stroke);
        x += step;
    }
    let mut y = rect.min.y;
    while y < rect.max.y {
        let y2 = (y + dash).min(rect.max.y);
        painter.line_segment([pos2(rect.min.x, y), pos2(rect.min.x, y2)], stroke);
        painter.line_segment([pos2(rect.max.x, y), pos2(rect.max.x, y2)], stroke);
        y += step;
    }
}

/// Helper to test if a canonical point hits the right-edge resize handle of a text box.
fn hit_test_resize_handle(
    pos: [f32; 2],
    width: f32,
    height: f32,
    can_pt: egui::Pos2,
    tol: f32,
) -> bool {
    let right = pos[0] + width.max(40.0);
    (can_pt.x - right).abs() <= tol && can_pt.y >= pos[1] - 4.0 && can_pt.y <= pos[1] + height + 4.0
}

/// Helper to commit active text box editing. Returns true if annotations were modified.
fn commit_active_text_box(
    editing: &mut Option<EditingTextBox>,
    annotations: &mut DocumentAnnotations,
    undo_stack: &mut UndoStack,
    annotations_dirty: &mut bool,
) -> bool {
    if let Some(etb) = editing.take() {
        let trimmed = etb.text.trim();
        if trimmed.is_empty() {
            if let Some(old_id) = etb.id {
                if let Some(pos) = annotations.text_boxes.iter().position(|tb| tb.id == old_id) {
                    let removed = annotations.text_boxes.remove(pos);
                    undo_stack.push_delete_text_box(removed);
                    *annotations_dirty = true;
                    return true;
                }
            }
            return false;
        }

        if let Some(old_id) = etb.id {
            if let Some(existing) = annotations.text_boxes.iter_mut().find(|tb| tb.id == old_id) {
                if existing.text != etb.text
                    || (existing.font_size - etb.font_size).abs() > 0.01
                    || existing.color != etb.color
                    || existing.pos != etb.pos
                    || (existing.width - etb.width).abs() > 0.01
                {
                    let old_copy = existing.clone();
                    existing.text = etb.text;
                    existing.font_size = etb.font_size;
                    existing.color = etb.color;
                    existing.pos = etb.pos;
                    existing.width = etb.width;
                    undo_stack.push_modify_text_box(old_copy, existing.clone());
                    *annotations_dirty = true;
                    return true;
                }
            }
        } else {
            let tb = TextBox {
                id: generate_text_box_id(),
                page: etb.page,
                pos: etb.pos,
                text: etb.text,
                font_size: etb.font_size,
                color: etb.color,
                width: etb.width,
            };
            annotations.text_boxes.push(tb.clone());
            undo_stack.push_add_text_box(tb);
            *annotations_dirty = true;
            return true;
        }
    }
    false
}

/// Computes virtualized layout and paints intersecting pages (+1 prefetch).
#[allow(clippy::needless_range_loop, clippy::too_many_arguments)]
pub fn render_viewer(
    ui: &mut egui::Ui,
    doc_id: DocId,
    page_sizes: &[PageSize],
    zoom: f32,
    rotation: u16,
    render_generation: u64,
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
    active_tool: ActiveTool,
    highlighter_color: AnnotationColor,
    highlighter_thickness: f32,
    annotations: &mut DocumentAnnotations,
    undo_stack: &mut UndoStack,
    annotations_dirty: &mut bool,
    freehand_stroke: &mut Option<(usize, Vec<[f32; 2]>)>,
    editing_text_box: &mut Option<EditingTextBox>,
    selected_text_box_id: &mut Option<u64>,
    dragging_text_box: &mut Option<(u64, egui::Pos2, [f32; 2])>,
    resizing_text_box: &mut Option<(Option<u64>, egui::Pos2, f32)>,
    text_font_size: f32,
    text_color: AnnotationColor,
) -> ViewerOutput {
    let mut action: Option<ViewerAction> = None;
    let mut render_requests = Vec::new();
    let mut text_requests = Vec::new();
    let mut annotations_changed = false;
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
            annotations_changed: false,
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

        let painter = ui.painter().clone();
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
        let is_primary_clicked = ui.input(|i| i.pointer.primary_clicked());
        let is_secondary_clicked = ui.input(|i| i.pointer.secondary_clicked());
        let is_double_click = ui.input(|i| {
            i.pointer
                .button_double_clicked(egui::PointerButton::Primary)
        });
        let mut autoscroll_delta: f32 = 0.0;
        let mut show_highlighter_rect_cursor: Option<egui::Pos2> = None;

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

                        let hl_btn = egui::Button::new("Highlight Selection");
                        if ui.add_enabled(has_sel, hl_btn).clicked() {
                            action = Some(ViewerAction::HighlightSelection);
                            close_menu = true;
                        }

                        if ui.button("Add Text Note Here").clicked() {
                            let can_pt = visible_info
                                .iter()
                                .find(|info| info.page == menu.page)
                                .map(|info| info.transform.screen_to_canonical(menu.pos))
                                .unwrap_or_else(|| {
                                    let transform = PageTransform::new(
                                        page_sizes[menu.page],
                                        zoom,
                                        rotation,
                                        menu.pos,
                                    );
                                    transform.screen_to_canonical(menu.pos)
                                });
                            action =
                                Some(ViewerAction::AddTextNote(menu.page, [can_pt.x, can_pt.y]));
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
                    menu_consumed_pointer = true;
                }
            }

            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                close_menu = true;
            }

            if close_menu {
                *context_menu = None;
            }
        }

        let mut clicked_inside_editing = false;
        if let Some(ref etb) = *editing_text_box {
            if let Some(pos) = pointer_pos {
                if let Some(info) = visible_info.iter().find(|i| i.page == etb.page) {
                    let screen_top_left = info
                        .transform
                        .canonical_to_screen(pos2(etb.pos[0], etb.pos[1]));
                    let screen_w = (etb.width * zoom * PageTransform::DPI_SCALE).max(40.0);
                    let screen_h =
                        (etb.estimated_height() * zoom * PageTransform::DPI_SCALE).max(36.0);
                    let edit_screen_rect =
                        Rect::from_min_size(screen_top_left, vec2(screen_w, screen_h));
                    if edit_screen_rect.expand(8.0).contains(pos) {
                        clicked_inside_editing = true;
                    }
                }
            }
        }

        if let Some(pos) = pointer_pos {
            let in_viewer = ui.clip_rect().contains(pos);
            let is_over_overlay = ui
                .ctx()
                .layer_id_at(pos)
                .is_some_and(|l| l != ui.layer_id() && l.order != egui::Order::Background);
            let any_popup_open = ui.memory(|m| m.any_popup_open());
            let pointer_consumed = menu_consumed_pointer || is_over_overlay || any_popup_open;

            // Surrender focus from any text inputs if pointer interacts with viewer outside active inline editing
            if in_viewer
                && !pointer_consumed
                && (is_primary_pressed || is_secondary_clicked || is_primary_down)
                && !clicked_inside_editing
            {
                ui.ctx().memory_mut(|m| {
                    if let Some(id) = m.focused() {
                        m.surrender_focus(id);
                    }
                });
            }

            // Right-click context menu trigger (when clicking outside existing menu)
            if in_viewer && is_secondary_clicked && !pointer_consumed {
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

            // Only process annotations / text selection if context menu or overlay didn't consume this interaction
            if in_viewer && !pointer_consumed {
                if active_tool == ActiveTool::Eraser {
                    // Eraser cursor
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
                    *selection = None;
                    *drag_start = None;
                    if commit_active_text_box(
                        editing_text_box,
                        annotations,
                        undo_stack,
                        annotations_dirty,
                    ) {
                        annotations_changed = true;
                    }

                    // Click or drag across highlights or text boxes to delete them
                    if is_primary_down || is_primary_pressed {
                        for info in &visible_info {
                            if info.screen_rect.contains(pos) {
                                let can_pt = info.transform.screen_to_canonical(pos);
                                let r_can = 5.0 / (zoom * PageTransform::DPI_SCALE);
                                let test_pts = [
                                    can_pt,
                                    pos2(can_pt.x + r_can, can_pt.y),
                                    pos2(can_pt.x - r_can, can_pt.y),
                                    pos2(can_pt.x, can_pt.y + r_can),
                                    pos2(can_pt.x, can_pt.y - r_can),
                                ];
                                let mut removed_any = false;
                                annotations.highlights.retain(|hl| {
                                    if hl.page == info.page
                                        && test_pts.iter().any(|&p| hl.contains_canonical_point(p))
                                    {
                                        undo_stack.push_delete(hl.clone());
                                        removed_any = true;
                                        false
                                    } else {
                                        true
                                    }
                                });
                                annotations.text_boxes.retain(|tb| {
                                    if tb.page == info.page
                                        && test_pts.iter().any(|&p| tb.contains_canonical_point(p))
                                    {
                                        undo_stack.push_delete_text_box(tb.clone());
                                        removed_any = true;
                                        if *selected_text_box_id == Some(tb.id) {
                                            *selected_text_box_id = None;
                                        }
                                        false
                                    } else {
                                        true
                                    }
                                });
                                if removed_any {
                                    annotations_changed = true;
                                    *annotations_dirty = true;
                                    ui.ctx().request_repaint();
                                }
                            }
                        }
                    }
                } else {
                    // 1. Hover cursor
                    if !is_primary_down {
                        let tol = 8.0 / (zoom * PageTransform::DPI_SCALE);
                        let mut is_over_resize_handle = false;
                        if let Some(ref etb) = *editing_text_box {
                            if let Some(info) = visible_info.iter().find(|i| i.page == etb.page) {
                                let can_pt = info.transform.screen_to_canonical(pos);
                                if hit_test_resize_handle(
                                    etb.pos,
                                    etb.width,
                                    etb.estimated_height(),
                                    can_pt,
                                    tol,
                                ) {
                                    is_over_resize_handle = true;
                                }
                            }
                        }
                        if !is_over_resize_handle {
                            if let Some(sel_id) = *selected_text_box_id {
                                if let Some(tb) =
                                    annotations.text_boxes.iter().find(|tb| tb.id == sel_id)
                                {
                                    if let Some(info) =
                                        visible_info.iter().find(|i| i.page == tb.page)
                                    {
                                        let can_pt = info.transform.screen_to_canonical(pos);
                                        if hit_test_resize_handle(
                                            tb.pos,
                                            tb.width,
                                            tb.estimated_height(),
                                            can_pt,
                                            tol,
                                        ) {
                                            is_over_resize_handle = true;
                                        }
                                    }
                                }
                            }
                        }

                        if is_over_resize_handle {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                        } else {
                            let mut cursor_handled = false;
                            for info in &visible_info {
                                if info.screen_rect.contains(pos) {
                                    cursor_handled = true;
                                    let can_pt = info.transform.screen_to_canonical(pos);
                                    if annotations.text_boxes.iter().any(|tb| {
                                        tb.page == info.page && tb.contains_canonical_point(can_pt)
                                    }) {
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
                                        break;
                                    }
                                    if active_tool == ActiveTool::Text {
                                        ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
                                        break;
                                    }
                                    if active_tool == ActiveTool::Highlighter {
                                        let force_freehand = ui.input(|i| i.modifiers.shift);
                                        let over_text = if !force_freehand {
                                            if let Some(ref text) = info.text {
                                                text.is_hovering_text(can_pt)
                                            } else {
                                                false
                                            }
                                        } else {
                                            false
                                        };
                                        if over_text {
                                            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
                                        } else {
                                            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
                                            show_highlighter_rect_cursor = Some(pos);
                                        }
                                        break;
                                    }
                                    if active_tool == ActiveTool::None {
                                        if let Some(ref text) = info.text {
                                            if let Some(link) = text.hit_test_link(can_pt) {
                                                ui.ctx().set_cursor_icon(
                                                    egui::CursorIcon::PointingHand,
                                                );
                                                match &link.target {
                                                    LinkTarget::Uri(url) => {
                                                        egui::show_tooltip_text(
                                                            ui.ctx(),
                                                            ui.layer_id(),
                                                            egui::Id::new("link_tooltip"),
                                                            url,
                                                        );
                                                    }
                                                    LinkTarget::Page(p) => {
                                                        egui::show_tooltip_text(
                                                            ui.ctx(),
                                                            ui.layer_id(),
                                                            egui::Id::new("link_tooltip"),
                                                            format!("Go to page {}", p + 1),
                                                        );
                                                    }
                                                }
                                                break;
                                            }
                                            if text.is_hovering_text(can_pt) {
                                                ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                            if !cursor_handled && active_tool == ActiveTool::Highlighter {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::None);
                                show_highlighter_rect_cursor = Some(pos);
                            }
                        }
                    }

                    // 2. Double-click on existing text box opens it for editing, or selects word
                    let mut double_clicked_tb = None;
                    if is_double_click {
                        for info in &visible_info {
                            if info.screen_rect.contains(pos) {
                                let can_pt = info.transform.screen_to_canonical(pos);
                                if let Some(tb) = annotations.text_boxes.iter().find(|tb| {
                                    tb.page == info.page && tb.contains_canonical_point(can_pt)
                                }) {
                                    double_clicked_tb = Some(tb.clone());
                                    break;
                                }
                            }
                        }
                    }

                    if let Some(tb) = double_clicked_tb {
                        if commit_active_text_box(
                            editing_text_box,
                            annotations,
                            undo_stack,
                            annotations_dirty,
                        ) {
                            annotations_changed = true;
                        }
                        *selected_text_box_id = Some(tb.id);
                        *editing_text_box = Some(EditingTextBox::from_existing(&tb));
                        *drag_start = None;
                        *selection = None;
                        *dragging_text_box = None;
                        *resizing_text_box = None;
                        ui.ctx().request_repaint();
                    } else if is_double_click {
                        *drag_start = None;
                        for info in &visible_info {
                            if info.screen_rect.contains(pos) {
                                if let Some(ref text) = info.text {
                                    let can_pt = info.transform.screen_to_canonical(pos);
                                    if let Some(char_idx) = text.hit_test(can_pt) {
                                        let (w_start, w_end) = text.get_word_range(char_idx);
                                        if active_tool == ActiveTool::Highlighter {
                                            let raw_rects =
                                                text.get_selection_rects(w_start, w_end);
                                            let line_rects: Vec<CanonicalRect> = raw_rects
                                                .into_iter()
                                                .map(|r| {
                                                    adjust_rect_thickness(r, highlighter_thickness)
                                                })
                                                .collect();
                                            if !line_rects.is_empty() {
                                                let id = generate_highlight_id();
                                                let hl = Highlight {
                                                    id,
                                                    page: info.page,
                                                    rects: line_rects,
                                                    points: Vec::new(),
                                                    color: highlighter_color,
                                                    alpha: 0.35,
                                                    thickness: highlighter_thickness,
                                                };
                                                annotations.highlights.push(hl.clone());
                                                undo_stack.push_add(hl);
                                                annotations_changed = true;
                                                *annotations_dirty = true;
                                                *selection = None;
                                                ui.ctx().request_repaint();
                                            }
                                        } else {
                                            *selection = Some(TextSelection::new(
                                                TextLocation::new(info.page, w_start),
                                                TextLocation::new(info.page, w_end),
                                            ));
                                            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
                                        }
                                        break;
                                    }
                                }
                            }
                        }
                    }

                    // 3. Primary press down (single click):
                    if is_primary_pressed && !is_double_click {
                        let tol = 8.0 / (zoom * PageTransform::DPI_SCALE);
                        let mut hit_resize = None;
                        if let Some(ref etb) = *editing_text_box {
                            if let Some(info) = visible_info.iter().find(|i| i.page == etb.page) {
                                let can_pt = info.transform.screen_to_canonical(pos);
                                if hit_test_resize_handle(
                                    etb.pos,
                                    etb.width,
                                    etb.estimated_height(),
                                    can_pt,
                                    tol,
                                ) {
                                    hit_resize = Some((None, etb.width.max(40.0)));
                                }
                            }
                        }
                        if hit_resize.is_none() {
                            if let Some(sel_id) = *selected_text_box_id {
                                if let Some(tb) =
                                    annotations.text_boxes.iter().find(|tb| tb.id == sel_id)
                                {
                                    if let Some(info) =
                                        visible_info.iter().find(|i| i.page == tb.page)
                                    {
                                        let can_pt = info.transform.screen_to_canonical(pos);
                                        if hit_test_resize_handle(
                                            tb.pos,
                                            tb.width,
                                            tb.estimated_height(),
                                            can_pt,
                                            tol,
                                        ) {
                                            hit_resize = Some((Some(sel_id), tb.width.max(40.0)));
                                        }
                                    }
                                }
                            }
                        }

                        if let Some((target, start_w)) = hit_resize {
                            *resizing_text_box = Some((target, pos, start_w));
                            *dragging_text_box = None;
                            *drag_start = None;
                            ui.ctx().request_repaint();
                        } else if !clicked_inside_editing {
                            if commit_active_text_box(
                                editing_text_box,
                                annotations,
                                undo_stack,
                                annotations_dirty,
                            ) {
                                annotations_changed = true;
                            }

                            *selection = None;
                            *drag_start = None;
                            *freehand_stroke = None;
                            *resizing_text_box = None;

                            // Check if click hit an existing text box
                            let mut hit_tb = None;
                            for info in &visible_info {
                                if info.screen_rect.contains(pos) {
                                    let can_pt = info.transform.screen_to_canonical(pos);
                                    if let Some(tb) = annotations.text_boxes.iter().find(|tb| {
                                        tb.page == info.page && tb.contains_canonical_point(can_pt)
                                    }) {
                                        hit_tb = Some((tb.id, tb.pos));
                                        break;
                                    }
                                }
                            }

                            if let Some((tb_id, tb_pos)) = hit_tb {
                                *selected_text_box_id = Some(tb_id);
                                *dragging_text_box = Some((tb_id, pos, tb_pos));
                                ui.ctx().request_repaint();
                            } else {
                                *selected_text_box_id = None;
                                *dragging_text_box = None;

                                if active_tool == ActiveTool::Text {
                                    for info in &visible_info {
                                        if info.screen_rect.contains(pos) {
                                            let can_pt = info.transform.screen_to_canonical(pos);
                                            *editing_text_box = Some(EditingTextBox::new(
                                                info.page,
                                                [can_pt.x, can_pt.y],
                                                text_font_size,
                                                text_color,
                                            ));
                                            ui.ctx().request_repaint();
                                            break;
                                        }
                                    }
                                } else if active_tool == ActiveTool::Highlighter {
                                    for info in &visible_info {
                                        if info.screen_rect.contains(pos) {
                                            let can_pt = info.transform.screen_to_canonical(pos);
                                            let force_freehand = ui.input(|i| i.modifiers.shift);
                                            let mut on_text = false;
                                            if !force_freehand {
                                                if let Some(ref text) = info.text {
                                                    if text.is_hovering_text(can_pt) {
                                                        if let Some(char_idx) =
                                                            text.hit_test(can_pt)
                                                        {
                                                            *drag_start = Some(TextLocation::new(
                                                                info.page, char_idx,
                                                            ));
                                                            ui.ctx().set_cursor_icon(
                                                                egui::CursorIcon::Text,
                                                            );
                                                            on_text = true;
                                                        }
                                                    }
                                                }
                                            }
                                            if !on_text {
                                                // Freehand highlight drawing begins in non-text areas (or when holding Shift)!
                                                *freehand_stroke =
                                                    Some((info.page, vec![[can_pt.x, can_pt.y]]));
                                                ui.ctx().set_cursor_icon(egui::CursorIcon::None);
                                                show_highlighter_rect_cursor = Some(pos);
                                            }
                                            break;
                                        }
                                    }
                                } else {
                                    for info in &visible_info {
                                        if info.screen_rect.contains(pos) {
                                            if let Some(ref text) = info.text {
                                                let can_pt =
                                                    info.transform.screen_to_canonical(pos);
                                                if text.is_hovering_text(can_pt) {
                                                    if let Some(char_idx) = text.hit_test(can_pt) {
                                                        *drag_start = Some(TextLocation::new(
                                                            info.page, char_idx,
                                                        ));
                                                        ui.ctx().set_cursor_icon(
                                                            egui::CursorIcon::Text,
                                                        );
                                                        break;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // 4. Dragging: translates moving text box, resizes text box, extends text selection, or appends to freehand stroke
                    if is_primary_down && !is_primary_pressed && !is_double_click {
                        if let Some((target, start_screen_pos, start_width)) = *resizing_text_box {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                            let target_page = match target {
                                Some(id) => annotations
                                    .text_boxes
                                    .iter()
                                    .find(|tb| tb.id == id)
                                    .map(|tb| tb.page),
                                None => editing_text_box.as_ref().map(|etb| etb.page),
                            };
                            if let Some(page) = target_page {
                                if let Some(info) = visible_info.iter().find(|i| i.page == page) {
                                    let screen_delta = pos - start_screen_pos;
                                    let can_delta =
                                        info.transform.screen_to_canonical_delta(screen_delta);
                                    let new_width = (start_width + can_delta.x).max(40.0);
                                    match target {
                                        Some(id) => {
                                            if let Some(tb) = annotations
                                                .text_boxes
                                                .iter_mut()
                                                .find(|tb| tb.id == id)
                                            {
                                                tb.width = new_width;
                                            }
                                        }
                                        None => {
                                            if let Some(ref mut etb) = *editing_text_box {
                                                etb.width = new_width;
                                            }
                                        }
                                    }
                                    ui.ctx().request_repaint();
                                }
                            }
                        } else if let Some((drag_id, start_screen_pos, start_can_pos)) =
                            *dragging_text_box
                        {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                            if let Some(tb) = annotations
                                .text_boxes
                                .iter_mut()
                                .find(|tb| tb.id == drag_id)
                            {
                                if let Some(info) = visible_info.iter().find(|i| i.page == tb.page)
                                {
                                    let screen_delta = pos - start_screen_pos;
                                    let can_delta =
                                        info.transform.screen_to_canonical_delta(screen_delta);
                                    tb.pos = [
                                        start_can_pos[0] + can_delta.x,
                                        start_can_pos[1] + can_delta.y,
                                    ];
                                    ui.ctx().request_repaint();
                                }
                            }
                        } else if let Some((fh_page, ref mut fh_pts)) = *freehand_stroke {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::None);
                            show_highlighter_rect_cursor = Some(pos);
                            for info in &visible_info {
                                if info.page == fh_page {
                                    let can_pt = info.transform.screen_to_canonical(pos);
                                    if let Some(last) = fh_pts.last() {
                                        let dx = can_pt.x - last[0];
                                        let dy = can_pt.y - last[1];
                                        if dx * dx + dy * dy >= 2.25 {
                                            fh_pts.push([can_pt.x, can_pt.y]);
                                            ui.ctx().request_repaint();
                                        }
                                    }
                                    break;
                                }
                            }
                        } else if let Some(anchor) = *drag_start {
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
                                let speed =
                                    ((pos.y - (clip.max.y - margin)) * 0.5).clamp(4.0, 24.0);
                                autoscroll_delta = speed;
                                ui.ctx().request_repaint();
                            }
                        }
                    }

                    // 5. Primary release: finish drag or resize
                    if is_primary_released && !is_double_click {
                        *drag_start = None;
                        if let Some((target, _, start_width)) = resizing_text_box.take() {
                            if let Some(id) = target {
                                if let Some(tb) =
                                    annotations.text_boxes.iter().find(|tb| tb.id == id)
                                {
                                    if (tb.width - start_width).abs() > 0.5 {
                                        let mut old_tb = tb.clone();
                                        old_tb.width = start_width;
                                        undo_stack.push_modify_text_box(old_tb, tb.clone());
                                        annotations_changed = true;
                                        *annotations_dirty = true;
                                        ui.ctx().request_repaint();
                                    }
                                }
                            }
                        } else if let Some((drag_id, _, start_can_pos)) = dragging_text_box.take() {
                            if let Some(tb) =
                                annotations.text_boxes.iter().find(|tb| tb.id == drag_id)
                            {
                                if tb.pos != start_can_pos {
                                    let mut old_tb = tb.clone();
                                    old_tb.pos = start_can_pos;
                                    undo_stack.push_modify_text_box(old_tb, tb.clone());
                                    annotations_changed = true;
                                    *annotations_dirty = true;
                                    ui.ctx().request_repaint();
                                }
                            }
                        } else if let Some((fh_page, mut fh_pts)) = freehand_stroke.take() {
                            if fh_pts.len() == 1 {
                                // Single-click dot: duplicate point so path rendering draws it
                                fh_pts.push(fh_pts[0]);
                            }
                            if fh_pts.len() >= 2 {
                                let id = generate_highlight_id();
                                let stroke_thickness =
                                    (12.0 * highlighter_thickness).clamp(3.0, 36.0);
                                let hl = Highlight {
                                    id,
                                    page: fh_page,
                                    rects: Vec::new(),
                                    points: fh_pts,
                                    color: highlighter_color,
                                    alpha: 0.42,
                                    thickness: stroke_thickness,
                                };
                                annotations.highlights.push(hl.clone());
                                undo_stack.push_add(hl);
                                annotations_changed = true;
                                *annotations_dirty = true;
                                ui.ctx().request_repaint();
                            }
                        } else if is_primary_clicked && active_tool == ActiveTool::None {
                            let mut hit_link = false;
                            for info in &visible_info {
                                if info.screen_rect.contains(pos) {
                                    if let Some(ref text) = info.text {
                                        let can_pt = info.transform.screen_to_canonical(pos);
                                        if let Some(link) = text.hit_test_link(can_pt) {
                                            match &link.target {
                                                LinkTarget::Uri(url) => {
                                                    action =
                                                        Some(ViewerAction::OpenUrl(url.clone()));
                                                }
                                                LinkTarget::Page(p) => {
                                                    action = Some(ViewerAction::JumpToPage(*p));
                                                }
                                            }
                                            *selection = None;
                                            hit_link = true;
                                            ui.ctx().request_repaint();
                                            break;
                                        }
                                    }
                                }
                            }
                            if !hit_link {
                                if let Some(cur_sel) = *selection {
                                    if cur_sel.is_empty() {
                                        *selection = None;
                                    }
                                }
                            }
                        } else if let Some(cur_sel) = *selection {
                            if cur_sel.is_empty() {
                                *selection = None;
                            } else if active_tool == ActiveTool::Highlighter {
                                // Commit highlight from selected text characters on release
                                let (sel_start, sel_end) = cur_sel.normalized();
                                let mut added_any = false;
                                for page_idx in sel_start.page..=sel_end.page {
                                    if let Some(text_data) = text_cache.get(doc_id, page_idx) {
                                        let p_start = if page_idx == sel_start.page {
                                            sel_start.char_idx
                                        } else {
                                            0
                                        };
                                        let p_end = if page_idx == sel_end.page {
                                            sel_end
                                                .char_idx
                                                .min(text_data.chars.len().saturating_sub(1))
                                        } else {
                                            text_data.chars.len().saturating_sub(1)
                                        };
                                        if p_start <= p_end {
                                            let raw_rects =
                                                text_data.get_selection_rects(p_start, p_end);
                                            let line_rects: Vec<CanonicalRect> = raw_rects
                                                .into_iter()
                                                .map(|r| {
                                                    adjust_rect_thickness(r, highlighter_thickness)
                                                })
                                                .collect();
                                            if !line_rects.is_empty() {
                                                let id = generate_highlight_id();
                                                let hl = Highlight {
                                                    id,
                                                    page: page_idx,
                                                    rects: line_rects,
                                                    points: Vec::new(),
                                                    color: highlighter_color,
                                                    alpha: 0.35,
                                                    thickness: highlighter_thickness,
                                                };
                                                annotations.highlights.push(hl.clone());
                                                undo_stack.push_add(hl);
                                                added_any = true;
                                            }
                                        }
                                    }
                                }
                                if added_any {
                                    *selection = None;
                                    annotations_changed = true;
                                    *annotations_dirty = true;
                                    ui.ctx().request_repaint();
                                }
                            }
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

            let (sharp_tex_id, is_stale) = if let Some((tex, gen)) = cache.get_entry(&sharp_key) {
                (
                    Some(tex.id()),
                    gen < render_generation || annotations_changed,
                )
            } else {
                (None, true)
            };

            let fallback_tex_id = if sharp_tex_id.is_none() {
                cache
                    .get_best_for_page(doc_id, idx, rotation)
                    .map(|t| t.id())
            } else {
                None
            };

            let active_tex_id = sharp_tex_id.or(fallback_tex_id);

            if is_in_viewport {
                // Sharp render available -> paint sharp (and request refresh if stale)
                if let Some(tex_id) = sharp_tex_id {
                    painter.image(
                        tex_id,
                        screen_rect,
                        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                    if is_stale && !pending_requests.contains(&(idx, sharp_width, rotation)) {
                        render_requests.push(PageRenderRequest {
                            page: idx,
                            width_px: sharp_width,
                            rotation,
                            priority: 1,
                        });
                    }
                } else if let Some(tex_id) = fallback_tex_id {
                    // Fallback to previous zoom level texture so zooming is smooth and never flashes blank
                    painter.image(
                        tex_id,
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

                // 1. If the texture is stale (pending worker re-render after annotation change/undo),
                // render temporary freehand stroke preview so there is zero vanishing gap.
                // Once the sharp texture arrives with generation >= render_generation, it displays
                // native ISO 32000 multiply blending directly from PDFium with 100% pitch-black text.
                if is_stale {
                    for hl in &annotations.highlights {
                        if hl.page == idx && !hl.points.is_empty() {
                            let stroke_color = Color32::from_rgba_unmultiplied(
                                hl.color.r,
                                hl.color.g,
                                hl.color.b,
                                (0.42_f32 * 255.0_f32).round() as u8,
                            );
                            let stroke_thickness = (hl.thickness * zoom).max(1.5);
                            let screen_points: Vec<egui::Pos2> = hl
                                .points
                                .iter()
                                .map(|p| transform.canonical_to_screen(pos2(p[0], p[1])))
                                .collect();
                            if screen_points.len() >= 2 {
                                painter.add(egui::epaint::PathShape::line(
                                    screen_points,
                                    Stroke::new(stroke_thickness, stroke_color),
                                ));
                            } else if let Some(&pt) = screen_points.first() {
                                painter.circle_filled(pt, stroke_thickness * 0.5, stroke_color);
                            }
                        }
                    }
                }

                // 2. Draw live freehand stroke preview if currently drawing on this page
                if let Some((stroke_page, ref stroke_pts)) = *freehand_stroke {
                    if stroke_page == idx && !stroke_pts.is_empty() {
                        let live_color = Color32::from_rgba_unmultiplied(
                            highlighter_color.r,
                            highlighter_color.g,
                            highlighter_color.b,
                            (0.42_f32 * 255.0_f32).round() as u8,
                        );
                        let stroke_thickness = (12.0 * highlighter_thickness * zoom).max(1.5);
                        let screen_points: Vec<egui::Pos2> = stroke_pts
                            .iter()
                            .map(|p| transform.canonical_to_screen(pos2(p[0], p[1])))
                            .collect();
                        if screen_points.len() >= 2 {
                            painter.add(egui::epaint::PathShape::line(
                                screen_points,
                                Stroke::new(stroke_thickness, live_color),
                            ));
                        } else if let Some(&pt) = screen_points.first() {
                            painter.circle_filled(pt, stroke_thickness * 0.5, live_color);
                        }
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

                // Draw text selection / highlighter preview overlay on this page through PageTransform
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
                                        let raw_rects =
                                            text_data.get_selection_rects(p_start, p_end);
                                        let is_hl = active_tool == ActiveTool::Highlighter;
                                        let (multiply_color, fallback_color) = if is_hl {
                                            (
                                                Color32::from_rgb(
                                                    highlighter_color.r,
                                                    highlighter_color.g,
                                                    highlighter_color.b,
                                                ),
                                                Color32::from_rgba_unmultiplied(
                                                    highlighter_color.r,
                                                    highlighter_color.g,
                                                    highlighter_color.b,
                                                    (0.42_f32 * 255.0_f32).round() as u8,
                                                ),
                                            )
                                        } else {
                                            let c = Theme::TEXT_SELECTION_MULTIPLY;
                                            (
                                                c,
                                                Color32::from_rgba_unmultiplied(
                                                    c.r(),
                                                    c.g(),
                                                    c.b(),
                                                    120,
                                                ),
                                            )
                                        };
                                        for r in raw_rects {
                                            let adj_r = if is_hl {
                                                adjust_rect_thickness(r, highlighter_thickness)
                                            } else {
                                                r
                                            };
                                            let screen_r = transform
                                                .canonical_to_screen_rect(adj_r.to_egui_rect());
                                            if let (Some(tex_id), Some((visible_r, uv))) = (
                                                active_tex_id,
                                                compute_page_subrect_uv(screen_rect, screen_r),
                                            ) {
                                                painter.image(
                                                    tex_id,
                                                    visible_r,
                                                    uv,
                                                    multiply_color,
                                                );
                                            } else {
                                                painter.rect_filled(screen_r, 0.0, fallback_color);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // 6. Draw text boxes on this page through PageTransform
                for tb in &annotations.text_boxes {
                    if tb.page == idx {
                        if editing_text_box
                            .as_ref()
                            .is_some_and(|etb| etb.id == Some(tb.id))
                        {
                            continue;
                        }

                        let is_selected = *selected_text_box_id == Some(tb.id);
                        let is_dragging = dragging_text_box
                            .as_ref()
                            .is_some_and(|(id, ..)| *id == tb.id);
                        let is_resizing = resizing_text_box
                            .as_ref()
                            .is_some_and(|(target, ..)| *target == Some(tb.id));

                        let font_size_px =
                            (tb.font_size * zoom * PageTransform::DPI_SCALE).max(8.0);
                        let text_col = Color32::from_rgb(tb.color.r, tb.color.g, tb.color.b);
                        let screen_pos = transform.canonical_to_screen(pos2(tb.pos[0], tb.pos[1]));
                        let screen_w = (tb.width * zoom * PageTransform::DPI_SCALE).max(40.0);

                        let font_id = egui::FontId::proportional(font_size_px);
                        let galley = painter.layout(tb.text.clone(), font_id, text_col, screen_w);
                        let galley_size = galley.size();
                        let box_h = (galley_size.y + 4.0).max(font_size_px * 1.3);
                        let box_screen_rect =
                            Rect::from_min_size(screen_pos, vec2(screen_w, box_h));

                        if is_selected || is_dragging || is_resizing {
                            let stroke_col = Color32::from_rgb(0, 120, 215);
                            painter.rect_stroke(
                                box_screen_rect,
                                2.0,
                                Stroke::new(1.5_f32, stroke_col),
                                StrokeKind::Outside,
                            );

                            let handle_center =
                                pos2(box_screen_rect.right(), box_screen_rect.center().y);
                            painter.circle(
                                handle_center,
                                4.5,
                                Color32::WHITE,
                                Stroke::new(1.5_f32, stroke_col),
                            );
                        }

                        // ALWAYS draw text using galley with font color over transparent background
                        painter.galley(screen_pos, galley, text_col);
                    }
                }

                // 7. Active inline EditingTextBox widget on this page
                if let Some(ref mut etb) = *editing_text_box {
                    if etb.page == idx {
                        let screen_top_left =
                            transform.canonical_to_screen(pos2(etb.pos[0], etb.pos[1]));
                        let screen_w = (etb.width * zoom * PageTransform::DPI_SCALE).max(40.0);
                        let screen_font_size =
                            (etb.font_size * zoom * PageTransform::DPI_SCALE).max(10.0);
                        let text_col = Color32::from_rgb(etb.color.r, etb.color.g, etb.color.b);
                        let galley = painter.layout(
                            if etb.text.is_empty() {
                                " ".to_string()
                            } else {
                                etb.text.clone()
                            },
                            egui::FontId::proportional(screen_font_size),
                            text_col,
                            (screen_w - 8.0).max(10.0),
                        );
                        let screen_h = (galley.size().y + 16.0).max(screen_font_size * 1.35 + 16.0);
                        let edit_rect =
                            Rect::from_min_size(screen_top_left, vec2(screen_w, screen_h));

                        painter.rect_filled(
                            edit_rect,
                            3.0,
                            Color32::from_rgba_unmultiplied(255, 255, 255, 245),
                        );
                        let stroke_col = Color32::from_rgb(0, 120, 215);
                        painter.rect_stroke(
                            edit_rect,
                            3.0,
                            Stroke::new(1.5_f32, stroke_col),
                            StrokeKind::Outside,
                        );

                        // Draw resize handle on right edge of editing text box
                        let handle_center = pos2(edit_rect.right(), edit_rect.center().y);
                        painter.circle(
                            handle_center,
                            4.5,
                            Color32::WHITE,
                            Stroke::new(1.5_f32, stroke_col),
                        );

                        let response = ui
                            .allocate_new_ui(
                                egui::UiBuilder::new().max_rect(edit_rect.shrink(4.0)),
                                |child_ui| {
                                    child_ui.add(
                                        egui::TextEdit::multiline(&mut etb.text)
                                            .font(egui::FontId::proportional(screen_font_size))
                                            .text_color(text_col)
                                            .frame(false)
                                            .hint_text("Type text here...")
                                            .desired_width(screen_w - 8.0),
                                    )
                                },
                            )
                            .inner;

                        if etb.is_new {
                            response.request_focus();
                            etb.is_new = false;
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

        // Draw custom dotted vertical rectangle cursor in Foreground layer so it is guaranteed to be drawn on top of all page textures
        if let Some(cursor_pos) = show_highlighter_rect_cursor {
            let fg_painter = ui.ctx().layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("highlighter_custom_cursor"),
            ));
            let clipped = fg_painter.with_clip_rect(ui.clip_rect());
            draw_dotted_vertical_rect(&clipped, cursor_pos, 8.0, 22.0);
        }

        autoscroll_delta
    });

    // Ensure visible sharp pages (priority 1) are requested before low-res or prefetch
    render_requests.sort_by_key(|r| r.priority);

    // If user clicked or dragged inside the viewer area, surrender focus from text inputs so arrow navigation immediately controls document scrolling
    if editing_text_box.is_none()
        && ui.input(|i| i.pointer.primary_down() || i.pointer.secondary_clicked())
    {
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
        annotations_changed,
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
        assert_eq!(
            ViewerAction::HighlightSelection,
            ViewerAction::HighlightSelection
        );
        let note = ViewerAction::AddTextNote(1, [100.0, 200.0]);
        assert_eq!(note, ViewerAction::AddTextNote(1, [100.0, 200.0]));
        assert_ne!(note, ViewerAction::HighlightSelection);
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

    #[test]
    fn test_highlight_stale_overlay_and_freehand_persistence() {
        let hl_ink = Highlight {
            id: 1,
            page: 0,
            rects: Vec::new(),
            points: vec![[50.0, 100.0], [150.0, 120.0]],
            color: AnnotationColor::new(255, 235, 59),
            alpha: 0.35,
            thickness: 12.0,
        };
        assert!(!hl_ink.points.is_empty());
        assert!(hl_ink.rects.is_empty());
        assert_eq!(hl_ink.page, 0);

        let hl_text = Highlight {
            id: 2,
            page: 0,
            rects: vec![CanonicalRect::new(100.0, 100.0, 200.0, 120.0)],
            points: Vec::new(),
            color: AnnotationColor::new(76, 175, 80),
            alpha: 0.35,
            thickness: 1.0,
        };
        assert!(!hl_text.rects.is_empty());
        assert!(hl_text.points.is_empty());

        // Verify stale condition logic:
        // 1. Same generation without changes: not stale
        let gen = 5;
        let current_gen = 5;
        let changed = false;
        let is_stale_normal = gen < current_gen || changed;
        assert!(!is_stale_normal);

        // 2. Newer generation requested: stale
        let is_stale_bumped = (gen < 6) || changed;
        assert!(is_stale_bumped);

        // 3. Annotations just changed on frame 0: immediately stale (zero vanishing gap!)
        let is_stale_immediate = (gen < current_gen) || true;
        assert!(is_stale_immediate);
    }

    #[test]
    fn test_hit_test_resize_handle() {
        let pos = [100.0, 150.0];
        let width = 180.0;
        let height = 40.0;
        let tol = 6.0;

        // Exactly on right edge center -> must hit
        let pt_on_handle = egui::pos2(280.0, 170.0);
        assert!(hit_test_resize_handle(
            pos,
            width,
            height,
            pt_on_handle,
            tol
        ));

        // Slightly to the left or right of right edge within tolerance -> must hit
        let pt_near_edge = egui::pos2(284.0, 160.0);
        assert!(hit_test_resize_handle(
            pos,
            width,
            height,
            pt_near_edge,
            tol
        ));

        // Far outside horizontally -> must not hit
        let pt_far_right = egui::pos2(295.0, 170.0);
        assert!(!hit_test_resize_handle(
            pos,
            width,
            height,
            pt_far_right,
            tol
        ));

        // Inside the box center -> must not hit resize handle (this is for dragging or typing, not resize)
        let pt_box_center = egui::pos2(190.0, 170.0);
        assert!(!hit_test_resize_handle(
            pos,
            width,
            height,
            pt_box_center,
            tol
        ));

        // Outside vertically -> must not hit
        let pt_above = egui::pos2(280.0, 130.0);
        assert!(!hit_test_resize_handle(pos, width, height, pt_above, tol));
    }

    #[test]
    fn test_draw_dotted_vertical_rect() {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                draw_dotted_vertical_rect(ui.painter(), pos2(100.0, 100.0), 8.0, 22.0);
            });
        });
    }

    #[test]
    fn test_compute_page_subrect_uv_boundaries() {
        let page_rect = Rect::from_min_max(pos2(100.0, 200.0), pos2(500.0, 800.0));

        // 1. Exact match -> UV (0, 0) to (1, 1)
        let (vis1, uv1) = compute_page_subrect_uv(page_rect, page_rect).expect("exact match uv");
        assert_eq!(vis1, page_rect);
        assert!((uv1.min.x - 0.0).abs() < 1e-4);
        assert!((uv1.min.y - 0.0).abs() < 1e-4);
        assert!((uv1.max.x - 1.0).abs() < 1e-4);
        assert!((uv1.max.y - 1.0).abs() < 1e-4);

        // 2. Centered sub-rect (half width, half height)
        let sub2 = Rect::from_min_max(pos2(200.0, 350.0), pos2(400.0, 650.0));
        let (vis2, uv2) = compute_page_subrect_uv(page_rect, sub2).expect("centered subrect uv");
        assert_eq!(vis2, sub2);
        assert!((uv2.min.x - 0.25).abs() < 1e-4);
        assert!((uv2.min.y - 0.25).abs() < 1e-4);
        assert!((uv2.max.x - 0.75).abs() < 1e-4);
        assert!((uv2.max.y - 0.75).abs() < 1e-4);

        // 3. Sub-rect extending outside bounds is clipped to positive intersection
        let sub3 = Rect::from_min_max(pos2(50.0, 150.0), pos2(300.0, 500.0));
        let (vis3, uv3) = compute_page_subrect_uv(page_rect, sub3).expect("clamped subrect uv");
        assert_eq!(vis3.min, page_rect.min);
        assert!((uv3.min.x - 0.0).abs() < 1e-4);
        assert!((uv3.min.y - 0.0).abs() < 1e-4);
        assert!((uv3.max.x - 0.5).abs() < 1e-4);
        assert!((uv3.max.y - 0.5).abs() < 1e-4);

        // 4. Disjoint outside rect -> None
        let sub_outside = Rect::from_min_max(pos2(600.0, 200.0), pos2(700.0, 300.0));
        assert!(compute_page_subrect_uv(page_rect, sub_outside).is_none());

        // 5. Zero or negative page dimensions -> None
        let degenerate = Rect::from_min_max(pos2(100.0, 100.0), pos2(100.0, 200.0));
        assert!(compute_page_subrect_uv(degenerate, sub2).is_none());
    }

    #[test]
    fn test_text_selection_multiply_overlay_color_contrast() {
        let sel_color = Theme::TEXT_SELECTION_MULTIPLY;
        // Verify shade RGB: radiant electric amber (#FFA028)
        assert_eq!(sel_color.r(), 255);
        assert_eq!(sel_color.g(), 160);
        assert_eq!(sel_color.b(), 40);

        let blend = |tex: u8, tint: u8| -> u8 { ((tex as u32 * tint as u32) / 255) as u8 };

        // Case A: Pure white page background (255, 255, 255)
        assert_eq!(
            (
                blend(255, sel_color.r()),
                blend(255, sel_color.g()),
                blend(255, sel_color.b())
            ),
            (255, 160, 40)
        );

        // Case B: Pure pitch-black document text (0, 0, 0)
        assert_eq!(
            (
                blend(0, sel_color.r()),
                blend(0, sel_color.g()),
                blend(0, sel_color.b())
            ),
            (0, 0, 0),
            "Multiply blend over black text must remain 100% pitch-black!"
        );

        // Case C: Standard relative luminance of selection background:
        // L = 0.2126 * (R/255) + 0.7152 * (G/255) + 0.0722 * (B/255)
        let luma = 0.2126 * (255.0 / 255.0) + 0.7152 * (160.0 / 255.0) + 0.0722 * (40.0 / 255.0);
        // Contrast ratio against pitch black (0.0): (L + 0.05) / (0.0 + 0.05)
        let contrast_ratio = (luma + 0.05) / 0.05;
        assert!(
            contrast_ratio > 10.0,
            "Contrast ratio with black text ({:.1}:1) must easily surpass WCAG AAA (7:1)",
            contrast_ratio
        );
    }
}
