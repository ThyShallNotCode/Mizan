//! Nearest-neighbour queries on a uniform grid (spatial hashing). Ties break on the lower point
//! index, so results never depend on iteration order.

use omr_model::PointPx;

/// A static set of points bucketed into square cells.
#[derive(Clone, Debug)]
pub struct PointGrid {
    pts: Vec<PointPx>,
    cell: f64,
    ox: f64,
    oy: f64,
    cols: usize,
    rows: usize,
    starts: Vec<u32>,
    order: Vec<u32>,
}

impl PointGrid {
    /// Buckets `pts` into cells of side `cell` px (choose ≈ the largest query radius).
    pub fn new(pts: &[PointPx], cell: f64) -> Self {
        let cell = cell.max(1e-6);
        let finite: Vec<&PointPx> = pts
            .iter()
            .filter(|p| p.x.is_finite() && p.y.is_finite())
            .collect();
        let (mut minx, mut miny, mut maxx, mut maxy) = (0.0f64, 0.0f64, 1.0f64, 1.0f64);
        if let Some(f) = finite.first() {
            (minx, miny, maxx, maxy) = (f.x, f.y, f.x, f.y);
        }
        for p in &finite {
            minx = minx.min(p.x);
            miny = miny.min(p.y);
            maxx = maxx.max(p.x);
            maxy = maxy.max(p.y);
        }
        let cols = (((maxx - minx) / cell).floor() as usize + 1).min(1 << 16);
        let rows = (((maxy - miny) / cell).floor() as usize + 1).min(1 << 16);
        let mut g = Self {
            pts: pts.to_vec(),
            cell,
            ox: minx,
            oy: miny,
            cols,
            rows,
            starts: vec![0; cols * rows + 1],
            order: Vec::with_capacity(pts.len()),
        };
        let cell_of: Vec<Option<usize>> = pts.iter().map(|p| g.cell_index(*p)).collect();
        for c in cell_of.iter().flatten() {
            g.starts[c + 1] += 1;
        }
        for i in 0..cols * rows {
            g.starts[i + 1] += g.starts[i];
        }
        let mut fill = g.starts.clone();
        g.order = vec![0; g.starts[cols * rows] as usize];
        for (i, c) in cell_of.iter().enumerate() {
            if let Some(c) = c {
                g.order[fill[*c] as usize] = i as u32;
                fill[*c] += 1;
            }
        }
        g
    }

    fn cell_index(&self, p: PointPx) -> Option<usize> {
        if !(p.x.is_finite() && p.y.is_finite()) {
            return None;
        }
        let cx = ((p.x - self.ox) / self.cell).floor();
        let cy = ((p.y - self.oy) / self.cell).floor();
        if cx < 0.0 || cy < 0.0 || cx >= self.cols as f64 || cy >= self.rows as f64 {
            return None;
        }
        Some(cy as usize * self.cols + cx as usize)
    }

    /// The indexed points.
    pub fn points(&self) -> &[PointPx] {
        &self.pts
    }

    /// Calls `f(index, squared distance)` for every point within `r` of `p`.
    pub fn for_each_within(&self, p: PointPx, r: f64, mut f: impl FnMut(usize, f64)) {
        if !(p.x.is_finite() && p.y.is_finite()) || r < 0.0 {
            return;
        }
        let r2 = r * r;
        let x0 = ((p.x - r - self.ox) / self.cell).floor().max(0.0);
        let x1 = ((p.x + r - self.ox) / self.cell).floor();
        let y0 = ((p.y - r - self.oy) / self.cell).floor().max(0.0);
        let y1 = ((p.y + r - self.oy) / self.cell).floor();
        if x1 < 0.0 || y1 < 0.0 {
            return;
        }
        let x1 = x1.min(self.cols as f64 - 1.0) as usize;
        let y1 = y1.min(self.rows as f64 - 1.0) as usize;
        let (x0, y0) = (x0 as usize, y0 as usize);
        if x0 > x1 || y0 > y1 {
            return;
        }
        for cy in y0..=y1 {
            for cx in x0..=x1 {
                let c = cy * self.cols + cx;
                for &i in &self.order[self.starts[c] as usize..self.starts[c + 1] as usize] {
                    let q = self.pts[i as usize];
                    let d2 = (q.x - p.x) * (q.x - p.x) + (q.y - p.y) * (q.y - p.y);
                    if d2 <= r2 {
                        f(i as usize, d2);
                    }
                }
            }
        }
    }

    /// The nearest point within `r` of `p`: `(index, distance)`.
    pub fn nearest_within(&self, p: PointPx, r: f64) -> Option<(usize, f64)> {
        let mut best: Option<(usize, f64)> = None;
        self.for_each_within(p, r, |i, d2| {
            if best.is_none_or(|(bi, bd)| d2 < bd || (d2 == bd && i < bi)) {
                best = Some((i, d2));
            }
        });
        best.map(|(i, d2)| (i, d2.sqrt()))
    }

    /// Up to `k` nearest points within `r` of `p`, nearest first, excluding index `skip`.
    pub fn k_nearest_within(
        &self,
        p: PointPx,
        k: usize,
        r: f64,
        skip: Option<usize>,
    ) -> Vec<(usize, f64)> {
        let mut v: Vec<(usize, f64)> = Vec::new();
        self.for_each_within(p, r, |i, d2| {
            if Some(i) != skip {
                v.push((i, d2));
            }
        });
        v.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        v.truncate(k);
        v.into_iter().map(|(i, d2)| (i, d2.sqrt())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_matches_brute_force() {
        let pts: Vec<PointPx> = (0..400)
            .map(|i| PointPx {
                x: ((i * 7919) % 1000) as f64 * 1.37,
                y: ((i * 104_729) % 997) as f64 * 0.91,
            })
            .collect();
        let g = PointGrid::new(&pts, 16.0);
        for q in 0..50 {
            let p = PointPx {
                x: q as f64 * 27.3,
                y: q as f64 * 17.9,
            };
            let brute = pts
                .iter()
                .enumerate()
                .map(|(i, a)| (i, ((a.x - p.x).powi(2) + (a.y - p.y).powi(2)).sqrt()))
                .filter(|(_, d)| *d <= 40.0)
                .min_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
            assert_eq!(g.nearest_within(p, 40.0), brute);
        }
        let kn = g.k_nearest_within(pts[3], 4, 1e9, Some(3));
        assert_eq!(kn.len(), 4);
        assert!(kn.windows(2).all(|w| w[0].1 <= w[1].1));
    }
}
