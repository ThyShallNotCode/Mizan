//! Bounded FlateDecode (zlib or raw deflate) and the PNG/TIFF predictors (PDF 32000-1 §7.4.4).
//!
//! The inflater (`miniz_oxide`, `#![forbid(unsafe_code)]`) never produces more than the
//! image's declared size plus one row, so a decompression bomb costs at most that much memory
//! (spec §7.4 input limits).

use crate::PdfError;
use miniz_oxide::inflate::{
    TINFLStatus, decompress_to_vec_with_limit, decompress_to_vec_zlib_with_limit,
};

/// `/DecodeParms` of a FlateDecode/LZWDecode filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PredictorParams {
    /// `/Predictor` (1 none, 2 TIFF, 10–15 PNG).
    pub predictor: i32,
    /// `/Colors` (default 1).
    pub colors: u32,
    /// `/BitsPerComponent` (default 8).
    pub bits_per_component: u32,
    /// `/Columns` (default 1).
    pub columns: u32,
}

impl Default for PredictorParams {
    fn default() -> Self {
        Self {
            predictor: 1,
            colors: 1,
            bits_per_component: 8,
            columns: 1,
        }
    }
}

impl PredictorParams {
    fn row_bytes(&self) -> usize {
        (self.colors as usize * self.bits_per_component as usize * self.columns as usize)
            .div_ceil(8)
    }

    fn bpp(&self) -> usize {
        (self.colors as usize * self.bits_per_component as usize)
            .div_ceil(8)
            .max(1)
    }
}

/// Outcome of a bounded inflate.
pub struct Inflated {
    /// The data, already un-predicted, at most `expected` bytes.
    pub data: Vec<u8>,
    /// The stream was damaged (truncated, bad checksum) but yielded every expected byte.
    pub damaged: bool,
}

/// Inflates `input` expecting `expected` bytes of *unpredicted* output (`rows × row_bytes`).
pub fn inflate(input: &[u8], expected: usize, p: &PredictorParams) -> Result<Inflated, PdfError> {
    let png = p.predictor >= 10;
    let rows = if p.row_bytes() == 0 {
        0
    } else {
        expected.div_ceil(p.row_bytes())
    };
    // PNG rows carry one filter byte each; allow one row of slack beyond that.
    let raw_expected = if png { expected + rows } else { expected };
    let limit = raw_expected + p.row_bytes() + 1;
    let (raw, damaged) = match decompress_to_vec_zlib_with_limit(input, limit) {
        Ok(v) => (v, false),
        Err(e) if e.status == TINFLStatus::HasMoreOutput => (e.output, false),
        Err(e) => {
            // Not zlib-wrapped, or damaged: try raw deflate, then keep the longer prefix.
            match decompress_to_vec_with_limit(input, limit) {
                Ok(v) => (v, false),
                Err(e2) if e2.status == TINFLStatus::HasMoreOutput => (e2.output, false),
                Err(e2) => {
                    let best = if e2.output.len() > e.output.len() {
                        e2.output
                    } else {
                        e.output
                    };
                    (best, true)
                }
            }
        }
    };
    if raw.len() < raw_expected {
        return Err(PdfError::Corrupt(format!(
            "Flate stream yields {} of {raw_expected} bytes",
            raw.len()
        )));
    }
    let mut data = unpredict(raw, p)?;
    if data.len() < expected {
        return Err(PdfError::Corrupt("predictor output too short".into()));
    }
    data.truncate(expected);
    Ok(Inflated { data, damaged })
}

/// Undoes the predictor in place (PNG) or into a new buffer.
pub fn unpredict(mut raw: Vec<u8>, p: &PredictorParams) -> Result<Vec<u8>, PdfError> {
    match p.predictor {
        0 | 1 => Ok(raw),
        2 => {
            if p.bits_per_component != 8 {
                return Err(PdfError::Unsupported(format!(
                    "TIFF predictor with {} bits per component",
                    p.bits_per_component
                )));
            }
            let rb = p.row_bytes();
            let c = p.colors as usize;
            for row in raw.chunks_mut(rb) {
                for i in c..row.len() {
                    row[i] = row[i].wrapping_add(row[i - c]);
                }
            }
            Ok(raw)
        }
        10..=15 => {
            let rb = p.row_bytes();
            let bpp = p.bpp();
            let n_rows = raw.len() / (rb + 1);
            let mut out = vec![0u8; n_rows * rb];
            let mut prev = vec![0u8; rb];
            for r in 0..n_rows {
                let src = &raw[r * (rb + 1)..(r + 1) * (rb + 1)];
                let (ft, line) = (src[0], &src[1..]);
                let cur = &mut out[r * rb..(r + 1) * rb];
                cur.copy_from_slice(line);
                match ft {
                    0 => {}
                    1 => {
                        for i in bpp..rb {
                            cur[i] = cur[i].wrapping_add(cur[i - bpp]);
                        }
                    }
                    2 => {
                        for i in 0..rb {
                            cur[i] = cur[i].wrapping_add(prev[i]);
                        }
                    }
                    3 => {
                        for i in 0..rb {
                            let left = if i >= bpp { u16::from(cur[i - bpp]) } else { 0 };
                            cur[i] = cur[i].wrapping_add(((left + u16::from(prev[i])) / 2) as u8);
                        }
                    }
                    4 => {
                        for i in 0..rb {
                            let a = if i >= bpp { cur[i - bpp] } else { 0 };
                            let c = if i >= bpp { prev[i - bpp] } else { 0 };
                            cur[i] = cur[i].wrapping_add(paeth(a, prev[i], c));
                        }
                    }
                    t => {
                        return Err(PdfError::Corrupt(format!("PNG row filter type {t}")));
                    }
                }
                prev.copy_from_slice(cur);
            }
            raw.clear();
            Ok(out)
        }
        other => Err(PdfError::Unsupported(format!("predictor {other}"))),
    }
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (ia, ib, ic) = (i16::from(a), i16::from(b), i16::from(c));
    let p = ia + ib - ic;
    let (pa, pb, pc) = ((p - ia).abs(), (p - ib).abs(), (p - ic).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use miniz_oxide::deflate::compress_to_vec_zlib;

    #[test]
    fn round_trip_and_bomb_limit() {
        let data: Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8).collect();
        let z = compress_to_vec_zlib(&data, 6);
        let out = inflate(&z, data.len(), &PredictorParams::default()).unwrap();
        assert_eq!(out.data, data);
        assert!(!out.damaged);
        // A stream far larger than declared stops at the limit (no unbounded allocation).
        let big = vec![0u8; 50_000_000];
        let zb = compress_to_vec_zlib(&big, 6);
        let out = inflate(&zb, 1000, &PredictorParams::default()).unwrap();
        assert_eq!(out.data.len(), 1000);
        // Too short → corrupt.
        assert!(inflate(&z, data.len() + 5000, &PredictorParams::default()).is_err());
    }

    #[test]
    fn truncated_stream_with_all_bytes_is_damaged_but_usable() {
        let data: Vec<u8> = (0..4096u32).map(|i| (i * 7 % 256) as u8).collect();
        let z = compress_to_vec_zlib(&data, 6);
        let cut = &z[..z.len() - 4]; // drop the Adler-32 checksum
        let out = inflate(cut, data.len(), &PredictorParams::default()).unwrap();
        assert_eq!(out.data, data);
    }

    #[test]
    fn png_predictors() {
        // Two rows of 4 gray bytes: Sub then Up then Average then Paeth.
        let p = PredictorParams {
            predictor: 15,
            colors: 1,
            bits_per_component: 8,
            columns: 4,
        };
        let raw = vec![
            1, 10, 1, 1, 1, // Sub: 10 11 12 13
            2, 1, 1, 1, 1, // Up: 11 12 13 14
            3, 0, 0, 0, 0, // Average: 5, (5+12)/2=8, (8+13)/2=10, (10+14)/2=12
            4, 1, 1, 1, 1, // Paeth
        ];
        let out = unpredict(raw, &p).unwrap();
        assert_eq!(&out[..12], &[10, 11, 12, 13, 11, 12, 13, 14, 5, 8, 10, 12]);
        assert_eq!(out.len(), 16);
        let tiff = PredictorParams {
            predictor: 2,
            colors: 1,
            bits_per_component: 8,
            columns: 3,
        };
        assert_eq!(
            unpredict(vec![5, 1, 1, 7, 0, 1], &tiff).unwrap(),
            vec![5, 6, 7, 7, 7, 8]
        );
    }
}
