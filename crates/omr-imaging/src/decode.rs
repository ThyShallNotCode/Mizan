//! Decoding PNG, JPEG and TIFF bytes to 8-bit luma (spec §7.3, §8.2). No file I/O: callers pass
//! bytes.
//!
//! Colour is reduced to Y = 0.299 R + 0.587 G + 0.114 B (integer, round half up); alpha is
//! composited on white; 16-bit samples are scaled to 8 bits. Decoder limits guard against
//! decompression bombs (CAP-001 is raised by the caller on [`ImagingError::Decode`] or
//! [`ImagingError::TooLarge`]).

use crate::ImagingError;
use crate::gray::GrayImage;
use crate::reduce::{CaptureKind, classify_capture};
use image::{DynamicImage, ImageFormat, ImageReader, Limits};
use serde::{Deserialize, Serialize};
use std::io::Cursor;

/// Decoder limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DecodeLimits {
    /// Maximum width in pixels.
    pub max_width: u32,
    /// Maximum height in pixels.
    pub max_height: u32,
    /// Maximum decoder allocation in bytes.
    pub max_alloc: u64,
}

impl Default for DecodeLimits {
    /// 20,000 px per side (A3 at 1,200 dpi fits) and 1 GiB of decoder memory.
    fn default() -> Self {
        Self {
            max_width: 20_000,
            max_height: 20_000,
            max_alloc: 1 << 30,
        }
    }
}

/// The container format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SourceFormat {
    /// PNG.
    Png,
    /// JPEG (baseline or progressive).
    Jpeg,
    /// TIFF (including CCITT G4 bilevel).
    Tiff,
}

/// A decoded page.
#[derive(Clone, Debug)]
pub struct Decoded {
    /// Luma, 255 = white.
    pub image: GrayImage,
    /// Container format.
    pub format: SourceFormat,
    /// Bits per sample of the source (1-bit TIFFs report 8 after expansion by the decoder; the
    /// histogram test in `capture` still flags them).
    pub source_bits: u8,
    /// True if the source had colour channels.
    pub had_colour: bool,
    /// Capture classification from the histogram (bitonal → CAP-003 `bitonal`).
    pub capture: CaptureKind,
}

/// Decodes with [`DecodeLimits::default`].
pub fn decode(bytes: &[u8]) -> Result<Decoded, ImagingError> {
    decode_with_limits(bytes, DecodeLimits::default())
}

/// Decodes PNG, JPEG or TIFF bytes to luma under explicit limits.
pub fn decode_with_limits(bytes: &[u8], limits: DecodeLimits) -> Result<Decoded, ImagingError> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| ImagingError::Decode(e.to_string()))?;
    let format = match reader.format() {
        Some(ImageFormat::Png) => SourceFormat::Png,
        Some(ImageFormat::Jpeg) => SourceFormat::Jpeg,
        Some(ImageFormat::Tiff) => SourceFormat::Tiff,
        Some(f) => return Err(ImagingError::Unsupported(format!("{f:?}"))),
        None => return Err(ImagingError::Unsupported("unknown format".into())),
    };
    let mut l = Limits::default();
    l.max_image_width = Some(limits.max_width);
    l.max_image_height = Some(limits.max_height);
    l.max_alloc = Some(limits.max_alloc);
    reader.limits(l);
    let img = reader.decode().map_err(|e| match e {
        image::ImageError::Limits(l) => ImagingError::TooLarge(l.to_string()),
        other => ImagingError::Decode(other.to_string()),
    })?;
    let (source_bits, had_colour) = describe(&img);
    let image = to_luma(img)?;
    let capture = classify_capture(&image);
    Ok(Decoded {
        image,
        format,
        source_bits,
        had_colour,
        capture,
    })
}

fn describe(img: &DynamicImage) -> (u8, bool) {
    let c = img.color();
    let bits = (c.bits_per_pixel() / u16::from(c.channel_count())) as u8;
    (bits, c.has_color())
}

#[inline]
fn y_of(r: u32, g: u32, b: u32) -> u8 {
    ((299 * r + 587 * g + 114 * b + 500) / 1000) as u8
}

#[inline]
fn over_white(v: u32, a: u32) -> u32 {
    (v * a + 255 * (255 - a) + 127) / 255
}

#[inline]
fn to8(v: u16) -> u32 {
    (u32::from(v) * 255 + 32_767) / 65_535
}

fn to_luma(img: DynamicImage) -> Result<GrayImage, ImagingError> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let data: Vec<u8> = match img {
        DynamicImage::ImageLuma8(b) => b.into_raw(),
        DynamicImage::ImageLumaA8(b) => b
            .pixels()
            .map(|p| over_white(u32::from(p.0[0]), u32::from(p.0[1])) as u8)
            .collect(),
        DynamicImage::ImageRgb8(b) => b
            .pixels()
            .map(|p| y_of(u32::from(p.0[0]), u32::from(p.0[1]), u32::from(p.0[2])))
            .collect(),
        DynamicImage::ImageRgba8(b) => b
            .pixels()
            .map(|p| {
                let a = u32::from(p.0[3]);
                y_of(
                    over_white(u32::from(p.0[0]), a),
                    over_white(u32::from(p.0[1]), a),
                    over_white(u32::from(p.0[2]), a),
                )
            })
            .collect(),
        DynamicImage::ImageLuma16(b) => b.pixels().map(|p| to8(p.0[0]) as u8).collect(),
        DynamicImage::ImageLumaA16(b) => b
            .pixels()
            .map(|p| over_white(to8(p.0[0]), to8(p.0[1])) as u8)
            .collect(),
        DynamicImage::ImageRgb16(b) => b
            .pixels()
            .map(|p| y_of(to8(p.0[0]), to8(p.0[1]), to8(p.0[2])))
            .collect(),
        other => other
            .to_rgba8()
            .pixels()
            .map(|p| {
                let a = u32::from(p.0[3]);
                y_of(
                    over_white(u32::from(p.0[0]), a),
                    over_white(u32::from(p.0[1]), a),
                    over_white(u32::from(p.0[2]), a),
                )
            })
            .collect(),
    };
    GrayImage::from_raw(w, h, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_bytes(img: image::DynamicImage) -> Vec<u8> {
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn decodes_gray_png() {
        let g = image::GrayImage::from_fn(5, 4, |x, y| image::Luma([(x * 40 + y) as u8]));
        let d = decode(&png_bytes(DynamicImage::ImageLuma8(g))).unwrap();
        assert_eq!(d.format, SourceFormat::Png);
        assert_eq!((d.image.width(), d.image.height()), (5, 4));
        assert_eq!(d.image.get(3, 2), 122);
        assert!(!d.had_colour);
    }

    #[test]
    fn rgba_composites_on_white() {
        let c = image::RgbaImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                image::Rgba([0, 0, 0, 0])
            } else {
                image::Rgba([0, 0, 0, 255])
            }
        });
        let d = decode(&png_bytes(DynamicImage::ImageRgba8(c))).unwrap();
        assert_eq!(d.image.row(0), &[255, 0]);
        assert!(d.had_colour);
    }

    #[test]
    fn rejects_garbage_and_limits() {
        assert!(decode(b"not an image").is_err());
        let g = image::GrayImage::from_fn(50, 50, |_, _| image::Luma([255]));
        let bytes = png_bytes(DynamicImage::ImageLuma8(g));
        let lim = DecodeLimits {
            max_width: 10,
            ..DecodeLimits::default()
        };
        assert!(matches!(
            decode_with_limits(&bytes, lim),
            Err(ImagingError::TooLarge(_))
        ));
    }
}
