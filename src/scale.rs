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
}

/// Returns the video rectangle as `[x, y, width, height]` relative to the
/// area's origin. The rectangle may extend past the area, which the caller
/// clips. Offsets are whole pixels so unscaled video samples texel centres.
pub fn place(scale: Scale, area: [u32; 2], video: [u32; 2]) -> Option<[f32; 4]> {
    if area.contains(&0) || video.contains(&0) {
        return None;
    }
    let [area_width, area_height] = area.map(f64::from);
    let [width, height] = video.map(f64::from);
    let [x_scale, y_scale] = match scale {
        Scale::Fit => [(area_width / width).min(area_height / height); 2],
        Scale::Fill => [(area_width / width).max(area_height / height); 2],
        Scale::Stretch => [area_width / width, area_height / height],
        Scale::Integer => {
            [(area_width / width)
                .min(area_height / height)
                .floor()
                .max(1.0); 2]
        }
        Scale::Native => [1.0; 2],
    };
    let width = width * x_scale;
    let height = height * y_scale;
    Some([
        ((area_width - width) / 2.0).round() as f32,
        ((area_height - height) / 2.0).round() as f32,
        width as f32,
        height as f32,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_centers_wide_video() {
        assert_eq!(
            place(Scale::Fit, [1920, 1200], [1920, 1080]),
            Some([0.0, 60.0, 1920.0, 1080.0])
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
}
