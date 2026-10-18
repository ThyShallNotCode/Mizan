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
    /// Maximum pixels of one page (width × height), checked from the header before any
    /// pixel is decoded: a small file cannot expand into a page that costs gigabytes and
    /// seconds of CPU (§22.3).
    pub max_pixels: u64,
}

impl Default for DecodeLimits {
    /// 20,000 px per side, 100 megapixels per page (A3 at 600 dpi is 70; the PDF extractor's
    /// limit) and 1 GiB of decoder memory.
    fn default() -> Self {
        Self {
            max_width: 20_000,
            max_height: 20_000,
            max_alloc: 1 << 30,
            max_pixels: 100_000_000,
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

/// Decodes PNG, JPEG or TIFF bytes to luma under explicit limits. A JPEG without its
/// end-of-image marker (a file cut short in transfer) is refused rather than decoded: the
/// decoder would fill the missing rows with gray, which can read like marks (§7.3: truncated
/// file or missing EOI is CAP-001). A multi-page TIFF decodes its first page here; see
/// [`tiff_pages`] and [`decode_tiff_page`] for the others.
pub fn decode_with_limits(bytes: &[u8], limits: DecodeLimits) -> Result<Decoded, ImagingError> {
    decode_reader(Cursor::new(bytes), bytes, limits)
}

fn decode_reader<R: std::io::BufRead + std::io::Seek>(
    reader: R,
    bytes: &[u8],
    limits: DecodeLimits,
) -> Result<Decoded, ImagingError> {
    let mut reader = ImageReader::new(reader)
        .with_guessed_format()
        .map_err(|e| ImagingError::Decode(e.to_string()))?;
    if reader.format() == Some(ImageFormat::Jpeg) && !jpeg_complete(bytes) {
        return Err(ImagingError::Decode(
            "truncated JPEG: the end-of-image marker is missing (the file was cut short)".into(),
        ));
    }
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
    let err = |e: image::ImageError| match e {
        image::ImageError::Limits(l) => ImagingError::TooLarge(l.to_string()),
        other => ImagingError::Decode(other.to_string()),
    };
    let decoder = reader.into_decoder().map_err(err)?;
    let (w, h) = image::ImageDecoder::dimensions(&decoder);
    if u64::from(w) * u64::from(h) > limits.max_pixels {
        return Err(ImagingError::TooLarge(format!(
            "{w} × {h} px exceeds the {} megapixel page limit (an A4 or A3 sheet at 200–600 \
             dpi is far smaller)",
            limits.max_pixels / 1_000_000
        )));
    }
    let img = DynamicImage::from_decoder(decoder).map_err(err)?;
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

/// A colour JPEG below this resolution is taken for a phone photo (§8.10): scanners deliver
/// 200–600 dpi; a phone photo of an A4 sheet lands at ≈ 120–160 ppi.
pub const PHOTO_MAX_DPI: u32 = 200;

/// True when a page image is taken for a phone photo from its file: a colour JPEG whose
/// resolution (the PDF placement; image files have none we trust) is known and below
/// [`PHOTO_MAX_DPI`]. EXIF camera tags decide image files ([`jpeg_from_camera`]).
pub fn photo_capture(colour_jpeg: bool, dpi: Option<u32>) -> bool {
    colour_jpeg && dpi.is_some_and(|d| d < PHOTO_MAX_DPI)
}

/// True when a JPEG carries a camera's EXIF exposure data (an Exif IFD with an exposure time,
/// f-number, ISO or focal length: tags 0x829A, 0x829D, 0x8827, 0x920A), as phones and cameras
/// write and scanners do not. Bytes that are not a JPEG with EXIF give false.
pub fn jpeg_from_camera(bytes: &[u8]) -> bool {
    exif_tiff(bytes).is_some_and(|t| exif_camera_tags(t).unwrap_or(false))
}

/// The TIFF structure inside a JPEG's APP1 `Exif` segment, if any (segments before SOS).
fn exif_tiff(bytes: &[u8]) -> Option<&[u8]> {
    if !bytes.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut i = 2;
    while i + 4 <= bytes.len() {
        if bytes[i] != 0xFF {
            return None;
        }
        let marker = bytes[i + 1];
        if marker == 0xD8 || (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            i += 2;
            continue;
        }
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        let len = usize::from(u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]));
        let end = (i + 2 + len).min(bytes.len());
        if len < 2 {
            return None;
        }
        let seg = &bytes[i + 4..end];
        if marker == 0xE1 && seg.starts_with(b"Exif\0\0") {
            return Some(&seg[6..]);
        }
        i = end;
    }
    None
}

fn exif_camera_tags(t: &[u8]) -> Option<bool> {
    let le = match t.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |o: usize| -> Option<u16> {
        let b = t.get(o..o + 2)?;
        Some(if le {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    };
    let u32_at = |o: usize| -> Option<u32> {
        let b = t.get(o..o + 4)?;
        Some(if le {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        } else {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        })
    };
    // The tags of one IFD at `off` (bounded: at most 512 entries).
    let tags = |off: usize| -> Option<Vec<(u16, u32)>> {
        let n = usize::from(u16_at(off)?).min(512);
        (0..n)
            .map(|k| {
                let e = off + 2 + 12 * k;
                Some((u16_at(e)?, u32_at(e + 8)?))
            })
            .collect()
    };
    let ifd0 = tags(u32_at(4)? as usize)?;
    let camera = |tag: u16| matches!(tag, 0x829A | 0x829D | 0x8827 | 0x920A);
    if ifd0.iter().any(|(tag, _)| camera(*tag)) {
        return Some(true);
    }
    let Some(&(_, exif_off)) = ifd0.iter().find(|(tag, _)| *tag == 0x8769) else {
        return Some(false);
    };
    Some(tags(exif_off as usize)?.iter().any(|(tag, _)| camera(*tag)))
}

/// True when a JPEG's marker structure runs to its end-of-image marker (EOI, `FF D9`).
/// Segments are skipped by their lengths (an EXIF thumbnail's own EOI sits inside APP1 and is
/// never seen) and entropy-coded data up to the next marker (stuffed `FF 00`, restart markers
/// and fill bytes are data); bytes after EOI are allowed. Running out of bytes first means the
/// file was truncated. Bytes that are not a JPEG return true (the decoder judges them).
pub fn jpeg_complete(bytes: &[u8]) -> bool {
    if !bytes.starts_with(&[0xFF, 0xD8]) {
        return true;
    }
    let n = bytes.len();
    let mut i = 2;
    loop {
        // A marker: one or more 0xFF, then the code.
        if i >= n {
            return false;
        }
        if bytes[i] != 0xFF {
            // Not at a marker where one must be: malformed, the decoder decides.
            return true;
        }
        while i < n && bytes[i] == 0xFF {
            i += 1;
        }
        if i >= n {
            return false;
        }
        let m = bytes[i];
        i += 1;
        match m {
            0xD9 => return true,
            0x01 | 0xD0..=0xD8 => {}
            _ => {
                if i + 2 > n {
                    return false;
                }
                let len = usize::from(u16::from_be_bytes([bytes[i], bytes[i + 1]]));
                if len < 2 {
                    return true;
                }
                i += len;
                if i > n {
                    return false;
                }
                if m == 0xDA {
                    // Entropy-coded data: up to the next marker that is not data.
                    loop {
                        match bytes[i..].iter().position(|&b| b == 0xFF) {
                            None => return false,
                            Some(k) => i += k,
                        }
                        match bytes.get(i + 1) {
                            None => return false,
                            Some(0x00 | 0xD0..=0xD7) => i += 2,
                            Some(0xFF) => i += 1,
                            Some(_) => break,
                        }
                    }
                }
            }
        }
    }
}

/// The pages of a TIFF: the offset of every image directory (IFD) along the chain, in page
/// order, for classic and BigTIFF files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TiffPages {
    /// IFD offsets, one per page.
    pub offsets: Vec<u64>,
    /// Why the chain stopped before its end marker (a link outside the file, a loop, more
    /// than [`TIFF_MAX_PAGES`] pages), if it did: pages after the last good one are lost.
    pub broken: Option<String>,
    big: bool,
    little: bool,
}

/// Most pages read from one TIFF (the PDF page limit of §7.4).
pub const TIFF_MAX_PAGES: usize = 5_000;

/// Walks a TIFF's IFD chain (§7.3 "per-page IFD isolation"). `None` when `bytes` is not a TIFF.
/// Scanner batches often arrive as one multi-page TIFF; decoding only the first image would
/// drop every other sheet silently.
pub fn tiff_pages(bytes: &[u8]) -> Option<TiffPages> {
    let little = match bytes.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let b: [u8; 2] = bytes.get(at..at + 2)?.try_into().ok()?;
        Some(if little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let b: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
        Some(if little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    let u64_at = |at: usize| -> Option<u64> {
        let b: [u8; 8] = bytes.get(at..at + 8)?.try_into().ok()?;
        Some(if little {
            u64::from_le_bytes(b)
        } else {
            u64::from_be_bytes(b)
        })
    };
    let big = match u16_at(2)? {
        42 => false,
        43 => true,
        _ => return None,
    };
    let mut next = if big {
        u64_at(8)?
    } else {
        u64::from(u32_at(4)?)
    };
    let mut offsets = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut broken = None;
    while next != 0 {
        if offsets.len() == TIFF_MAX_PAGES {
            broken = Some(format!("more than {TIFF_MAX_PAGES} pages"));
            break;
        }
        if !seen.insert(next) {
            broken = Some(format!(
                "page {} links back to an earlier page",
                offsets.len()
            ));
            break;
        }
        let at = usize::try_from(next).unwrap_or(usize::MAX);
        let link = if big {
            u64_at(at).and_then(|c| {
                let end = at
                    .checked_add(8)?
                    .checked_add(usize::try_from(c).ok()?.checked_mul(20)?)?;
                Some((end, u64_at(end)?))
            })
        } else {
            u16_at(at).and_then(|c| {
                let end = at.checked_add(2 + usize::from(c) * 12)?;
                Some((end, u64::from(u32_at(end)?)))
            })
        };
        match link {
            Some((_, n)) => {
                offsets.push(next);
                next = n;
            }
            None => {
                broken = Some(format!(
                    "the directory of page {} lies outside the file (truncated or damaged)",
                    offsets.len() + 1
                ));
                break;
            }
        }
    }
    Some(TiffPages {
        offsets,
        broken,
        big,
        little,
    })
}

/// Decodes page `index` (0-based) of a TIFF walked by [`tiff_pages`], with the same decoder
/// and conversions as [`decode_with_limits`]: the header's first-directory pointer is served
/// as that page's IFD offset (the bytes are not copied; TIFF offsets are absolute).
pub fn decode_tiff_page(
    bytes: &[u8],
    pages: &TiffPages,
    index: usize,
    limits: DecodeLimits,
) -> Result<Decoded, ImagingError> {
    let off = *pages
        .offsets
        .get(index)
        .ok_or_else(|| ImagingError::Decode(format!("TIFF page {} does not exist", index + 1)))?;
    if index == 0 {
        return decode_with_limits(bytes, limits);
    }
    let (at, patch) = if pages.big {
        (
            8,
            if pages.little {
                off.to_le_bytes().to_vec()
            } else {
                off.to_be_bytes().to_vec()
            },
        )
    } else {
        let o = u32::try_from(off).map_err(|_| ImagingError::Decode("TIFF offset".into()))?;
        (
            4,
            if pages.little {
                o.to_le_bytes().to_vec()
            } else {
                o.to_be_bytes().to_vec()
            },
        )
    };
    decode_reader(
        Patched {
            bytes,
            at,
            patch,
            pos: 0,
        },
        bytes,
        limits,
    )
}

/// `bytes` read through a cursor that serves `patch` in place of `bytes[at..at + patch.len()]`.
struct Patched<'a> {
    bytes: &'a [u8],
    at: usize,
    patch: Vec<u8>,
    pos: u64,
}

impl std::io::Read for Patched<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let avail = std::io::BufRead::fill_buf(self)?;
        let k = avail.len().min(buf.len());
        buf[..k].copy_from_slice(&avail[..k]);
        std::io::BufRead::consume(self, k);
        Ok(k)
    }
}

impl std::io::BufRead for Patched<'_> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        let p = usize::try_from(self.pos).unwrap_or(usize::MAX);
        let end = self.at + self.patch.len();
        Ok(if p >= self.bytes.len() {
            &[]
        } else if p < self.at {
            &self.bytes[p..self.at.min(self.bytes.len())]
        } else if p < end {
            &self.patch[p - self.at..]
        } else {
            &self.bytes[p..]
        })
    }

    fn consume(&mut self, amt: usize) {
        self.pos += amt as u64;
    }
}

impl std::io::Seek for Patched<'_> {
    fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
        let len = self.bytes.len() as i128;
        let p = match to {
            std::io::SeekFrom::Start(o) => i128::from(o),
            std::io::SeekFrom::End(o) => len + i128::from(o),
            std::io::SeekFrom::Current(o) => i128::from(self.pos) + i128::from(o),
        };
        if p < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek before the start",
            ));
        }
        self.pos = p as u64;
        Ok(self.pos)
    }
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

    /// A JPEG prefix with an APP1 Exif segment whose Exif IFD holds `tag` (little-endian).
    fn exif_jpeg(tag: u16) -> Vec<u8> {
        let mut t: Vec<u8> = b"II*\0".to_vec();
        t.extend(8u32.to_le_bytes()); // IFD0 at 8
        t.extend(1u16.to_le_bytes()); // one entry: the Exif IFD pointer
        t.extend(0x8769u16.to_le_bytes());
        t.extend(4u16.to_le_bytes()); // LONG
        t.extend(1u32.to_le_bytes());
        t.extend(26u32.to_le_bytes()); // Exif IFD at 26
        t.extend(0u32.to_le_bytes()); // no next IFD
        t.extend(1u16.to_le_bytes());
        t.extend(tag.to_le_bytes());
        t.extend(3u16.to_le_bytes());
        t.extend(1u32.to_le_bytes());
        t.extend(100u32.to_le_bytes());
        t.extend(0u32.to_le_bytes());
        let mut seg = b"Exif\0\0".to_vec();
        seg.extend(t);
        let mut j = vec![0xFF, 0xD8, 0xFF, 0xE1];
        j.extend(((seg.len() + 2) as u16).to_be_bytes());
        j.extend(seg);
        j.extend([0xFF, 0xDA, 0x00, 0x02, 0xFF, 0xD9]);
        j
    }

    #[test]
    fn camera_exif_and_photo_resolution_mark_a_phone_capture() {
        // C3: a phone photo is recognised from its file, never left a scanner capture.
        assert!(jpeg_from_camera(&exif_jpeg(0x829A)), "exposure time");
        assert!(jpeg_from_camera(&exif_jpeg(0x920A)), "focal length");
        // A scanner's EXIF (e.g. resolution or software only) is not a camera's.
        assert!(!jpeg_from_camera(&exif_jpeg(0x0131)));
        assert!(!jpeg_from_camera(&[0xFF, 0xD8, 0xFF, 0xD9]));
        assert!(!jpeg_from_camera(b"not a jpeg"));
        // Truncated or hostile EXIF never panics.
        let j = exif_jpeg(0x829A);
        for cut in 0..j.len() {
            let _ = jpeg_from_camera(&j[..cut]);
        }
        assert!(photo_capture(true, Some(142)));
        assert!(!photo_capture(true, Some(200)));
        assert!(
            !photo_capture(false, Some(142)),
            "a gray scan is a scanner page"
        );
        assert!(
            !photo_capture(true, None),
            "an unknown resolution decides nothing"
        );
    }

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

    fn jpeg_bytes(w: u32, h: u32) -> Vec<u8> {
        let g =
            image::GrayImage::from_fn(w, h, |x, y| image::Luma([((x * 7 + y * 3) % 256) as u8]));
        let mut out = Vec::new();
        DynamicImage::ImageLuma8(g)
            .write_to(&mut Cursor::new(&mut out), ImageFormat::Jpeg)
            .unwrap();
        out
    }

    #[test]
    fn truncated_jpeg_is_refused_but_trailing_bytes_are_not() {
        let j = jpeg_bytes(64, 48);
        assert!(jpeg_complete(&j));
        assert!(decode(&j).is_ok());
        for cut in [j.len() - 2, j.len() - 40, j.len() / 2, 30] {
            assert!(!jpeg_complete(&j[..cut]), "cut at {cut}");
            let e = decode(&j[..cut]).unwrap_err();
            assert!(matches!(e, ImagingError::Decode(_)), "{e}");
        }
        let mut trailing = j.clone();
        trailing.extend_from_slice(b"\0\0trailer data after EOI");
        assert!(jpeg_complete(&trailing));
        assert!(decode(&trailing).is_ok());
        assert!(jpeg_complete(b"not a jpeg"));
    }

    /// A minimal classic TIFF: 8-bit gray, uncompressed, one strip per page.
    fn tiff_bytes(pages: &[image::GrayImage]) -> Vec<u8> {
        let mut out = b"II*\0\0\0\0\0".to_vec();
        let mut link = 4usize;
        for img in pages {
            let data = out.len();
            out.extend_from_slice(img.as_raw());
            let ifd = out.len();
            out[link..link + 4].copy_from_slice(&(ifd as u32).to_le_bytes());
            let tags: [(u16, u16, u32); 8] = [
                (256, 4, img.width()),
                (257, 4, img.height()),
                (258, 3, 8),
                (259, 3, 1),
                (262, 3, 1),
                (273, 4, data as u32),
                (278, 4, img.height()),
                (279, 4, img.width() * img.height()),
            ];
            out.extend_from_slice(&(tags.len() as u16).to_le_bytes());
            for (tag, ty, v) in tags {
                out.extend_from_slice(&tag.to_le_bytes());
                out.extend_from_slice(&ty.to_le_bytes());
                out.extend_from_slice(&1u32.to_le_bytes());
                if ty == 3 {
                    out.extend_from_slice(&(v as u16).to_le_bytes());
                    out.extend_from_slice(&[0, 0]);
                } else {
                    out.extend_from_slice(&v.to_le_bytes());
                }
            }
            link = out.len();
            out.extend_from_slice(&0u32.to_le_bytes());
        }
        out
    }

    #[test]
    fn every_page_of_a_multi_page_tiff_decodes() {
        let pages: Vec<image::GrayImage> = (0..3u8)
            .map(|k| {
                image::GrayImage::from_fn(9 + u32::from(k), 7, |x, _| {
                    image::Luma([k * 60 + x as u8])
                })
            })
            .collect();
        let t = tiff_bytes(&pages);
        let tp = tiff_pages(&t).unwrap();
        assert_eq!((tp.offsets.len(), tp.broken.as_deref()), (3, None));
        for (k, want) in pages.iter().enumerate() {
            let d = decode_tiff_page(&t, &tp, k, DecodeLimits::default()).unwrap();
            assert_eq!(d.format, SourceFormat::Tiff);
            assert_eq!(
                (d.image.width(), d.image.height()),
                (want.width() as usize, 7)
            );
            assert_eq!(d.image.get(2, 3), want.get_pixel(2, 3).0[0], "page {k}");
        }
        assert!(decode_tiff_page(&t, &tp, 3, DecodeLimits::default()).is_err());
        // A chain cut short: the pages before the damage remain, the loss is reported.
        let cut = &t[..tp.offsets[2] as usize + 4];
        let tp = tiff_pages(cut).unwrap();
        assert_eq!(tp.offsets.len(), 2);
        assert!(tp.broken.as_deref().unwrap().contains("page 3"), "{tp:?}");
        // A loop back to the first page stops.
        let mut looped = t.clone();
        let last = tp.offsets[1] as usize;
        let end = last + 2 + 8 * 12;
        let first = (tp.offsets[0] as u32).to_le_bytes();
        looped[end..end + 4].copy_from_slice(&first);
        let tl = tiff_pages(&looped).unwrap();
        assert_eq!(tl.offsets.len(), 2);
        assert!(tl.broken.is_some());
        assert!(tiff_pages(b"\x89PNG").is_none());
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
        // The page-area limit is checked from the header (SEC-11): 50 × 50 within each side
        // limit but over 2,000 pixels.
        let lim = DecodeLimits {
            max_pixels: 2_000,
            ..DecodeLimits::default()
        };
        match decode_with_limits(&bytes, lim) {
            Err(ImagingError::TooLarge(m)) => assert!(m.contains("megapixel"), "{m}"),
            other => panic!("{other:?}"),
        }
        assert!(decode(&bytes).is_ok());
    }
}
