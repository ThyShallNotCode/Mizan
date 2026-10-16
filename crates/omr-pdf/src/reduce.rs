//! Area-coverage reduction of 1-bit scans to 8-bit gray (spec §7.2 "sites that insist on
//! bitonal"; the IAU brief's step 1: 600 dpi 1-bit → 200 dpi coverage gray, 255 = white).
//!
//! Each output pixel is the white fraction of its source footprint, `round(255 · white/area)`
//! (round half up), so a 600 → 200 dpi reduction is a 3 × 3 box average of the 0/255 image.
//! Partial footprints at the right and bottom edges average only the source pixels they cover
//! (output size = ⌈w/f⌉ × ⌈h/f⌉), matching PIL's `Image.reduce`. Non-integer ratios use exact
//! integer area weights.
//!
//! `omr-imaging` has no such kernel yet, so it lives here (see README "Deviations").

use crate::PdfError;
use crate::raster::{BitImage, GrayImage8};

/// Gray value for `black` black pixels out of `area` (round half up).
#[inline]
fn gray(black: u32, area: u32) -> u8 {
    let white = area - black;
    // round(255·white/area) = floor((510·white + area) / (2·area))
    ((510 * white + area) / (2 * area)) as u8
}

/// For each 12-bit value, the black counts of its four 3-bit groups (MSB group first), one per
/// byte lane: lane 0 (least significant byte) = leftmost group.
const fn table12() -> [u32; 4096] {
    let mut t = [0u32; 4096];
    let mut v = 0;
    while v < 4096 {
        let mut lanes = 0u32;
        let mut g = 0;
        while g < 4 {
            let bits = (v >> (9 - 3 * g)) & 7;
            let c = (bits & 1) + ((bits >> 1) & 1) + ((bits >> 2) & 1);
            lanes |= (c as u32) << (8 * g);
            g += 1;
        }
        t[v] = lanes;
        v += 1;
    }
    t
}

static T12: [u32; 4096] = table12();

/// Reduces a 1-bit image by an integer factor `f` (≥ 1): each output pixel averages an `f × f`
/// block. 600 dpi → 200 dpi is `f = 3`.
pub fn reduce_bilevel(img: &BitImage, f: u32) -> Result<GrayImage8, PdfError> {
    if f == 0 {
        return Err(PdfError::Unsupported("reduction factor 0".into()));
    }
    if f == 1 {
        return Ok(img.to_gray());
    }
    if f == 3 {
        return reduce3(img);
    }
    reduce_generic(img, f)
}

/// Fast path for `f = 3` (SWAR over 24-bit chunks: 8 output pixels per 3 source bytes).
fn reduce3(img: &BitImage) -> Result<GrayImage8, PdfError> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let (ow, oh) = (w.div_ceil(3), h.div_ceil(3));
    let mut out = GrayImage8::new_white(ow as u32, oh as u32)?;
    // Chunks of 3 bytes that lie fully inside the row's stride.
    let chunks = (ow / 8).min(img.stride() / 3);
    let full_cols = chunks * 8; // output columns served by the SWAR loop
    let last_full_col = w / 3; // columns with a complete 3-pixel footprint
    let mut acc = vec![0u32; chunks * 2];
    let lut: [u8; 10] = std::array::from_fn(|b| gray(b as u32, 9));
    for oy in 0..oh {
        let y0 = oy * 3;
        let rows = (h - y0).min(3);
        acc.iter_mut().for_each(|a| *a = 0);
        for r in 0..rows {
            let row = img.row((y0 + r) as u32);
            for (k, pair) in acc.chunks_exact_mut(2).enumerate() {
                let i = 3 * k;
                let v = (u32::from(row[i]) << 16)
                    | (u32::from(row[i + 1]) << 8)
                    | u32::from(row[i + 2]);
                pair[0] += T12[(v >> 12) as usize];
                pair[1] += T12[(v & 0xFFF) as usize];
            }
        }
        let orow = &mut out.as_raw_mut()[oy * ow..(oy + 1) * ow];
        let area_row = rows as u32;
        for (k, lanes) in acc.iter().enumerate() {
            for lane in 0..4 {
                let col = 4 * k + lane;
                let black = (lanes >> (8 * lane)) & 0xFF;
                orow[col] = if rows == 3 && col < last_full_col {
                    lut[black as usize]
                } else {
                    let cw = (w - 3 * col).min(3) as u32;
                    gray(black, cw * area_row)
                };
            }
        }
        // Remaining columns (row tail): count per pixel.
        for (col, o) in orow.iter_mut().enumerate().skip(full_cols) {
            let x0 = 3 * col;
            let cw = (w - x0).min(3);
            let mut black = 0u32;
            for r in 0..rows {
                let row = img.row((y0 + r) as u32);
                for x in x0..x0 + cw {
                    black += u32::from(row[x >> 3] >> (7 - (x & 7)) & 1);
                }
            }
            *o = gray(black, cw as u32 * area_row);
        }
    }
    Ok(out)
}

/// Any integer factor: per-row column counts via 64-bit windows.
fn reduce_generic(img: &BitImage, f: u32) -> Result<GrayImage8, PdfError> {
    let f = f as usize;
    let (w, h) = (img.width() as usize, img.height() as usize);
    let (ow, oh) = (w.div_ceil(f), h.div_ceil(f));
    let mut out = GrayImage8::new_white(ow as u32, oh as u32)?;
    let mut acc = vec![0u32; ow];
    for oy in 0..oh {
        let y0 = oy * f;
        let rows = (h - y0).min(f);
        acc.iter_mut().for_each(|a| *a = 0);
        for r in 0..rows {
            let row = img.row((y0 + r) as u32);
            for (col, a) in acc.iter_mut().enumerate() {
                let x0 = col * f;
                let x1 = (x0 + f).min(w);
                *a += count_bits(row, x0, x1);
            }
        }
        let orow = &mut out.as_raw_mut()[oy * ow..(oy + 1) * ow];
        for (col, (o, &black)) in orow.iter_mut().zip(&acc).enumerate() {
            let cw = (w - col * f).min(f);
            *o = gray(black, (cw * rows) as u32);
        }
    }
    Ok(out)
}

/// Black pixels in `[x0, x1)` of a packed row.
#[inline]
fn count_bits(row: &[u8], x0: usize, x1: usize) -> u32 {
    let mut n = 0;
    let mut x = x0;
    while x < x1 {
        let byte = x >> 3;
        let off = x & 7;
        let take = (8 - off).min(x1 - x);
        let bits = (row[byte] << off) >> (8 - take);
        n += bits.count_ones();
        x += take;
    }
    n
}

/// Reduces a 1-bit image from `src_dpi` to `dst_dpi` by exact area coverage. Integer ratios use
/// [`reduce_bilevel`]; others weight each source pixel by its overlap with the output pixel.
/// `dst_dpi ≥ src_dpi` only expands to 0/255 at the native resolution.
pub fn reduce_bilevel_to_dpi(
    img: &BitImage,
    src_dpi: u32,
    dst_dpi: u32,
) -> Result<GrayImage8, PdfError> {
    if src_dpi == 0 || dst_dpi == 0 {
        return Err(PdfError::Unsupported("zero dpi".into()));
    }
    if dst_dpi >= src_dpi {
        return Ok(img.to_gray());
    }
    if src_dpi.is_multiple_of(dst_dpi) {
        return reduce_bilevel(img, src_dpi / dst_dpi);
    }
    reduce_rational(img, src_dpi as u64, dst_dpi as u64)
}

/// Source pixel `i` spans `[i·d, (i+1)·d)`, output pixel `j` spans `[j·s, (j+1)·s)` in common
/// units; weights are the integer overlaps.
fn spans(n_src: usize, s: u64, d: u64) -> Vec<Vec<(usize, u64)>> {
    let n_out = ((n_src as u64 * d).div_ceil(s)) as usize;
    let mut out = Vec::with_capacity(n_out);
    for j in 0..n_out as u64 {
        let (a, b) = (j * s, ((j + 1) * s).min(n_src as u64 * d));
        let mut v = Vec::new();
        let mut i = a / d;
        while i * d < b {
            let lo = (i * d).max(a);
            let hi = ((i + 1) * d).min(b);
            if hi > lo {
                v.push((i as usize, hi - lo));
            }
            i += 1;
        }
        out.push(v);
    }
    out
}

fn reduce_rational(img: &BitImage, s: u64, d: u64) -> Result<GrayImage8, PdfError> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let xs = spans(w, s, d);
    let ys = spans(h, s, d);
    let (ow, oh) = (xs.len(), ys.len());
    let mut out = GrayImage8::new_white(ow as u32, oh as u32)?;
    let wx: Vec<u64> = xs.iter().map(|v| v.iter().map(|p| p.1).sum()).collect();
    let mut hrow = vec![0u64; ow];
    let mut acc = vec![0u64; ow];
    for (oy, yspan) in ys.iter().enumerate() {
        acc.iter_mut().for_each(|a| *a = 0);
        let wy: u64 = yspan.iter().map(|p| p.1).sum();
        for &(sy, wyi) in yspan {
            let row = img.row(sy as u32);
            for (j, xspan) in xs.iter().enumerate() {
                hrow[j] = xspan
                    .iter()
                    .filter(|(sx, _)| row[sx >> 3] & (0x80 >> (sx & 7)) != 0)
                    .map(|p| p.1)
                    .sum();
            }
            for (a, hv) in acc.iter_mut().zip(&hrow) {
                *a += hv * wyi;
            }
        }
        let orow = &mut out.as_raw_mut()[oy * ow..(oy + 1) * ow];
        for (j, o) in orow.iter_mut().enumerate() {
            let area = wx[j] * wy;
            let white = area - acc[j];
            *o = ((510 * white as u128 + area as u128) / (2 * area as u128)) as u8;
        }
    }
    Ok(out)
}

/// Box-averages an 8-bit gray image by an integer factor (edge blocks average what they cover).
pub fn reduce_gray(img: &GrayImage8, f: u32) -> Result<GrayImage8, PdfError> {
    if f <= 1 {
        return Ok(img.clone());
    }
    let f = f as usize;
    let (w, h) = (img.width() as usize, img.height() as usize);
    let (ow, oh) = (w.div_ceil(f), h.div_ceil(f));
    let mut out = GrayImage8::new_white(ow as u32, oh as u32)?;
    let src = img.as_raw();
    let mut acc = vec![0u32; ow];
    for oy in 0..oh {
        let y0 = oy * f;
        let rows = (h - y0).min(f);
        acc.iter_mut().for_each(|a| *a = 0);
        for r in 0..rows {
            let row = &src[(y0 + r) * w..(y0 + r + 1) * w];
            for (x, &v) in row.iter().enumerate() {
                acc[x / f] += u32::from(v);
            }
        }
        let orow = &mut out.as_raw_mut()[oy * ow..(oy + 1) * ow];
        for (col, o) in orow.iter_mut().enumerate() {
            let n = ((w - col * f).min(f) * rows) as u32;
            *o = ((2 * acc[col] + n) / (2 * n)) as u8;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive(img: &BitImage, f: usize) -> GrayImage8 {
        let (w, h) = (img.width() as usize, img.height() as usize);
        let (ow, oh) = (w.div_ceil(f), h.div_ceil(f));
        let mut data = vec![0u8; ow * oh];
        for oy in 0..oh {
            for ox in 0..ow {
                let (mut b, mut n) = (0u32, 0u32);
                for y in oy * f..((oy + 1) * f).min(h) {
                    for x in ox * f..((ox + 1) * f).min(w) {
                        n += 1;
                        b += u32::from(img.get(x as u32, y as u32));
                    }
                }
                data[oy * ow + ox] = gray(b, n);
            }
        }
        GrayImage8::from_raw(ow as u32, oh as u32, data).unwrap()
    }

    fn pattern(w: u32, h: u32, seed: u32) -> BitImage {
        BitImage::from_fn(w, h, |x, y| {
            let v = x.wrapping_mul(2654435761) ^ y.wrapping_mul(40503) ^ seed;
            (v >> 7).is_multiple_of(3)
        })
        .unwrap()
    }

    #[test]
    fn gray_table_matches_pil_reduce3() {
        // PIL's ImagingReduce3x3: ((sum + 4) · 1864135) >> 24 with sum = 255 · white.
        for white in 0..=9u32 {
            let pil = (((255 * white + 4) as u64 * 1_864_135) >> 24) as u8;
            assert_eq!(gray(9 - white, 9), pil, "white = {white}");
        }
    }

    #[test]
    fn reduce3_matches_naive_on_odd_sizes() {
        for (w, h) in [
            (1, 1),
            (7, 5),
            (24, 3),
            (25, 4),
            (100, 31),
            (191, 17),
            (200, 200),
        ] {
            let img = pattern(w, h, w * 31 + h);
            assert_eq!(reduce3(&img).unwrap(), naive(&img, 3), "{w}x{h}");
        }
    }

    #[test]
    fn generic_factors_match_naive() {
        for f in [2u32, 4, 5, 8] {
            for (w, h) in [(9, 7), (64, 33), (101, 50)] {
                let img = pattern(w, h, f);
                assert_eq!(
                    reduce_bilevel(&img, f).unwrap(),
                    naive(&img, f as usize),
                    "f={f} {w}x{h}"
                );
            }
        }
    }

    #[test]
    fn rational_reduction_is_area_exact() {
        // 300 → 200 dpi: 3 source px per 2 output px; a black/white column pair alternates.
        let img = BitImage::from_fn(6, 3, |x, _| x % 3 == 0).unwrap();
        let g = reduce_bilevel_to_dpi(&img, 300, 200).unwrap();
        assert_eq!((g.width(), g.height()), (4, 2));
        // Output px 0 covers src x ∈ [0, 1.5): 1 black + ½ white → 1/3 white → 85.
        assert_eq!(g.get(0, 0), gray(2, 3));
        assert_eq!(g.get(1, 0), 255);
        // Integer ratios route to the box path.
        let p = pattern(30, 30, 9);
        assert_eq!(reduce_bilevel_to_dpi(&p, 600, 200).unwrap(), naive(&p, 3));
        // All-black stays black, all-white stays white at any ratio.
        let black = BitImage::from_fn(17, 13, |_, _| true).unwrap();
        assert!(
            reduce_bilevel_to_dpi(&black, 400, 300)
                .unwrap()
                .as_raw()
                .iter()
                .all(|&v| v == 0)
        );
    }

    #[test]
    fn gray_box_reduce() {
        let g = GrayImage8::from_raw(3, 2, vec![0, 255, 100, 255, 0, 101]).unwrap();
        let r = reduce_gray(&g, 2).unwrap();
        assert_eq!(r.as_raw(), &[128, 101]);
    }
}
