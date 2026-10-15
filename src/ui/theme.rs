use egui::{Color32, CornerRadius, Margin, Stroke, Vec2};

/// Global design system tokens and visual theme for Verso Reader.
/// Inspired by the clean, minimal aesthetic of Microsoft Edge PDF Reader.
pub struct Theme;

#[allow(dead_code)]
impl Theme {
    // Canvas & Surfaces
    pub const BG_CANVAS: Color32 = Color32::from_rgb(26, 26, 26);
    pub const BG_SURFACE: Color32 = Color32::from_rgb(30, 30, 30);
    pub const BG_PANEL: Color32 = Color32::from_rgb(37, 37, 38);
    pub const BG_PANEL_HOVER: Color32 = Color32::from_rgb(48, 48, 52);
    pub const BG_ELEVATED: Color32 = Color32::from_rgb(44, 44, 48);

    // Borders & Dividers
    pub const BORDER_SUBTLE: Color32 = Color32::from_rgb(52, 52, 56);
    pub const BORDER_FOCUS: Color32 = Color32::from_rgb(0, 120, 212);
    pub const DIVIDER: Color32 = Color32::from_rgb(60, 60, 64);

    // Accent Colors (Microsoft Fluent Blue)
    pub const ACCENT_PRIMARY: Color32 = Color32::from_rgb(0, 120, 212);
    pub const ACCENT_HOVER: Color32 = Color32::from_rgb(24, 140, 235);
    pub const ACCENT_TINT: Color32 = Color32::from_rgba_premultiplied(0, 120, 212, 35);

    // Text & Foregrounds
    pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(235, 235, 235);
    pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(170, 170, 175);
    pub const TEXT_MUTED: Color32 = Color32::from_rgb(125, 125, 130);

    // Tab Specific
    pub const TAB_ACTIVE_BG: Color32 = Color32::from_rgb(42, 42, 45);
    pub const TAB_INACTIVE_BG: Color32 = Color32::from_rgb(30, 30, 30);
    pub const TAB_INACTIVE_HOVER_BG: Color32 = Color32::from_rgb(38, 38, 42);
    pub const TAB_INDICATOR: Color32 = Color32::from_rgb(0, 120, 212);

    // Action Colors
    pub const CLOSE_RED: Color32 = Color32::from_rgb(232, 17, 35);
    pub const CLOSE_RED_HOVER: Color32 = Color32::from_rgb(240, 40, 55);

    // Standard Control Heights & Metrics
    pub const TOOLBAR_HEIGHT: f32 = 44.0;
    pub const TAB_HEIGHT: f32 = 38.0;
    pub const PATHBAR_HEIGHT: f32 = 32.0;
    pub const BUTTON_HIT_SIZE: Vec2 = Vec2::new(34.0, 34.0);
    pub const RADIUS: CornerRadius = CornerRadius::same(6);
    pub const TAB_RADIUS: CornerRadius = CornerRadius {
        nw: 6,
        ne: 6,
        sw: 0,
        se: 0,
    };

    /// Applies the unified Verso Reader visual theme and control metrics to the egui context.
    pub fn apply(ctx: &egui::Context) {
        let mut visuals = egui::Visuals::dark();

        visuals.window_fill = Self::BG_SURFACE;
        visuals.panel_fill = Self::BG_PANEL;
        visuals.faint_bg_color = Self::BG_PANEL;
        visuals.extreme_bg_color = Self::BG_CANVAS;
        visuals.window_stroke = Stroke::new(1.0_f32, Self::BORDER_SUBTLE);

        // Non-interactive widgets (labels, frames, backgrounds)
        visuals.widgets.noninteractive.bg_fill = Self::BG_SURFACE;
        visuals.widgets.noninteractive.weak_bg_fill = Self::BG_SURFACE;
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, Self::BORDER_SUBTLE);
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, Self::TEXT_PRIMARY);
        visuals.widgets.noninteractive.corner_radius = Self::RADIUS;

        // Inactive buttons / controls
        visuals.widgets.inactive.bg_fill = Color32::TRANSPARENT;
        visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::TRANSPARENT);
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, Self::TEXT_PRIMARY);
        visuals.widgets.inactive.corner_radius = Self::RADIUS;

        // Hovered controls
        visuals.widgets.hovered.bg_fill = Self::BG_PANEL_HOVER;
        visuals.widgets.hovered.weak_bg_fill = Self::BG_PANEL_HOVER;
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Self::BORDER_SUBTLE);
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
        visuals.widgets.hovered.corner_radius = Self::RADIUS;

        // Active / Pressed controls
        visuals.widgets.active.bg_fill = Self::ACCENT_PRIMARY;
        visuals.widgets.active.weak_bg_fill = Self::ACCENT_TINT;
        visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, Self::ACCENT_PRIMARY);
        visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
        visuals.widgets.active.corner_radius = Self::RADIUS;

        // Open popovers / menus
        visuals.widgets.open.bg_fill = Self::BG_PANEL_HOVER;
        visuals.widgets.open.weak_bg_fill = Self::BG_PANEL_HOVER;
        visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, Self::ACCENT_PRIMARY);
        visuals.widgets.open.corner_radius = Self::RADIUS;

        // Selection styling
        visuals.selection.bg_fill = Color32::from_rgba_premultiplied(0, 120, 212, 120);
        visuals.selection.stroke = Stroke::new(1.0_f32, Self::ACCENT_PRIMARY);

        ctx.set_visuals(visuals);

        // Standardized spacing and padding metrics
        let mut style = (*ctx.style()).clone();
        style.spacing.item_spacing = Vec2::new(6.0, 6.0);
        style.spacing.button_padding = Vec2::new(8.0, 5.0);
        style.spacing.window_margin = Margin::same(8);
        style.spacing.menu_margin = Margin::same(6);
        ctx.set_style(style);
    }
}
