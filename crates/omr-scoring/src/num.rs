//! Exact numbers and the rounding policy (spec §10.2).
//!
//! Points, credits and totals are exact rationals: [`Points`] (`Ratio<i64>`) for inputs such as
//! M_i and credit proportions, and [`Exact`] (`Ratio<i128>`) for aggregates such as totals and
//! percentages, whose denominators can grow. Nothing on a scoring path goes through `f64`, so
//! 0.3 + 0.3 + 0.3 ≥ 0.9 holds exactly. Rounding happens once, at a declared stage, with a
//! declared strategy ([`RoundingPolicy`]).

use num_rational::Ratio;
use num_traits::{Signed, ToPrimitive, Zero};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// An exact point or credit value (M_i, c_o, penalties, score-box values).
pub type Points = Ratio<i64>;

/// An exact aggregate (totals, x%, P_{n,c}); wider so sums of many items never overflow.
pub type Exact = Ratio<i128>;

/// `n/d` as [`Points`]. Panics if `d == 0` (a programming error, never input-dependent).
pub fn pts(n: i64, d: i64) -> Points {
    Ratio::new(n, d)
}

/// `n/d` as [`Exact`]. Panics if `d == 0`.
pub fn exact(n: i128, d: i128) -> Exact {
    Ratio::new(n, d)
}

/// Widens [`Points`] to [`Exact`].
pub fn widen(p: &Points) -> Exact {
    Ratio::new_raw(i128::from(*p.numer()), i128::from(*p.denom()))
}

/// Narrows an [`Exact`] to [`Points`] if numerator and denominator fit in `i64`.
pub fn narrow(x: &Exact) -> Option<Points> {
    let n = i64::try_from(*x.numer()).ok()?;
    let d = i64::try_from(*x.denom()).ok()?;
    Some(Ratio::new(n, d))
}

/// Display/analytics conversion; never used for a decision.
pub fn to_f64(x: &Exact) -> f64 {
    x.to_f64().unwrap_or(f64::NAN)
}

/// Display/analytics conversion of [`Points`].
pub fn points_f64(x: &Points) -> f64 {
    x.to_f64().unwrap_or(f64::NAN)
}

/// Errors parsing an exact number.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseNumberError {
    /// Not a decimal (`1`, `0.5`, `.5`, `-2.25`) or fraction (`1/3`).
    #[error("not an exact number: {0:?}")]
    Invalid(String),
    /// Too many digits for an `i64` rational.
    #[error("number out of range: {0:?}")]
    Overflow(String),
}

/// Parses `"1"`, `"0.5"`, `".5"`, `"-2.25"` or `"1/3"` exactly; `½ = 0.5 = .5` (§10.4).
pub fn parse_points(s: &str) -> Result<Points, ParseNumberError> {
    let t = s.trim();
    let bad = || ParseNumberError::Invalid(s.to_owned());
    let over = || ParseNumberError::Overflow(s.to_owned());
    if let Some((a, b)) = t.split_once('/') {
        let n: i64 = a.trim().parse().map_err(|_| bad())?;
        let d: i64 = b.trim().parse().map_err(|_| bad())?;
        if d == 0 {
            return Err(bad());
        }
        // Normalize the sign ourselves: `Ratio::new` would negate i64::MIN and overflow.
        let (n, d) = if d < 0 {
            (
                n.checked_neg().ok_or_else(over)?,
                d.checked_neg().ok_or_else(over)?,
            )
        } else {
            (n, d)
        };
        return Ok(Ratio::new(n, d));
    }
    let (neg, body) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let (ip, fp) = body.split_once('.').unwrap_or((body, ""));
    if (ip.is_empty() && fp.is_empty())
        || !ip.bytes().all(|c| c.is_ascii_digit())
        || !fp.bytes().all(|c| c.is_ascii_digit())
    {
        return Err(bad());
    }
    let digits = format!("{ip}{fp}");
    let n: i64 = if digits.is_empty() {
        0
    } else {
        digits.parse().map_err(|_| over())?
    };
    let d = 10i64.checked_pow(fp.len() as u32).ok_or_else(over)?;
    let v = Ratio::new(n, d);
    Ok(if neg { -v } else { v })
}

/// Rounding strategies (§10.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum RoundingStrategy {
    /// Half away from zero (Rust `f64::round`); the default.
    #[default]
    MidpointAwayFromZero,
    /// Banker's rounding.
    MidpointNearestEven,
    /// Truncation.
    ToZero,
    /// Any remainder rounds away from zero.
    AwayFromZero,
}

/// The pipeline stage at which rounding happens (§10.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum RoundingStage {
    /// Per item score.
    Item,
    /// Per section.
    Section,
    /// Per assessment percentage.
    Assessment,
    /// Once, at the course total (default).
    #[default]
    Course,
}

/// Which value a letter grade maps from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum LetterBasis {
    /// The displayed, rounded value (default: displayed and mapped values never disagree).
    #[default]
    Rounded,
    /// The exact, unrounded value.
    Unrounded,
}

/// `RoundingPolicy { stage, decimals, strategy, letter_basis }` (§10.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RoundingPolicy {
    /// Where rounding happens.
    pub stage: RoundingStage,
    /// Decimal places kept.
    pub decimals: u32,
    /// Tie and remainder handling.
    pub strategy: RoundingStrategy,
    /// Which value maps to the letter.
    pub letter_basis: LetterBasis,
}

impl Default for RoundingPolicy {
    /// Course stage, 2 dp, half away from zero, letter from the rounded value.
    fn default() -> Self {
        Self {
            stage: RoundingStage::Course,
            decimals: 2,
            strategy: RoundingStrategy::MidpointAwayFromZero,
            letter_basis: LetterBasis::Rounded,
        }
    }
}

impl RoundingPolicy {
    /// Rounds `x` under this policy.
    pub fn round(&self, x: &Exact) -> Exact {
        round_exact(x, self.decimals, self.strategy)
    }
}

fn pow10(d: u32) -> i128 {
    10i128.pow(d.min(30))
}

/// Rounds an exact value to `decimals` places with `strategy`, exactly.
pub fn round_exact(x: &Exact, decimals: u32, strategy: RoundingStrategy) -> Exact {
    let scale = pow10(decimals);
    let (p, q) = match x.numer().checked_mul(scale) {
        Some(p) => (p, *x.denom()), // q > 0 (Ratio invariant)
        None => {
            let s = x * Ratio::from_integer(scale);
            (*s.numer(), *s.denom())
        }
    };
    let floor = p.div_euclid(q);
    let rem = p - floor * q; // 0 <= rem < q
    let r = if rem == 0 {
        floor
    } else {
        let twice = 2 * rem;
        match strategy {
            RoundingStrategy::ToZero => {
                if p >= 0 {
                    floor
                } else {
                    floor + 1
                }
            }
            RoundingStrategy::AwayFromZero => {
                if p >= 0 {
                    floor + 1
                } else {
                    floor
                }
            }
            RoundingStrategy::MidpointAwayFromZero => {
                if twice > q || (twice == q && p >= 0) {
                    floor + 1
                } else {
                    floor
                }
            }
            RoundingStrategy::MidpointNearestEven => {
                if twice > q || (twice == q && floor % 2 != 0) {
                    floor + 1
                } else {
                    floor
                }
            }
        }
    };
    reduce(r, scale)
}

/// `n/d` reduced, with a fast 64-bit binary gcd when both fit (the common case). `d ≠ 0`.
pub fn reduce(n: i128, d: i128) -> Exact {
    assert!(d != 0, "zero denominator");
    let g = gcd_u128(n.unsigned_abs(), d.unsigned_abs()) as i128;
    let (mut n, mut d) = (n / g, d / g);
    if d < 0 {
        n = -n;
        d = -d;
    }
    Ratio::new_raw(n, d)
}

fn gcd_u128(a: u128, b: u128) -> u128 {
    if a == 0 {
        return b.max(1);
    }
    if b == 0 {
        return a;
    }
    if a <= u128::from(u64::MAX) && b <= u128::from(u64::MAX) {
        let (mut a, mut b) = (a as u64, b as u64);
        let shift = (a | b).trailing_zeros();
        a >>= a.trailing_zeros();
        loop {
            b >>= b.trailing_zeros();
            if a > b {
                std::mem::swap(&mut a, &mut b);
            }
            b -= a;
            if b == 0 {
                return u128::from(a << shift);
            }
        }
    }
    let (mut a, mut b) = (a, b);
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

/// Exact comparison by cross-multiplication (denominators are positive), falling back to
/// `Ord` only if the products overflow. Much cheaper than `Ratio`'s general `cmp`.
pub fn cmp_exact(a: &Exact, b: &Exact) -> std::cmp::Ordering {
    if a.denom() == b.denom() {
        return a.numer().cmp(b.numer());
    }
    match (
        a.numer().checked_mul(*b.denom()),
        b.numer().checked_mul(*a.denom()),
    ) {
        (Some(x), Some(y)) => x.cmp(&y),
        _ => a.cmp(b),
    }
}

/// Formats `x` with exactly `decimals` places ("." separator, locale-neutral, §10.2).
pub fn decimal_string(x: &Exact, decimals: u32, strategy: RoundingStrategy) -> String {
    let r = round_exact(x, decimals, strategy);
    let scale = pow10(decimals);
    let n = (r * Ratio::from_integer(scale)).to_integer();
    let neg = n < 0;
    let a = n.unsigned_abs();
    let s = scale as u128;
    let ip = a / s;
    let fp = a % s;
    let sign = if neg { "-" } else { "" };
    if decimals == 0 {
        format!("{sign}{ip}")
    } else {
        format!("{sign}{ip}.{fp:0width$}", width = decimals as usize)
    }
}

/// `max(0, x)`.
pub fn clamp_min_zero(x: &Exact) -> Exact {
    if x.is_negative() { Exact::zero() } else { *x }
}

/// Clamps to [lo, hi].
pub fn clamp(x: &Exact, lo: &Exact, hi: &Exact) -> Exact {
    use std::cmp::Ordering::{Greater, Less};
    if cmp_exact(x, lo) == Less {
        *lo
    } else if cmp_exact(x, hi) == Greater {
        *hi
    } else {
        *x
    }
}

/// Greatest common divisor (non-negative).
pub fn gcd_i128(a: i128, b: i128) -> i128 {
    let (mut a, mut b) = (a.abs(), b.abs());
    while b != 0 {
        let t = a % b;
        a = b;
        b = t;
    }
    a
}

/// Least common multiple, `None` on overflow.
pub fn lcm_i128(a: i128, b: i128) -> Option<i128> {
    if a == 0 || b == 0 {
        return Some(0);
    }
    let g = gcd_i128(a, b);
    (a / g).checked_mul(b)?.checked_abs()
}

/// Exact √x floored to `decimals` places (x ≥ 0), via integer square root; used by the root
/// curve so the transform stays deterministic and exact at its declared precision.
pub fn sqrt_floor(x: &Exact, decimals: u32) -> Exact {
    if !x.is_positive() {
        return Exact::zero();
    }
    let scale = pow10(decimals);
    // floor(sqrt(p/q) * scale) = floor(sqrt(p * scale^2 / q)) = isqrt(floor(p*scale^2/q)).
    let v = (x * Ratio::from_integer(scale * scale)).to_integer();
    Ratio::new(isqrt(v.max(0) as u128) as i128, scale)
}

fn isqrt(n: u128) -> u128 {
    if n < 2 {
        return n;
    }
    let mut x = (n as f64).sqrt() as u128;
    // Correct the float estimate exactly.
    while x.checked_mul(x).is_none_or(|v| v > n) {
        x -= 1;
    }
    while (x + 1).checked_mul(x + 1).is_some_and(|v| v <= n) {
        x += 1;
    }
    x
}

/// Rational approximation of a float to `decimals` places (half away from zero), used only
/// to *freeze* fitted parameters (§10.6), never on a decision path.
pub fn from_f64_fixed(v: f64, decimals: u32) -> Exact {
    let scale = pow10(decimals);
    let s = v * scale as f64;
    let r = if s >= 0.0 {
        (s + 0.5).floor()
    } else {
        (s - 0.5).ceil()
    };
    Ratio::new(r as i128, scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_point_three_sums_to_point_nine() {
        // §10.2: 0.3 + 0.3 + 0.3 >= 0.9 must be true exactly (false in f64).
        let p = parse_points("0.3").unwrap();
        assert!(p + p + p >= parse_points("0.9").unwrap());
        assert_eq!(p + p + p, pts(9, 10));
    }

    #[test]
    fn parse_forms() {
        assert_eq!(parse_points("1").unwrap(), pts(1, 1));
        assert_eq!(parse_points(".5").unwrap(), pts(1, 2));
        assert_eq!(parse_points("0.5").unwrap(), pts(1, 2));
        assert_eq!(parse_points("1/2").unwrap(), pts(1, 2));
        assert_eq!(parse_points("-2.25").unwrap(), pts(-9, 4));
        assert_eq!(parse_points("1/3").unwrap(), pts(1, 3));
        assert!(parse_points("1/0").is_err());
        assert_eq!(parse_points("1/-2").unwrap(), pts(-1, 2));
        // i64::MIN over a negative denominator cannot be normalized in i64: an error, never
        // an overflow panic on an input path.
        assert!(matches!(
            parse_points("-9223372036854775808/-1"),
            Err(ParseNumberError::Overflow(_))
        ));
        assert!(parse_points("abc").is_err());
        assert!(parse_points(".").is_err());
    }

    #[test]
    fn rounding_strategies() {
        let r = |n, d, s| decimal_string(&exact(n, d), 0, s);
        use RoundingStrategy::*;
        // 2.5
        assert_eq!(r(5, 2, MidpointAwayFromZero), "3");
        assert_eq!(r(5, 2, MidpointNearestEven), "2");
        assert_eq!(r(5, 2, ToZero), "2");
        assert_eq!(r(5, 2, AwayFromZero), "3");
        // -2.5
        assert_eq!(r(-5, 2, MidpointAwayFromZero), "-3");
        assert_eq!(r(-5, 2, MidpointNearestEven), "-2");
        assert_eq!(r(-5, 2, ToZero), "-2");
        assert_eq!(r(-5, 2, AwayFromZero), "-3");
        // 3.5 nearest even -> 4; 2.4 -> 2
        assert_eq!(r(7, 2, MidpointNearestEven), "4");
        assert_eq!(r(12, 5, MidpointAwayFromZero), "2");
        // 89.995 at 2 dp -> 90.00
        assert_eq!(
            decimal_string(&exact(89995, 1000), 2, MidpointAwayFromZero),
            "90.00"
        );
        // 1/3 at 4 dp
        assert_eq!(
            decimal_string(&exact(1, 3), 4, MidpointAwayFromZero),
            "0.3333"
        );
        assert_eq!(
            decimal_string(&exact(-1, 8), 2, MidpointAwayFromZero),
            "-0.13"
        );
    }

    #[test]
    fn sqrt_is_exact_floor() {
        assert_eq!(sqrt_floor(&exact(81, 1), 2), exact(9, 1));
        assert_eq!(sqrt_floor(&exact(2, 1), 6), exact(1_414_213, 1_000_000));
        assert_eq!(sqrt_floor(&exact(-4, 1), 3), exact(0, 1));
    }

    #[test]
    fn reduce_and_compare() {
        assert_eq!(reduce(6, -4), exact(-3, 2));
        assert_eq!(reduce(0, 7), exact(0, 1));
        assert_eq!(
            reduce(i128::from(u64::MAX) * 4, 8),
            exact(i128::from(u64::MAX), 2)
        );
        use std::cmp::Ordering::*;
        assert_eq!(cmp_exact(&exact(1, 3), &exact(33, 100)), Greater);
        assert_eq!(cmp_exact(&exact(-1, 3), &exact(-33, 100)), Less);
        assert_eq!(cmp_exact(&exact(2, 4), &exact(1, 2)), Equal);
    }

    #[test]
    fn lcm_and_gcd() {
        assert_eq!(gcd_i128(12, 18), 6);
        assert_eq!(lcm_i128(4, 6), Some(12));
        assert_eq!(lcm_i128(i128::MAX, i128::MAX - 1), None);
    }
}
