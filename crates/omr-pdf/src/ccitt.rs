//! CCITTFaxDecode (ITU-T T.4 / T.6) into a packed [`BitImage`], via `hayro-ccitt`.
//!
//! **Why `hayro-ccitt` and not `fax`.** `hayro-ccitt` 0.4 implements every PDF
//! `CCITTFaxDecode` parameter combination: `/K < 0` (Group 4), `/K = 0` (Group 3 1-D),
//! `/K > 0` (Group 3 mixed 1-D/2-D), `/EncodedByteAlign`, `/EndOfLine`, `/EndOfBlock`, and it
//! reports truncation and line-length errors instead of silently padding. `fax` 0.3 decodes
//! Group 4 and Group 3 1-D only (no 2-D Group 3, no byte-aligned rows without EOL) and pads
//! missing rows with white. On the 143 real IAU G4 pages both decoders produce bit-identical
//! images; the differential test (`tests/real_iau.rs`) times both and checks the equality.
//! `fax` stays a dev-dependency, used to encode synthetic Group 4 test fixtures.

use crate::PdfError;
use crate::raster::BitImage;
use hayro_ccitt::{DecodeSettings, Decoder, DecoderContext, EncodingMode};

/// The `/DecodeParms` of a `CCITTFaxDecode` filter (PDF 32000-1 Table 11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CcittParams {
    /// `/K`: < 0 Group 4, 0 Group 3 1-D, > 0 Group 3 mixed.
    pub k: i32,
    /// `/Columns` (defaults to the image `/Width` when absent; the PDF default 1728 is a common
    /// producer bug and never matches a page scan).
    pub columns: u32,
    /// `/Rows` (defaults to the image `/Height`).
    pub rows: u32,
    /// `/EndOfLine`.
    pub end_of_line: bool,
    /// `/EncodedByteAlign`.
    pub encoded_byte_align: bool,
    /// `/EndOfBlock` (default true).
    pub end_of_block: bool,
    /// `/BlackIs1` (default false: 0 bits are black).
    pub black_is_1: bool,
}

impl CcittParams {
    /// Group 4 defaults for a `width × height` image.
    pub fn group4(width: u32, height: u32) -> Self {
        Self {
            k: -1,
            columns: width,
            rows: height,
            end_of_line: false,
            encoded_byte_align: false,
            end_of_block: true,
            black_is_1: false,
        }
    }

    /// Human-readable coding scheme.
    pub fn scheme(&self) -> &'static str {
        match self.k {
            k if k < 0 => "G4",
            0 => "G3-1D",
            _ => "G3-2D",
        }
    }
}

/// Result of a CCITT decode.
#[derive(Debug)]
pub struct CcittDecoded {
    /// The image, 1 = CCITT black run (polarity mapping to ink is applied by the caller).
    pub image: BitImage,
    /// Rows actually present in the stream (the rest were padded white after an EOFB).
    pub rows_decoded: u32,
}

/// Writes CCITT runs into a packed bitmap: black runs set bits, white runs only advance.
struct Sink<'a> {
    img: &'a mut BitImage,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl Decoder for Sink<'_> {
    #[inline]
    fn push_pixels(&mut self, white: bool, count: u32) {
        let x0 = self.x;
        let x1 = x0.saturating_add(count).min(self.width);
        self.x = x1;
        if white || x1 <= x0 || self.y >= self.height {
            return;
        }
        let y = self.y;
        set_run(self.img.row_mut(y), x0 as usize, x1 as usize);
    }

    #[inline]
    fn next_line(&mut self) {
        self.x = 0;
        self.y = self.y.saturating_add(1);
    }
}

/// Sets bits `[x0, x1)` in a packed MSB-first row.
#[inline]
pub(crate) fn set_run(row: &mut [u8], x0: usize, x1: usize) {
    let (b0, b1) = (x0 >> 3, (x1 - 1) >> 3);
    let first = 0xFFu8 >> (x0 & 7);
    let last = 0xFFu8 << (7 - ((x1 - 1) & 7));
    if b0 == b1 {
        row[b0] |= first & last;
    } else {
        row[b0] |= first;
        for b in &mut row[b0 + 1..b1] {
            *b = 0xFF;
        }
        row[b1] |= last;
    }
}

/// Decodes a CCITT stream of a `width × height` image. Bits set = CCITT **black** runs.
///
/// Errors: a stream that ends or breaks before `height` rows (without an end-of-block marker)
/// is [`PdfError::Corrupt`]; geometry that does not fit is [`PdfError::TooLarge`].
pub fn decode(
    data: &[u8],
    width: u32,
    height: u32,
    p: &CcittParams,
) -> Result<CcittDecoded, PdfError> {
    if width == 0 || height == 0 || p.columns == 0 {
        return Err(PdfError::Corrupt("CCITT image with zero size".into()));
    }
    let columns = p.columns;
    let rows = if p.rows == 0 { height } else { p.rows };
    let mut img = BitImage::new(columns, rows.max(height))?;
    let settings = DecodeSettings {
        columns,
        rows,
        end_of_block: p.end_of_block,
        end_of_line: p.end_of_line,
        // With EOL markers, the alignment fill sits *before* each EOL (TIFF Group3Options
        // bit 2, pdf.js) and the EOL reader absorbs up to 24 fill zeros. Aligning after each row
        // as well would eat the EOL's leading zeros when it starts mid-byte (hayro-ccitt 0.4
        // then needs ≥ 11 zeros and fails), so alignment is only applied without EOLs.
        rows_are_byte_aligned: p.encoded_byte_align && !p.end_of_line,
        encoding: match p.k {
            k if k < 0 => EncodingMode::Group4,
            0 => EncodingMode::Group3_1D,
            k => EncodingMode::Group3_2D { k: k as u32 },
        },
        invert_black: false,
    };
    let mut ctx = DecoderContext::new(settings);
    let mut sink = Sink {
        img: &mut img,
        x: 0,
        y: 0,
        width: columns,
        height: rows,
    };
    let res = hayro_ccitt::decode(data, &mut sink, &mut ctx);
    let rows_done = sink.y.min(rows);
    match res {
        Ok(_) => {}
        // Trailing garbage after a complete image is tolerated; a short image is not.
        Err(_) if rows_done >= rows => {}
        Err(e) => {
            return Err(PdfError::Corrupt(format!(
                "CCITT {} stream broken after {rows_done} of {rows} rows: {e}",
                p.scheme()
            )));
        }
    }
    if rows_done == 0 {
        return Err(PdfError::Corrupt("CCITT stream decoded no rows".into()));
    }
    // A partially written last row (stream ended mid-row) would carry stray bits; rows after
    // `rows_done` were never written and stay white.
    let image = if columns == width && img.height() == height {
        img
    } else {
        crop_or_pad(&img, width, height)?
    };
    Ok(CcittDecoded {
        image,
        rows_decoded: rows_done,
    })
}

/// Copies the overlap of `src` into a white `width × height` image.
fn crop_or_pad(src: &BitImage, width: u32, height: u32) -> Result<BitImage, PdfError> {
    let mut out = BitImage::new(width, height)?;
    let w = width.min(src.width()) as usize;
    let full = w / 8;
    for y in 0..height.min(src.height()) {
        let s = src.row(y);
        let d = out.row_mut(y);
        d[..full].copy_from_slice(&s[..full]);
        if !w.is_multiple_of(8) {
            d[full] = s[full] & (0xFFu8 << (8 - w % 8));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_run_edges() {
        let mut r = [0u8; 4];
        set_run(&mut r, 3, 5);
        assert_eq!(r, [0b0001_1000, 0, 0, 0]);
        let mut r = [0u8; 4];
        set_run(&mut r, 6, 18);
        assert_eq!(r, [0b0000_0011, 0xFF, 0b1100_0000, 0]);
        let mut r = [0u8; 4];
        set_run(&mut r, 0, 32);
        assert_eq!(r, [0xFF; 4]);
        let mut r = [0u8; 2];
        set_run(&mut r, 7, 8);
        assert_eq!(r, [1, 0]);
    }
}
