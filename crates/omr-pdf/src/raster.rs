//! Raster types returned by the extractor: packed 1-bit images, 8-bit gray images and
//! pass-through JPEG streams.
//!
//! **Polarity.** [`BitImage`] stores **1 = black (ink)**, MSB first within each byte, so that
//! coverage is a popcount. [`GrayImage8`] stores 0 = black, 255 = white. Both are row-major with
//! the origin at the top-left of the *stored* image (PDF image space, before page rotation).

use crate::PdfError;
use serde::{Deserialize, Serialize};

/// Bytes per row are padded to a multiple of this many bytes (keeps word-wise kernels in bounds).
const ROW_ALIGN: usize = 8;

/// A packed bilevel image: 1 bit per pixel, **1 = black**, MSB first, rows padded to whole
/// 8-byte words. Padding bits are always 0 (white).
#[derive(Clone, PartialEq, Eq)]
pub struct BitImage {
    width: u32,
    height: u32,
    stride: usize,
    data: Vec<u8>,
}

impl std::fmt::Debug for BitImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "BitImage({}x{})", self.width, self.height)
    }
}

impl BitImage {
    /// An all-white image. Fails (instead of aborting) when the buffer cannot be reserved.
    pub fn new(width: u32, height: u32) -> Result<Self, PdfError> {
        let stride = (width as usize).div_ceil(8).div_ceil(ROW_ALIGN) * ROW_ALIGN;
        let len = stride
            .checked_mul(height as usize)
            .ok_or(PdfError::TooLarge { width, height })?;
        let mut data = Vec::new();
        data.try_reserve_exact(len)
            .map_err(|_| PdfError::TooLarge { width, height })?;
        data.resize(len, 0);
        Ok(Self {
            width,
            height,
            stride,
            data,
        })
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Bytes per row (≥ ⌈width/8⌉, a multiple of 8).
    pub fn stride(&self) -> usize {
        self.stride
    }

    /// The packed rows (`height × stride` bytes).
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// One packed row, including padding bytes.
    pub fn row(&self, y: u32) -> &[u8] {
        let s = y as usize * self.stride;
        &self.data[s..s + self.stride]
    }

    /// One packed row, mutable. Callers must keep padding bits 0.
    pub(crate) fn row_mut(&mut self, y: u32) -> &mut [u8] {
        let s = y as usize * self.stride;
        &mut self.data[s..s + self.stride]
    }

    /// True when the pixel is black. Out-of-range pixels read as white.
    pub fn get(&self, x: u32, y: u32) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        let b = self.data[y as usize * self.stride + (x as usize >> 3)];
        b & (0x80 >> (x & 7)) != 0
    }

    /// Sets one pixel (true = black). Out-of-range writes are ignored.
    pub fn set(&mut self, x: u32, y: u32, black: bool) {
        if x >= self.width || y >= self.height {
            return;
        }
        let i = y as usize * self.stride + (x as usize >> 3);
        let m = 0x80u8 >> (x & 7);
        if black {
            self.data[i] |= m;
        } else {
            self.data[i] &= !m;
        }
    }

    /// Number of black pixels.
    pub fn black_count(&self) -> u64 {
        self.data.iter().map(|b| u64::from(b.count_ones())).sum()
    }

    /// The rows packed to exactly ⌈width/8⌉ bytes each (no word padding), 1 = black.
    /// This is the layout used by PBM and by the differential tests' fingerprints.
    pub fn to_packed_rows(&self) -> Vec<u8> {
        let rb = (self.width as usize).div_ceil(8);
        let mut out = Vec::with_capacity(rb * self.height as usize);
        for y in 0..self.height {
            out.extend_from_slice(&self.row(y)[..rb]);
        }
        out
    }

    /// Inverts every pixel, keeping padding bits white.
    pub fn invert(&mut self) {
        for b in &mut self.data {
            *b = !*b;
        }
        self.clear_padding();
    }

    /// Forces all padding bits (beyond `width` in each row) to 0.
    pub(crate) fn clear_padding(&mut self) {
        let w = self.width as usize;
        let full = w / 8;
        let rem = w % 8;
        for y in 0..self.height {
            let row = self.row_mut(y);
            let mut start = full;
            if rem != 0 {
                row[full] &= 0xFFu8 << (8 - rem);
                start += 1;
            }
            for b in &mut row[start..] {
                *b = 0;
            }
        }
    }

    /// Expands to 8-bit gray: black → 0, white → 255.
    pub fn to_gray(&self) -> GrayImage8 {
        let w = self.width as usize;
        let mut data = vec![255u8; w * self.height as usize];
        for y in 0..self.height {
            let row = self.row(y);
            let out = &mut data[y as usize * w..(y as usize + 1) * w];
            for (x, px) in out.iter_mut().enumerate() {
                if row[x >> 3] & (0x80 >> (x & 7)) != 0 {
                    *px = 0;
                }
            }
        }
        GrayImage8 {
            width: self.width,
            height: self.height,
            data,
        }
    }

    /// The image rotated clockwise by `quarter_turns × 90°` (then mirrored horizontally if
    /// `mirror`), i.e. the [`Orientation`] that maps stored pixels to the displayed page.
    pub fn oriented(&self, o: Orientation) -> Result<BitImage, PdfError> {
        if o.quarter_turns_cw.is_multiple_of(4) && !o.mirrored {
            return Ok(self.clone());
        }
        let (w, h) = (self.width, self.height);
        let (nw, nh) = if o.quarter_turns_cw % 2 == 1 {
            (h, w)
        } else {
            (w, h)
        };
        let mut out = BitImage::new(nw, nh)?;
        for y in 0..h {
            let row = self.row(y);
            for x in 0..w {
                if row[(x >> 3) as usize] & (0x80 >> (x & 7)) != 0 {
                    let (nx, ny) = o.map(x, y, w, h);
                    out.set(nx, ny, true);
                }
            }
        }
        Ok(out)
    }

    /// Builds an image from rows of booleans (true = black). Test and tooling helper.
    pub fn from_fn(
        width: u32,
        height: u32,
        f: impl Fn(u32, u32) -> bool,
    ) -> Result<Self, PdfError> {
        let mut img = Self::new(width, height)?;
        for y in 0..height {
            for x in 0..width {
                if f(x, y) {
                    img.set(x, y, true);
                }
            }
        }
        Ok(img)
    }
}

/// An 8-bit gray image, row-major, stride = width; 0 = black, 255 = white.
#[derive(Clone, PartialEq, Eq)]
pub struct GrayImage8 {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

impl std::fmt::Debug for GrayImage8 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GrayImage8({}x{})", self.width, self.height)
    }
}

impl GrayImage8 {
    /// Wraps a row-major buffer; `None` when the length does not match.
    pub fn from_raw(width: u32, height: u32, data: Vec<u8>) -> Option<Self> {
        (data.len() == width as usize * height as usize).then_some(Self {
            width,
            height,
            data,
        })
    }

    /// A white image, reserving fallibly.
    pub fn new_white(width: u32, height: u32) -> Result<Self, PdfError> {
        let len = (width as usize)
            .checked_mul(height as usize)
            .ok_or(PdfError::TooLarge { width, height })?;
        let mut data = Vec::new();
        data.try_reserve_exact(len)
            .map_err(|_| PdfError::TooLarge { width, height })?;
        data.resize(len, 255);
        Ok(Self {
            width,
            height,
            data,
        })
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The pixel buffer (row-major).
    pub fn as_raw(&self) -> &[u8] {
        &self.data
    }

    /// Mutable pixel buffer (row-major).
    pub fn as_raw_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    /// Consumes the image, returning the buffer (e.g. for `image::GrayImage::from_raw`).
    pub fn into_raw(self) -> Vec<u8> {
        self.data
    }

    /// One pixel.
    pub fn get(&self, x: u32, y: u32) -> u8 {
        self.data[y as usize * self.width as usize + x as usize]
    }

    /// The image under an [`Orientation`] (rotate clockwise, then mirror).
    pub fn oriented(&self, o: Orientation) -> GrayImage8 {
        if o.quarter_turns_cw.is_multiple_of(4) && !o.mirrored {
            return self.clone();
        }
        let (w, h) = (self.width, self.height);
        let (nw, nh) = if o.quarter_turns_cw % 2 == 1 {
            (h, w)
        } else {
            (w, h)
        };
        let mut data = vec![0u8; self.data.len()];
        for y in 0..h {
            for x in 0..w {
                let (nx, ny) = o.map(x, y, w, h);
                data[ny as usize * nw as usize + nx as usize] =
                    self.data[y as usize * w as usize + x as usize];
            }
        }
        GrayImage8 {
            width: nw,
            height: nh,
            data,
        }
    }
}

/// How stored image pixels map to the displayed page: rotate clockwise by
/// `quarter_turns_cw × 90°`, then mirror left↔right if `mirrored`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Orientation {
    /// Clockwise quarter turns (0–3).
    pub quarter_turns_cw: u8,
    /// Horizontal mirror after the rotation (a reflected placement CTM; rare).
    pub mirrored: bool,
}

impl Orientation {
    /// No rotation, no mirror.
    pub const UPRIGHT: Self = Self {
        quarter_turns_cw: 0,
        mirrored: false,
    };

    /// Maps a stored pixel `(x, y)` of a `w × h` image to its displayed position.
    pub fn map(self, x: u32, y: u32, w: u32, h: u32) -> (u32, u32) {
        let (rx, ry, rw) = match self.quarter_turns_cw % 4 {
            0 => (x, y, w),
            1 => (h - 1 - y, x, h),
            2 => (w - 1 - x, h - 1 - y, w),
            _ => (y, w - 1 - x, h),
        };
        if self.mirrored {
            (rw - 1 - rx, ry)
        } else {
            (rx, ry)
        }
    }
}

/// A DCT (JPEG) image stream passed through without decoding (decode happens in `omr-imaging`).
#[derive(Clone, PartialEq, Eq)]
pub struct JpegStream {
    /// The complete JPEG file bytes as stored in the PDF.
    pub data: Vec<u8>,
    /// Width from the JPEG SOF marker.
    pub width: u32,
    /// Height from the JPEG SOF marker.
    pub height: u32,
    /// Colour components from the SOF marker (1 gray, 3 YCbCr/RGB, 4 CMYK/YCCK).
    pub components: u8,
    /// Progressive (SOF2) coding.
    pub progressive: bool,
    /// The decoded samples must be inverted (`/Decode [1 0 …]`).
    pub invert: bool,
    /// Adobe APP14 transform flag, when present (0 = none/CMYK, 1 = YCbCr, 2 = YCCK).
    pub adobe_transform: Option<u8>,
}

impl std::fmt::Debug for JpegStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "JpegStream({}x{}, {} comp, {} bytes)",
            self.width,
            self.height,
            self.components,
            self.data.len()
        )
    }
}

/// The page's scan image as extracted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageRaster {
    /// 1-bit image (CCITT, 1-bit Flate/raw, image masks), 1 = black.
    Bilevel(BitImage),
    /// 8-bit gray (Flate/raw gray, RGB converted to luma, indexed, 2/4/16-bit).
    Gray(GrayImage8),
    /// JPEG bytes passed through.
    Jpeg(JpegStream),
}

impl PageRaster {
    /// Stored width in pixels.
    pub fn width(&self) -> u32 {
        match self {
            PageRaster::Bilevel(b) => b.width(),
            PageRaster::Gray(g) => g.width(),
            PageRaster::Jpeg(j) => j.width,
        }
    }

    /// Stored height in pixels.
    pub fn height(&self) -> u32 {
        match self {
            PageRaster::Bilevel(b) => b.height(),
            PageRaster::Gray(g) => g.height(),
            PageRaster::Jpeg(j) => j.height,
        }
    }

    /// True for a 1-bit raster (bitonal capture, CAP-003 `bitonal` downstream).
    pub fn is_bilevel(&self) -> bool {
        matches!(self, PageRaster::Bilevel(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_and_padding() {
        let mut b = BitImage::new(13, 3).unwrap();
        assert_eq!(b.stride(), 8);
        b.set(0, 0, true);
        b.set(12, 2, true);
        assert!(b.get(0, 0) && b.get(12, 2) && !b.get(1, 0));
        assert_eq!(b.black_count(), 2);
        b.invert();
        assert_eq!(b.black_count(), 13 * 3 - 2);
        assert!(!b.get(13, 0), "out of range reads white");
        let packed = b.to_packed_rows();
        assert_eq!(packed.len(), 2 * 3);
        assert_eq!(packed[1] & 0b0000_0111, 0, "padding bits stay white");
    }

    #[test]
    fn orientation_round_trips() {
        let b = BitImage::from_fn(5, 3, |x, y| (x + 2 * y) % 3 == 0).unwrap();
        for q in 0..4u8 {
            for m in [false, true] {
                let o = Orientation {
                    quarter_turns_cw: q,
                    mirrored: m,
                };
                let r = b.oriented(o).unwrap();
                let g = b.to_gray().oriented(o);
                assert_eq!(r.to_gray(), g);
                for y in 0..3 {
                    for x in 0..5 {
                        let (nx, ny) = o.map(x, y, 5, 3);
                        assert_eq!(r.get(nx, ny), b.get(x, y));
                    }
                }
            }
        }
        // 90° clockwise: the top-left pixel goes to the top-right corner.
        let one = BitImage::from_fn(5, 3, |x, y| x == 0 && y == 0).unwrap();
        let r = one
            .oriented(Orientation {
                quarter_turns_cw: 1,
                mirrored: false,
            })
            .unwrap();
        assert_eq!((r.width(), r.height()), (3, 5));
        assert!(r.get(2, 0));
    }
}
