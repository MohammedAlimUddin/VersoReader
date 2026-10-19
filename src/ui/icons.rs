//! Cohesive Lucide-style vector icon system and toolbar icon buttons (Polishing Phase 4).

use egui::{
    pos2, vec2, Color32, CornerRadius, Painter, Pos2, Rect, Response, Sense, Stroke, StrokeKind,
    Ui, Vec2,
};

use crate::ui::theme::Theme;

/// Renders a standardized interactive icon button with consistent hover, active,
/// pressed, and disabled states.
pub fn render_icon_button(
    ui: &mut Ui,
    size: Vec2,
    is_selected: bool,
    is_enabled: bool,
    tooltip: &str,
    paint_icon: impl FnOnce(&Painter, Rect, Color32),
) -> Response {
    let (rect, response) = ui.allocate_exact_size(
        size,
        if is_enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);

    let (bg_color, stroke_color, border_stroke, show_bottom_line) = if !is_enabled {
        (
            Color32::TRANSPARENT,
            Color32::from_rgb(90, 90, 95),
            Stroke::NONE,
            false,
        )
    } else if is_selected {
        let bg = if response.hovered() {
            Color32::from_rgba_unmultiplied(255, 255, 255, 20)
        } else {
            Color32::from_rgba_unmultiplied(255, 255, 255, 10)
        };
        (bg, Color32::WHITE, Stroke::NONE, true)
    } else if response.is_pointer_button_down_on() {
        (
            Color32::from_rgba_unmultiplied(255, 255, 255, 22),
            Color32::WHITE,
            Stroke::NONE,
            false,
        )
    } else if response.hovered() {
        (Theme::BG_PANEL_HOVER, Color32::WHITE, Stroke::NONE, false)
    } else {
        (
            Color32::TRANSPARENT,
            Theme::TEXT_SECONDARY,
            Stroke::NONE,
            false,
        )
    };

    let p = ui.painter();
    if bg_color != Color32::TRANSPARENT {
        p.rect_filled(rect, Theme::RADIUS, bg_color);
    }
    if border_stroke != Stroke::NONE {
        p.rect_stroke(rect, Theme::RADIUS, border_stroke, StrokeKind::Inside);
    }

    if show_bottom_line {
        let line_rect = Rect::from_min_max(
            pos2(rect.left() + 4.0, rect.bottom() - 2.5),
            pos2(rect.right() - 4.0, rect.bottom()),
        );
        p.rect_filled(line_rect, CornerRadius::same(1), Theme::TAB_INDICATOR);
    }

    let icon_rect = Rect::from_center_size(rect.center(), vec2(19.0, 19.0));
    paint_icon(p, icon_rect, stroke_color);

    if !tooltip.is_empty() {
        response.on_hover_text(tooltip)
    } else {
        response
    }
}

/// Paints the sidebar/outline panel toggle icon (Lucide `panel-left`).
pub fn paint_sidebar(painter: &Painter, rect: Rect, stroke_color: Color32, is_open: bool) {
    let stroke = Stroke::new(1.4_f32, stroke_color);
    let box_rect = Rect::from_center_size(rect.center(), vec2(15.0, 13.0));
    painter.rect_stroke(box_rect, CornerRadius::same(2), stroke, StrokeKind::Inside);

    let div_x = box_rect.left() + 4.5;
    painter.line_segment(
        [
            pos2(div_x, box_rect.top() + 1.4),
            pos2(div_x, box_rect.bottom() - 1.4),
        ],
        stroke,
    );

    if is_open {
        let panel_rect = Rect::from_min_max(box_rect.left_top(), pos2(div_x, box_rect.bottom()));
        let left_radius = CornerRadius {
            nw: 2,
            ne: 0,
            sw: 2,
            se: 0,
        };
        painter.rect_filled(
            panel_rect,
            left_radius,
            Color32::from_rgba_unmultiplied(0, 120, 212, 120),
        );
    }
}

/// Paints the zoom out icon (Lucide `minus`).
pub fn paint_zoom_out(painter: &Painter, rect: Rect, stroke_color: Color32) {
    let c = rect.center();
    let hw = 5.0_f32;
    painter.line_segment(
        [pos2(c.x - hw, c.y), pos2(c.x + hw, c.y)],
        Stroke::new(1.8_f32, stroke_color),
    );
}

/// Paints the zoom in icon (Lucide `plus`).
pub fn paint_zoom_in(painter: &Painter, rect: Rect, stroke_color: Color32) {
    let c = rect.center();
    let hw = 5.0_f32;
    let stroke = Stroke::new(1.8_f32, stroke_color);
    painter.line_segment([pos2(c.x - hw, c.y), pos2(c.x + hw, c.y)], stroke);
    painter.line_segment([pos2(c.x, c.y - hw), pos2(c.x, c.y + hw)], stroke);
}

/// Paints the Fit Width icon (Lucide `fold-horizontal` / margin expand).
pub fn paint_fit_width(painter: &Painter, rect: Rect, stroke_color: Color32) {
    let c = rect.center();
    let stroke = Stroke::new(1.4_f32, stroke_color);
    let arr_stroke = Stroke::new(1.3_f32, stroke_color);

    // Left and right boundary bars
    painter.line_segment(
        [pos2(c.x - 7.0, c.y - 6.0), pos2(c.x - 7.0, c.y + 6.0)],
        stroke,
    );
    painter.line_segment(
        [pos2(c.x + 7.0, c.y - 6.0), pos2(c.x + 7.0, c.y + 6.0)],
        stroke,
    );

    // Horizontal arrow connecting them
    painter.line_segment([pos2(c.x - 4.5, c.y), pos2(c.x + 4.5, c.y)], stroke);

    // Left arrowhead
    painter.line_segment(
        [pos2(c.x - 2.5, c.y - 2.2), pos2(c.x - 4.5, c.y)],
        arr_stroke,
    );
    painter.line_segment(
        [pos2(c.x - 2.5, c.y + 2.2), pos2(c.x - 4.5, c.y)],
        arr_stroke,
    );

    // Right arrowhead
    painter.line_segment(
        [pos2(c.x + 2.5, c.y - 2.2), pos2(c.x + 4.5, c.y)],
        arr_stroke,
    );
    painter.line_segment(
        [pos2(c.x + 2.5, c.y + 2.2), pos2(c.x + 4.5, c.y)],
        arr_stroke,
    );
}

/// Paints the Fit Page icon (Lucide `file-text` / page aspect frame).
pub fn paint_fit_page(painter: &Painter, rect: Rect, stroke_color: Color32) {
    let c = rect.center();
    let stroke = Stroke::new(1.4_f32, stroke_color);

    // Top and bottom boundary bars
    painter.line_segment(
        [pos2(c.x - 6.5, c.y - 7.0), pos2(c.x + 6.5, c.y - 7.0)],
        stroke,
    );
    painter.line_segment(
        [pos2(c.x - 6.5, c.y + 7.0), pos2(c.x + 6.5, c.y + 7.0)],
        stroke,
    );

    // Centered page outline
    let page_rect = Rect::from_center_size(c, vec2(8.5, 10.5));
    painter.rect_stroke(page_rect, CornerRadius::same(1), stroke, StrokeKind::Inside);
}

/// Paints the 90° clockwise rotation icon (Lucide `rotate-cw`).
pub fn paint_rotate(painter: &Painter, rect: Rect, stroke_color: Color32) {
    let map = |x: f32, y: f32| -> Pos2 {
        pos2(
            rect.left() + (x / 24.0) * rect.width(),
            rect.top() + (y / 24.0) * rect.height(),
        )
    };
    let stroke = Stroke::new(1.4_f32, stroke_color);

    // Arrowhead / direction pin at top-right pointing clockwise
    // Lucide path: M21 3v5h-5
    painter.line_segment([map(21.0, 3.0), map(21.0, 8.0)], stroke);
    painter.line_segment([map(21.0, 8.0), map(16.0, 8.0)], stroke);

    // Lucide arc: M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8
    let c = map(12.0, 12.0);
    let r = 9.0 / 24.0 * rect.width();
    let start_angle = 0.0_f32;
    let end_angle = 1.74 * std::f32::consts::PI;
    let steps = 18;
    let mut points: Vec<Pos2> = (0..=steps)
        .map(|i| {
            let t = i as f32 / steps as f32;
            let angle = start_angle + t * end_angle;
            pos2(c.x + r * angle.cos(), c.y + r * angle.sin())
        })
        .collect();
    points.push(map(21.0, 8.0));
    painter.add(egui::Shape::line(points, stroke));
}

/// Paints the Highlighter icon reflecting the active highlight color (Lucide `highlighter`).
pub fn paint_highlighter(
    painter: &Painter,
    rect: Rect,
    stroke_color: Color32,
    highlight_color: Color32,
) {
    let map = |x: f32, y: f32| -> Pos2 {
        pos2(
            rect.left() + (x / 24.0) * rect.width(),
            rect.top() + (y / 24.0) * rect.height(),
        )
    };
    let stroke = Stroke::new(1.5_f32, stroke_color);

    // 1. Fill only the chisel nib polygon with the selected highlight color (Stroke::NONE so no dividing line is drawn)
    let nib_fill_pts = vec![
        map(9.0, 11.0),
        map(3.0, 17.0),
        map(3.0, 20.0),
        map(12.0, 20.0),
        map(15.0, 17.0),
    ];
    painter.add(egui::Shape::convex_polygon(
        nib_fill_pts,
        highlight_color,
        Stroke::NONE,
    ));

    // 2. Lucide path 1 (chisel tip outline): m9 11-6 6v3h9l3-3
    let path1_pts = vec![
        map(9.0, 11.0),
        map(3.0, 17.0),
        map(3.0, 20.0),
        map(12.0, 20.0),
        map(15.0, 17.0),
    ];
    painter.add(egui::Shape::line(path1_pts, stroke));

    // 3. Lucide path 2 (marker body outline): m22 12-4.6 4.6a2 2 0 0 1-2.8 0l-5.2-5.2a2 2 0 0 1 0-2.8L14 4
    let path2_pts = vec![
        map(15.0, 17.0),
        map(17.4, 16.6),
        map(22.0, 12.0),
        map(14.0, 4.0),
        map(9.4, 8.6),
        map(9.0, 11.0),
    ];
    painter.add(egui::Shape::line(path2_pts, stroke));
}
/// Paints the Eraser icon (Lucide `eraser`).
pub fn paint_eraser(painter: &Painter, rect: Rect, stroke_color: Color32, is_active: bool) {
    let map = |x: f32, y: f32| -> Pos2 {
        pos2(
            rect.left() + (x / 24.0) * rect.width(),
            rect.top() + (y / 24.0) * rect.height(),
        )
    };
    let stroke = Stroke::new(1.4_f32, stroke_color);

    // Sample a quadratic Bézier (approximates the r=2 arcs) and append to `pts`.
    let quad = |pts: &mut Vec<Pos2>, p0: (f32, f32), c: (f32, f32), p1: (f32, f32)| {
        const N: usize = 8;
        for i in 1..=N {
            let t = i as f32 / N as f32;
            let u = 1.0 - t;
            pts.push(map(
                u * u * p0.0 + 2.0 * u * t * c.0 + t * t * p1.0,
                u * u * p0.1 + 2.0 * u * t * c.1 + t * t * p1.1,
            ));
        }
    };

    // Body outline: M8 21 → … → L12.834 21
    let mut body = vec![map(8.0, 21.0)];
    quad(&mut body, (8.0, 21.0), (7.173, 21.0), (6.586, 20.413)); // bottom-left corner
    body.push(map(2.586, 16.414));
    quad(&mut body, (2.586, 16.414), (1.414, 15.0), (2.586, 13.586)); // left end
    body.push(map(12.586, 3.586));
    quad(&mut body, (12.586, 3.586), (14.0, 2.414), (15.415, 3.586)); // top
    body.push(map(21.414, 9.586));
    quad(&mut body, (21.414, 9.586), (22.586, 11.0), (21.414, 12.414)); // right end
    body.push(map(12.834, 21.0));

    if is_active {
        painter.add(egui::Shape::convex_polygon(
            body.clone(),
            Color32::from_rgba_unmultiplied(255, 255, 255, 20),
            Stroke::NONE,
        ));
    }

    painter.add(egui::Shape::closed_line(body, stroke));

    // Base tail: (12.834,21) → (21,21)
    painter.line_segment([map(12.834, 21.0), map(21.0, 21.0)], stroke);

    // Sleeve divider: m5.082 11.09 8.828 8.828
    painter.line_segment([map(5.082, 11.09), map(13.91, 19.918)], stroke);
}

/// Paints the Text Tool icon (Lucide `type` serif "T").
pub fn paint_text_tool(painter: &Painter, rect: Rect, stroke_color: Color32, _is_active: bool) {
    let c = rect.center();
    let stroke = Stroke::new(1.6_f32, stroke_color);
    let serif_stroke = Stroke::new(1.3_f32, stroke_color);

    // Top horizontal crossbar
    painter.line_segment(
        [pos2(c.x - 5.5, c.y - 5.5), pos2(c.x + 5.5, c.y - 5.5)],
        stroke,
    );

    // Serifs hanging from top crossbar
    painter.line_segment(
        [pos2(c.x - 5.5, c.y - 5.5), pos2(c.x - 5.5, c.y - 3.2)],
        serif_stroke,
    );
    painter.line_segment(
        [pos2(c.x + 5.5, c.y - 5.5), pos2(c.x + 5.5, c.y - 3.2)],
        serif_stroke,
    );

    // Vertical stem
    painter.line_segment([pos2(c.x, c.y - 5.5), pos2(c.x, c.y + 5.5)], stroke);

    // Bottom base serif
    painter.line_segment(
        [pos2(c.x - 3.2, c.y + 5.5), pos2(c.x + 3.2, c.y + 5.5)],
        serif_stroke,
    );
}

/// Paints the Save icon (Lucide `save` floppy disk) with optional unsaved changes indicator dot.
pub fn paint_save(painter: &Painter, rect: Rect, stroke_color: Color32, has_unsaved_changes: bool) {
    let c = rect.center();
    let stroke = Stroke::new(1.3_f32, stroke_color);

    // Outer 3.5" disk boundary with top-right beveled write-protect notch
    let disk_pts = vec![
        pos2(c.x - 6.0, c.y - 6.0),
        pos2(c.x + 3.5, c.y - 6.0),
        pos2(c.x + 6.0, c.y - 3.5),
        pos2(c.x + 6.0, c.y + 6.0),
        pos2(c.x - 6.0, c.y + 6.0),
    ];
    painter.add(egui::Shape::closed_line(disk_pts, stroke));

    // Top metal shutter cutout
    let shutter_rect = Rect::from_min_max(pos2(c.x - 3.5, c.y - 6.0), pos2(c.x + 1.5, c.y - 2.5));
    painter.rect_stroke(
        shutter_rect,
        CornerRadius::ZERO,
        Stroke::new(1.1_f32, stroke_color),
        StrokeKind::Inside,
    );

    // Bottom paper label window
    let label_rect = Rect::from_min_max(pos2(c.x - 3.5, c.y + 1.0), pos2(c.x + 3.5, c.y + 6.0));
    painter.rect_stroke(
        label_rect,
        CornerRadius::ZERO,
        Stroke::new(1.1_f32, stroke_color),
        StrokeKind::Inside,
    );

    // Subtle dirty / unsaved changes indicator dot at top-right
    if has_unsaved_changes {
        painter.circle_filled(pos2(c.x + 6.5, c.y - 6.5), 2.8, Theme::TAB_INDICATOR);
    }
}

/// Paints the Fullscreen icon (Lucide `maximize` / `minimize`).
pub fn paint_fullscreen(painter: &Painter, rect: Rect, stroke_color: Color32, is_fullscreen: bool) {
    let c = rect.center();
    let stroke = Stroke::new(1.4_f32, stroke_color);

    if !is_fullscreen {
        // Expand corners pointing outwards
        // Top-left
        painter.line_segment(
            [pos2(c.x - 6.5, c.y - 3.5), pos2(c.x - 6.5, c.y - 6.5)],
            stroke,
        );
        painter.line_segment(
            [pos2(c.x - 6.5, c.y - 6.5), pos2(c.x - 3.5, c.y - 6.5)],
            stroke,
        );

        // Top-right
        painter.line_segment(
            [pos2(c.x + 3.5, c.y - 6.5), pos2(c.x + 6.5, c.y - 6.5)],
            stroke,
        );
        painter.line_segment(
            [pos2(c.x + 6.5, c.y - 6.5), pos2(c.x + 6.5, c.y - 3.5)],
            stroke,
        );

        // Bottom-left
        painter.line_segment(
            [pos2(c.x - 6.5, c.y + 3.5), pos2(c.x - 6.5, c.y + 6.5)],
            stroke,
        );
        painter.line_segment(
            [pos2(c.x - 6.5, c.y + 6.5), pos2(c.x - 3.5, c.y + 6.5)],
            stroke,
        );

        // Bottom-right
        painter.line_segment(
            [pos2(c.x + 3.5, c.y + 6.5), pos2(c.x + 6.5, c.y + 6.5)],
            stroke,
        );
        painter.line_segment(
            [pos2(c.x + 6.5, c.y + 6.5), pos2(c.x + 6.5, c.y + 3.5)],
            stroke,
        );
    } else {
        // Shrink corners pointing inwards
        // Top-left
        painter.line_segment(
            [pos2(c.x - 2.0, c.y - 5.5), pos2(c.x - 2.0, c.y - 2.0)],
            stroke,
        );
        painter.line_segment(
            [pos2(c.x - 2.0, c.y - 2.0), pos2(c.x - 5.5, c.y - 2.0)],
            stroke,
        );

        // Top-right
        painter.line_segment(
            [pos2(c.x + 2.0, c.y - 5.5), pos2(c.x + 2.0, c.y - 2.0)],
            stroke,
        );
        painter.line_segment(
            [pos2(c.x + 2.0, c.y - 2.0), pos2(c.x + 5.5, c.y - 2.0)],
            stroke,
        );

        // Bottom-left
        painter.line_segment(
            [pos2(c.x - 2.0, c.y + 5.5), pos2(c.x - 2.0, c.y + 2.0)],
            stroke,
        );
        painter.line_segment(
            [pos2(c.x - 2.0, c.y + 2.0), pos2(c.x - 5.5, c.y + 2.0)],
            stroke,
        );

        // Bottom-right
        painter.line_segment(
            [pos2(c.x + 2.0, c.y + 5.5), pos2(c.x + 2.0, c.y + 2.0)],
            stroke,
        );
        painter.line_segment(
            [pos2(c.x + 2.0, c.y + 2.0), pos2(c.x + 5.5, c.y + 2.0)],
            stroke,
        );
    }
}

/// Paints the Search / Find icon (Lucide `search`).
pub fn paint_search(painter: &Painter, rect: Rect, stroke_color: Color32) {
    let c = rect.center();
    let lens_center = pos2(c.x - 1.5, c.y - 1.5);
    painter.circle_stroke(lens_center, 4.5, Stroke::new(1.5_f32, stroke_color));
    painter.line_segment(
        [pos2(c.x + 2.0, c.y + 2.0), pos2(c.x + 6.0, c.y + 6.0)],
        Stroke::new(1.8_f32, stroke_color),
    );
}

/// Paints a downwards chevron (Lucide `chevron-down`).
pub fn paint_chevron_down(painter: &Painter, rect: Rect, stroke_color: Color32) {
    let c = rect.center();
    let stroke = Stroke::new(1.4_f32, stroke_color);
    painter.line_segment([pos2(c.x - 3.5, c.y - 1.8), pos2(c.x, c.y + 1.8)], stroke);
    painter.line_segment([pos2(c.x, c.y + 1.8), pos2(c.x + 3.5, c.y - 1.8)], stroke);
}

/// Paints an upwards chevron (Lucide `chevron-up`).
pub fn paint_chevron_up(painter: &Painter, rect: Rect, stroke_color: Color32) {
    let c = rect.center();
    let stroke = Stroke::new(1.4_f32, stroke_color);
    painter.line_segment([pos2(c.x - 3.5, c.y + 1.8), pos2(c.x, c.y - 1.8)], stroke);
    painter.line_segment([pos2(c.x, c.y - 1.8), pos2(c.x + 3.5, c.y + 1.8)], stroke);
}

/// Paints a vector cross (Lucide `x` close icon).
pub fn paint_close(painter: &Painter, rect: Rect, stroke_color: Color32) {
    let c = rect.center();
    let half = 3.5_f32;
    let stroke = Stroke::new(1.3_f32, stroke_color);
    painter.line_segment(
        [pos2(c.x - half, c.y - half), pos2(c.x + half, c.y + half)],
        stroke,
    );
    painter.line_segment(
        [pos2(c.x + half, c.y - half), pos2(c.x - half, c.y + half)],
        stroke,
    );
}

/// Paints the folder open icon (Lucide `folder-open`).
pub fn paint_folder_open(painter: &Painter, rect: Rect, stroke_color: Color32) {
    let c = rect.center();
    let stroke = Stroke::new(1.3_f32, stroke_color);

    // Back tab of folder
    painter.line_segment(
        [pos2(c.x - 6.5, c.y - 3.0), pos2(c.x - 6.5, c.y - 5.0)],
        stroke,
    );
    painter.line_segment(
        [pos2(c.x - 6.5, c.y - 5.0), pos2(c.x - 2.5, c.y - 5.0)],
        stroke,
    );
    painter.line_segment(
        [pos2(c.x - 2.5, c.y - 5.0), pos2(c.x - 1.0, c.y - 3.0)],
        stroke,
    );
    painter.line_segment(
        [pos2(c.x - 1.0, c.y - 3.0), pos2(c.x + 5.5, c.y - 3.0)],
        stroke,
    );
    painter.line_segment(
        [pos2(c.x + 5.5, c.y - 3.0), pos2(c.x + 5.5, c.y + 5.0)],
        stroke,
    );

    // Front open flap
    let flap = vec![
        pos2(c.x - 6.5, c.y + 5.0),
        pos2(c.x - 4.5, c.y - 0.5),
        pos2(c.x + 6.5, c.y - 0.5),
        pos2(c.x + 4.5, c.y + 5.0),
    ];
    painter.add(egui::Shape::closed_line(flap, stroke));
}

/// Paints the undo icon (Lucide `undo-2`).
pub fn paint_undo(painter: &Painter, rect: Rect, stroke_color: Color32) {
    let map = |x: f32, y: f32| -> Pos2 {
        pos2(
            rect.left() + (x / 24.0) * rect.width(),
            rect.top() + (y / 24.0) * rect.height(),
        )
    };
    let stroke = Stroke::new(1.4_f32, stroke_color);

    // Direction pin / arrowhead: crisp symmetric '<' chevron pointing left to (4, 9)
    // Lucide path: M9 14 4 9l5-5
    painter.line_segment([map(9.0, 14.0), map(4.0, 9.0)], stroke);
    painter.line_segment([map(9.0, 4.0), map(4.0, 9.0)], stroke);

    // Loop stem: M4 9h10.5a5.5 5.5 0 0 1 5.5 5.5a5.5 5.5 0 0 1-5.5 5.5H11
    let r = 5.5 / 24.0 * rect.width();
    let arc_center = map(14.5, 14.5);
    let mut stem_pts = vec![map(4.0, 9.0), map(14.5, 9.0)];
    let steps = 10;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let angle = -std::f32::consts::FRAC_PI_2 + t * std::f32::consts::PI;
        stem_pts.push(pos2(
            arc_center.x + r * angle.cos(),
            arc_center.y + r * angle.sin(),
        ));
    }
    stem_pts.push(map(11.0, 20.0));
    painter.add(egui::Shape::line(stem_pts, stroke));
}

/// Paints the redo icon (Lucide `redo-2`).
pub fn paint_redo(painter: &Painter, rect: Rect, stroke_color: Color32) {
    let map = |x: f32, y: f32| -> Pos2 {
        pos2(
            rect.left() + (x / 24.0) * rect.width(),
            rect.top() + (y / 24.0) * rect.height(),
        )
    };
    let stroke = Stroke::new(1.4_f32, stroke_color);

    // Direction pin / arrowhead: crisp symmetric '>' chevron pointing right to (20, 9)
    // Lucide path: m15 14 5-5-5-5
    painter.line_segment([map(15.0, 14.0), map(20.0, 9.0)], stroke);
    painter.line_segment([map(15.0, 4.0), map(20.0, 9.0)], stroke);

    // Loop stem: M20 9H9.5A5.5 5.5 0 0 0 4 14.5v0A5.5 5.5 0 0 0 9.5 20H13
    let r = 5.5 / 24.0 * rect.width();
    let arc_center = map(9.5, 14.5);
    let mut stem_pts = vec![map(20.0, 9.0), map(9.5, 9.0)];
    let steps = 10;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let angle = -std::f32::consts::FRAC_PI_2 - t * std::f32::consts::PI;
        stem_pts.push(pos2(
            arc_center.x + r * angle.cos(),
            arc_center.y + r * angle.sin(),
        ));
    }
    stem_pts.push(map(13.0, 20.0));
    painter.add(egui::Shape::line(stem_pts, stroke));
}

/// Paints the navigation back arrow (Lucide `arrow-left`).
pub fn paint_nav_back(painter: &Painter, rect: Rect, stroke_color: Color32) {
    let map = |x: f32, y: f32| -> Pos2 {
        pos2(
            rect.left() + (x / 24.0) * rect.width(),
            rect.top() + (y / 24.0) * rect.height(),
        )
    };
    let stroke = Stroke::new(1.4_f32, stroke_color);
    painter.line_segment([map(12.0, 19.0), map(5.0, 12.0)], stroke);
    painter.line_segment([map(12.0, 5.0), map(5.0, 12.0)], stroke);
    painter.line_segment([map(5.0, 12.0), map(19.0, 12.0)], stroke);
}

/// Paints the navigation forward arrow (Lucide `arrow-right`).
pub fn paint_nav_forward(painter: &Painter, rect: Rect, stroke_color: Color32) {
    let map = |x: f32, y: f32| -> Pos2 {
        pos2(
            rect.left() + (x / 24.0) * rect.width(),
            rect.top() + (y / 24.0) * rect.height(),
        )
    };
    let stroke = Stroke::new(1.4_f32, stroke_color);
    painter.line_segment([map(12.0, 5.0), map(19.0, 12.0)], stroke);
    painter.line_segment([map(12.0, 19.0), map(19.0, 12.0)], stroke);
    painter.line_segment([map(5.0, 12.0), map(19.0, 12.0)], stroke);
}

/// Paints the Settings / Preferences gear icon (Lucide `settings`).
pub fn paint_gear(painter: &Painter, rect: Rect, stroke_color: Color32) {
    let c = rect.center();
    let stroke = Stroke::new(1.35_f32, stroke_color);

    // Inner axle hole
    painter.circle_stroke(c, 2.6, stroke);

    // 6 cogs around perimeter
    let inner_r = 4.8_f32;
    let outer_r = 7.0_f32;
    let mut cog_pts = Vec::with_capacity(25);
    for i in 0..6 {
        let base_angle = (i as f32) * std::f32::consts::PI / 3.0;
        let half_cog = 0.22_f32;
        let half_valley = 0.30_f32;

        let a0 = base_angle - half_cog;
        let a1 = base_angle + half_cog;
        let a2 = base_angle + half_valley;
        let next_a0 = base_angle + std::f32::consts::PI / 3.0 - half_cog;

        cog_pts.push(pos2(c.x + outer_r * a0.cos(), c.y + outer_r * a0.sin()));
        cog_pts.push(pos2(c.x + outer_r * a1.cos(), c.y + outer_r * a1.sin()));
        cog_pts.push(pos2(c.x + inner_r * a2.cos(), c.y + inner_r * a2.sin()));
        cog_pts.push(pos2(
            c.x + inner_r * next_a0.cos(),
            c.y + inner_r * next_a0.sin(),
        ));
    }
    painter.add(egui::Shape::closed_line(cog_pts, stroke));
}

/// Paints the Verso Reader brand logo mark.
pub fn paint_verso_logo(painter: &Painter, rect: Rect) {
    let c = rect.center();
    let box_r = Rect::from_center_size(c, vec2(18.0, 18.0));
    painter.rect_filled(box_r, CornerRadius::same(4), Color32::from_rgb(0, 120, 212));

    let v_stroke = Stroke::new(1.8_f32, Color32::WHITE);
    painter.line_segment([pos2(c.x - 5.5, c.y - 4.0), pos2(c.x, c.y + 4.5)], v_stroke);
    painter.line_segment([pos2(c.x, c.y + 4.5), pos2(c.x + 5.5, c.y - 4.0)], v_stroke);
    let thin_stroke = Stroke::new(
        1.1_f32,
        Color32::from_rgba_premultiplied(255, 255, 255, 180),
    );
    painter.line_segment(
        [pos2(c.x - 5.5, c.y - 4.0), pos2(c.x - 1.0, c.y - 2.5)],
        thin_stroke,
    );
    painter.line_segment(
        [pos2(c.x + 1.0, c.y - 2.5), pos2(c.x + 5.5, c.y - 4.0)],
        thin_stroke,
    );
}

/// Variants for Adobe Spectrum button styling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdobeButtonVariant {
    Primary,
    Secondary,
    Destructive,
}

/// Renders a modern Adobe Spectrum-style button with variant support:
/// Rounded corners, transparent/outline base state, filling with color on hover.
pub fn render_adobe_button_variant(
    ui: &mut egui::Ui,
    text: &str,
    variant: AdobeButtonVariant,
) -> egui::Response {
    let font_id = egui::FontId::proportional(13.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font_id, Color32::WHITE);
    let padding = vec2(18.0, 6.0);
    let desired_size = vec2(galley.size().x + padding.x * 2.0, 32.0);
    let (rect, response) = ui.allocate_exact_size(desired_size, Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);

    let is_hovered = response.hovered();
    let is_active = response.is_pointer_button_down_on();

    let radius = CornerRadius::same(6);
    let accent_blue = Color32::from_rgb(0, 120, 212); // Modern Adobe / Fluent accent blue
    let accent_red = Color32::from_rgb(215, 55, 55); // Adobe destructive red

    let (bg_color, stroke_color, text_color) = match variant {
        AdobeButtonVariant::Primary => {
            if is_active {
                (
                    Color32::from_rgb(0, 90, 170),
                    Color32::from_rgb(0, 90, 170),
                    Color32::WHITE,
                )
            } else if is_hovered {
                (accent_blue, accent_blue, Color32::WHITE)
            } else {
                (
                    Color32::TRANSPARENT,
                    accent_blue,
                    Color32::from_rgb(90, 165, 250),
                )
            }
        }
        AdobeButtonVariant::Secondary => {
            if is_active {
                (
                    Color32::from_rgb(0, 90, 170),
                    Color32::from_rgb(0, 90, 170),
                    Color32::WHITE,
                )
            } else if is_hovered {
                (accent_blue, accent_blue, Color32::WHITE)
            } else {
                (
                    Color32::TRANSPARENT,
                    Color32::from_rgb(110, 110, 115),
                    Color32::from_rgb(220, 220, 225),
                )
            }
        }
        AdobeButtonVariant::Destructive => {
            if is_active {
                (
                    Color32::from_rgb(160, 40, 40),
                    Color32::from_rgb(160, 40, 40),
                    Color32::WHITE,
                )
            } else if is_hovered {
                (accent_red, accent_red, Color32::WHITE)
            } else {
                (
                    Color32::TRANSPARENT,
                    accent_red,
                    Color32::from_rgb(250, 120, 120),
                )
            }
        }
    };

    if bg_color != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, radius, bg_color);
    }
    ui.painter().rect_stroke(
        rect,
        radius,
        Stroke::new(1.4_f32, stroke_color),
        StrokeKind::Inside,
    );

    let text_pos = pos2(
        rect.center().x - galley.size().x / 2.0,
        rect.center().y - galley.size().y / 2.0,
    );
    ui.painter().text(
        text_pos,
        egui::Align2::LEFT_TOP,
        text,
        egui::FontId::proportional(13.0),
        text_color,
    );

    response
}

/// Renders a modern Adobe Spectrum-style button:
/// Rounded corners, transparent/outline base state, filling with blue on hover.
pub fn render_adobe_button(ui: &mut egui::Ui, text: &str, is_primary: bool) -> egui::Response {
    let variant = if is_primary {
        AdobeButtonVariant::Primary
    } else {
        AdobeButtonVariant::Secondary
    };
    render_adobe_button_variant(ui, text, variant)
}

/// Renders a modern, enlarged (22x22 pt) Adobe-style checkbox:
/// Rounded corners, subtle outline in dark mode, vibrant accent blue fill with crisp checkmark when checked.
pub fn render_adobe_checkbox(ui: &mut egui::Ui, value: &mut bool) -> egui::Response {
    let size = vec2(22.0, 22.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    response = response.on_hover_cursor(egui::CursorIcon::PointingHand);

    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }

    let is_hovered = response.hovered();
    let is_active = response.is_pointer_button_down_on();
    let is_checked = *value;

    let painter = ui.painter();
    let radius = CornerRadius::same(5);

    let (bg_color, stroke_color) = if is_checked {
        if is_active {
            (Color32::from_rgb(0, 90, 170), Color32::from_rgb(0, 90, 170))
        } else if is_hovered {
            (
                Color32::from_rgb(30, 140, 235),
                Color32::from_rgb(30, 140, 235),
            )
        } else {
            (
                Color32::from_rgb(0, 120, 212),
                Color32::from_rgb(0, 120, 212),
            )
        }
    } else if is_active {
        (
            Color32::from_rgb(50, 50, 58),
            Color32::from_rgb(130, 130, 145),
        )
    } else if is_hovered {
        (
            Color32::from_rgb(45, 45, 52),
            Color32::from_rgb(115, 115, 128),
        )
    } else {
        (Color32::from_rgb(32, 32, 36), Color32::from_rgb(80, 80, 90))
    };

    painter.rect(
        rect,
        radius,
        bg_color,
        Stroke::new(1.4_f32, stroke_color),
        StrokeKind::Inside,
    );

    if is_checked {
        // Crisp checkmark path centered within the 22x22 square
        let p1 = pos2(rect.left() + 5.5, rect.center().y);
        let p2 = pos2(rect.left() + 9.0, rect.bottom() - 6.0);
        let p3 = pos2(rect.right() - 5.5, rect.top() + 6.0);
        let stroke = Stroke::new(2.2_f32, Color32::WHITE);
        painter.line_segment([p1, p2], stroke);
        painter.line_segment([p2, p3], stroke);
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_icons_render_shapes_without_panic() {
        let dummy_rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(20.0, 20.0));
        assert_eq!(dummy_rect.center(), pos2(10.0, 10.0));
    }
}
