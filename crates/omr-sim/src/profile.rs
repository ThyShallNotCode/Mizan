//! Capture profiles (spec §23.3 stage 7, narrowed to scanner capture) and the per-sheet capture
//! parameters drawn from them.

use rand::RngExt;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

/// A difficulty profile for marks and capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub enum Profile {
    /// Near-ideal flatbed capture: tiny skew, little blur and noise, high JPEG quality.
    Clean,
    /// The common case (§2.4 standard slice): ADF skew up to ±1.5°, 200–300 dpi, mild blur,
    /// noise, illumination and gamma variation, JPEG quality 75–95.
    Standard,
    /// Hard captures: skew up to ±4°, stronger blur, noise, illumination and gamma, JPEG 70–85,
    /// scanner streaks and dark backing.
    Hard,
}

impl Profile {
    /// Lower-case name.
    pub fn name(self) -> &'static str {
        match self {
            Profile::Clean => "clean",
            Profile::Standard => "standard",
            Profile::Hard => "hard",
        }
    }

    /// Parses a lower-case name.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "clean" => Some(Profile::Clean),
            "standard" => Some(Profile::Standard),
            "hard" => Some(Profile::Hard),
            _ => None,
        }
    }

    /// The ranges this profile draws from.
    pub fn ranges(self) -> CaptureRanges {
        match self {
            Profile::Clean => CaptureRanges {
                rotation_deg: 0.2,
                shift_mm: 0.5,
                scale: 0.002,
                anisotropy: 0.0005,
                shear: 0.0,
                perspective: 0.0,
                blur_sigma: (0.4, 0.5),
                noise_sigma: (0.5, 1.0),
                illumination: 0.01,
                gamma: (1.0, 1.0),
                paper: (245.0, 250.0),
                black: (15.0, 25.0),
                jpeg_quality: (92, 95),
                streak_prob: 0.0,
                margin_mm: 2.0,
                dark_backing_prob: 0.0,
            },
            Profile::Standard => CaptureRanges {
                rotation_deg: 1.5,
                shift_mm: 3.0,
                scale: 0.015,
                anisotropy: 0.004,
                shear: 0.003,
                perspective: 2.0e-5,
                blur_sigma: (0.4, 1.0),
                noise_sigma: (1.0, 3.0),
                illumination: 0.06,
                gamma: (0.85, 1.15),
                paper: (232.0, 250.0),
                black: (10.0, 40.0),
                jpeg_quality: (75, 95),
                streak_prob: 0.0,
                margin_mm: 4.0,
                dark_backing_prob: 0.0,
            },
            Profile::Hard => CaptureRanges {
                rotation_deg: 4.0,
                shift_mm: 5.0,
                scale: 0.03,
                anisotropy: 0.01,
                shear: 0.008,
                perspective: 6.0e-5,
                blur_sigma: (0.6, 1.2),
                noise_sigma: (2.0, 6.0),
                illumination: 0.15,
                gamma: (0.7, 1.4),
                paper: (215.0, 245.0),
                black: (20.0, 60.0),
                jpeg_quality: (70, 85),
                streak_prob: 0.4,
                margin_mm: 8.0,
                dark_backing_prob: 0.5,
            },
        }
    }
}

/// Parameter ranges of a profile (symmetric ranges are ± the value).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaptureRanges {
    /// Rotation, ± degrees.
    pub rotation_deg: f64,
    /// Translation per axis, ± mm.
    pub shift_mm: f64,
    /// Isotropic scale, ± fraction (print "fit" and feed scale).
    pub scale: f64,
    /// Anisotropy s_x/s_y − 1, ± fraction.
    pub anisotropy: f64,
    /// Shear, ± fraction.
    pub shear: f64,
    /// Perspective terms, ± per mm.
    pub perspective: f64,
    /// Gaussian blur σ in output pixels.
    pub blur_sigma: (f64, f64),
    /// Additive Gaussian noise σ in gray levels.
    pub noise_sigma: (f64, f64),
    /// Linear illumination gradient amplitude, ± fraction across the page.
    pub illumination: f64,
    /// Gamma.
    pub gamma: (f64, f64),
    /// Captured paper white level.
    pub paper: (f64, f64),
    /// Captured toner black level.
    pub black: (f64, f64),
    /// JPEG quality.
    pub jpeg_quality: (u8, u8),
    /// Probability of a scanner streak.
    pub streak_prob: f64,
    /// Scan margin around the page, mm.
    pub margin_mm: f64,
    /// Probability of a dark (instead of light) scanner backing.
    pub dark_backing_prob: f64,
}

/// A dirty-glass streak: a vertical band of altered intensity.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Streak {
    /// Left column, px.
    pub x: u32,
    /// Width, px.
    pub width: u32,
    /// Intensity change (negative = darker).
    pub delta: f64,
}

/// The capture parameters of one page image (recorded in the truth file).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaptureParams {
    /// Output resolution.
    pub dpi: u32,
    /// Rotation, degrees (positive = clockwise on the image).
    pub rotation_deg: f64,
    /// Translation of the page in the frame, mm.
    pub shift_mm: (f64, f64),
    /// Scale per axis.
    pub scale: (f64, f64),
    /// Shear.
    pub shear: f64,
    /// Perspective terms (per mm, page-centred).
    pub perspective: (f64, f64),
    /// Blur σ, px.
    pub blur_sigma: f64,
    /// Noise σ, gray levels.
    pub noise_sigma: f64,
    /// Illumination gradient (x, y), fraction across the page.
    pub illumination: (f64, f64),
    /// Gamma.
    pub gamma: f64,
    /// Paper white level.
    pub paper: f64,
    /// Toner black level.
    pub black: f64,
    /// JPEG quality (ignored for PNG output).
    pub jpeg_quality: u8,
    /// Optional streak.
    pub streak: Option<Streak>,
    /// Scan margin, mm.
    pub margin_mm: f64,
    /// Backing gray level outside the paper.
    pub backing: u8,
}

fn sym(rng: &mut ChaCha8Rng, a: f64) -> f64 {
    if a == 0.0 {
        0.0
    } else {
        rng.random_range(-a..=a)
    }
}

fn span(rng: &mut ChaCha8Rng, r: (f64, f64)) -> f64 {
    if r.0 >= r.1 {
        r.0
    } else {
        rng.random_range(r.0..=r.1)
    }
}

impl CaptureParams {
    /// Draws one page's parameters.
    pub fn sample(profile: Profile, dpi: u32, rng: &mut ChaCha8Rng) -> Self {
        let r = profile.ranges();
        let s = 1.0 + sym(rng, r.scale);
        let an = sym(rng, r.anisotropy);
        let w_px = ((210.0 + 2.0 * r.margin_mm) * f64::from(dpi) / 25.4) as u32;
        let streak = if r.streak_prob > 0.0 && rng.random_bool(r.streak_prob) {
            let dark = rng.random_bool(0.8);
            let mag = rng.random_range(30.0..=120.0);
            Some(Streak {
                x: rng.random_range(0..w_px.max(1)),
                width: rng.random_range(1..=3),
                delta: if dark { -mag } else { mag * 0.5 },
            })
        } else {
            None
        };
        let backing = if r.dark_backing_prob > 0.0 && rng.random_bool(r.dark_backing_prob) {
            rng.random_range(10..=40)
        } else {
            rng.random_range(200..=235)
        };
        let (q0, q1) = r.jpeg_quality;
        CaptureParams {
            dpi,
            rotation_deg: sym(rng, r.rotation_deg),
            shift_mm: (sym(rng, r.shift_mm), sym(rng, r.shift_mm)),
            scale: (s * (1.0 + an / 2.0), s * (1.0 - an / 2.0)),
            shear: sym(rng, r.shear),
            perspective: (sym(rng, r.perspective), sym(rng, r.perspective)),
            blur_sigma: span(rng, r.blur_sigma),
            noise_sigma: span(rng, r.noise_sigma),
            illumination: (sym(rng, r.illumination), sym(rng, r.illumination)),
            gamma: span(rng, r.gamma),
            paper: span(rng, r.paper),
            black: span(rng, r.black),
            jpeg_quality: if q0 >= q1 {
                q0
            } else {
                rng.random_range(q0..=q1)
            },
            streak,
            margin_mm: r.margin_mm,
            backing,
        }
    }

    /// Output image size.
    pub fn frame_px(&self) -> (u32, u32) {
        let k = f64::from(self.dpi) / 25.4;
        (
            ((210.0 + 2.0 * self.margin_mm) * k).round() as u32,
            ((297.0 + 2.0 * self.margin_mm) * k).round() as u32,
        )
    }

    /// Row-major 3 × 3 homography from template millimetres to output pixel coordinates
    /// (pixel `(i, j)` covers `[i, i+1) × [j, j+1)`).
    pub fn homography(&self) -> [f64; 9] {
        let (cx, cy) = (105.0, 148.5);
        let k = f64::from(self.dpi) / 25.4;
        let th = self.rotation_deg.to_radians();
        let (c, s) = (th.cos(), th.sin());
        // A = R · [[sx, shear], [0, sy]]
        let (sx, sy) = self.scale;
        let a = [
            c * sx,
            c * self.shear - s * sy,
            s * sx,
            s * self.shear + c * sy,
        ];
        let (g, h) = self.perspective;
        let tx = cx + self.margin_mm + self.shift_mm.0;
        let ty = cy + self.margin_mm + self.shift_mm.1;
        // H = S_k · T(t) · P(g, h) · [A | 0] · T(−c)
        // Rows of P·[A|0]·T(−c) acting on (X, Y, 1):
        let r0 = [a[0], a[1], -(a[0] * cx + a[1] * cy)];
        let r1 = [a[2], a[3], -(a[2] * cx + a[3] * cy)];
        let r2 = [
            g * r0[0] + h * r1[0],
            g * r0[1] + h * r1[1],
            g * r0[2] + h * r1[2] + 1.0,
        ];
        [
            k * (r0[0] + tx * r2[0]),
            k * (r0[1] + tx * r2[1]),
            k * (r0[2] + tx * r2[2]),
            k * (r1[0] + ty * r2[0]),
            k * (r1[1] + ty * r2[1]),
            k * (r1[2] + ty * r2[2]),
            r2[0],
            r2[1],
            r2[2],
        ]
    }
}

/// Applies a row-major homography to a point.
pub fn apply_h(h: &[f64; 9], x: f64, y: f64) -> (f64, f64) {
    let w = h[6] * x + h[7] * y + h[8];
    (
        (h[0] * x + h[1] * y + h[2]) / w,
        (h[3] * x + h[4] * y + h[5]) / w,
    )
}

/// Inverts a row-major 3 × 3 matrix (returns `None` if singular).
pub fn invert(m: &[f64; 9]) -> Option<[f64; 9]> {
    let det = m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6])
        + m[2] * (m[3] * m[7] - m[4] * m[6]);
    if det.abs() < 1e-18 {
        return None;
    }
    let d = 1.0 / det;
    Some([
        (m[4] * m[8] - m[5] * m[7]) * d,
        (m[2] * m[7] - m[1] * m[8]) * d,
        (m[1] * m[5] - m[2] * m[4]) * d,
        (m[5] * m[6] - m[3] * m[8]) * d,
        (m[0] * m[8] - m[2] * m[6]) * d,
        (m[2] * m[3] - m[0] * m[5]) * d,
        (m[3] * m[7] - m[4] * m[6]) * d,
        (m[1] * m[6] - m[0] * m[7]) * d,
        (m[0] * m[4] - m[1] * m[3]) * d,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;

    #[test]
    fn identity_capture_maps_mm_to_px() {
        let mut p = CaptureParams::sample(Profile::Clean, 200, &mut ChaCha8Rng::seed_from_u64(1));
        p.rotation_deg = 0.0;
        p.shift_mm = (0.0, 0.0);
        p.scale = (1.0, 1.0);
        p.shear = 0.0;
        p.perspective = (0.0, 0.0);
        p.margin_mm = 0.0;
        let h = p.homography();
        let (u, v) = apply_h(&h, 25.4, 50.8);
        assert!((u - 200.0).abs() < 1e-9 && (v - 400.0).abs() < 1e-9);
    }

    #[test]
    fn homography_inverts() {
        let p = CaptureParams::sample(Profile::Hard, 300, &mut ChaCha8Rng::seed_from_u64(9));
        let h = p.homography();
        let hi = invert(&h).unwrap();
        let (u, v) = apply_h(&h, 30.0, 200.0);
        let (x, y) = apply_h(&hi, u, v);
        assert!((x - 30.0).abs() < 1e-9 && (y - 200.0).abs() < 1e-9);
        // The page centre moves only by the shift.
        let (u, v) = apply_h(&h, 105.0, 148.5);
        let k = 300.0 / 25.4;
        assert!((u / k - (105.0 + p.margin_mm + p.shift_mm.0)).abs() < 1e-9);
        assert!((v / k - (148.5 + p.margin_mm + p.shift_mm.1)).abs() < 1e-9);
    }

    #[test]
    fn profiles_order_by_difficulty() {
        let c = Profile::Clean.ranges();
        let s = Profile::Standard.ranges();
        let h = Profile::Hard.ranges();
        assert!(c.rotation_deg < s.rotation_deg && s.rotation_deg < h.rotation_deg);
        assert!(c.noise_sigma.1 <= s.noise_sigma.1 && s.noise_sigma.1 < h.noise_sigma.1);
        assert_eq!(s.rotation_deg, 1.5);
        assert_eq!(h.rotation_deg, 4.0);
        assert_eq!(Profile::parse("standard"), Some(Profile::Standard));
    }
}
