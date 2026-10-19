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

    // PDF Text Selection Color (Multiply Blend)
    // Radiant, modern electric amber (#FFA028) that keeps underlying text pitch-black and razor sharp.
    pub const TEXT_SELECTION_MULTIPLY: Color32 = Color32::from_rgb(255, 160, 40);

    // Text & Foregrounds
    pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(235, 235, 235);
    pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(170, 170, 175);
    pub const TEXT_MUTED: Color32 = Color32::from_rgb(125, 125, 130);

    // Tab Specific
    pub const TAB_ACTIVE_BG: Color32 = Color32::from_rgb(42, 42, 45);
    pub const TAB_INACTIVE_BG: Color32 = Color32::from_rgb(30, 30, 30);
    pub const TAB_INACTIVE_HOVER_BG: Color32 = Color32::from_rgb(58, 60, 68);
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
}

/// Concrete color tokens resolved for a specific theme mode (Dark or Light).
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct ThemePalette {
    pub is_dark: bool,
    pub bg_canvas: Color32,
    pub bg_surface: Color32,
    pub bg_panel: Color32,
    pub bg_panel_hover: Color32,
    pub bg_elevated: Color32,
    pub tabbar_bg: Color32,
    pub tab_active_bg: Color32,
    pub tab_inactive_bg: Color32,
    pub tab_inactive_hover_bg: Color32,
    pub tab_indicator: Color32,
    pub border_subtle: Color32,
    pub divider: Color32,
    pub card_bg: Color32,
    pub card_border: Color32,
    pub text_primary: Color32,
    pub text_secondary: Color32,
    pub text_muted: Color32,
    pub icon_fg: Color32,
    pub icon_fg_hover: Color32,
}

impl Theme {
    /// Resolves the comprehensive theme palette for dark or light mode.
    pub fn palette(is_dark: bool) -> ThemePalette {
        if is_dark {
            ThemePalette {
                is_dark: true,
                bg_canvas: Color32::from_rgb(26, 26, 28),
                bg_surface: Color32::from_rgb(30, 30, 32),
                bg_panel: Color32::from_rgb(32, 33, 36),
                bg_panel_hover: Color32::from_rgb(48, 48, 52),
                bg_elevated: Color32::from_rgb(44, 44, 48),
                tabbar_bg: Color32::from_rgb(20, 20, 22),
                tab_active_bg: Color32::from_rgb(42, 42, 45),
                tab_inactive_bg: Color32::from_rgb(20, 20, 22),
                tab_inactive_hover_bg: Color32::from_rgb(48, 48, 52),
                tab_indicator: Self::ACCENT_PRIMARY,
                border_subtle: Color32::from_rgb(52, 52, 56),
                divider: Color32::from_rgb(60, 60, 64),
                card_bg: Color32::from_rgb(28, 28, 30),
                card_border: Color32::from_rgb(48, 48, 52),
                text_primary: Color32::from_rgb(235, 235, 235),
                text_secondary: Color32::from_rgb(170, 170, 175),
                text_muted: Color32::from_rgb(125, 125, 130),
                icon_fg: Color32::from_rgb(170, 170, 175),
                icon_fg_hover: Color32::WHITE,
            }
        } else {
            ThemePalette {
                is_dark: false,
                bg_canvas: Color32::from_rgb(238, 240, 244), // Edge light PDF canvas background
                bg_surface: Color32::WHITE,
                bg_panel: Color32::from_rgb(246, 248, 250), // Clean off-white chrome
                bg_panel_hover: Color32::from_rgb(230, 233, 238),
                bg_elevated: Color32::WHITE,
                tabbar_bg: Color32::from_rgb(228, 231, 236), // Authentic Edge light titlebar
                tab_active_bg: Color32::WHITE, // Active tab is crisp white merging into toolbar
                tab_inactive_bg: Color32::from_rgb(228, 231, 236),
                tab_inactive_hover_bg: Color32::from_rgb(238, 240, 244),
                tab_indicator: Self::ACCENT_PRIMARY,
                border_subtle: Color32::from_rgb(218, 222, 228),
                divider: Color32::from_rgb(215, 219, 225),
                card_bg: Color32::WHITE,
                card_border: Color32::from_rgb(218, 222, 228),
                text_primary: Color32::from_rgb(25, 25, 30),
                text_secondary: Color32::from_rgb(90, 95, 105),
                text_muted: Color32::from_rgb(140, 145, 155),
                icon_fg: Color32::from_rgb(70, 75, 85),
                icon_fg_hover: Color32::from_rgb(15, 15, 20),
            }
        }
    }

    /// Applies the unified Verso Reader visual theme and control metrics to the egui context.
    pub fn apply(ctx: &egui::Context, is_dark: bool) {
        let palette = Self::palette(is_dark);
        let mut visuals = if is_dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };

        visuals.dark_mode = is_dark;
        visuals.window_fill = palette.bg_surface;
        visuals.panel_fill = palette.bg_panel;
        visuals.faint_bg_color = palette.bg_panel;
        visuals.extreme_bg_color = palette.bg_canvas;
        visuals.window_stroke = Stroke::new(1.0_f32, palette.border_subtle);

        // Non-interactive widgets (labels, frames, backgrounds)
        visuals.widgets.noninteractive.bg_fill = palette.bg_surface;
        visuals.widgets.noninteractive.weak_bg_fill = palette.bg_surface;
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, palette.border_subtle);
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, palette.text_primary);
        visuals.widgets.noninteractive.corner_radius = Self::RADIUS;

        // Inactive buttons / controls
        visuals.widgets.inactive.bg_fill = Color32::TRANSPARENT;
        visuals.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::TRANSPARENT);
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, palette.icon_fg);
        visuals.widgets.inactive.corner_radius = Self::RADIUS;

        // Hovered controls
        visuals.widgets.hovered.bg_fill = palette.bg_panel_hover;
        visuals.widgets.hovered.weak_bg_fill = palette.bg_panel_hover;
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, palette.border_subtle);
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, palette.icon_fg_hover);
        visuals.widgets.hovered.corner_radius = Self::RADIUS;

        // Active / Pressed controls
        visuals.widgets.active.bg_fill = Self::ACCENT_PRIMARY;
        visuals.widgets.active.weak_bg_fill = Self::ACCENT_TINT;
        visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, Self::ACCENT_PRIMARY);
        visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
        visuals.widgets.active.corner_radius = Self::RADIUS;

        // Open popovers / menus
        visuals.widgets.open.bg_fill = palette.bg_panel_hover;
        visuals.widgets.open.weak_bg_fill = palette.bg_panel_hover;
        visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, Self::ACCENT_PRIMARY);
        visuals.widgets.open.corner_radius = Self::RADIUS;

        // Selection styling
        visuals.selection.bg_fill = Color32::from_rgba_premultiplied(0, 120, 212, 55);
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
