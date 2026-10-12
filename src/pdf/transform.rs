use egui::{pos2, vec2, Pos2, Rect, Vec2};

use crate::pdf::types::PageSize;

/// Helper mapping between canonical page coordinates (points, top-left origin, unrotated)
/// and screen coordinates for a given zoom, rotation, page size, and screen origin.
#[derive(Debug, Clone, Copy)]
pub struct PageTransform {
    pub canonical_size: PageSize,
    pub zoom: f32,
    pub rotation: u16,
    pub screen_origin: Pos2,
}

impl PageTransform {
    /// Conversion factor from canonical PDF points (72 DPI) to standard screen display points (96 DPI).
    pub const DPI_SCALE: f32 = 96.0 / 72.0;

    pub fn new(canonical_size: PageSize, zoom: f32, rotation: u16, screen_origin: Pos2) -> Self {
        Self {
            canonical_size,
            zoom,
            rotation: rotation % 360,
            screen_origin,
        }
    }

    /// Returns unscaled visual dimensions (width, height) in points after rotation,
    /// scaled from standard PDF points (72 DPI) to screen display points (96 DPI).
    pub fn visual_size_points(&self) -> (f32, f32) {
        let (w, h) = match self.rotation {
            90 | 270 => (self.canonical_size.height, self.canonical_size.width),
            _ => (self.canonical_size.width, self.canonical_size.height),
        };
        (w * Self::DPI_SCALE, h * Self::DPI_SCALE)
    }

    /// Returns screen dimensions (width, height) including zoom scaling.
    pub fn screen_size(&self) -> Vec2 {
        let (w, h) = self.visual_size_points();
        vec2(w * self.zoom, h * self.zoom)
    }

    /// Returns the full screen bounding rectangle of the page.
    pub fn screen_rect(&self) -> Rect {
        Rect::from_min_size(self.screen_origin, self.screen_size())
    }

    /// Maps a canonical point (origin top-left, unrotated) to a screen coordinate.
    #[allow(dead_code)]
    pub fn canonical_to_screen(&self, point: Pos2) -> Pos2 {
        let w = self.canonical_size.width;
        let h = self.canonical_size.height;
        let z = self.zoom * Self::DPI_SCALE;

        let (rx, ry) = match self.rotation {
            90 => (h - point.y, point.x),
            180 => (w - point.x, h - point.y),
            270 => (point.y, w - point.x),
            _ => (point.x, point.y),
        };

        pos2(self.screen_origin.x + rx * z, self.screen_origin.y + ry * z)
    }

    /// Maps a screen coordinate back to a canonical point.
    #[allow(dead_code)]
    pub fn screen_to_canonical(&self, screen_pos: Pos2) -> Pos2 {
        let w = self.canonical_size.width;
        let h = self.canonical_size.height;
        let z = self.zoom * Self::DPI_SCALE;

        let u = (screen_pos.x - self.screen_origin.x) / z;
        let v = (screen_pos.y - self.screen_origin.y) / z;

        match self.rotation {
            90 => pos2(v, h - u),
            180 => pos2(w - u, h - v),
            270 => pos2(w - v, u),
            _ => pos2(u, v),
        }
    }

    /// Maps a canonical rectangle to an axis-aligned bounding screen rectangle.
    #[allow(dead_code)]
    pub fn canonical_to_screen_rect(&self, rect: Rect) -> Rect {
        let p1 = self.canonical_to_screen(rect.min);
        let p2 = self.canonical_to_screen(pos2(rect.max.x, rect.min.y));
        let p3 = self.canonical_to_screen(rect.max);
        let p4 = self.canonical_to_screen(pos2(rect.min.x, rect.max.y));

        let min_x = p1.x.min(p2.x).min(p3.x).min(p4.x);
        let max_x = p1.x.max(p2.x).max(p3.x).max(p4.x);
        let min_y = p1.y.min(p2.y).min(p3.y).min(p4.y);
        let max_y = p1.y.max(p2.y).max(p3.y).max(p4.y);

        Rect::from_min_max(pos2(min_x, min_y), pos2(max_x, max_y))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_page_transform_roundtrip_all_rotations() {
        let size = PageSize {
            width: 612.0,
            height: 792.0,
        };
        let origin = pos2(50.0, 100.0);
        let zoom = 1.5;

        for rotation in [0, 90, 180, 270] {
            let transform = PageTransform::new(size, zoom, rotation, origin);

            let test_points = [
                pos2(0.0, 0.0),
                pos2(size.width, 0.0),
                pos2(size.width, size.height),
                pos2(0.0, size.height),
                pos2(123.4, 567.8),
            ];

            for p in test_points {
                let screen = transform.canonical_to_screen(p);
                let canonical_back = transform.screen_to_canonical(screen);

                assert!(
                    (canonical_back.x - p.x).abs() < 1e-3,
                    "X roundtrip mismatch at rot {rotation}: expected {}, got {}",
                    p.x,
                    canonical_back.x
                );
                assert!(
                    (canonical_back.y - p.y).abs() < 1e-3,
                    "Y roundtrip mismatch at rot {rotation}: expected {}, got {}",
                    p.y,
                    canonical_back.y
                );
            }
        }
    }

    #[test]
    fn test_page_transform_visual_dimensions() {
        let size = PageSize {
            width: 600.0,
            height: 800.0,
        };
        let origin = pos2(0.0, 0.0);

        let t0 = PageTransform::new(size, 1.0, 0, origin);
        let expected_w = 600.0 * (96.0 / 72.0);
        let expected_h = 800.0 * (96.0 / 72.0);
        assert_eq!(t0.visual_size_points(), (expected_w, expected_h));
        assert_eq!(t0.screen_size(), vec2(expected_w, expected_h));

        let t90 = PageTransform::new(size, 1.0, 90, origin);
        assert_eq!(t90.visual_size_points(), (expected_h, expected_w));
        assert_eq!(t90.screen_size(), vec2(expected_h, expected_w));

        let t180 = PageTransform::new(size, 2.0, 180, origin);
        assert_eq!(t180.visual_size_points(), (expected_w, expected_h));
        assert_eq!(t180.screen_size(), vec2(expected_w * 2.0, expected_h * 2.0));

        let t270 = PageTransform::new(size, 0.5, 270, origin);
        assert_eq!(t270.visual_size_points(), (expected_h, expected_w));
        assert_eq!(t270.screen_size(), vec2(expected_h * 0.5, expected_w * 0.5));
    }
}
