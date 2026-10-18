//! Row ticks (§6.3: 1.2 × 0.6 mm at both ends of every answer row; §8.4 Stage C control
//! points, simplified). Each tick is located by the dark-mass centroid in a window around its
//! Stage A prediction; its residual is the local registration error of that row end. A row's
//! bubbles are shifted by the residual interpolated linearly between its two ticks (one tick
//! alone shifts the row rigidly). This absorbs ADF feed-speed variation and photocopy stretch
//! that a homography cannot model. On captures a homography already models exactly, tick
//! centroids are noisier than the 16-corner fit, so corrections are applied only when the
//! page's median tick residual exceeds a floor (configurable).

use super::config::TickConfig;
use omr_forms::{PageGeometry, Side};
use omr_imaging::{GrayImage, Homography, Warp};
use omr_model::PointPx;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One located tick.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct TickHit {
    /// Tick centre on the page (mm).
    pub mm: PointPx,
    /// Residual: measured − predicted (image px).
    pub residual: PointPx,
}

/// The tick evidence of one page.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TickField {
    /// Per question: (left, right) ticks that passed the outlier check.
    pub rows: BTreeMap<u16, (Option<TickHit>, Option<TickHit>)>,
    /// Ticks in the layout.
    pub expected: u32,
    /// Ticks located.
    pub found: u32,
    /// Ticks rejected as outliers.
    pub rejected: u32,
    /// Median residual length (µm on the page).
    pub median_um: f64,
    /// 95th percentile residual length (µm).
    pub p95_um: f64,
    /// True if corrections are applied to bubble centres.
    pub applied: bool,
}

impl TickField {
    /// The correction (image px) for a point of question `q`'s row at page position `mm`.
    pub fn correction(&self, q: u16, mm: PointPx) -> PointPx {
        if !self.applied {
            return PointPx::default();
        }
        match self.rows.get(&q) {
            Some((Some(l), Some(r))) => {
                let span = r.mm.x - l.mm.x;
                let t = if span.abs() > 1e-6 {
                    ((mm.x - l.mm.x) / span).clamp(0.0, 1.0)
                } else {
                    0.5
                };
                PointPx {
                    x: l.residual.x + t * (r.residual.x - l.residual.x),
                    y: l.residual.y + t * (r.residual.y - l.residual.y),
                }
            }
            Some((Some(one), None)) | Some((None, Some(one))) => one.residual,
            _ => PointPx::default(),
        }
    }
}

fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

/// Dark-mass centroid of the tick near `pred` (image px) in a square window of half-size
/// `half` px; `None` if the window holds too little ink.
fn centroid(
    img: &GrayImage,
    pred: PointPx,
    half: f64,
    black: u8,
    min_mass: f64,
) -> Option<PointPx> {
    let x0 = (pred.x - half).floor() as isize;
    let y0 = (pred.y - half).floor() as isize;
    let n = (2.0 * half).ceil() as isize + 1;
    if n <= 2 {
        return None;
    }
    let mut vals = Vec::with_capacity((n * n) as usize);
    for y in y0..y0 + n {
        for x in x0..x0 + n {
            vals.push(img.get_or(x, y, 255));
        }
    }
    let mut sorted = vals.clone();
    sorted.sort_unstable();
    let paper = f64::from(sorted[sorted.len() / 2]);
    let contrast = paper - f64::from(black);
    if contrast < 30.0 {
        return None;
    }
    let tau = 0.25 * contrast;
    let (mut m, mut mx, mut my) = (0.0, 0.0, 0.0);
    for (i, v) in vals.iter().enumerate() {
        let w = paper - f64::from(*v) - tau;
        if w <= 0.0 {
            continue;
        }
        let x = (x0 + (i as isize % n)) as f64 + 0.5;
        let y = (y0 + (i as isize / n)) as f64 + 0.5;
        m += w;
        mx += w * x;
        my += w * y;
    }
    if m < min_mass * contrast {
        return None;
    }
    Some(PointPx {
        x: mx / m,
        y: my / m,
    })
}

/// Locates every tick of `page` under Stage A's map `h` (template mm → image px). `force`
/// applies the corrections whatever the median residual (Stage A from two tags only, §8.4:
/// "Stage C needs row ticks").
pub(crate) fn locate(
    img: &GrayImage,
    h: &Homography,
    page: &PageGeometry,
    black: u8,
    px_per_mm: f64,
    cfg: &TickConfig,
    force: bool,
) -> TickField {
    let mut field = TickField {
        expected: page.ticks.len() as u32,
        ..TickField::default()
    };
    if page.ticks.is_empty() || px_per_mm <= 0.0 {
        return field;
    }
    let tick_half_mm = 0.6;
    let half = (tick_half_mm + f64::from(cfg.search_um) / 1000.0) * px_per_mm;
    // Expected ink: 1.2 × 0.6 mm at full contrast; accept from a third of it.
    let min_mass = 0.72 * px_per_mm * px_per_mm / 3.0 * 0.5;
    let mut hits: Vec<(u16, Side, TickHit)> = Vec::new();
    for t in &page.ticks {
        let c = t.centre_mm();
        let mm = PointPx { x: c.x, y: c.y };
        let pred = h.map(mm);
        if let Some(got) = centroid(img, pred, half, black, min_mass) {
            hits.push((
                t.question,
                t.side,
                TickHit {
                    mm,
                    residual: PointPx {
                        x: got.x - pred.x,
                        y: got.y - pred.y,
                    },
                },
            ));
        }
    }
    field.found = hits.len() as u32;
    // Outliers: compare each residual with the median of the same side's ticks within ±3 rows
    // of the same column (neighbours by page position).
    let outlier_px = f64::from(cfg.outlier_um) / 1000.0 * px_per_mm;
    let mut kept: Vec<(u16, Side, TickHit)> = Vec::new();
    for (i, (q, side, hit)) in hits.iter().enumerate() {
        let mut nx: Vec<f64> = Vec::new();
        let mut ny: Vec<f64> = Vec::new();
        for (j, (_, s2, h2)) in hits.iter().enumerate() {
            if i == j || s2 != side {
                continue;
            }
            if (h2.mm.x - hit.mm.x).abs() < 3.0 && (h2.mm.y - hit.mm.y).abs() < 25.0 {
                nx.push(h2.residual.x);
                ny.push(h2.residual.y);
            }
        }
        if nx.len() >= 2 {
            let (mx, my) = (median(&mut nx), median(&mut ny));
            if (hit.residual.x - mx).hypot(hit.residual.y - my) > outlier_px {
                field.rejected += 1;
                continue;
            }
        }
        kept.push((*q, *side, *hit));
    }
    let mut lens: Vec<f64> = kept
        .iter()
        .map(|(_, _, h)| h.residual.x.hypot(h.residual.y) / px_per_mm * 1000.0)
        .collect();
    lens.sort_by(f64::total_cmp);
    if !lens.is_empty() {
        field.median_um = lens[lens.len() / 2];
        field.p95_um = lens[((lens.len() as f64 * 0.95).ceil() as usize).clamp(1, lens.len()) - 1];
    }
    for (q, side, hit) in kept {
        let e = field.rows.entry(q).or_insert((None, None));
        match side {
            Side::Left => e.0 = Some(hit),
            Side::Right => e.1 = Some(hit),
        }
    }
    field.applied = cfg.apply
        && (force || field.median_um > f64::from(cfg.min_median_residual_um))
        && field.found * 2 >= field.expected;
    field
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centroid_of_a_bar() {
        // A 10 × 5 px bar centred at (20.5, 30.25) (rows 28–32.5 approximated by coverage).
        let img = GrayImage::from_fn(60, 60, |x, y| {
            if (16..26).contains(&x) && (28..33).contains(&y) {
                30
            } else {
                240
            }
        });
        let c = centroid(&img, PointPx { x: 19.0, y: 29.0 }, 12.0, 30, 5.0).unwrap();
        assert!(
            (c.x - 21.0).abs() < 1e-9 && (c.y - 30.5).abs() < 1e-9,
            "{c:?}"
        );
        let f = TickField {
            rows: BTreeMap::from([(
                3,
                (
                    Some(TickHit {
                        mm: PointPx { x: 10.0, y: 50.0 },
                        residual: PointPx { x: 1.0, y: 0.0 },
                    }),
                    Some(TickHit {
                        mm: PointPx { x: 50.0, y: 50.0 },
                        residual: PointPx { x: 3.0, y: 2.0 },
                    }),
                ),
            )]),
            applied: true,
            ..TickField::default()
        };
        let c = f.correction(3, PointPx { x: 30.0, y: 50.0 });
        assert!((c.x - 2.0).abs() < 1e-12 && (c.y - 1.0).abs() < 1e-12);
        assert_eq!(
            f.correction(4, PointPx { x: 30.0, y: 50.0 }),
            PointPx::default()
        );
    }
}
