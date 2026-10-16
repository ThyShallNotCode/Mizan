//! Synthetic scan crops for the demo backend: bubble rows and page thumbnails drawn with
//! `tiny-skia` and a little per-pixel raster code. No real scan is used anywhere: paper grain,
//! printed bubbles, letters (a tiny stroke font) and pencil marks are all procedural and seeded,
//! so every crop is reproducible byte for byte.

use crate::backend::CropImage;
use std::sync::Arc;
use tiny_skia::{Color, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

/// How a bubble was marked on the synthetic paper.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MarkStyle {
    /// Untouched.
    Empty,
    /// A pencil fill of the given darkness (0..1), covering most of the bubble.
    Filled(f32),
    /// A light, patchy fill (a hesitant mark).
    Light(f32),
    /// Eraser residue: a wide, streaky, light smear.
    Erased(f32),
    /// A scanner streak or smudge running over the bubble.
    Smudge,
    /// A tick instead of a fill.
    Tick,
    /// A dark vertical scanner streak (dirty glass) through the bubble.
    Streak,
}

/// One bubble row to draw.
#[derive(Debug, Clone)]
pub struct RowSpec {
    pub question: u32,
    pub labels: Vec<char>,
    pub marks: Vec<MarkStyle>,
    pub seed: u64,
}

/// Geometry of the drawn row, in crop pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct RowGeometry {
    pub centres: Vec<(f32, f32)>,
    pub radius: f32,
}

/// Crop size of a bubble row (about 54 × 14 mm at 300 dpi).
pub const ROW_W: u32 = 640;
pub const ROW_H: u32 = 168;
const BUBBLE_R: f32 = 23.0;
const FIRST_CX: f32 = 168.0;
const PITCH: f32 = 96.0;
const ROW_CY: f32 = 80.0;

const PAPER: [u8; 3] = [0xF2, 0xEF, 0xE8];
const PRINT: [u8; 3] = [0x8C, 0x87, 0x80];
const INK: [u8; 3] = [0x3A, 0x3D, 0x42];
const GRAPHITE: [f32; 3] = [43.0, 46.0, 49.0];

/// Draws a registered answer row; returns the image and the bubble geometry.
pub fn answer_row(spec: &RowSpec) -> (CropImage, RowGeometry) {
    let mut pm = Pixmap::new(ROW_W, ROW_H).expect("pixmap");
    paper(&mut pm, spec.seed, PAPER);
    let centres: Vec<(f32, f32)> = (0..spec.labels.len())
        .map(|i| (FIRST_CX + PITCH * i as f32, ROW_CY))
        .collect();
    // printed: question number, bubbles and letters
    let num = format!("{}.", spec.question);
    stroke_text(&mut pm, &num, 38.0, ROW_CY - 15.0, 30.0, 3.0, INK);
    for (i, &(cx, cy)) in centres.iter().enumerate() {
        circle(&mut pm, cx, cy, BUBBLE_R, 2.0, PRINT);
        let label = spec.labels[i].to_string();
        stroke_text(&mut pm, &label, cx - 6.0, cy - 9.0, 18.0, 1.7, PRINT);
    }
    // timing marks on the right edge, as on the legacy form
    for k in 0..2 {
        fill_rect(
            &mut pm,
            606.0,
            30.0 + 86.0 * k as f32,
            18.0,
            22.0,
            [0x30, 0x32, 0x36],
        );
    }
    // pencil
    for (i, &(cx, cy)) in centres.iter().enumerate() {
        let mark = spec.marks.get(i).copied().unwrap_or(MarkStyle::Empty);
        pencil(
            &mut pm,
            cx,
            cy,
            BUBBLE_R,
            mark,
            spec.seed.wrapping_add(i as u64 * 7919),
        );
    }
    grain(&mut pm, spec.seed ^ 0x5eed, 5.0);
    (
        to_crop(&pm),
        RowGeometry {
            centres,
            radius: BUBBLE_R,
        },
    )
}

/// The same row as it sits on the unregistered scan: rotated about 1.4°, shifted, greyer.
pub fn original_of(registered: &CropImage, seed: u64) -> CropImage {
    let (w, h) = (registered.width as i64, registered.height as i64);
    let angle = 1.4_f32.to_radians();
    let (s, c) = angle.sin_cos();
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let (dx, dy) = (9.0_f32, -6.0_f32);
    let mut out = vec![0u8; registered.rgba.len()];
    for y in 0..h {
        for x in 0..w {
            // inverse map: rotate (x - dx, y - dy) by -angle about the centre
            let (fx, fy) = (x as f32 - cx - dx, y as f32 - cy - dy);
            let sx = c * fx + s * fy + cx;
            let sy = -s * fx + c * fy + cy;
            let px = sample(registered, sx, sy);
            let idx = ((y * w + x) * 4) as usize;
            // greyer, lower-contrast original with a faint vertical scanner streak
            let streak = if (x - 431).abs() < 2 { 10.0 } else { 0.0 };
            let n = (hash(x as i32, y as i32, seed) - 0.5) * 8.0;
            for ch in 0..3 {
                let v = px[ch] as f32 * 0.93 + 233.0 * 0.07 - streak + n - 6.0;
                out[idx + ch] = v.clamp(0.0, 255.0) as u8;
            }
            out[idx + 3] = 255;
        }
    }
    CropImage {
        width: registered.width,
        height: registered.height,
        rgba: Arc::from(out),
    }
}

/// A small synthetic page thumbnail (generated sheet: fiducials, header, bubble columns).
pub fn page_thumbnail(seed: u64, w: u32, h: u32) -> CropImage {
    let mut pm = Pixmap::new(w, h).expect("pixmap");
    paper(&mut pm, seed, [0xFA, 0xF8, 0xF3]);
    let (wf, hf) = (w as f32, h as f32);
    let f = 0.075 * wf;
    for &(x, y) in &[
        (3.0, 3.0),
        (wf - 3.0 - f, 3.0),
        (3.0, hf - 3.0 - f),
        (wf - 3.0 - f, hf - 3.0 - f),
    ] {
        fill_rect(&mut pm, x, y, f, f, [0x22, 0x24, 0x28]);
    }
    // header lines
    fill_rect(
        &mut pm,
        wf * 0.18,
        hf * 0.07,
        wf * 0.42,
        2.0,
        [0xA8, 0xA4, 0x9C],
    );
    fill_rect(
        &mut pm,
        wf * 0.18,
        hf * 0.11,
        wf * 0.28,
        2.0,
        [0xC4, 0xC0, 0xB8],
    );
    // four columns of bubble rows
    let cols = 4;
    let rows = 12;
    let col_w = wf * 0.84 / cols as f32;
    let top = hf * 0.2;
    let row_h = (hf * 0.72) / rows as f32;
    let mut rng = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    for c in 0..cols {
        for r in 0..rows {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let marked = (rng % 5) as usize;
            for o in 0..5 {
                let x = wf * 0.08 + c as f32 * col_w + 4.0 + o as f32 * (col_w - 8.0) / 5.0;
                let y = top + r as f32 * row_h + row_h * 0.3;
                let dark = o == marked;
                let shade = if dark {
                    [0x2E, 0x31, 0x34]
                } else {
                    [0xC9, 0xC5, 0xBD]
                };
                fill_rect(
                    &mut pm,
                    x,
                    y,
                    (col_w - 8.0) / 5.0 - 1.5,
                    row_h * 0.42,
                    shade,
                );
            }
        }
    }
    grain(&mut pm, seed ^ 0xabcd, 4.0);
    to_crop(&pm)
}

/// One student-number grid to draw (identity items, §9.4).
#[derive(Debug, Clone)]
pub struct IdGridSpec {
    /// Handwritten digits in the boxes above the columns ('?' or ' ' leaves a box empty).
    pub boxes: String,
    /// Per column, the marks on digits 0..=9.
    pub marks: Vec<[MarkStyle; 10]>,
    pub seed: u64,
}

/// Geometry of the drawn ID grid: centres indexed `[column][digit]`.
#[derive(Debug, Clone, PartialEq)]
pub struct GridGeometry {
    pub centres: Vec<[(f32, f32); 10]>,
    pub radius: f32,
}

pub const GRID_W: u32 = 640;
pub const GRID_H: u32 = 372;

/// Draws a 10-column student-number grid with write-in boxes.
pub fn id_grid(spec: &IdGridSpec) -> (CropImage, GridGeometry) {
    let mut pm = Pixmap::new(GRID_W, GRID_H).expect("pixmap");
    paper(&mut pm, spec.seed, PAPER);
    let cols = spec.marks.len().max(1);
    let pitch_x = 46.0;
    let x0 = (GRID_W as f32 - pitch_x * (cols as f32 - 1.0)) / 2.0;
    let (box_top, box_h) = (18.0, 44.0);
    let (row0, pitch_y, r) = (92.0, 29.0, 11.0);
    let mut centres = Vec::with_capacity(cols);
    let boxes: Vec<char> = spec.boxes.chars().collect();
    for c in 0..cols {
        let cx = x0 + pitch_x * c as f32;
        // write-in box
        rect_outline(&mut pm, cx - 18.0, box_top, 36.0, box_h, 1.6, PRINT);
        if let Some(&ch) = boxes.get(c)
            && ch.is_ascii_digit()
        {
            // handwriting: graphite stroke font with a little per-glyph jitter
            let jx = (hash(c as i32, 1, spec.seed) - 0.5) * 6.0;
            let jy = (hash(c as i32, 2, spec.seed) - 0.5) * 5.0;
            stroke_text(
                &mut pm,
                &ch.to_string(),
                cx - 9.0 + jx,
                box_top + 9.0 + jy,
                26.0,
                2.6,
                [0x34, 0x37, 0x3B],
            );
        }
        let mut col = [(0.0, 0.0); 10];
        for (d, slot) in col.iter_mut().enumerate() {
            let cy = row0 + pitch_y * d as f32;
            *slot = (cx, cy);
            circle(&mut pm, cx, cy, r, 1.5, PRINT);
            stroke_text(
                &mut pm,
                &d.to_string(),
                cx - 3.6,
                cy - 5.4,
                10.8,
                1.2,
                PRINT,
            );
        }
        centres.push(col);
    }
    for (c, col) in spec.marks.iter().enumerate() {
        for (d, &mark) in col.iter().enumerate() {
            let (cx, cy) = centres[c][d];
            pencil(
                &mut pm,
                cx,
                cy,
                r,
                mark,
                spec.seed.wrapping_add((c * 10 + d) as u64 * 31),
            );
        }
    }
    grain(&mut pm, spec.seed ^ 0x1d, 5.0);
    (to_crop(&pm), GridGeometry { centres, radius: r })
}

/// A page-sized synthetic scan for notice items (rejected pages, batch evidence).
pub fn page_scan(seed: u64, low_dpi: bool) -> CropImage {
    let thumb = page_thumbnail(
        seed,
        if low_dpi { 120 } else { 240 },
        if low_dpi { 160 } else { 320 },
    );
    if !low_dpi {
        return thumb;
    }
    // upscale 2x with nearest neighbour: what a 92 dpi page looks like at review zoom
    let (w, h) = (thumb.width * 2, thumb.height * 2);
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let si = (((y / 2) * thumb.width + x / 2) * 4) as usize;
            let di = ((y * w + x) * 4) as usize;
            out[di..di + 4].copy_from_slice(&thumb.rgba[si..si + 4]);
        }
    }
    CropImage {
        width: w,
        height: h,
        rgba: Arc::from(out),
    }
}

// ------------------------------------------------------------------------------------ raster

fn to_crop(pm: &Pixmap) -> CropImage {
    // The pixmap is fully opaque, so premultiplied == straight alpha.
    CropImage {
        width: pm.width(),
        height: pm.height(),
        rgba: Arc::from(pm.data()),
    }
}

fn hash(x: i32, y: i32, seed: u64) -> f32 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ seed.wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    (h >> 40) as f32 / (1u64 << 24) as f32
}

fn value_noise(x: f32, y: f32, seed: u64) -> f32 {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (tx, ty) = (x - x.floor(), y - y.floor());
    let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
    let a = hash(xi, yi, seed);
    let b = hash(xi + 1, yi, seed);
    let c = hash(xi, yi + 1, seed);
    let d = hash(xi + 1, yi + 1, seed);
    let top = a + (b - a) * sx;
    let bottom = c + (d - c) * sx;
    top + (bottom - top) * sy
}

fn paper(pm: &mut Pixmap, seed: u64, base: [u8; 3]) {
    let w = pm.width() as i32;
    let data = pm.data_mut();
    for (i, px) in data.chunks_exact_mut(4).enumerate() {
        let (x, y) = (i as i32 % w, i as i32 / w);
        let low = (value_noise(x as f32 / 90.0, y as f32 / 90.0, seed) - 0.5) * 6.0;
        let fibre = (value_noise(x as f32 / 3.0, y as f32 / 9.0, seed ^ 0x77) - 0.5) * 4.0;
        for ch in 0..3 {
            px[ch] = (base[ch] as f32 + low + fibre).clamp(0.0, 255.0) as u8;
        }
        px[3] = 255;
    }
}

fn grain(pm: &mut Pixmap, seed: u64, amp: f32) {
    let w = pm.width() as i32;
    for (i, px) in pm.data_mut().chunks_exact_mut(4).enumerate() {
        let n = (hash(i as i32 % w, i as i32 / w, seed) - 0.5) * amp;
        for ch in px.iter_mut().take(3) {
            *ch = (*ch as f32 + n).clamp(0.0, 255.0) as u8;
        }
    }
}

fn sample(img: &CropImage, x: f32, y: f32) -> [u8; 3] {
    let (w, h) = (img.width as i32, img.height as i32);
    let (x0, y0) = (x.floor() as i32, y.floor() as i32);
    let (tx, ty) = (x - x.floor(), y - y.floor());
    let get = |xx: i32, yy: i32| -> [f32; 3] {
        if xx < 0 || yy < 0 || xx >= w || yy >= h {
            return [228.0, 226.0, 220.0];
        }
        let i = ((yy * w + xx) * 4) as usize;
        [
            img.rgba[i] as f32,
            img.rgba[i + 1] as f32,
            img.rgba[i + 2] as f32,
        ]
    };
    let (a, b, c, d) = (
        get(x0, y0),
        get(x0 + 1, y0),
        get(x0, y0 + 1),
        get(x0 + 1, y0 + 1),
    );
    let mut out = [0u8; 3];
    for ch in 0..3 {
        let top = a[ch] + (b[ch] - a[ch]) * tx;
        let bottom = c[ch] + (d[ch] - c[ch]) * tx;
        out[ch] = (top + (bottom - top) * ty).clamp(0.0, 255.0) as u8;
    }
    out
}

fn paint(rgb: [u8; 3]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(rgb[0], rgb[1], rgb[2], 255));
    p.anti_alias = true;
    p
}

fn circle(pm: &mut Pixmap, cx: f32, cy: f32, r: f32, width: f32, rgb: [u8; 3]) {
    if let Some(path) = PathBuilder::from_circle(cx, cy, r) {
        let stroke = Stroke {
            width,
            ..Stroke::default()
        };
        pm.stroke_path(&path, &paint(rgb), &stroke, Transform::identity(), None);
    }
}

fn rect_outline(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, width: f32, rgb: [u8; 3]) {
    if let Some(rect) = tiny_skia::Rect::from_xywh(x, y, w, h) {
        let path = PathBuilder::from_rect(rect);
        let stroke = Stroke {
            width,
            ..Stroke::default()
        };
        pm.stroke_path(&path, &paint(rgb), &stroke, Transform::identity(), None);
    }
}

fn fill_rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, rgb: [u8; 3]) {
    if let Some(rect) = tiny_skia::Rect::from_xywh(x, y, w.max(0.5), h.max(0.5)) {
        pm.fill_rect(rect, &paint(rgb), Transform::identity(), None);
    }
}

/// Glyphs of a 4 × 6 stroke font: polylines in glyph units.
fn glyph(ch: char) -> &'static [&'static [(f32, f32)]] {
    match ch {
        'A' => &[
            &[(0.0, 6.0), (2.0, 0.0), (4.0, 6.0)],
            &[(0.7, 4.0), (3.3, 4.0)],
        ],
        'B' => &[
            &[
                (0.0, 6.0),
                (0.0, 0.0),
                (2.8, 0.0),
                (3.8, 0.8),
                (3.8, 2.2),
                (2.8, 3.0),
                (0.0, 3.0),
            ],
            &[(2.8, 3.0), (4.0, 3.8), (4.0, 5.2), (3.0, 6.0), (0.0, 6.0)],
        ],
        'C' => &[&[
            (4.0, 1.0),
            (3.0, 0.0),
            (1.0, 0.0),
            (0.0, 1.2),
            (0.0, 4.8),
            (1.0, 6.0),
            (3.0, 6.0),
            (4.0, 5.0),
        ]],
        'D' => &[&[
            (0.0, 0.0),
            (0.0, 6.0),
            (2.4, 6.0),
            (4.0, 4.5),
            (4.0, 1.5),
            (2.4, 0.0),
            (0.0, 0.0),
        ]],
        'E' => &[
            &[(4.0, 0.0), (0.0, 0.0), (0.0, 6.0), (4.0, 6.0)],
            &[(0.0, 3.0), (3.0, 3.0)],
        ],
        '0' => &[&[
            (1.0, 0.0),
            (3.0, 0.0),
            (4.0, 1.0),
            (4.0, 5.0),
            (3.0, 6.0),
            (1.0, 6.0),
            (0.0, 5.0),
            (0.0, 1.0),
            (1.0, 0.0),
        ]],
        '1' => &[
            &[(1.0, 1.2), (2.4, 0.0), (2.4, 6.0)],
            &[(1.0, 6.0), (3.8, 6.0)],
        ],
        '2' => &[&[
            (0.0, 1.2),
            (1.0, 0.0),
            (3.0, 0.0),
            (4.0, 1.0),
            (4.0, 2.2),
            (0.0, 6.0),
            (4.0, 6.0),
        ]],
        '3' => &[&[
            (0.0, 0.0),
            (4.0, 0.0),
            (1.8, 2.6),
            (3.0, 2.6),
            (4.0, 3.6),
            (4.0, 5.0),
            (3.0, 6.0),
            (1.0, 6.0),
            (0.0, 5.0),
        ]],
        '4' => &[&[(3.0, 6.0), (3.0, 0.0), (0.0, 4.2), (4.0, 4.2)]],
        '5' => &[&[
            (4.0, 0.0),
            (0.4, 0.0),
            (0.2, 2.6),
            (3.0, 2.6),
            (4.0, 3.6),
            (4.0, 5.0),
            (3.0, 6.0),
            (0.0, 6.0),
        ]],
        '6' => &[&[
            (3.6, 0.0),
            (1.6, 0.0),
            (0.0, 2.0),
            (0.0, 5.0),
            (1.0, 6.0),
            (3.0, 6.0),
            (4.0, 5.0),
            (4.0, 3.6),
            (3.0, 2.6),
            (1.0, 2.6),
            (0.0, 3.6),
        ]],
        '7' => &[&[(0.0, 0.0), (4.0, 0.0), (1.4, 6.0)]],
        '8' => &[
            &[
                (1.0, 0.0),
                (3.0, 0.0),
                (3.8, 0.8),
                (3.8, 2.2),
                (3.0, 3.0),
                (1.0, 3.0),
                (0.2, 2.2),
                (0.2, 0.8),
                (1.0, 0.0),
            ],
            &[
                (1.0, 3.0),
                (3.0, 3.0),
                (4.0, 4.0),
                (4.0, 5.0),
                (3.0, 6.0),
                (1.0, 6.0),
                (0.0, 5.0),
                (0.0, 4.0),
                (1.0, 3.0),
            ],
        ],
        '9' => &[&[
            (0.4, 6.0),
            (2.4, 6.0),
            (4.0, 4.0),
            (4.0, 1.0),
            (3.0, 0.0),
            (1.0, 0.0),
            (0.0, 1.0),
            (0.0, 2.4),
            (1.0, 3.4),
            (3.0, 3.4),
            (4.0, 2.4),
        ]],
        '.' => &[&[(2.0, 5.75), (2.0, 5.9)]],
        _ => &[],
    }
}

/// Draws `text` with the stroke font; `x, y` is the top-left, `h` the cap height.
fn stroke_text(pm: &mut Pixmap, text: &str, x: f32, y: f32, h: f32, width: f32, rgb: [u8; 3]) {
    let unit = h / 6.0;
    let advance = unit * 5.6;
    let stroke = Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    for (i, ch) in text.chars().enumerate() {
        let ox = x + i as f32 * advance;
        for line in glyph(ch) {
            let mut pb = PathBuilder::new();
            for (k, &(gx, gy)) in line.iter().enumerate() {
                let (px, py) = (ox + gx * unit, y + gy * unit);
                if k == 0 {
                    pb.move_to(px, py);
                } else {
                    pb.line_to(px, py);
                }
            }
            if let Some(path) = pb.finish() {
                pm.stroke_path(&path, &paint(rgb), &stroke, Transform::identity(), None);
            }
        }
    }
}

/// Blends graphite into the pixmap with a procedural coverage function.
fn blend_graphite(pm: &mut Pixmap, bbox: (i32, i32, i32, i32), alpha: impl Fn(f32, f32) -> f32) {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let (x0, y0, x1, y1) = (
        bbox.0.max(0),
        bbox.1.max(0),
        bbox.2.min(w - 1),
        bbox.3.min(h - 1),
    );
    let data = pm.data_mut();
    for y in y0..=y1 {
        for x in x0..=x1 {
            let a = alpha(x as f32 + 0.5, y as f32 + 0.5).clamp(0.0, 1.0);
            if a <= 0.0 {
                continue;
            }
            let i = ((y * w + x) * 4) as usize;
            for ch in 0..3 {
                let v = data[i + ch] as f32;
                data[i + ch] = (v + (GRAPHITE[ch] - v) * a).round() as u8;
            }
        }
    }
}

fn pencil(pm: &mut Pixmap, cx: f32, cy: f32, r: f32, mark: MarkStyle, seed: u64) {
    let bbox = |rx: f32, ry: f32| {
        (
            (cx - rx - 3.0) as i32,
            (cy - ry - 3.0) as i32,
            (cx + rx + 3.0) as i32,
            (cy + ry + 3.0) as i32,
        )
    };
    // irregular fill boundary: radius modulated by angle noise
    let boundary = move |x: f32, y: f32, scale: f32| -> f32 {
        let (dx, dy) = (x - cx, y - cy);
        let theta = dy.atan2(dx);
        let wobble = value_noise(theta * 2.2 + 10.0, 0.5, seed) - 0.5;
        let reach = r * scale * (1.0 + 0.10 * wobble);
        reach - (dx * dx + dy * dy).sqrt()
    };
    // scribble texture: oblique strokes of varying pressure
    let strokes = move |x: f32, y: f32| -> f32 {
        let t = (x * 0.62 + y * 0.78) * 0.9 + value_noise(x / 6.0, y / 6.0, seed ^ 1) * 6.0;
        0.72 + 0.28 * (0.5 + 0.5 * t.sin()) + (hash(x as i32, y as i32, seed) - 0.5) * 0.18
    };
    match mark {
        MarkStyle::Empty => {}
        MarkStyle::Filled(d) => blend_graphite(pm, bbox(r * 1.1, r * 1.1), |x, y| {
            let edge = (boundary(x, y, 0.96) / 1.6).clamp(0.0, 1.0);
            edge * d * strokes(x, y)
        }),
        MarkStyle::Light(d) => blend_graphite(pm, bbox(r, r), |x, y| {
            let edge = (boundary(x, y, 0.82) / 2.0).clamp(0.0, 1.0);
            let patch = 0.45 + 0.55 * value_noise(x / 7.0, y / 7.0, seed ^ 9);
            edge * d * patch * strokes(x, y)
        }),
        MarkStyle::Erased(d) => blend_graphite(pm, bbox(r * 1.6, r * 1.2), |x, y| {
            let (dx, dy) = ((x - cx) / (r * 1.45), (y - cy) / (r * 1.05));
            let fall = (1.0 - (dx * dx + dy * dy)).clamp(0.0, 1.0);
            let streak = 0.5 + 0.5 * ((y * 1.7 + value_noise(x / 9.0, 0.0, seed) * 9.0).sin());
            fall * d * (0.55 + 0.45 * streak)
        }),
        MarkStyle::Smudge => blend_graphite(pm, bbox(r * 1.8, r * 1.4), |x, y| {
            // a diagonal smear (fold or roller streak) crossing the bubble
            let u = ((x - cx) * 0.55 - (y - cy) * 0.84).abs();
            let band = (1.0 - u / (r * 0.55)).clamp(0.0, 1.0);
            let along = ((x - cx) * 0.84 + (y - cy) * 0.55) / (r * 1.7);
            let fade = (1.0 - along * along).clamp(0.0, 1.0);
            band * fade * 0.78 * strokes(x, y)
        }),
        MarkStyle::Streak => {
            let h = pm.height() as i32;
            blend_graphite(
                pm,
                ((cx - 6.0) as i32, 0, (cx + 6.0) as i32, h - 1),
                |x, y| {
                    let u = (x - (cx - 9.0)).abs();
                    let core = (1.6 - u).clamp(0.0, 1.0);
                    core * (0.55 + 0.25 * value_noise(0.0, y / 5.0, seed))
                },
            );
        }
        MarkStyle::Tick => {
            let stroke = Stroke {
                width: 3.4,
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                ..Stroke::default()
            };
            let mut pb = PathBuilder::new();
            pb.move_to(cx - r * 0.55, cy + r * 0.05);
            pb.line_to(cx - r * 0.12, cy + r * 0.5);
            pb.line_to(cx + r * 0.62, cy - r * 0.55);
            if let Some(path) = pb.finish() {
                pm.stroke_path(
                    &path,
                    &paint([0x3A, 0x3C, 0x40]),
                    &stroke,
                    Transform::identity(),
                    None,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn darkness(img: &CropImage, cx: f32, cy: f32, r: f32) -> f32 {
        let mut sum = 0.0;
        let mut n = 0.0;
        for y in (cy - r * 0.6) as u32..(cy + r * 0.6) as u32 {
            for x in (cx - r * 0.6) as u32..(cx + r * 0.6) as u32 {
                let i = ((y * img.width + x) * 4) as usize;
                sum += 255.0 - img.rgba[i] as f32;
                n += 1.0;
            }
        }
        sum / n / 255.0
    }

    #[test]
    fn rows_are_deterministic_and_marks_are_darker_than_paper() {
        let spec = RowSpec {
            question: 17,
            labels: vec!['A', 'B', 'C', 'D', 'E'],
            marks: vec![
                MarkStyle::Empty,
                MarkStyle::Filled(0.9),
                MarkStyle::Empty,
                MarkStyle::Light(0.5),
                MarkStyle::Erased(0.3),
            ],
            seed: 42,
        };
        let (a, geo) = answer_row(&spec);
        let (b, _) = answer_row(&spec);
        assert_eq!(a, b, "seeded rendering is reproducible");
        assert_eq!(a.rgba.len(), (ROW_W * ROW_H * 4) as usize);
        assert!(a.rgba.chunks_exact(4).all(|p| p[3] == 255), "opaque");
        let d: Vec<f32> = geo
            .centres
            .iter()
            .map(|&(x, y)| darkness(&a, x, y, geo.radius))
            .collect();
        assert!(d[1] > 0.5, "filled bubble is dark: {d:?}");
        assert!(d[1] > d[3] && d[3] > d[0], "fill > light > empty: {d:?}");
        let orig = original_of(&a, 1);
        assert_eq!(orig.rgba.len(), a.rgba.len());
        assert_ne!(orig, a);
    }

    #[test]
    fn id_grid_marks_the_bubbled_digits() {
        let mut marks = vec![[MarkStyle::Empty; 10]; 10];
        for (c, d) in [9, 2, 6, 1, 0, 3, 4, 4, 7, 1].iter().enumerate() {
            marks[c][*d] = MarkStyle::Filled(0.85);
        }
        let (img, geo) = id_grid(&IdGridSpec {
            boxes: "9261034471".into(),
            marks,
            seed: 7,
        });
        assert_eq!((img.width, img.height), (GRID_W, GRID_H));
        let (x, y) = geo.centres[0][9];
        let (ex, ey) = geo.centres[0][0];
        assert!(darkness(&img, x, y, geo.radius) > darkness(&img, ex, ey, geo.radius) + 0.3);
    }

    #[test]
    fn thumbnails_have_the_requested_size() {
        let t = page_thumbnail(3, 84, 108);
        assert_eq!((t.width, t.height), (84, 108));
        assert_eq!(t.rgba.len(), 84 * 108 * 4);
    }
}
