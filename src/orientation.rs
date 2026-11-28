use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rotation {
    #[default]
    #[serde(rename = "0")]
    Deg0,
    #[serde(rename = "90")]
    Deg90,
    #[serde(rename = "180")]
    Deg180,
    #[serde(rename = "270")]
    Deg270,
}

impl Rotation {
    pub const ALL: [Self; 4] = [Self::Deg0, Self::Deg90, Self::Deg180, Self::Deg270];

    pub fn label(self) -> &'static str {
        match self {
            Self::Deg0 => "0\u{b0}",
            Self::Deg90 => "90\u{b0}",
            Self::Deg180 => "180\u{b0}",
            Self::Deg270 => "270\u{b0}",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Orientation {
    pub rotation: Rotation,
    pub flip_horizontal: bool,
    pub flip_vertical: bool,
}

impl Orientation {
    pub fn size(self, [width, height]: [u32; 2]) -> [u32; 2] {
        match self.rotation {
            Rotation::Deg0 | Rotation::Deg180 => [width, height],
            Rotation::Deg90 | Rotation::Deg270 => [height, width],
        }
    }

    /// Rows of the inverse transform about the center of the frame.
    pub fn uv_rows(self) -> [f32; 4] {
        let [ux, uy, vx, vy] = match self.rotation {
            Rotation::Deg0 => [1.0, 0.0, 0.0, 1.0],
            Rotation::Deg90 => [0.0, 1.0, -1.0, 0.0],
            Rotation::Deg180 => [-1.0, 0.0, 0.0, -1.0],
            Rotation::Deg270 => [0.0, -1.0, 1.0, 0.0],
        };
        let x = if self.flip_horizontal { -1.0 } else { 1.0 };
        let y = if self.flip_vertical { -1.0 } else { 1.0 };
        [ux * x, uy * y, vx * x, vy * y]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarter_turns_swap_dimensions() {
        for (rotation, expected) in
            Rotation::ALL
                .into_iter()
                .zip([[640, 480], [480, 640], [640, 480], [480, 640]])
        {
            let orientation = Orientation {
                rotation,
                ..Orientation::default()
            };
            assert_eq!(orientation.size([640, 480]), expected);
        }
    }

    #[test]
    fn every_rotation_and_flip_maps_screen_corners_to_source() {
        let corners = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]];
        let rotated_corners = [[0, 1, 2, 3], [2, 0, 3, 1], [3, 2, 1, 0], [1, 3, 0, 2]];
        for (rotation, source_order) in Rotation::ALL.into_iter().zip(rotated_corners) {
            for (flip, (flip_horizontal, flip_vertical)) in
                [(false, false), (true, false), (false, true), (true, true)]
                    .into_iter()
                    .enumerate()
            {
                let orientation = Orientation {
                    rotation,
                    flip_horizontal,
                    flip_vertical,
                };
                let [ux, uy, vx, vy] = orientation.uv_rows();
                for (index, [x, y]) in corners.into_iter().enumerate() {
                    let x = x - 0.5;
                    let y = y - 0.5;
                    let source = [ux * x + uy * y + 0.5, vx * x + vy * y + 0.5];
                    assert_eq!(
                        source,
                        corners[source_order[index ^ flip]],
                        "{orientation:?}"
                    );
                }
            }
        }
    }
}
