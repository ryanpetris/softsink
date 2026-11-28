#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scale {
    #[default]
    Fit,
    Fill,
    Stretch,
    Integer,
    Native,
}

impl Scale {
    pub const ALL: [Self; 5] = [
        Self::Fit,
        Self::Fill,
        Self::Stretch,
        Self::Integer,
        Self::Native,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Fit => "Fit",
            Self::Fill => "Fill",
            Self::Stretch => "Stretch",
            Self::Integer => "Integer",
            Self::Native => "1:1",
        }
    }

    pub fn can_zoom(self) -> bool {
        matches!(self, Self::Fit | Self::Fill | Self::Integer)
    }

    pub fn can_pan(self) -> bool {
        self.can_zoom() || self == Self::Native
    }
}

const MAX_ZOOM: f64 = 8.0;
const WHEEL_ZOOM: f64 = 1.2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    // Fit and Fill store a factor relative to their base. Integer stores an absolute multiple.
    zoom: f64,
    center: [f64; 2],
    scroll_remainder: f64,
}

impl Default for View {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            center: [0.5; 2],
            scroll_remainder: 0.0,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Geometry {
    scale: Scale,
    area: [f64; 2],
    video: [f64; 2],
    base: f64,
}

impl Geometry {
    pub fn new(scale: Scale, area: [u32; 2], video: [u32; 2]) -> Option<Self> {
        if area.contains(&0) || video.contains(&0) {
            return None;
        }
        let area = area.map(f64::from);
        let video = video.map(f64::from);
        let x = area[0] / video[0];
        let y = area[1] / video[1];
        let base = match scale {
            Scale::Fit => x.min(y),
            Scale::Fill => x.max(y),
            Scale::Integer => x.min(y).floor().max(1.0),
            Scale::Stretch | Scale::Native => 1.0,
        };
        Some(Self {
            scale,
            area,
            video,
            base,
        })
    }

    pub fn can_pan(self) -> bool {
        self.scale.can_pan()
    }

    fn multiplier(self, view: View) -> f64 {
        match self.scale {
            Scale::Fit | Scale::Fill => self.base * view.zoom,
            Scale::Integer => self.base.max(view.zoom),
            _ => self.base,
        }
    }

    fn size(self, view: View) -> [f64; 2] {
        if self.scale == Scale::Stretch {
            self.area
        } else {
            self.video.map(|length| length * self.multiplier(view))
        }
    }

    fn offset(self, axis: usize, size: f64, center: f64, snap: bool) -> f64 {
        let area = self.area[axis];
        if size <= area || !self.scale.can_pan() {
            return ((area - size) / 2.0).round();
        }
        let offset = area / 2.0 - center * size;
        let offset = if snap { offset.round() } else { offset };
        offset.clamp(area - size, 0.0)
    }

    fn origin(self, view: View, size: [f64; 2]) -> [f64; 2] {
        // Whole-pixel offsets keep Integer and 1:1 aligned with texel centres.
        let snap = !matches!(self.scale, Scale::Fit | Scale::Fill) || view == View::default();
        [0, 1].map(|axis| self.offset(axis, size[axis], view.center[axis], snap))
    }

    /// Returns the video rectangle as `[x, y, width, height]` relative to the
    /// area's origin. The video size follows its displayed orientation.
    pub fn place(self, view: View) -> [f64; 4] {
        let size = self.size(view);
        let [x, y] = self.origin(view, size);
        [x, y, size[0], size[1]]
    }

    pub fn center(self) -> [f32; 2] {
        self.area.map(|value| (value / 2.0) as f32)
    }

    pub fn pan(self, view: &mut View, delta: [f32; 2]) {
        // Rounding can put the fitted size slightly past the area's edge.
        if !self.scale.can_pan() || (self.scale == Scale::Fit && view.zoom == 1.0) {
            return;
        }
        let size = self.size(*view);
        let snap = matches!(self.scale, Scale::Fit | Scale::Fill) && *view == View::default();
        for (axis, delta) in delta.map(f64::from).into_iter().enumerate() {
            let area = self.area[axis];
            let size = size[axis];
            if size > area && delta != 0.0 {
                // Start at the visible center when the stored point is past
                // an edge. Keep subpixel motion between snapped placements.
                let offset = self.offset(axis, size, view.center[axis], snap);
                view.center[axis] = ((area / 2.0 - offset - delta) / size).clamp(0.0, 1.0);
            }
        }
    }

    pub fn scroll(self, view: &mut View, points: f64, points_per_step: f64, anchor: [f32; 2]) {
        if !points.is_finite() || points_per_step <= 0.0 {
            return;
        }
        let steps = if self.scale == Scale::Integer {
            view.scroll_remainder += points;
            let steps = (view.scroll_remainder / points_per_step).trunc();
            view.scroll_remainder %= points_per_step;
            steps
        } else {
            points / points_per_step
        };
        self.zoom(view, steps, anchor);
    }

    pub fn zoom(self, view: &mut View, steps: f64, anchor: [f32; 2]) {
        if !steps.is_finite() || steps == 0.0 {
            return;
        }
        let zoom = match self.scale {
            Scale::Fit | Scale::Fill => (view.zoom * WHEEL_ZOOM.powf(steps)).clamp(1.0, MAX_ZOOM),
            Scale::Integer => {
                let current = self.multiplier(*view);
                let next = current + steps.trunc();
                // A smaller window can leave the selected multiple above
                // the current limit. Zoom out from it in the usual steps.
                let limit = (self.base * MAX_ZOOM).max(current);
                if next <= self.base {
                    1.0
                } else {
                    next.min(limit)
                }
            }
            _ => return,
        };

        let previous_size = self.size(*view);
        let previous_origin = self.origin(*view, previous_size);
        view.zoom = zoom;
        let size = self.size(*view);
        for (axis, anchor) in anchor.map(f64::from).into_iter().enumerate() {
            if size[axis] > self.area[axis] && size[axis] != previous_size[axis] {
                let point = (anchor - previous_origin[axis]) / previous_size[axis];
                view.center[axis] =
                    (point + (self.area[axis] / 2.0 - anchor) / size[axis]).clamp(0.0, 1.0);
            }
        }
    }

    pub fn zoom_label(self, view: View) -> Option<String> {
        match self.scale {
            Scale::Fit | Scale::Fill if view.zoom > 1.0 => {
                Some(format!("{} {:.0}%", self.scale.name(), view.zoom * 100.0))
            }
            Scale::Integer if self.multiplier(view) > self.base => {
                Some(format!("Integer {:.0}x", self.multiplier(view)))
            }
            _ => None,
        }
    }
}

/// Intersects a placed video rectangle with the video area. The crop holds
/// the visible origin and size in normalized displayed video coordinates.
pub fn clip(
    [x, y, width, height]: [f64; 4],
    [area_width, area_height]: [u32; 2],
) -> ([f32; 4], [f32; 4]) {
    let left = x.max(0.0);
    let top = y.max(0.0);
    let visible_width = (x + width).min(f64::from(area_width)) - left;
    let visible_height = (y + height).min(f64::from(area_height)) - top;
    (
        [left, top, visible_width, visible_height].map(|value| value as f32),
        [
            (left - x) / width,
            (top - y) / height,
            visible_width / width,
            visible_height / height,
        ]
        .map(|value| value as f32),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn place(scale: Scale, area: [u32; 2], video: [u32; 2]) -> Option<[f32; 4]> {
        Some(
            Geometry::new(scale, area, video)?
                .place(View::default())
                .map(|value| value as f32),
        )
    }

    #[test]
    fn fit_centers_wide_video() {
        assert_eq!(
            place(Scale::Fit, [1920, 1201], [1920, 1080]),
            Some([0.0, 61.0, 1920.0, 1080.0])
        );
    }

    #[test]
    fn fit_centers_tall_video() {
        assert_eq!(
            place(Scale::Fit, [1920, 1080], [640, 480]),
            Some([240.0, 0.0, 1440.0, 1080.0])
        );
    }

    #[test]
    fn fit_fills_matching_aspect_ratio() {
        assert_eq!(
            place(Scale::Fit, [800, 450], [1920, 1080]),
            Some([0.0, 0.0, 800.0, 450.0])
        );
    }

    #[test]
    fn fill_crops_to_cover() {
        assert_eq!(
            place(Scale::Fill, [1920, 1200], [1920, 1080]),
            Some([-107.0, 0.0, 2133.3333, 1200.0])
        );
    }

    #[test]
    fn stretch_matches_area() {
        assert_eq!(
            place(Scale::Stretch, [1000, 1000], [1920, 1080]),
            Some([0.0, 0.0, 1000.0, 1000.0])
        );
    }

    #[test]
    fn integer_uses_largest_whole_scale() {
        assert_eq!(
            place(Scale::Integer, [2000, 1500], [640, 480]),
            Some([40.0, 30.0, 1920.0, 1440.0])
        );
    }

    #[test]
    fn integer_shows_large_video_unscaled() {
        assert_eq!(
            place(Scale::Integer, [1280, 720], [1920, 1080]),
            Some([-320.0, -180.0, 1920.0, 1080.0])
        );
    }

    #[test]
    fn native_centers_unscaled_video() {
        assert_eq!(
            place(Scale::Native, [1921, 1081], [1280, 720]),
            Some([321.0, 181.0, 1280.0, 720.0])
        );
    }

    #[test]
    fn skips_empty_sizes() {
        assert_eq!(place(Scale::Fit, [0, 720], [1280, 720]), None);
        assert_eq!(place(Scale::Fill, [1280, 720], [1280, 0]), None);
    }

    #[test]
    fn zoom_limits_follow_the_scale_mode() {
        for (scale, max_size) in [
            (Scale::Fit, 2400.0),
            (Scale::Fill, 2400.0),
            (Scale::Stretch, 300.0),
            (Scale::Integer, 2400.0),
            (Scale::Native, 100.0),
        ] {
            let geometry = Geometry::new(scale, [300, 300], [100, 100]).unwrap();
            let mut view = View::default();
            let base = geometry.place(view);
            geometry.zoom(&mut view, -100.0, geometry.center());
            assert_eq!(view, View::default(), "{scale:?}");
            geometry.zoom(&mut view, 100.0, geometry.center());
            assert_eq!(geometry.place(view)[2], max_size, "{scale:?}");
            geometry.zoom(&mut view, -100.0, geometry.center());
            assert_eq!(geometry.place(view), base, "{scale:?}");
        }
    }

    #[test]
    fn smooth_zoom_remains_relative_to_the_window() {
        for (scale, label, placement) in [
            (Scale::Fit, "Fit 120%", [25.0, -12.5, 150.0, 150.0]),
            (Scale::Fill, "Fill 120%", [-20.0, -57.5, 240.0, 240.0]),
        ] {
            let mut view = View::default();
            let first = Geometry::new(scale, [100; 2], [100; 2]).unwrap();
            assert_eq!(first.zoom_label(view), None);
            first.zoom(&mut view, 1.0, first.center());
            assert_eq!(first.zoom_label(view).as_deref(), Some(label));
            let resized = Geometry::new(scale, [200, 125], [100; 2]).unwrap();
            assert_eq!(resized.place(view), placement);
        }
    }

    #[test]
    fn integer_zoom_keeps_its_absolute_multiple_on_resize() {
        let geometry = |side| Geometry::new(Scale::Integer, [side; 2], [100; 2]).unwrap();
        let first = geometry(350);
        let mut view = View::default();
        first.zoom(&mut view, 1.0, first.center());
        assert_eq!(first.zoom_label(view).as_deref(), Some("Integer 4x"));
        assert_eq!(first.place(view)[2], 400.0);
        assert_eq!(geometry(175).place(view)[2], 400.0);
        let larger = geometry(550);
        assert_eq!(larger.place(view)[2], 500.0);
        assert_eq!(larger.zoom_label(view), None);
        larger.zoom(&mut view, -1.0, larger.center());
        assert_eq!(first.place(view)[2], 300.0);

        first.zoom(&mut view, 10.0, first.center());
        let smaller = geometry(175);
        assert_eq!(smaller.place(view)[2], 1300.0);
        smaller.zoom(&mut view, 1.0, smaller.center());
        assert_eq!(smaller.place(view)[2], 1300.0);
        smaller.zoom(&mut view, -1.0, smaller.center());
        assert_eq!(smaller.place(view)[2], 1200.0);
    }

    #[test]
    fn integer_scroll_steps_on_each_notch_equivalent() {
        let geometry = Geometry::new(Scale::Integer, [350; 2], [100; 2]).unwrap();
        let mut view = View::default();
        for (points, expected) in [(10.0, 300.0), (30.0, 400.0), (20.0, 400.0), (-45.0, 400.0)] {
            geometry.scroll(&mut view, points, 40.0, geometry.center());
            assert_eq!(geometry.place(view)[2], expected);
        }
        geometry.zoom(&mut view, 1.0, geometry.center());
        assert_eq!(geometry.place(view)[2], 500.0);
        geometry.scroll(&mut view, -15.0, 40.0, geometry.center());
        assert_eq!(geometry.place(view)[2], 400.0);
    }

    #[test]
    fn pan_clamps_overflow_and_centers_the_fitting_axis() {
        for scale in [Scale::Fill, Scale::Integer, Scale::Native] {
            let geometry = Geometry::new(scale, [100, 90], [150, 60]).unwrap();
            let mut view = View::default();
            for delta in [-1000.0, 1000.0] {
                geometry.pan(&mut view, [delta, delta]);
                let [x, y, w, h] = geometry.place(view);
                assert_eq!(x, if delta < 0.0 { 100.0 - w } else { 0.0 });
                assert_eq!(y, ((90.0 - h) / 2.0).round());
                assert_eq!(view.center[1], 0.5);
            }
        }
        for scale in [Scale::Fit, Scale::Stretch] {
            let geometry = Geometry::new(scale, [7, 11], [25, 16]).unwrap();
            let mut view = View::default();
            geometry.pan(&mut view, [20.0, 20.0]);
            assert_eq!(view, View::default(), "{scale:?}");
        }
    }

    #[test]
    fn fit_pan_moves_only_the_overflowing_axis() {
        let geometry = Geometry::new(Scale::Fit, [100, 101], [100, 50]).unwrap();
        let mut view = View::default();
        geometry.zoom(&mut view, 1.0, geometry.center());
        geometry.pan(&mut view, [0.25, 20.0]);
        assert_eq!(geometry.place(view), [-9.75, 21.0, 120.0, 60.0]);
        assert_eq!(view.center[1], 0.5);
    }

    #[test]
    fn fill_pan_returns_after_an_axis_stops_overflowing() {
        let wide = Geometry::new(Scale::Fill, [120, 60], [100; 2]).unwrap();
        let narrow = Geometry::new(Scale::Fill, [60; 2], [100; 2]).unwrap();
        let mut view = View::default();
        wide.pan(&mut view, [0.0, -24.0]);
        let panned = wide.place(view);
        assert!((panned[1] - -54.0).abs() < 1e-9);
        narrow.pan(&mut view, [20.0, 20.0]);
        assert_eq!(narrow.place(view), [0.0, 0.0, 60.0, 60.0]);
        assert_eq!(wide.place(view), panned);
    }

    #[test]
    fn pan_starts_at_the_effective_edge_after_resize() {
        let narrow = Geometry::new(Scale::Native, [40, 100], [100; 2]).unwrap();
        let wide = Geometry::new(Scale::Native, [80, 100], [100; 2]).unwrap();
        let mut view = View::default();
        narrow.pan(&mut view, [-25.0, 0.0]);
        assert_eq!(wide.place(view)[0], -20.0);
        assert_eq!(narrow.place(view)[0], -55.0);
        wide.pan(&mut view, [1.0, 0.0]);
        assert_eq!(wide.place(view)[0], -19.0);
    }

    #[test]
    fn zoom_keeps_the_video_point_at_the_anchor() {
        for scale in [Scale::Fit, Scale::Fill, Scale::Integer] {
            let geometry = Geometry::new(scale, [80; 2], [40; 2]).unwrap();
            let anchor = [30.0, 40.0];
            let mut view = View::default();
            let point = |[x, y, w, h]: [f64; 4]| {
                [
                    (f64::from(anchor[0]) - x) / w,
                    (f64::from(anchor[1]) - y) / h,
                ]
            };
            let previous = point(geometry.place(view));
            geometry.zoom(&mut view, 1.0, anchor);
            let current = point(geometry.place(view));
            assert!((previous[0] - current[0]).abs() < 1e-9);
            assert!((previous[1] - current[1]).abs() < 1e-9);
        }
    }

    #[test]
    fn viewport_and_crop_map_visible_pixels_back_to_the_same_video_points() {
        assert_eq!(
            clip([-25.0, 10.0, 200.0, 80.0], [100; 2]),
            ([0.0, 10.0, 100.0, 80.0], [0.125, 0.0, 0.5, 1.0])
        );
        assert_eq!(
            clip([10.0, 20.0, 80.0, 60.0], [100; 2]),
            ([10.0, 20.0, 80.0, 60.0], [0.0, 0.0, 1.0, 1.0])
        );

        let area = [5120, 1440];
        let geometry = Geometry::new(Scale::Fill, area, [1080, 1920]).unwrap();
        let mut view = View::default();
        geometry.zoom(&mut view, 100.0, geometry.center());
        geometry.pan(&mut view, [-123.0, 456.0]);
        let placed = geometry.place(view);
        assert!(placed[3] > 65536.0);
        let (viewport, crop) = clip(placed, area);
        assert_eq!(viewport, [0.0, 0.0, 5120.0, 1440.0]);
        for axis in 0..2 {
            let pixel = f64::from(area[axis]) - 0.5;
            let old = (pixel - placed[axis]) / placed[axis + 2];
            let new = f64::from(crop[axis])
                + (pixel / f64::from(viewport[axis + 2])) * f64::from(crop[axis + 2]);
            assert!((old - new).abs() < 1e-7);
        }
    }
}
