//! Sub-pixel refinement of a tag's black-square corners (§8.4 Stage A: "refined to 0.05–0.1 px
//! by gradient-weighted line fits").
//!
//! Each of the four edges is located independently: along every integer pixel column (for a
//! near-horizontal edge) or row (near-vertical) inside the trimmed edge span, the 50 % crossing
//! between the tag's white border and black border levels is found by linear interpolation of
//! the raw pixel values; a total-least-squares line is fitted to the crossings with one outlier
//! pass, and adjacent lines are intersected. The crossings depend only on pixel values at
//! integer positions, so the result is insensitive to small differences in the detector's
//! seed corners (§8.11).

use omr_imaging::GrayImage;
use omr_model::PointPx;

/// A refined quad.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Refined {
    /// Corners, in the input order.
    pub corners: [PointPx; 4],
    /// RMS distance of the edge crossings to their fitted lines (px).
    pub edge_rms: f64,
    /// Median black level K inside the border (gray level).
    pub black: u8,
    /// Median white level W outside the border.
    pub white: u8,
}

/// A line `n · p = c` with unit normal `n`.
#[derive(Clone, Copy, Debug)]
struct Line {
    nx: f64,
    ny: f64,
    c: f64,
}

impl Line {
    fn dist(&self, p: PointPx) -> f64 {
        self.nx * p.x + self.ny * p.y - self.c
    }

    fn intersect(&self, o: &Line) -> Option<PointPx> {
        let det = self.nx * o.ny - self.ny * o.nx;
        if det.abs() < 1e-9 {
            return None;
        }
        Some(PointPx {
            x: (self.c * o.ny - self.ny * o.c) / det,
            y: (self.nx * o.c - self.c * o.nx) / det,
        })
    }
}

/// Total-least-squares line through points (needs ≥ 2).
fn fit_line(pts: &[PointPx]) -> Option<Line> {
    let n = pts.len();
    if n < 2 {
        return None;
    }
    let (mx, my) = pts.iter().fold((0.0, 0.0), |(x, y), p| (x + p.x, y + p.y));
    let (mx, my) = (mx / n as f64, my / n as f64);
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    for p in pts {
        let (dx, dy) = (p.x - mx, p.y - my);
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
    }
    // Direction = eigenvector of the larger eigenvalue; normal = the other.
    let theta = 0.5 * (2.0 * sxy).atan2(sxx - syy);
    let (dx, dy) = (theta.cos(), theta.sin());
    let (nx, ny) = (-dy, dx);
    Some(Line {
        nx,
        ny,
        c: nx * mx + ny * my,
    })
}

fn median_u8(v: &mut [u8]) -> Option<u8> {
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    Some(v[v.len() / 2])
}

/// Refines a clockwise (on screen) quad around a tag's black square. `search` is the
/// half-width of the crossing search along the normal, `trim` the fraction of each edge
/// skipped next to the corners, `outlier` the distance beyond which crossings are dropped.
pub(crate) fn refine_quad(
    img: &GrayImage,
    quad: &[PointPx; 4],
    search: f64,
    trim: f64,
    outlier: f64,
) -> Option<Refined> {
    // The black square is 8 modules wide: sample the levels half a module either side.
    let side = (0..4)
        .map(|i| {
            let (a, b) = (quad[i], quad[(i + 1) % 4]);
            (b.x - a.x).hypot(b.y - a.y)
        })
        .sum::<f64>()
        / 4.0;
    if !(side.is_finite() && side >= 8.0) {
        return None;
    }
    let module = side / 8.0;
    let (black, white) = levels(img, quad, module * 0.5)?;
    if white <= black.saturating_add(20) {
        return None;
    }
    let search = search.min(module * 0.75).max(1.5);

    let mut lines = [None; 4];
    let mut sq = 0.0;
    let mut cnt = 0usize;
    for (i, slot) in lines.iter_mut().enumerate() {
        let (a, b) = (quad[i], quad[(i + 1) % 4]);
        let pts = edge_crossings(img, a, b, f64::from(black), f64::from(white), search, trim);
        let mut line = fit_line(&pts)?;
        let kept: Vec<PointPx> = pts
            .iter()
            .copied()
            .filter(|p| line.dist(*p).abs() <= outlier)
            .collect();
        if kept.len() < 4 {
            return None;
        }
        line = fit_line(&kept)?;
        for p in &kept {
            sq += line.dist(*p).powi(2);
        }
        cnt += kept.len();
        *slot = Some(line);
    }
    let l: Vec<Line> = lines.into_iter().flatten().collect();
    let mut corners = [PointPx::default(); 4];
    for (i, c) in corners.iter_mut().enumerate() {
        // Corner i joins edge i−1 (into it) and edge i (out of it).
        *c = l[(i + 3) % 4].intersect(&l[i])?;
        let d = (c.x - quad[i].x).hypot(c.y - quad[i].y);
        if !(d.is_finite() && d <= 2.0 * module + 2.0) {
            return None;
        }
    }
    Some(Refined {
        corners,
        edge_rms: (sq / cnt.max(1) as f64).sqrt(),
        black,
        white,
    })
}

/// Median black level half a module inside each edge and white level half a module outside.
fn levels(img: &GrayImage, quad: &[PointPx; 4], depth: f64) -> Option<(u8, u8)> {
    let mut inside = Vec::new();
    let mut outside = Vec::new();
    for i in 0..4 {
        let (a, b) = (quad[i], quad[(i + 1) % 4]);
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let len = dx.hypot(dy);
        if len < 1.0 {
            return None;
        }
        // Inward normal of a clockwise-on-screen quad (y down).
        let (nx, ny) = (-dy / len, dx / len);
        let steps = len.floor() as usize;
        for s in 0..steps {
            let t = (s as f64 + 0.5) / steps as f64;
            if !(0.2..=0.8).contains(&t) {
                continue;
            }
            let (px, py) = (a.x + t * dx, a.y + t * dy);
            let at = |k: f64| {
                img.get_or(
                    (px + k * nx).floor() as isize,
                    (py + k * ny).floor() as isize,
                    255,
                )
            };
            inside.push(at(depth));
            outside.push(at(-depth));
        }
    }
    Some((median_u8(&mut inside)?, median_u8(&mut outside)?))
}

/// Edge positions along integer columns or rows of edge `a → b`: the 50 % crossing locates
/// the edge to a pixel, then the ink mass in a 5-pixel window centred on it gives the
/// sub-pixel position (exact for a blurred step edge with a symmetric point-spread function,
/// whatever the blur, because blur conserves mass).
#[allow(clippy::too_many_arguments)]
fn edge_crossings(
    img: &GrayImage,
    a: PointPx,
    b: PointPx,
    black: f64,
    white: f64,
    search: f64,
    trim: f64,
) -> Vec<PointPx> {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = dx.hypot(dy);
    let mut out = Vec::new();
    if len < 1.0 || white - black < 1.0 {
        return out;
    }
    let level = (white + black) / 2.0;
    // Inward normal of a clockwise-on-screen quad (y down): values fall along it.
    let (nx, ny) = (-dy / len, dx / len);
    let horizontal = dx.abs() >= dy.abs();
    // Trimmed span along the scan axis, snapped to half-pixels for seed insensitivity.
    let snap = |v: f64| (v * 2.0).round() / 2.0;
    let (s0, s1) = if horizontal {
        let (x0, x1) = (a.x + trim * dx, b.x - trim * dx);
        (snap(x0.min(x1)), snap(x0.max(x1)))
    } else {
        let (y0, y1) = (a.y + trim * dy, b.y - trim * dy);
        (snap(y0.min(y1)), snap(y0.max(y1)))
    };
    let first = (s0 - 0.5).ceil() as i64;
    let last = (s1 - 0.5).floor() as i64;
    const HALF: f64 = 2.5;
    for k in first..=last {
        let centre = k as f64 + 0.5;
        // Predicted crossing on this scan line, and the direction values fall along it.
        let (pred, falls_forward) = if horizontal {
            let t = (centre - a.x) / dx;
            (a.y + t * dy, ny > 0.0)
        } else {
            let t = (centre - a.y) / dy;
            (a.x + t * dx, nx > 0.0)
        };
        let lo = (pred - search - 0.5).floor() as i64;
        let hi = (pred + search - 0.5).ceil() as i64;
        let sample = |j: i64| -> f64 {
            let (x, y) = if horizontal { (k, j) } else { (j, k) };
            f64::from(img.get_or(x as isize, y as isize, 255))
        };
        let mut best: Option<(f64, f64)> = None;
        for j in lo..hi {
            let (v0, v1) = (sample(j), sample(j + 1));
            let crosses = if falls_forward {
                v0 >= level && v1 < level
            } else {
                v0 < level && v1 >= level
            };
            if !crosses || v0 == v1 {
                continue;
            }
            let pos = j as f64 + 0.5 + (v0 - level) / (v0 - v1);
            let d = (pos - pred).abs();
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, pos));
            }
        }
        let Some((_, pos)) = best else {
            continue;
        };
        // Mass estimate over pixels [w0, w0 + 5) centred on the crossing.
        let w0 = (pos - HALF).round() as i64;
        let mut mass = 0.0;
        for j in w0..w0 + 5 {
            let v = sample(j);
            mass += if falls_forward {
                (v - black) / (white - black)
            } else {
                (white - v) / (white - black)
            };
        }
        let e = w0 as f64 + mass;
        if (e - pos).abs() > 1.0 {
            continue;
        }
        out.push(if horizontal {
            PointPx { x: centre, y: e }
        } else {
            PointPx { x: e, y: centre }
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A blurred, rotated black square on white, with known corners.
    fn square(angle: f64, cx: f64, cy: f64, half: f64) -> (GrayImage, [PointPx; 4]) {
        let (c, s) = (angle.cos(), angle.sin());
        let rot = |x: f64, y: f64| PointPx {
            x: cx + c * x - s * y,
            y: cy + s * x + c * y,
        };
        let corners = [
            rot(-half, -half),
            rot(half, -half),
            rot(half, half),
            rot(-half, half),
        ];
        // Supersampled coverage of the square.
        let img = GrayImage::from_fn(120, 120, |x, y| {
            let mut ink = 0.0f64;
            for sy in 0..16 {
                for sx in 0..16 {
                    let px = x as f64 + (sx as f64 + 0.5) / 16.0 - cx;
                    let py = y as f64 + (sy as f64 + 0.5) / 16.0 - cy;
                    let (u, v) = (c * px + s * py, -s * px + c * py);
                    if u.abs() <= half && v.abs() <= half {
                        ink += 1.0 / 256.0;
                    }
                }
            }
            (245.0 - ink * 220.0).round() as u8
        });
        (img, corners)
    }

    #[test]
    fn corners_to_a_twentieth_of_a_pixel() {
        for &angle in &[0.0, 0.03, -0.07, 0.5] {
            let (img, truth) = square(angle, 60.3, 58.7, 25.2);
            // Seed off by up to a pixel.
            let seed = [
                PointPx {
                    x: truth[0].x + 0.8,
                    y: truth[0].y - 0.6,
                },
                PointPx {
                    x: truth[1].x - 0.7,
                    y: truth[1].y + 0.9,
                },
                PointPx {
                    x: truth[2].x + 0.4,
                    y: truth[2].y + 0.5,
                },
                PointPx {
                    x: truth[3].x - 0.9,
                    y: truth[3].y - 0.3,
                },
            ];
            let r = refine_quad(&img, &seed, 3.0, 0.18, 0.6).unwrap();
            for (c, t) in r.corners.iter().zip(&truth) {
                let e = (c.x - t.x).hypot(c.y - t.y);
                assert!(e < 0.05, "angle {angle}: error {e}");
            }
            assert!(r.black < 40 && r.white > 230);
        }
    }
}
