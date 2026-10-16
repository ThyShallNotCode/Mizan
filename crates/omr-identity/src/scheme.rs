//! Per-university student-ID schemes (§9.2): an anchored regex, an optional check digit and a
//! decoder of named capture groups into typed fields. A scheme validates roster imports and
//! ID reads, defines `P_fmt` and the admission-year prior of the ∅ hypothesis (§9.5) and
//! separates student IDs from other identifiers (§9.9 row 8).

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

/// Check-digit rules over ASCII digit strings (§6.5 "Luhn, mod-11, Damm or institutional").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckDigit {
    /// Luhn (mod 10) with the check digit last.
    Luhn,
    /// Damm (totally anti-symmetric quasigroup) with the check digit last.
    Damm,
    /// Weighted mod 11 over the payload (weights left to right, check digit last):
    /// `check = (11 − Σ wᵢ·dᵢ mod 11) mod 11`; a check value of 10 makes the ID invalid.
    Mod11 {
        /// Weights for the payload digits, left to right (cycled if shorter).
        weights: Vec<u32>,
    },
}

impl CheckDigit {
    /// True when the last digit is the correct check digit for the rest.
    pub fn verify(&self, digits: &str) -> bool {
        let d: Vec<u32> = match digits
            .chars()
            .map(|c| c.to_digit(10))
            .collect::<Option<_>>()
        {
            Some(d) => d,
            None => return false,
        };
        if d.len() < 2 {
            return false;
        }
        match self {
            CheckDigit::Luhn => luhn_valid(&d),
            CheckDigit::Damm => {
                let mut interim = 0usize;
                for &x in &d {
                    interim = DAMM[interim][x as usize] as usize;
                }
                interim == 0
            }
            CheckDigit::Mod11 { weights } => {
                if weights.is_empty() {
                    return false;
                }
                let (payload, check) = d.split_at(d.len() - 1);
                let sum: u32 = payload
                    .iter()
                    .zip(weights.iter().cycle())
                    .map(|(a, w)| a * w)
                    .sum();
                let c = (11 - sum % 11) % 11;
                c < 10 && c == check[0]
            }
        }
    }
}

/// Luhn over digit values, check digit last.
pub(crate) fn luhn_valid(d: &[u32]) -> bool {
    let sum: u32 = d
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &x)| {
            if i % 2 == 1 {
                let y = 2 * x;
                y / 10 + y % 10
            } else {
                x
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

const DAMM: [[u8; 10]; 10] = [
    [0, 3, 1, 7, 5, 9, 8, 6, 4, 2],
    [7, 0, 9, 2, 1, 5, 4, 8, 6, 3],
    [4, 2, 0, 6, 8, 7, 1, 3, 5, 9],
    [1, 7, 5, 0, 9, 8, 3, 4, 2, 6],
    [6, 1, 2, 3, 0, 4, 5, 9, 7, 8],
    [3, 6, 7, 4, 2, 0, 9, 5, 8, 1],
    [5, 8, 6, 9, 7, 2, 0, 1, 3, 4],
    [8, 9, 4, 5, 3, 6, 2, 0, 1, 7],
    [9, 4, 3, 8, 6, 1, 7, 2, 0, 5],
    [2, 5, 8, 1, 4, 3, 6, 7, 9, 0],
];

/// How a named capture group is decoded and range-checked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldKind {
    /// Two-digit Gregorian year (`yy` → `2000 + yy`) within `[min, max]`.
    GregorianYear2 {
        /// Earliest valid year.
        min: u16,
        /// Latest valid year.
        max: u16,
    },
    /// Three-digit Hijri year (`433` → `1433`) within `[min, max]`.
    HijriYear3 {
        /// Earliest valid year.
        min: u16,
        /// Latest valid year.
        max: u16,
    },
    /// An integer within `[min, max]`.
    Range {
        /// Minimum.
        min: u64,
        /// Maximum.
        max: u64,
    },
    /// Any digits (kept as text).
    Any,
}

/// One decoded field rule: capture-group name and kind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldRule {
    /// Named capture group in the pattern.
    pub group: String,
    /// Decoder.
    pub kind: FieldKind,
}

/// A decoded field value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldValue {
    /// A calendar year (Gregorian or Hijri, per the rule).
    Year(u16),
    /// A number.
    Number(u64),
    /// Raw text.
    Text(String),
}

/// The typed fields of a valid ID.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DecodedId {
    /// Field name → value.
    pub fields: BTreeMap<String, FieldValue>,
}

impl DecodedId {
    /// The admission year, if the scheme decodes one.
    pub fn year(&self) -> Option<u16> {
        self.fields.values().find_map(|v| match v {
            FieldValue::Year(y) => Some(*y),
            _ => None,
        })
    }
}

/// Why an ID does not satisfy a scheme.
#[derive(Clone, Debug, PartialEq, Eq, Error, Serialize, Deserialize)]
pub enum SchemeViolation {
    /// The pattern does not match.
    #[error("does not match the {0} pattern")]
    Pattern(String),
    /// The check digit is wrong.
    #[error("check digit fails")]
    CheckDigit,
    /// A decoded field is out of range.
    #[error("field `{0}` out of range")]
    FieldRange(String),
}

/// Errors building a scheme.
#[derive(Debug, Error)]
pub enum SchemeError {
    /// Bad regex.
    #[error("invalid pattern: {0}")]
    Pattern(#[from] regex::Error),
    /// A field rule names a group the pattern lacks.
    #[error("field rule `{0}` has no capture group in the pattern")]
    MissingGroup(String),
    /// The pattern must be anchored (`^…$`).
    #[error("pattern must be anchored with ^ and $")]
    NotAnchored,
}

/// The serialisable definition of a scheme (versioned in the workspace library, §14.9).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SchemeSpec {
    /// Scheme name (e.g. `IAU`).
    pub name: String,
    /// Version (snapshots pin it).
    pub version: u32,
    /// Anchored regex with named groups for the fields.
    pub pattern: String,
    /// Optional check digit.
    pub check_digit: Option<CheckDigit>,
    /// Field decoders.
    pub fields: Vec<FieldRule>,
    /// Number of distinct valid IDs (`P_fmt` is uniform over them, §9.5).
    pub id_space: f64,
    /// True when this definition is inferred from data or public sources and still needs the
    /// registrar's confirmation.
    pub provisional: bool,
}

/// A compiled student-ID scheme.
#[derive(Clone, Debug)]
pub struct StudentIdScheme {
    spec: SchemeSpec,
    re: Regex,
}

impl PartialEq for StudentIdScheme {
    fn eq(&self, other: &Self) -> bool {
        self.spec == other.spec
    }
}

impl StudentIdScheme {
    /// Compiles a scheme.
    pub fn new(spec: SchemeSpec) -> Result<Self, SchemeError> {
        if !spec.pattern.starts_with('^') || !spec.pattern.ends_with('$') {
            return Err(SchemeError::NotAnchored);
        }
        let re = Regex::new(&spec.pattern)?;
        let names: Vec<&str> = re.capture_names().flatten().collect();
        for f in &spec.fields {
            if !names.contains(&f.group.as_str()) {
                return Err(SchemeError::MissingGroup(f.group.clone()));
            }
        }
        Ok(Self { spec, re })
    }

    /// The definition.
    pub fn spec(&self) -> &SchemeSpec {
        &self.spec
    }

    /// Scheme name.
    pub fn name(&self) -> &str {
        &self.spec.name
    }

    /// True when the scheme defines a check digit (needed for `id_only_auto_accept`, §9.7).
    pub fn has_check_digit(&self) -> bool {
        self.spec.check_digit.is_some()
    }

    /// Validates a normalised ID and decodes its fields.
    pub fn validate(&self, id: &str) -> Result<DecodedId, SchemeViolation> {
        let caps = self
            .re
            .captures(id)
            .ok_or_else(|| SchemeViolation::Pattern(self.spec.name.clone()))?;
        if let Some(cd) = &self.spec.check_digit
            && !cd.verify(id)
        {
            return Err(SchemeViolation::CheckDigit);
        }
        let mut out = DecodedId::default();
        for f in &self.spec.fields {
            let Some(m) = caps.name(&f.group) else {
                return Err(SchemeViolation::FieldRange(f.group.clone()));
            };
            let text = m.as_str();
            let num = text.parse::<u64>().ok();
            let v = match (&f.kind, num) {
                (FieldKind::GregorianYear2 { min, max }, Some(n)) if text.len() == 2 => {
                    let y = 2000 + n as u16;
                    (y >= *min && y <= *max).then_some(FieldValue::Year(y))
                }
                (FieldKind::HijriYear3 { min, max }, Some(n)) if text.len() == 3 => {
                    let y = 1000 + n as u16;
                    (y >= *min && y <= *max).then_some(FieldValue::Year(y))
                }
                (FieldKind::Range { min, max }, Some(n)) => {
                    (n >= *min && n <= *max).then_some(FieldValue::Number(n))
                }
                (FieldKind::Any, _) => Some(FieldValue::Text(text.to_owned())),
                _ => None,
            };
            match v {
                Some(v) => {
                    out.fields.insert(f.group.clone(), v);
                }
                None => return Err(SchemeViolation::FieldRange(f.group.clone())),
            }
        }
        Ok(out)
    }

    /// True when the scheme can produce `id` (pattern, check digit and field ranges).
    pub fn accepts(&self, id: &str) -> bool {
        self.validate(id).is_ok()
    }

    /// `log P_fmt(id)` for the ∅ hypothesis (§9.5): uniform over valid IDs, re-weighted by the
    /// admission-year histogram `year_counts` (add-one smoothed over the scheme's year window)
    /// when the scheme decodes a year. Invalid IDs get 10⁻³ of a uniform share.
    pub fn log_p_fmt(&self, id: &str, year_counts: &BTreeMap<u16, u32>) -> f64 {
        let base = -self.spec.id_space.max(1.0).ln();
        match self.validate(id) {
            Ok(dec) => match (dec.year(), self.year_window()) {
                (Some(y), Some((lo, hi))) => {
                    let n_years = f64::from(hi - lo + 1);
                    let total: u32 = year_counts.values().sum();
                    let c = year_counts.get(&y).copied().unwrap_or(0);
                    let p_year = (f64::from(c) + 1.0) / (f64::from(total) + n_years);
                    base + (p_year * n_years).ln()
                }
                _ => base,
            },
            Err(_) => base + (1e-3f64).ln(),
        }
    }

    /// The year field's window, if any.
    pub fn year_window(&self) -> Option<(u16, u16)> {
        self.spec.fields.iter().find_map(|f| match f.kind {
            FieldKind::GregorianYear2 { min, max } | FieldKind::HijriYear3 { min, max } => {
                Some((min, max))
            }
            _ => None,
        })
    }

    /// IAU (Imam Abdulrahman Bin Faisal University) student numbers, **provisional**: inferred
    /// from 141 real CCSIT student numbers (10 digits `2 YY NNNNNNN`, YY = Gregorian admission
    /// year 19–22 in the sample). No check digit is known. Admission years 2010–2030.
    pub fn iau() -> Self {
        Self::new(SchemeSpec {
            name: "IAU".into(),
            version: 1,
            pattern: r"^2(?P<year>[0-9]{2})(?P<serial>[0-9]{7})$".into(),
            check_digit: None,
            fields: vec![
                FieldRule {
                    group: "year".into(),
                    kind: FieldKind::GregorianYear2 {
                        min: 2010,
                        max: 2030,
                    },
                },
                FieldRule {
                    group: "serial".into(),
                    kind: FieldKind::Any,
                },
            ],
            id_space: 21.0 * 1e7,
            provisional: true,
        })
        .expect("built-in IAU scheme")
    }

    /// PNU example from §9.2 (provisional): 9 digits = 3-digit Hijri admission year + semester
    /// digit + 5-digit serial.
    pub fn pnu() -> Self {
        Self::new(SchemeSpec {
            name: "PNU".into(),
            version: 1,
            pattern: r"^(?P<year>4[0-9]{2})(?P<semester>[1-3])(?P<serial>[0-9]{5})$".into(),
            check_digit: None,
            fields: vec![
                FieldRule {
                    group: "year".into(),
                    kind: FieldKind::HijriYear3 {
                        min: 1430,
                        max: 1460,
                    },
                },
                FieldRule {
                    group: "semester".into(),
                    kind: FieldKind::Range { min: 1, max: 3 },
                },
                FieldRule {
                    group: "serial".into(),
                    kind: FieldKind::Any,
                },
            ],
            id_space: 31.0 * 3.0 * 1e5,
            provisional: true,
        })
        .expect("built-in PNU scheme")
    }

    /// KSU example from §9.2 (provisional): 9 digits, e.g. 433000000 (Hijri year + serial).
    pub fn ksu() -> Self {
        Self::new(SchemeSpec {
            name: "KSU".into(),
            version: 1,
            pattern: r"^(?P<year>4[0-9]{2})(?P<serial>[0-9]{6})$".into(),
            check_digit: None,
            fields: vec![
                FieldRule {
                    group: "year".into(),
                    kind: FieldKind::HijriYear3 {
                        min: 1425,
                        max: 1460,
                    },
                },
                FieldRule {
                    group: "serial".into(),
                    kind: FieldKind::Any,
                },
            ],
            id_space: 36.0 * 1e6,
            provisional: true,
        })
        .expect("built-in KSU scheme")
    }

    /// Any string of exactly `n` digits (rosterless candidate numbers without a format).
    pub fn digits(n: usize, check_digit: Option<CheckDigit>) -> Self {
        Self::new(SchemeSpec {
            name: format!("DIGITS{n}"),
            version: 1,
            pattern: format!("^[0-9]{{{n}}}$"),
            check_digit,
            fields: Vec::new(),
            id_space: 10f64.powi(n as i32),
            provisional: false,
        })
        .expect("digit scheme")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_digits() {
        assert!(CheckDigit::Luhn.verify("79927398713"));
        assert!(!CheckDigit::Luhn.verify("79927398710"));
        assert!(CheckDigit::Damm.verify("5724"));
        assert!(!CheckDigit::Damm.verify("5727"));
        let m11 = CheckDigit::Mod11 {
            weights: vec![10, 9, 8, 7, 6, 5, 4, 3, 2],
        };
        // ISBN-10-style: 0-306-40615-2 → payload 030640615, check 2.
        assert!(m11.verify("0306406152"));
        assert!(!m11.verify("0306406153"));
        assert!(!CheckDigit::Luhn.verify("12a"));
    }

    #[test]
    fn iau_scheme_decodes_admission_year() {
        let s = StudentIdScheme::iau();
        let d = s.validate("2220001234").unwrap();
        assert_eq!(d.year(), Some(2022));
        assert!(s.accepts("2190000001"));
        assert_eq!(
            s.validate("1220001234"),
            Err(SchemeViolation::Pattern("IAU".into()))
        );
        assert_eq!(
            s.validate("2990001234"),
            Err(SchemeViolation::FieldRange("year".into()))
        );
        assert!(!s.accepts("222000123"), "9 digits");
        assert!(!s.has_check_digit());
    }

    #[test]
    fn hijri_schemes() {
        assert_eq!(
            StudentIdScheme::ksu().validate("433000000").unwrap().year(),
            Some(1433)
        );
        let p = StudentIdScheme::pnu();
        assert!(p.accepts("443112345"));
        assert!(!p.accepts("443412345"), "semester 4");
    }

    #[test]
    fn p_fmt_prefers_enrolled_cohorts() {
        let s = StudentIdScheme::iau();
        let mut counts = BTreeMap::new();
        counts.insert(2022u16, 80u32);
        counts.insert(2021, 40);
        let common = s.log_p_fmt("2220001234", &counts);
        let rare = s.log_p_fmt("2150001234", &counts);
        let invalid = s.log_p_fmt("9999999999", &counts);
        assert!(common > rare && rare > invalid);
    }

    #[test]
    fn bad_specs_are_rejected() {
        let mut spec = StudentIdScheme::iau().spec().clone();
        spec.pattern = "2[0-9]{9}".into();
        assert!(matches!(
            StudentIdScheme::new(spec.clone()),
            Err(SchemeError::NotAnchored)
        ));
        spec.pattern = "^2[0-9]{9}$".into();
        assert!(matches!(
            StudentIdScheme::new(spec),
            Err(SchemeError::MissingGroup(_))
        ));
    }
}
