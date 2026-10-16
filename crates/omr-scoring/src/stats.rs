//! Advisory statistics shared by scoring, outcomes and analytics (spec §10.2 "Statistics" row).
//!
//! These are `f64` functions for confidence intervals, p-values and quantiles. They never
//! decide a score: grade boundaries, τ and T comparisons stay exact. The PRNG is a seeded
//! SplitMix64 so bootstrap CIs are reproducible (§11.3, §12.3).

use sha2::{Digest, Sha384};

/// z for a two-sided 95 % interval (§11.3 uses 1.959964).
pub const Z95: f64 = 1.959_964;

/// ln Γ(x) for x > 0 (Lanczos, g = 7, n = 9; ~15 significant digits).
pub fn ln_gamma(x: f64) -> f64 {
    const G: f64 = 7.0;
    const C: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    if x < 0.5 {
        // Reflection.
        let pi = std::f64::consts::PI;
        return (pi / (pi * x).sin()).ln() - ln_gamma(1.0 - x);
    }
    let x = x - 1.0;
    let mut a = C[0];
    let t = x + G + 0.5;
    for (i, c) in C.iter().enumerate().skip(1) {
        a += c / (x + i as f64);
    }
    0.5 * (2.0 * std::f64::consts::PI).ln() + (x + 0.5) * t.ln() - t + a.ln()
}

/// Regularized lower incomplete gamma P(a, x).
pub fn gamma_p(a: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x < a + 1.0 {
        gamma_series(a, x)
    } else {
        1.0 - gamma_cf(a, x)
    }
}

/// Regularized upper incomplete gamma Q(a, x) = 1 − P(a, x).
pub fn gamma_q(a: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 1.0;
    }
    if x < a + 1.0 {
        1.0 - gamma_series(a, x)
    } else {
        gamma_cf(a, x)
    }
}

fn gamma_series(a: f64, x: f64) -> f64 {
    let mut ap = a;
    let mut sum = 1.0 / a;
    let mut del = sum;
    for _ in 0..1000 {
        ap += 1.0;
        del *= x / ap;
        sum += del;
        if del.abs() < sum.abs() * 1e-16 {
            break;
        }
    }
    sum * (-x + a * x.ln() - ln_gamma(a)).exp()
}

fn gamma_cf(a: f64, x: f64) -> f64 {
    const FPMIN: f64 = 1e-300;
    let mut b = x + 1.0 - a;
    let mut c = 1.0 / FPMIN;
    let mut d = 1.0 / b;
    let mut h = d;
    for i in 1..1000 {
        let an = -(i as f64) * (i as f64 - a);
        b += 2.0;
        d = an * d + b;
        if d.abs() < FPMIN {
            d = FPMIN;
        }
        c = b + an / c;
        if c.abs() < FPMIN {
            c = FPMIN;
        }
        d = 1.0 / d;
        let del = d * c;
        h *= del;
        if (del - 1.0).abs() < 1e-16 {
            break;
        }
    }
    (-x + a * x.ln() - ln_gamma(a)).exp() * h
}

/// Survival function of χ²(k): P(X ≥ x).
pub fn chi2_sf(x: f64, k: f64) -> f64 {
    gamma_q(k / 2.0, x / 2.0)
}

/// Regularized incomplete beta I_x(a, b).
pub fn beta_inc(a: f64, b: f64, x: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    let ln_bt = ln_gamma(a + b) - ln_gamma(a) - ln_gamma(b) + a * x.ln() + b * (1.0 - x).ln();
    let bt = ln_bt.exp();
    if x < (a + 1.0) / (a + b + 2.0) {
        bt * beta_cf(a, b, x) / a
    } else {
        1.0 - bt * beta_cf(b, a, 1.0 - x) / b
    }
}

fn beta_cf(a: f64, b: f64, x: f64) -> f64 {
    const FPMIN: f64 = 1e-300;
    let qab = a + b;
    let qap = a + 1.0;
    let qam = a - 1.0;
    let mut c = 1.0;
    let mut d = 1.0 - qab * x / qap;
    if d.abs() < FPMIN {
        d = FPMIN;
    }
    d = 1.0 / d;
    let mut h = d;
    for m in 1..10_000 {
        let m = m as f64;
        let m2 = 2.0 * m;
        let aa = m * (b - m) * x / ((qam + m2) * (a + m2));
        d = 1.0 + aa * d;
        if d.abs() < FPMIN {
            d = FPMIN;
        }
        c = 1.0 + aa / c;
        if c.abs() < FPMIN {
            c = FPMIN;
        }
        d = 1.0 / d;
        h *= d * c;
        let aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2));
        d = 1.0 + aa * d;
        if d.abs() < FPMIN {
            d = FPMIN;
        }
        c = 1.0 + aa / c;
        if c.abs() < FPMIN {
            c = FPMIN;
        }
        d = 1.0 / d;
        let del = d * c;
        h *= del;
        if (del - 1.0).abs() < 1e-15 {
            break;
        }
    }
    h
}

/// Standard normal CDF Φ(x).
pub fn normal_cdf(x: f64) -> f64 {
    let p = gamma_p(0.5, x * x / 2.0);
    if x >= 0.0 {
        0.5 + 0.5 * p
    } else {
        0.5 - 0.5 * p
    }
}

/// Standard normal quantile Φ⁻¹(p) (Wichura AS241, ~1e-16 relative).
pub fn normal_quantile(p: f64) -> f64 {
    if p <= 0.0 {
        return f64::NEG_INFINITY;
    }
    if p >= 1.0 {
        return f64::INFINITY;
    }
    let q = p - 0.5;
    if q.abs() <= 0.425 {
        let r = 0.180_625 - q * q;
        return q
            * (((((((r * 2_509.080_928_730_122_7 + 33_430.575_583_588_13) * r
                + 67_265.770_927_008_7)
                * r
                + 45_921.953_931_549_87)
                * r
                + 13_731.693_765_509_46)
                * r
                + 1_971.590_950_306_551_3)
                * r
                + 133.141_667_891_784_38)
                * r
                + 3.387_132_872_796_366_5)
            / (((((((r * 5_226.495_278_852_545 + 28_729.085_735_721_943) * r
                + 39_307.895_800_092_71)
                * r
                + 21_213.794_301_586_597)
                * r
                + 5_394.196_021_424_751)
                * r
                + 687.187_007_492_057_9)
                * r
                + 42.313_330_701_600_91)
                * r
                + 1.0);
    }
    let mut r = if q < 0.0 { p } else { 1.0 - p };
    r = (-r.ln()).sqrt();
    let val = if r <= 5.0 {
        let r = r - 1.6;
        (((((((r * 7.745_450_142_783_414e-4 + 0.022_723_844_989_269_184) * r
            + 0.241_780_725_177_450_6)
            * r
            + 1.270_458_252_452_368_4)
            * r
            + 3.647_848_324_763_204_5)
            * r
            + 5.769_497_221_460_691)
            * r
            + 4.630_337_846_156_546)
            * r
            + 1.423_437_110_749_683_5)
            / (((((((r * 1.050_750_071_644_416_9e-9 + 5.475_938_084_995_345e-4) * r
                + 0.015_198_666_563_616_457)
                * r
                + 0.148_103_976_427_480_08)
                * r
                + 0.689_767_334_985_1)
                * r
                + 1.676_384_830_183_803_8)
                * r
                + 2.053_191_626_637_759)
                * r
                + 1.0)
    } else {
        let r = r - 5.0;
        (((((((r * 2.010_334_399_292_288_1e-7 + 2.711_555_568_743_487_6e-5) * r
            + 0.001_242_660_947_388_078_4)
            * r
            + 0.026_532_189_526_576_124)
            * r
            + 0.296_560_571_828_504_9)
            * r
            + 1.784_826_539_917_291_3)
            * r
            + 5.463_784_911_164_114)
            * r
            + 6.657_904_643_501_103)
            / (((((((r * 2.044_263_103_389_939_7e-15 + 1.421_511_758_316_446e-7) * r
                + 1.846_318_317_510_054_8e-5)
                * r
                + 7.868_691_311_456_133e-4)
                * r
                + 0.014_875_361_290_850_615)
                * r
                + 0.136_929_880_922_735_8)
                * r
                + 0.599_832_206_555_888)
                * r
                + 1.0)
    };
    if q < 0.0 { -val } else { val }
}

/// Student t CDF with `df` degrees of freedom.
pub fn t_cdf(t: f64, df: f64) -> f64 {
    let x = df / (df + t * t);
    let tail = 0.5 * beta_inc(df / 2.0, 0.5, x);
    if t > 0.0 { 1.0 - tail } else { tail }
}

/// Student t quantile (bisection on [`t_cdf`]; ~1e-12 absolute).
pub fn t_quantile(p: f64, df: f64) -> f64 {
    bisect(|t| t_cdf(t, df) - p, -1e4, 1e4)
}

/// F(d1, d2) CDF.
pub fn f_cdf(x: f64, d1: f64, d2: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    beta_inc(d1 / 2.0, d2 / 2.0, d1 * x / (d1 * x + d2))
}

/// F(d1, d2) quantile.
pub fn f_quantile(p: f64, d1: f64, d2: f64) -> f64 {
    let mut hi = 2.0;
    while f_cdf(hi, d1, d2) < p && hi < 1e12 {
        hi *= 2.0;
    }
    bisect(|x| f_cdf(x, d1, d2) - p, 0.0, hi)
}

fn bisect(f: impl Fn(f64) -> f64, mut lo: f64, mut hi: f64) -> f64 {
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if f(mid) < 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
        if hi - lo <= 1e-13 * (1.0 + mid.abs()) {
            break;
        }
    }
    0.5 * (lo + hi)
}

/// Wilson score interval for x successes out of n (§11.3), as (low, high). `n == 0` → (0, 1).
pub fn wilson_ci(x: u64, n: u64, z: f64) -> (f64, f64) {
    if n == 0 {
        return (0.0, 1.0);
    }
    let nf = n as f64;
    let p = x as f64 / nf;
    let z2 = z * z;
    let den = 1.0 + z2 / nf;
    let centre = (p + z2 / (2.0 * nf)) / den;
    let half = z * (p * (1.0 - p) / nf + z2 / (4.0 * nf * nf)).sqrt() / den;
    ((centre - half).max(0.0), (centre + half).min(1.0))
}

/// Mean ± t_{n−1, 0.975}·s/√n with the sample SD (divisor n − 1), as (low, high) (§11.3).
/// `None` when n < 2.
pub fn t_interval(mean: f64, sample_sd: f64, n: u64) -> Option<(f64, f64)> {
    if n < 2 {
        return None;
    }
    let t = t_quantile(0.975, (n - 1) as f64);
    let h = t * sample_sd / (n as f64).sqrt();
    Some((mean - h, mean + h))
}

/// Fisher-z 95 % CI for a correlation r with n observations (§12.1): tanh(atanh r ± 1.96/√(n−3)).
pub fn fisher_z_ci(r: f64, n: u64) -> Option<(f64, f64)> {
    if n <= 3 || !r.is_finite() {
        return None;
    }
    let r = r.clamp(-0.999_999_999, 0.999_999_999);
    let z = r.atanh();
    let h = 1.96 / ((n - 3) as f64).sqrt();
    Some(((z - h).tanh(), (z + h).tanh()))
}

/// Seeded SplitMix64: a counter-based, platform-independent PRNG for bootstrap resampling.
#[derive(Clone, Debug)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// A generator from a 64-bit seed.
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// A generator seeded from the first 64 bits of SHA-384(`run_id` ‖ `statistic_id`)
    /// (§12, "Recompute"; §11.3 bootstrap seeded from the run ID).
    pub fn from_run(run_id: &str, statistic_id: &str) -> Self {
        let mut h = Sha384::new();
        h.update(run_id.as_bytes());
        h.update([0u8]);
        h.update(statistic_id.as_bytes());
        let d = h.finalize();
        let mut b = [0u8; 8];
        b.copy_from_slice(&d[..8]);
        Self::new(u64::from_le_bytes(b))
    }

    /// Next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform integer in `0..n` (rejection sampling, unbiased). `n` must be > 0.
    pub fn below(&mut self, n: u64) -> u64 {
        let zone = u64::MAX - (u64::MAX % n);
        loop {
            let v = self.next_u64();
            if v < zone {
                return v % n;
            }
        }
    }
}

/// Percentile of sorted data by the nearest-rank-with-interpolation rule (type 7, as numpy).
pub fn percentile_sorted(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let h = (sorted.len() - 1) as f64 * q.clamp(0.0, 1.0);
    let lo = h.floor() as usize;
    let hi = h.ceil() as usize;
    sorted[lo] + (h - lo as f64) * (sorted[hi] - sorted[lo])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn reference_values() {
        // Reference values from scipy 1.18 (scipy.stats).
        assert!(close(normal_quantile(0.975), 1.959_963_984_540_054, 1e-12));
        assert!(close(normal_cdf(1.0), 0.841_344_746_068_542_9, 1e-12));
        assert!(close(t_quantile(0.975, 24.0), 2.063_898_561_628_024_5, 1e-9));
        assert!(close(t_quantile(0.975, 9.0), 2.262_157_162_798_205, 1e-9));
        assert!(close(chi2_sf(3.84, 1.0), 0.050_043_521_248_705_19, 1e-10));
        assert!(close(chi2_sf(10.0, 4.0), 0.040_427_681_994_512_8, 1e-10));
        assert!(close(f_quantile(0.975, 9.0, 18.0), 2.929_112_493_123_264, 1e-8));
        assert!(close(f_quantile(0.025, 9.0, 18.0), 0.270_162_139_362_321, 1e-8));
        assert!(close(ln_gamma(10.0), 12.801_827_480_081_469, 1e-11));
    }

    #[test]
    fn wilson_worked_example() {
        // §11.3 worked example: 18/25 -> [52.4 %, 85.7 %]; pooled 24/35 -> [52.0 %, 81.4 %].
        let (l, h) = wilson_ci(18, 25, Z95);
        assert_eq!((l * 1000.0).round() as i64, 524);
        assert_eq!((h * 1000.0).round() as i64, 857);
        let (l, h) = wilson_ci(24, 35, Z95);
        assert_eq!((l * 1000.0).round() as i64, 520);
        assert_eq!((h * 1000.0).round() as i64, 814);
    }

    #[test]
    fn t_interval_worked_example() {
        // §11.3: mu = 0.684, s = 0.155, N = 25 -> [62.0 %, 74.8 %].
        let (l, h) = t_interval(0.684, 0.155, 25).unwrap();
        assert_eq!((l * 1000.0).round() as i64, 620);
        assert_eq!((h * 1000.0).round() as i64, 748);
    }

    #[test]
    fn prng_is_reproducible_and_in_range() {
        let mut a = SplitMix64::from_run("run", "stat");
        let mut b = SplitMix64::from_run("run", "stat");
        for _ in 0..100 {
            let x = a.below(7);
            assert_eq!(x, b.below(7));
            assert!(x < 7);
        }
        let mut c = SplitMix64::from_run("run", "other");
        assert_ne!(SplitMix64::from_run("run", "stat").next_u64(), c.next_u64());
    }

    #[test]
    fn percentiles() {
        let v = [1.0, 2.0, 3.0, 4.0];
        assert!(close(percentile_sorted(&v, 0.5), 2.5, 1e-12));
        assert!(close(percentile_sorted(&v, 0.0), 1.0, 1e-12));
        assert!(close(percentile_sorted(&v, 1.0), 4.0, 1e-12));
    }
}
