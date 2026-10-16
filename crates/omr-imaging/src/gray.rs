//! The 8-bit gray image used by every measurement (255 = white, 0 = black).
//!
//! Pixel `(x, y)` covers `[x, x + 1) × [y, y + 1)` (corner convention), so its centre is at
//! `(x + 0.5, y + 0.5)`. Every point type in this crate and in `omr-recog` uses this convention.

use crate::ImagingError;
use std::fmt;

/// A row-major 8-bit gray image.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct GrayImage {
    width: usize,
    height: usize,
    data: Vec<u8>,
}

impl fmt::Debug for GrayImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GrayImage({}x{})", self.width, self.height)
    }
}

impl GrayImage {
    /// A `width × height` image filled with `fill`.
    pub fn new(width: usize, height: usize, fill: u8) -> Self {
        Self {
            width,
            height,
            data: vec![fill; width * height],
        }
    }

    /// Wraps row-major pixels; fails if `data.len() != width * height`.
    pub fn from_raw(width: usize, height: usize, data: Vec<u8>) -> Result<Self, ImagingError> {
        if width.checked_mul(height) != Some(data.len()) {
            return Err(ImagingError::BadDimensions {
                width,
                height,
                len: data.len(),
            });
        }
        Ok(Self {
            width,
            height,
            data,
        })
    }

    /// Builds an image from a per-pixel function `f(x, y)`.
    pub fn from_fn(width: usize, height: usize, mut f: impl FnMut(usize, usize) -> u8) -> Self {
        let mut data = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                data.push(f(x, y));
            }
        }
        Self {
            width,
            height,
            data,
        }
    }

    /// Width in pixels.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> usize {
        self.height
    }

    /// True if the image has no pixels.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Row-major pixels.
    pub fn as_raw(&self) -> &[u8] {
        &self.data
    }

    /// Consumes the image, returning its pixels.
    pub fn into_raw(self) -> Vec<u8> {
        self.data
    }

    /// Row `y` (panics if out of range).
    pub fn row(&self, y: usize) -> &[u8] {
        &self.data[y * self.width..(y + 1) * self.width]
    }

    /// Mutable row `y` (panics if out of range).
    pub fn row_mut(&mut self, y: usize) -> &mut [u8] {
        let w = self.width;
        &mut self.data[y * w..(y + 1) * w]
    }

    /// Pixel at `(x, y)` (panics if out of range).
    pub fn get(&self, x: usize, y: usize) -> u8 {
        self.data[y * self.width + x]
    }

    /// Pixel at signed `(x, y)`, or `outside` beyond the image.
    pub fn get_or(&self, x: isize, y: isize, outside: u8) -> u8 {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            outside
        } else {
            self.data[y as usize * self.width + x as usize]
        }
    }

    /// Sets the pixel at `(x, y)` (panics if out of range).
    pub fn set(&mut self, x: usize, y: usize, v: u8) {
        self.data[y * self.width + x] = v;
    }

    /// A copy of the rectangle at signed `(x0, y0)` of size `w × h`; pixels beyond the image
    /// read as `outside`.
    pub fn crop(&self, x0: isize, y0: isize, w: usize, h: usize, outside: u8) -> GrayImage {
        let mut out = GrayImage::new(w, h, outside);
        for yy in 0..h {
            let sy = y0 + yy as isize;
            if sy < 0 || sy as usize >= self.height {
                continue;
            }
            let src = self.row(sy as usize);
            let dst = out.row_mut(yy);
            let xa = x0.max(0);
            let xb = (x0 + w as isize).min(self.width as isize);
            if xb <= xa {
                continue;
            }
            let (da, db) = ((xa - x0) as usize, (xb - x0) as usize);
            dst[da..db].copy_from_slice(&src[xa as usize..xb as usize]);
        }
        out
    }

    /// The photometric inverse (`255 − v`): ink becomes bright.
    pub fn inverted(&self) -> GrayImage {
        GrayImage {
            width: self.width,
            height: self.height,
            data: self.data.iter().map(|&v| 255 - v).collect(),
        }
    }

    /// The 256-bin histogram.
    pub fn histogram(&self) -> [u64; 256] {
        let mut h = [0u64; 256];
        for &v in &self.data {
            h[v as usize] += 1;
        }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_pads_outside() {
        let img = GrayImage::from_fn(4, 3, |x, y| (x + 10 * y) as u8);
        let c = img.crop(-1, 1, 3, 3, 255);
        assert_eq!(c.row(0), &[255, 10, 11]);
        assert_eq!(c.row(1), &[255, 20, 21]);
        assert_eq!(c.row(2), &[255, 255, 255]);
    }

    #[test]
    fn from_raw_checks_len() {
        assert!(GrayImage::from_raw(2, 2, vec![0; 3]).is_err());
        assert!(GrayImage::from_raw(2, 2, vec![0; 4]).is_ok());
    }
}
