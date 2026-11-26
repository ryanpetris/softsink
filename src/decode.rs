use turbojpeg::{Decompressor, Subsamp, YuvImage, yuv_pixels_len};
use v4l::FourCC;
use v4l::v4l_sys::{
    v4l2_colorspace_V4L2_COLORSPACE_SMPTE240M as V4L2_COLORSPACE_SMPTE240M, v4l2_pix_format,
    v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_709 as V4L2_YCBCR_ENC_709,
    v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_DEFAULT as V4L2_YCBCR_ENC_DEFAULT,
    v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_SMPTE240M as V4L2_YCBCR_ENC_SMPTE240M,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PixelFormat {
    Mjpeg,
    Yuyv,
    Nv12,
}

impl PixelFormat {
    pub fn from_fourcc(fourcc: FourCC) -> Option<Self> {
        match &fourcc.repr {
            b"MJPG" => Some(Self::Mjpeg),
            b"YUYV" => Some(Self::Yuyv),
            b"NV12" => Some(Self::Nv12),
            _ => None,
        }
    }

    pub fn fourcc(self) -> FourCC {
        FourCC::new(match self {
            Self::Mjpeg => b"MJPG",
            Self::Yuyv => b"YUYV",
            Self::Nv12 => b"NV12",
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Mjpeg => "MJPEG",
            Self::Yuyv => "YUYV",
            Self::Nv12 => "NV12",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub kr: f32,
    pub kb: f32,
    pub full_range: bool,
}

pub fn color(format: v4l2_pix_format, pixel: PixelFormat) -> Color {
    // YUV capture formats use the YCbCr member of the union.
    let encoding = unsafe { format.__bindgen_anon_1.ycbcr_enc };
    // The default encoding is BT.601 for every colorspace uvcvideo reports
    // except SMPTE 240M.
    let (kr, kb) = match (encoding, format.colorspace) {
        (V4L2_YCBCR_ENC_709, _) => (0.2126, 0.0722),
        (V4L2_YCBCR_ENC_SMPTE240M, _) | (V4L2_YCBCR_ENC_DEFAULT, V4L2_COLORSPACE_SMPTE240M) => {
            (0.2122, 0.0865)
        }
        _ => (0.299, 0.114),
    };
    Color {
        kr,
        kb,
        full_range: pixel == PixelFormat::Mjpeg,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LayoutKind {
    Planar,
    Nv12,
    Yuyv,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub kind: LayoutKind,
    pub planes: [Option<Plane>; 3],
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Plane {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub channels: u32,
    pub offset: usize,
}

impl Plane {
    fn end(self) -> usize {
        self.offset
            + self.stride as usize * (self.height - 1) as usize
            + self.width as usize * self.channels as usize
    }
}

impl Layout {
    pub fn raw(pixel: PixelFormat, width: u32, height: u32, stride: u32) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }
        let (kind, planes) = match pixel {
            PixelFormat::Yuyv => (
                LayoutKind::Yuyv,
                [
                    Some(Plane {
                        width: width.div_ceil(2),
                        height,
                        stride,
                        channels: 4,
                        offset: 0,
                    }),
                    None,
                    None,
                ],
            ),
            PixelFormat::Nv12 => (
                LayoutKind::Nv12,
                [
                    Some(Plane {
                        width,
                        height,
                        stride,
                        channels: 1,
                        offset: 0,
                    }),
                    Some(Plane {
                        width: width.div_ceil(2),
                        height: height.div_ceil(2),
                        stride,
                        channels: 2,
                        offset: stride as usize * height as usize,
                    }),
                    None,
                ],
            ),
            PixelFormat::Mjpeg => return None,
        };
        Some(Self { kind, planes })
    }

    pub fn fits(self, len: usize) -> bool {
        self.planes
            .into_iter()
            .flatten()
            .all(|plane| plane.stride >= plane.width * plane.channels && plane.end() <= len)
    }
}

pub fn decode_jpeg(
    decoder: &mut Decompressor,
    jpeg: &[u8],
    bytes: &mut Vec<u8>,
    width: u32,
    height: u32,
) -> Option<Layout> {
    let header = decoder.read_header(jpeg).ok()?;
    if header.subsamp == Subsamp::Gray
        || (header.width, header.height) != (width as usize, height as usize)
    {
        return None;
    }
    let len = yuv_pixels_len(header.width, 4, header.height, header.subsamp).ok()?;
    bytes.resize(len, 0);
    let image = YuvImage {
        pixels: bytes.as_mut_slice(),
        width: header.width,
        height: header.height,
        align: 4,
        subsamp: header.subsamp,
    };
    let y = Plane {
        width,
        height,
        stride: image.y_width() as u32,
        channels: 1,
        offset: 0,
    };
    let u = Plane {
        width: image.width.div_ceil(image.subsamp.width()) as u32,
        height: image.uv_height() as u32,
        stride: image.uv_width() as u32,
        channels: 1,
        offset: image.y_width() * image.y_height(),
    };
    let v = Plane {
        offset: u.offset + image.uv_width() * image.uv_height(),
        ..u
    };
    decoder.decompress_to_yuv(jpeg, image).ok()?;
    Some(Layout {
        kind: LayoutKind::Planar,
        planes: [Some(y), Some(u), Some(v)],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use v4l::v4l_sys::{
        v4l2_colorspace_V4L2_COLORSPACE_SMPTE170M as V4L2_COLORSPACE_SMPTE170M,
        v4l2_colorspace_V4L2_COLORSPACE_SRGB as V4L2_COLORSPACE_SRGB,
        v4l2_ycbcr_encoding_V4L2_YCBCR_ENC_601 as V4L2_YCBCR_ENC_601,
    };

    fn format(colorspace: u32, encoding: u32) -> v4l2_pix_format {
        let mut format = v4l2_pix_format {
            width: 1920,
            height: 1080,
            colorspace,
            ..unsafe { std::mem::zeroed() }
        };
        format.__bindgen_anon_1.ycbcr_enc = encoding;
        format
    }

    #[test]
    fn encoding_selects_the_matrix() {
        for (colorspace, encoding, kr, kb) in [
            (V4L2_COLORSPACE_SRGB, V4L2_YCBCR_ENC_709, 0.2126, 0.0722),
            (V4L2_COLORSPACE_SMPTE170M, V4L2_YCBCR_ENC_601, 0.299, 0.114),
            (V4L2_COLORSPACE_SRGB, V4L2_YCBCR_ENC_DEFAULT, 0.299, 0.114),
            (
                V4L2_COLORSPACE_SMPTE240M,
                V4L2_YCBCR_ENC_DEFAULT,
                0.2122,
                0.0865,
            ),
        ] {
            let color = color(format(colorspace, encoding), PixelFormat::Nv12);
            assert_eq!((color.kr, color.kb), (kr, kb));
        }
    }

    #[test]
    fn only_mjpeg_is_full_range() {
        let format = format(V4L2_COLORSPACE_SRGB, V4L2_YCBCR_ENC_DEFAULT);
        assert!(color(format, PixelFormat::Mjpeg).full_range);
        assert!(!color(format, PixelFormat::Yuyv).full_range);
    }
}
