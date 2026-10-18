//! Procedural marks painted into the clean raster in image space (spec §23.3 stages 3–5).
//!
//! Ink darkens multiplicatively, `I ← I · (1 − a)`, so graphite over printed outlines and over
//! other marks behaves like layered pigment. Every random choice comes from the caller's
//! seeded generator; noise fields are hashed lattices keyed by a per-mark seed.

use crate::script::{BubbleMark, Instrument};
use image::GrayImage;
use omr_model::MarkClass;
use rand::RngExt;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

/// What a painted mark actually achieved inside the measured interior (inner 80 % of r₀).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct MarkStats {
    /// Fraction of interior pixels inked above half the mark's darkness.
    pub coverage: f32,
    /// Mean ink opacity over the interior (0 = untouched, 1 = black).
    pub mean_ink: f32,
}

/// Image-space context: the raster and its scale.
pub struct Canvas<'a> {
    img: &'a mut GrayImage,
    ppm: f64,
}

fn hash01(seed: u64, x: i64, y: i64) -> f32 {
    let mut z = seed
        ^ (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    ((z >> 40) as f32) / ((1u64 << 24) as f32)
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Value noise in [0, 1) with lattice cell `cell` (same units as `x`, `y`).
fn value_noise(seed: u64, x: f64, y: f64, cell: f64) -> f32 {
    let (gx, gy) = (x / cell, y / cell);
    let (ix, iy) = (gx.floor() as i64, gy.floor() as i64);
    let (fx, fy) = (
        smooth((gx - gx.floor()) as f32),
        smooth((gy - gy.floor()) as f32),
    );
    let a = hash01(seed, ix, iy);
    let b = hash01(seed, ix + 1, iy);
    let c = hash01(seed, ix, iy + 1);
    let d = hash01(seed, ix + 1, iy + 1);
    let top = a + (b - a) * fx;
    let bot = c + (d - c) * fx;
    top + (bot - top) * fy
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if e1 <= e0 {
        return if x < e0 { 0.0 } else { 1.0 };
    }
    smooth(((x - e0) / (e1 - e0)).clamp(0.0, 1.0))
}

/// Shape and texture of a filled blob.
#[derive(Clone, Debug)]
struct Blob {
    cx: f64,
    cy: f64,
    radius: f64,
    harm: [(f64, f64, f64); 3],
    darkness: f32,
    coverage: f32,
    grain: f32,
    hatch: f32,
    hatch_dir: (f64, f64),
    hole_cell: f64,
    soft_px: f64,
    half_plane: Option<(f64, f64, f64)>,
    seed: u64,
}

/// A stroked polyline.
#[derive(Clone, Debug)]
struct Stroke {
    pts: Vec<(f64, f64)>,
    width: f64,
    darkness: f32,
    grain: f32,
    seed: u64,
}

impl<'a> Canvas<'a> {
    /// A canvas over `img` rendered at `dpi`.
    pub fn new(img: &'a mut GrayImage, dpi: f64) -> Self {
        Self {
            img,
            ppm: dpi / 25.4,
        }
    }

    #[inline]
    fn darken(&mut self, x: u32, y: u32, a: f32) {
        if a <= 0.0 {
            return;
        }
        let p = self.img.get_pixel_mut(x, y);
        let v = f32::from(p.0[0]) * (1.0 - a.min(0.995));
        p.0[0] = (v + 0.5) as u8;
    }

    /// Pixel bounding box of a millimetre circle.
    fn px_box(&self, cx: f64, cy: f64, r: f64) -> (u32, u32, u32, u32) {
        let (w, h) = self.img.dimensions();
        let f = |v: f64, max: u32| (v * self.ppm).floor().clamp(0.0, f64::from(max)) as u32;
        (
            f(cx - r, w),
            f(cy - r, h),
            ((cx + r) * self.ppm).ceil().clamp(0.0, f64::from(w)) as u32,
            ((cy + r) * self.ppm).ceil().clamp(0.0, f64::from(h)) as u32,
        )
    }

    fn paint_blob(&mut self, b: &Blob) {
        let reach = b.radius * 1.25 + 2.0 / self.ppm;
        let (x0, y0, x1, y1) = self.px_box(b.cx, b.cy, reach);
        let (hc, hs) = b.hatch_dir;
        for py in y0..y1 {
            let ym = (f64::from(py) + 0.5) / self.ppm;
            for px in x0..x1 {
                let xm = (f64::from(px) + 0.5) / self.ppm;
                let (dx, dy) = (xm - b.cx, ym - b.cy);
                let rho = (dx * dx + dy * dy).sqrt();
                // Boundary R(φ) = r·(1 + Σ a_k cos(kφ + p_k)) via Chebyshev recurrences.
                let (c1, s1) = if rho > 1e-9 {
                    (dx / rho, dy / rho)
                } else {
                    (1.0, 0.0)
                };
                let (c2, s2) = (c1 * c1 - s1 * s1, 2.0 * c1 * s1);
                let (c3, s3) = (c2 * c1 - s2 * s1, s2 * c1 + c2 * s1);
                let mut wob = 0.0;
                for (k, (a, cp, sp)) in b.harm.iter().enumerate() {
                    let (ck, sk) = match k {
                        0 => (c2, s2),
                        1 => (c3, s3),
                        _ => (c1, s1),
                    };
                    wob += a * (ck * cp - sk * sp);
                }
                let r_edge = b.radius * (1.0 + wob);
                let mut edge =
                    (((r_edge - rho) * self.ppm / b.soft_px) + 0.5).clamp(0.0, 1.0) as f32;
                if edge <= 0.0 {
                    continue;
                }
                if let Some((hc2, hs2, off)) = b.half_plane {
                    let proj = dx * hc2 + dy * hs2 - off;
                    edge *= ((proj * self.ppm) + 0.5).clamp(0.0, 1.0) as f32;
                    if edge <= 0.0 {
                        continue;
                    }
                }
                let hole = if b.coverage >= 0.985 {
                    0.0
                } else {
                    let n = value_noise(b.seed, xm, ym, b.hole_cell);
                    smoothstep(b.coverage - 0.08, b.coverage + 0.08, n)
                };
                let fine = hash01(b.seed ^ 0xA5A5, i64::from(px), i64::from(py));
                let hatch = ((xm * hc + ym * hs) * std::f64::consts::TAU / 0.38).sin() as f32;
                let g = 1.0 + b.grain * (fine - 0.5) + b.hatch * hatch;
                let a = (b.darkness * g).clamp(0.0, 0.985) * edge * (1.0 - hole);
                self.darken(px, py, a);
            }
        }
    }

    fn paint_stroke(&mut self, s: &Stroke) {
        if s.pts.len() < 2 {
            return;
        }
        let half = s.width / 2.0;
        let (mut lx, mut ly, mut hx, mut hy) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for &(x, y) in &s.pts {
            lx = lx.min(x);
            ly = ly.min(y);
            hx = hx.max(x);
            hy = hy.max(y);
        }
        let pad = half + 2.0 / self.ppm;
        let (w, h) = self.img.dimensions();
        let x0 = ((lx - pad) * self.ppm).floor().clamp(0.0, f64::from(w)) as u32;
        let y0 = ((ly - pad) * self.ppm).floor().clamp(0.0, f64::from(h)) as u32;
        let x1 = ((hx + pad) * self.ppm).ceil().clamp(0.0, f64::from(w)) as u32;
        let y1 = ((hy + pad) * self.ppm).ceil().clamp(0.0, f64::from(h)) as u32;
        for py in y0..y1 {
            let ym = (f64::from(py) + 0.5) / self.ppm;
            for px in x0..x1 {
                let xm = (f64::from(px) + 0.5) / self.ppm;
                let mut d = f64::MAX;
                for seg in s.pts.windows(2) {
                    d = d.min(seg_dist(xm, ym, seg[0], seg[1]));
                }
                let edge = (((half - d) * self.ppm) + 0.5).clamp(0.0, 1.0) as f32;
                if edge <= 0.0 {
                    continue;
                }
                let fine = hash01(s.seed, i64::from(px), i64::from(py));
                let a = (s.darkness * (1.0 + s.grain * (fine - 0.5))).clamp(0.0, 0.985) * edge;
                self.darken(px, py, a);
            }
        }
    }

    /// Paints one deliberate mark on the bubble at (`cx`, `cy`) mm with radius `r` mm, and
    /// returns what it achieved inside the measured interior.
    pub fn paint_mark(
        &mut self,
        cx: f64,
        cy: f64,
        r: f64,
        m: &BubbleMark,
        rng: &mut ChaCha8Rng,
    ) -> MarkStats {
        let before = self.interior(cx, cy, r);
        let pen = m.instrument == Instrument::Pen;
        let seed = rng.random::<u64>();
        let jitter = |rng: &mut ChaCha8Rng, a: f64| rng.random_range(-a..=a);
        let blob = |rng: &mut ChaCha8Rng, radius: f64, darkness: f32, coverage: f32| Blob {
            cx: cx + jitter(rng, 0.10 * r),
            cy: cy + jitter(rng, 0.10 * r),
            radius,
            harm: [0, 1, 2].map(|_| {
                let a = rng.random_range(-0.045..=0.045);
                let p: f64 = rng.random_range(0.0..std::f64::consts::TAU);
                (a, p.cos(), p.sin())
            }),
            darkness,
            coverage,
            grain: if pen { 0.04 } else { 0.55 },
            hatch: if pen { 0.0 } else { 0.12 },
            hatch_dir: {
                let t: f64 = rng.random_range(0.4..1.2);
                (t.cos(), t.sin())
            },
            hole_cell: 0.45,
            soft_px: if pen { 0.7 } else { 1.1 },
            half_plane: None,
            seed,
        };
        match m.class {
            MarkClass::Filled | MarkClass::LightFill => {
                let radius = r * rng.random_range(0.84..=1.06);
                let b = blob(rng, radius, m.darkness, m.coverage);
                self.paint_blob(&b);
            }
            MarkClass::ErasedResidue => {
                let radius = r * rng.random_range(0.95..=1.15);
                let mut b = blob(rng, radius, m.darkness, m.coverage);
                b.grain = 0.3;
                b.hatch = 0.0;
                b.hole_cell = 0.8;
                b.soft_px = 3.0;
                self.paint_blob(&b);
            }
            MarkClass::PartialOrDot => {
                if m.coverage < 0.12 {
                    let radius = r * rng.random_range(0.2..=0.45);
                    let mut b = blob(rng, radius, m.darkness, 1.0);
                    let (ox, oy) = (jitter(rng, 0.4 * r), jitter(rng, 0.4 * r));
                    b.cx = cx + ox;
                    b.cy = cy + oy;
                    self.paint_blob(&b);
                } else {
                    let mut b = blob(rng, r * 0.92, m.darkness, 0.95);
                    let t: f64 = rng.random_range(0.0..std::f64::consts::TAU);
                    let off = r * (1.0 - 2.0 * f64::from(m.coverage)).clamp(-0.9, 0.9) * 0.85;
                    b.half_plane = Some((t.cos(), t.sin(), off));
                    self.paint_blob(&b);
                }
            }
            MarkClass::CheckOrX => {
                let w = rng.random_range(0.35..=0.6);
                let mk = |pts: Vec<(f64, f64)>| Stroke {
                    pts,
                    width: w,
                    darkness: m.darkness,
                    grain: if pen { 0.05 } else { 0.5 },
                    seed,
                };
                let tick_shape = rng.random_bool(0.5);
                let mut j = |v: f64| v + rng.random_range(-0.08..=0.08) * r;
                if tick_shape {
                    let pts = vec![
                        (j(cx - 0.55 * r), j(cy)),
                        (j(cx - 0.1 * r), j(cy + 0.55 * r)),
                        (j(cx + 0.75 * r), j(cy - 0.75 * r)),
                    ];
                    self.paint_stroke(&mk(pts));
                } else {
                    let a = vec![
                        (j(cx - 0.7 * r), j(cy - 0.7 * r)),
                        (j(cx + 0.7 * r), j(cy + 0.7 * r)),
                    ];
                    let b = vec![
                        (j(cx - 0.7 * r), j(cy + 0.7 * r)),
                        (j(cx + 0.7 * r), j(cy - 0.7 * r)),
                    ];
                    self.paint_stroke(&mk(a));
                    self.paint_stroke(&mk(b));
                }
            }
            MarkClass::CrossedOut => {
                let radius = r * rng.random_range(0.85..=1.0);
                let b = blob(rng, radius, m.darkness, m.coverage);
                self.paint_blob(&b);
                let w = rng.random_range(0.4..=0.6);
                let d = m.darkness.max(0.75);
                for (a0, a1) in [
                    ((-1.25, -1.25), (1.25, 1.25)),
                    ((-1.25, 1.25), (1.25, -1.25)),
                ] {
                    let s = Stroke {
                        pts: vec![
                            (cx + a0.0 * r, cy + a0.1 * r),
                            (cx + a1.0 * r, cy + a1.1 * r),
                        ],
                        width: w,
                        darkness: d,
                        grain: 0.4,
                        seed: seed ^ 7,
                    };
                    self.paint_stroke(&s);
                }
            }
            MarkClass::CircledLabel => {
                let mut pts = Vec::new();
                for i in 0..=24 {
                    let t = f64::from(i) / 24.0 * std::f64::consts::TAU;
                    pts.push((cx + 1.3 * r * t.cos(), cy + 1.3 * r * t.sin()));
                }
                let s = Stroke {
                    pts,
                    width: 0.4,
                    darkness: m.darkness,
                    grain: 0.4,
                    seed,
                };
                self.paint_stroke(&s);
            }
            MarkClass::Whiteout => {
                let (x0, y0, x1, y1) = self.px_box(cx, cy, r * 1.2);
                for py in y0..y1 {
                    for px in x0..x1 {
                        let xm = (f64::from(px) + 0.5) / self.ppm - cx;
                        let ym = (f64::from(py) + 0.5) / self.ppm - cy;
                        if xm * xm + ym * ym <= (1.2 * r) * (1.2 * r) {
                            self.img.get_pixel_mut(px, py).0[0] = 252;
                        }
                    }
                }
            }
            MarkClass::Empty | MarkClass::StrayOrDamage => {}
        }
        let after = self.interior(cx, cy, r);
        stats(&before, &after, m.darkness)
    }

    /// Paints a stray stroke (outside bubbles) through `pts` (mm).
    pub fn paint_stray(&mut self, pts: Vec<(f64, f64)>, width: f64, darkness: f32, seed: u64) {
        self.paint_stroke(&Stroke {
            pts,
            width,
            darkness,
            grain: 0.5,
            seed,
        });
    }

    /// Pixel values of the measured interior (inner 80 % of `r`).
    fn interior(&self, cx: f64, cy: f64, r: f64) -> Vec<u8> {
        let ri = 0.8 * r;
        let (x0, y0, x1, y1) = self.px_box(cx, cy, ri);
        let mut v = Vec::new();
        for py in y0..y1 {
            let ym = (f64::from(py) + 0.5) / self.ppm - cy;
            for px in x0..x1 {
                let xm = (f64::from(px) + 0.5) / self.ppm - cx;
                if xm * xm + ym * ym <= ri * ri {
                    v.push(self.img.get_pixel(px, py).0[0]);
                }
            }
        }
        v
    }
}

fn stats(before: &[u8], after: &[u8], darkness: f32) -> MarkStats {
    if before.is_empty() || before.len() != after.len() {
        return MarkStats::default();
    }
    let mut inked = 0usize;
    let mut sum = 0.0f32;
    for (b, a) in before.iter().zip(after) {
        let ink = if *b == 0 {
            0.0
        } else {
            1.0 - f32::from(*a) / f32::from(*b)
        };
        sum += ink;
        if ink > 0.5 * darkness {
            inked += 1;
        }
    }
    MarkStats {
        coverage: inked as f32 / before.len() as f32,
        mean_ink: sum / before.len() as f32,
    }
}

fn seg_dist(x: f64, y: f64, a: (f64, f64), b: (f64, f64)) -> f64 {
    let (vx, vy) = (b.0 - a.0, b.1 - a.1);
    let (wx, wy) = (x - a.0, y - a.1);
    let l2 = vx * vx + vy * vy;
    let t = if l2 > 0.0 {
        ((wx * vx + wy * vy) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (px, py) = (a.0 + t * vx - x, a.1 + t * vy - y);
    (px * px + py * py).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;
    use rand::SeedableRng;

    fn mark(class: MarkClass, darkness: f32, coverage: f32, inst: Instrument) -> BubbleMark {
        BubbleMark {
            option: 0,
            class,
            darkness,
            coverage,
            instrument: inst,
        }
    }

    #[test]
    fn mark_classes_have_the_intended_ink() {
        let mut rng = ChaCha8Rng::seed_from_u64(5);
        let mut run = |m: BubbleMark| {
            let mut img = GrayImage::from_pixel(120, 120, Luma([255]));
            let mut c = Canvas::new(&mut img, 200.0);
            c.paint_mark(7.5, 7.5, 2.1, &m, &mut rng)
        };
        let filled = run(mark(MarkClass::Filled, 0.8, 0.9, Instrument::Pencil));
        assert!(filled.mean_ink > 0.5, "{filled:?}");
        assert!(filled.coverage > 0.7, "{filled:?}");
        let pen = run(mark(MarkClass::Filled, 0.92, 1.0, Instrument::Pen));
        assert!(pen.mean_ink > 0.85, "{pen:?}");
        let light = run(mark(MarkClass::LightFill, 0.35, 0.9, Instrument::Pencil));
        assert!(light.mean_ink > 0.15 && light.mean_ink < 0.45, "{light:?}");
        let residue = run(mark(
            MarkClass::ErasedResidue,
            0.12,
            0.8,
            Instrument::Pencil,
        ));
        assert!(
            residue.mean_ink > 0.02 && residue.mean_ink < 0.2,
            "{residue:?}"
        );
        let tick = run(mark(MarkClass::CheckOrX, 0.8, 0.2, Instrument::Pencil));
        assert!(tick.mean_ink > 0.05 && tick.mean_ink < 0.5, "{tick:?}");
        let dot = run(mark(MarkClass::PartialOrDot, 0.8, 0.05, Instrument::Pencil));
        assert!(dot.mean_ink > 0.01 && dot.mean_ink < 0.4, "{dot:?}");
    }

    #[test]
    fn painting_is_deterministic() {
        let paint = || {
            let mut img = GrayImage::from_pixel(80, 80, Luma([250]));
            let mut c = Canvas::new(&mut img, 300.0);
            let mut rng = ChaCha8Rng::seed_from_u64(77);
            c.paint_mark(
                3.4,
                3.4,
                2.1,
                &mark(MarkClass::Filled, 0.7, 0.8, Instrument::Pencil),
                &mut rng,
            );
            img
        };
        assert_eq!(paint(), paint());
    }
}
