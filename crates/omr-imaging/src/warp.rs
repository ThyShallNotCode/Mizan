//! Planar warps: affine (with the §8.4 decomposition), the 12-parameter quadratic polynomial
//! warp used for legacy forms, and the homography.

use omr_model::PointPx;
use serde::{Deserialize, Serialize};

/// A map from template pixels to image pixels.
pub trait Warp {
    /// Maps a point.
    fn map(&self, p: PointPx) -> PointPx;

    /// The 2 × 2 Jacobian `[[∂x'/∂x, ∂x'/∂y], [∂y'/∂x, ∂y'/∂y]]` at `p` (default: central
    /// differences over ±0.5 px).
    fn jacobian(&self, p: PointPx) -> [[f64; 2]; 2] {
        let h = 0.5;
        let xp = self.map(PointPx { x: p.x + h, y: p.y });
        let xm = self.map(PointPx { x: p.x - h, y: p.y });
        let yp = self.map(PointPx { x: p.x, y: p.y + h });
        let ym = self.map(PointPx { x: p.x, y: p.y - h });
        [
            [(xp.x - xm.x) / (2.0 * h), (yp.x - ym.x) / (2.0 * h)],
            [(xp.y - xm.y) / (2.0 * h), (yp.y - ym.y) / (2.0 * h)],
        ]
    }

    /// Local linear scale `√|det J|` at `p`.
    fn local_scale(&self, p: PointPx) -> f64 {
        let j = self.jacobian(p);
        (j[0][0] * j[1][1] - j[0][1] * j[1][0]).abs().sqrt()
    }
}

/// `x' = a·x + b·y + tx`, `y' = c·x + d·y + ty`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Affine {
    /// `[a, b, tx, c, d, ty]`.
    pub m: [f64; 6],
}

/// The §8.4 decomposition `A = R(θ) · [[s_x, k], [0, s_y]]`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Decomposition {
    /// Rotation θ in radians.
    pub rotation: f64,
    /// √(s_x · s_y).
    pub scale: f64,
    /// s_x / s_y − 1.
    pub anisotropy: f64,
    /// Shear k / s_y.
    pub shear: f64,
}

impl Affine {
    /// The identity map.
    pub const IDENTITY: Affine = Affine {
        m: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
    };

    /// From the six coefficients.
    pub fn new(a: f64, b: f64, tx: f64, c: f64, d: f64, ty: f64) -> Self {
        Self {
            m: [a, b, tx, c, d, ty],
        }
    }

    /// `s · R(θ) · p + t`.
    pub fn similarity(scale: f64, angle: f64, tx: f64, ty: f64) -> Self {
        let (s, c) = angle.sin_cos();
        Self::new(scale * c, -scale * s, tx, scale * s, scale * c, ty)
    }

    /// A pure translation.
    pub fn translation(tx: f64, ty: f64) -> Self {
        Self::new(1.0, 0.0, tx, 0.0, 1.0, ty)
    }

    /// `other ∘ self` (apply `self` first).
    pub fn then(&self, other: &Affine) -> Affine {
        let [a, b, tx, c, d, ty] = self.m;
        let [e, f, ux, g, h, uy] = other.m;
        Affine::new(
            e * a + f * c,
            e * b + f * d,
            e * tx + f * ty + ux,
            g * a + h * c,
            g * b + h * d,
            g * tx + h * ty + uy,
        )
    }

    /// The inverse map, if the linear part is invertible.
    pub fn inverse(&self) -> Option<Affine> {
        let [a, b, tx, c, d, ty] = self.m;
        let det = a * d - b * c;
        if det.abs() < 1e-15 || !det.is_finite() {
            return None;
        }
        let (ia, ib, ic, id) = (d / det, -b / det, -c / det, a / det);
        Some(Affine::new(
            ia,
            ib,
            -(ia * tx + ib * ty),
            ic,
            id,
            -(ic * tx + id * ty),
        ))
    }

    /// The §8.4 decomposition of the linear part.
    pub fn decompose(&self) -> Decomposition {
        let [a, b, _, c, d, _] = self.m;
        let rotation = c.atan2(a);
        let sx = (a * a + c * c).sqrt();
        let det = a * d - b * c;
        let sy = if sx > 0.0 { det / sx } else { 0.0 };
        let k = if sx > 0.0 { (a * b + c * d) / sx } else { 0.0 };
        Decomposition {
            rotation,
            scale: det.abs().sqrt(),
            anisotropy: if sy != 0.0 { sx / sy - 1.0 } else { 0.0 },
            shear: if sy != 0.0 { k / sy } else { 0.0 },
        }
    }
}

impl Warp for Affine {
    fn map(&self, p: PointPx) -> PointPx {
        let [a, b, tx, c, d, ty] = self.m;
        PointPx {
            x: a * p.x + b * p.y + tx,
            y: c * p.x + d * p.y + ty,
        }
    }

    fn jacobian(&self, _p: PointPx) -> [[f64; 2]; 2] {
        [[self.m[0], self.m[1]], [self.m[3], self.m[4]]]
    }
}

/// The quadratic polynomial warp (12 parameters): with `x = (X − c_x)/n`, `y = (Y − c_y)/n`,
/// `x' = Σ cx_k · f_k`, `y' = Σ cy_k · f_k` over `f = [1, x, y, x², x·y, y²]`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct QuadWarp {
    /// Normalisation centre (template pixels).
    pub centre: PointPx,
    /// Normalisation scale (template pixels per unit).
    pub norm: f64,
    /// Coefficients of x'.
    pub cx: [f64; 6],
    /// Coefficients of y'.
    pub cy: [f64; 6],
}

impl QuadWarp {
    /// The basis `[1, x, y, x², x·y, y²]` at `p`.
    pub fn basis(&self, p: PointPx) -> [f64; 6] {
        let x = (p.x - self.centre.x) / self.norm;
        let y = (p.y - self.centre.y) / self.norm;
        [1.0, x, y, x * x, x * y, y * y]
    }

    /// The same map as an affine, expressed in this normalisation.
    pub fn from_affine(a: &Affine, centre: PointPx, norm: f64) -> Self {
        let o = a.map(centre);
        let [aa, b, _, c, d, _] = a.m;
        Self {
            centre,
            norm,
            cx: [o.x, aa * norm, b * norm, 0.0, 0.0, 0.0],
            cy: [o.y, c * norm, d * norm, 0.0, 0.0, 0.0],
        }
    }
}

impl Warp for QuadWarp {
    fn map(&self, p: PointPx) -> PointPx {
        let f = self.basis(p);
        let mut x = 0.0;
        let mut y = 0.0;
        for ((fk, a), b) in f.iter().zip(&self.cx).zip(&self.cy) {
            x += a * fk;
            y += b * fk;
        }
        PointPx { x, y }
    }

    fn jacobian(&self, p: PointPx) -> [[f64; 2]; 2] {
        let x = (p.x - self.centre.x) / self.norm;
        let y = (p.y - self.centre.y) / self.norm;
        let n = self.norm;
        let d = |c: &[f64; 6]| {
            (
                (c[1] + 2.0 * c[3] * x + c[4] * y) / n,
                (c[2] + c[4] * x + 2.0 * c[5] * y) / n,
            )
        };
        let (xx, xy) = d(&self.cx);
        let (yx, yy) = d(&self.cy);
        [[xx, xy], [yx, yy]]
    }
}

/// A plane projective map, `h` row-major 3 × 3.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Homography {
    /// Row-major coefficients.
    pub h: [f64; 9],
}

impl Homography {
    /// The identity.
    pub const IDENTITY: Homography = Homography {
        h: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
    };

    /// The same map as an affine.
    pub fn from_affine(a: &Affine) -> Self {
        let [aa, b, tx, c, d, ty] = a.m;
        Self {
            h: [aa, b, tx, c, d, ty, 0.0, 0.0, 1.0],
        }
    }

    /// The inverse map, if invertible.
    pub fn inverse(&self) -> Option<Homography> {
        let h = &self.h;
        let c00 = h[4] * h[8] - h[5] * h[7];
        let c01 = h[5] * h[6] - h[3] * h[8];
        let c02 = h[3] * h[7] - h[4] * h[6];
        let det = h[0] * c00 + h[1] * c01 + h[2] * c02;
        if det.abs() < 1e-15 || !det.is_finite() {
            return None;
        }
        let inv = [
            c00 / det,
            (h[2] * h[7] - h[1] * h[8]) / det,
            (h[1] * h[5] - h[2] * h[4]) / det,
            c01 / det,
            (h[0] * h[8] - h[2] * h[6]) / det,
            (h[2] * h[3] - h[0] * h[5]) / det,
            c02 / det,
            (h[1] * h[6] - h[0] * h[7]) / det,
            (h[0] * h[4] - h[1] * h[3]) / det,
        ];
        Some(Homography { h: inv })
    }
}

impl Warp for Homography {
    fn map(&self, p: PointPx) -> PointPx {
        let h = &self.h;
        let w = h[6] * p.x + h[7] * p.y + h[8];
        PointPx {
            x: (h[0] * p.x + h[1] * p.y + h[2]) / w,
            y: (h[3] * p.x + h[4] * p.y + h[5]) / w,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: PointPx, b: PointPx) -> bool {
        (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9
    }

    #[test]
    fn affine_inverse_and_compose() {
        let a = Affine::similarity(0.9, 0.05, 12.0, -7.0);
        let i = a.inverse().unwrap();
        let p = PointPx { x: 100.0, y: 50.0 };
        assert!(close(i.map(a.map(p)), p));
        assert!(close(a.then(&i).map(p), p));
        let d = a.decompose();
        assert!((d.scale - 0.9).abs() < 1e-12 && (d.rotation - 0.05).abs() < 1e-12);
        assert!(d.anisotropy.abs() < 1e-12 && d.shear.abs() < 1e-12);
    }

    #[test]
    fn quad_from_affine_agrees() {
        let a = Affine::new(1.01, 0.02, 5.0, -0.01, 0.99, 3.0);
        let q = QuadWarp::from_affine(&a, PointPx { x: 500.0, y: 400.0 }, 1000.0);
        for p in [
            PointPx { x: 0.0, y: 0.0 },
            PointPx {
                x: 2000.0,
                y: 1500.0,
            },
        ] {
            assert!(close(q.map(p), a.map(p)));
            let j = q.jacobian(p);
            assert!((j[0][0] - 1.01).abs() < 1e-12 && (j[1][0] + 0.01).abs() < 1e-12);
        }
    }

    #[test]
    fn homography_inverse() {
        let h = Homography {
            h: [1.0, 0.1, 5.0, -0.05, 0.95, 2.0, 1e-5, 2e-5, 1.0],
        };
        let p = PointPx { x: 300.0, y: 200.0 };
        assert!(close(h.inverse().unwrap().map(h.map(p)), p));
    }
}
