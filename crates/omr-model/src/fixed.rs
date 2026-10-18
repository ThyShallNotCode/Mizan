//! Fixed-point quantities that make recognition decisions reproducible across CPUs (§8.11, §14.6).
//!
//! Darkness is quantised to 1/1024 before any decision, and logits are carried in milli-nats,
//! so the decision path compares integers and hysteresis bands absorb float noise (§23.6).

use serde::{Deserialize, Serialize};

/// Normalised darkness `D = (B − I)/(B − K)` in units of 1/1024, clamped to `0..=1024` (§8.5).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
pub struct Darkness(pub u16);

impl Darkness {
    /// Scale: 1.0 darkness = 1024 units.
    pub const ONE: u16 = 1024;

    /// Quantises a float darkness (clamped to [0, 1]); round-half-up for determinism.
    pub fn from_f64(d: f64) -> Self {
        let c = if d.is_nan() { 0.0 } else { d.clamp(0.0, 1.0) };
        Self((c * f64::from(Self::ONE) + 0.5).floor() as u16)
    }

    /// The darkness as a float in [0, 1].
    pub fn as_f64(self) -> f64 {
        f64::from(self.0) / f64::from(Self::ONE)
    }
}

/// A log-odds value in milli-nats (e.g. 6.9 nat = 6900), used for auto-accept thresholds (§8.8).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
pub struct LogitMilli(pub i32);

impl LogitMilli {
    /// Standard-mode question threshold L_q = 6.9 nat (posterior 0.999, §8.8).
    pub const STANDARD_LQ: Self = Self(6900);
    /// High-stakes question threshold L_q = 9.2 nat (posterior 0.9999, §8.8).
    pub const HIGH_STAKES_LQ: Self = Self(9200);

    /// Quantises a float logit (nats), saturating at ±1,000 nat; round-half-up.
    pub fn from_nats(x: f64) -> Self {
        let c = if x.is_nan() {
            0.0
        } else {
            x.clamp(-1.0e3, 1.0e3)
        };
        Self((c * 1000.0 + 0.5).floor() as i32)
    }

    /// Log-odds of a probability, quantised. `p` is clamped to [1e-12, 1 − 1e-12].
    pub fn from_prob(p: f64) -> Self {
        let p = p.clamp(1e-12, 1.0 - 1e-12);
        Self::from_nats((p / (1.0 - p)).ln())
    }

    /// The value in nats.
    pub fn nats(self) -> f64 {
        f64::from(self.0) / 1000.0
    }
}
