//! Saudi national ID and Iqama detection (§9.2 "National ID and Iqama", §9.9 row 8).
//!
//! A national ID/Iqama is a **secret, never a key**: 10 digits, first digit 1 (citizen) or 2
//! (resident), the 10th a Luhn-variant check digit (odd positions of the first nine doubled,
//! digit sums added, `check = (10 − sum mod 10) mod 10`). A read that is a valid national ID
//! the student-ID scheme cannot produce is IDN-004 `wrong_identifier` (exact); when the scheme
//! *can* produce it (e.g. IAU numbers also start with 2), it is only a fusion note, because
//! about one genuine student number in ten passes the check by chance.

use crate::normalize::digits_only;
use crate::scheme::StudentIdScheme;
use serde::{Deserialize, Serialize};

/// Kind of national identifier, from the first digit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NationalIdKind {
    /// First digit 1: Saudi national ID.
    Citizen,
    /// First digit 2: Iqama (resident permit).
    Resident,
}

/// The national-ID check of §9.2 on ASCII digits; `None` when `s` is not a valid one.
pub fn national_id_kind(s: &str) -> Option<NationalIdKind> {
    let b = s.as_bytes();
    if b.len() != 10 || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let kind = match b[0] {
        b'1' => NationalIdKind::Citizen,
        b'2' => NationalIdKind::Resident,
        _ => return None,
    };
    let sum: u32 = b[..9]
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            let d = u32::from(c - b'0');
            if i % 2 == 0 {
                let x = 2 * d;
                x / 10 + x % 10
            } else {
                d
            }
        })
        .sum();
    let check = (10 - sum % 10) % 10;
    (check == u32::from(b[9] - b'0')).then_some(kind)
}

/// True for a valid national ID or Iqama number.
pub fn is_national_id(s: &str) -> bool {
    national_id_kind(s).is_some()
}

/// Masks every digit (national-ID digits never appear unmasked in items, evidence or logs).
pub fn mask_digits(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_digit() { '•' } else { c })
        .collect()
}

/// What an identifier string most plausibly is, given the institution's scheme.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdentifierClass {
    /// A valid student ID under the scheme that is not national-ID shaped.
    StudentId,
    /// A valid national ID/Iqama the scheme cannot produce: exact `wrong_identifier`.
    NationalId(NationalIdKind),
    /// Valid under the scheme *and* a valid national ID/Iqama: a fusion note only.
    NationalIdShaped(NationalIdKind),
    /// A Saudi mobile number (`05…` 10 digits, `9665…` 12 digits, `5…` 9 digits) the scheme
    /// cannot produce: `wrong_identifier`.
    Phone,
    /// Neither: fails the scheme for another reason.
    Unknown,
}

impl IdentifierClass {
    /// True when the class is an exact `wrong_identifier` (§9.9 row 8).
    pub fn is_wrong_identifier(self) -> bool {
        matches!(
            self,
            IdentifierClass::NationalId(_) | IdentifierClass::Phone
        )
    }

    /// True when the digits must be masked wherever shown.
    pub fn must_mask(self) -> bool {
        matches!(
            self,
            IdentifierClass::NationalId(_) | IdentifierClass::NationalIdShaped(_)
        )
    }
}

/// Classifies a (normalised or raw) identifier string against the scheme.
pub fn classify_identifier(raw: &str, scheme: &StudentIdScheme) -> IdentifierClass {
    let Some(d) = digits_only(raw) else {
        return IdentifierClass::Unknown;
    };
    let in_scheme = scheme.accepts(&d);
    if let Some(k) = national_id_kind(&d) {
        return if in_scheme {
            IdentifierClass::NationalIdShaped(k)
        } else {
            IdentifierClass::NationalId(k)
        };
    }
    if in_scheme {
        return IdentifierClass::StudentId;
    }
    let phone = (d.len() == 10 && d.starts_with("05"))
        || (d.len() == 12 && d.starts_with("9665"))
        || (d.len() == 9 && d.starts_with('5'));
    if phone {
        IdentifierClass::Phone
    } else {
        IdentifierClass::Unknown
    }
}

/// Column-level national-ID detection for roster imports (§9.2: "An import column passing the
/// check is flagged, never taken as the ID column"). Genuine 10-digit student numbers starting
/// with 1 or 2 pass the check by chance about 10 % of the time, so a column is flagged when
/// at least half of its non-empty values are valid national IDs (expected ≈ 10 % by chance, so
/// this is far outside chance for any column of ≥ 10 values), or when any value is a national
/// ID the scheme cannot produce.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColumnVerdict {
    /// Non-empty values examined.
    pub values: usize,
    /// Values that are valid national IDs/Iqamas.
    pub national_ids: usize,
    /// Of those, values the student-ID scheme cannot produce (exact detections).
    pub exact: usize,
    /// The column must not be used as the student-ID key.
    pub flagged: bool,
}

/// Runs [`ColumnVerdict`] over a column's values.
pub fn national_id_column(values: &[&str], scheme: &StudentIdScheme) -> ColumnVerdict {
    let (mut n, mut nid, mut exact) = (0, 0, 0);
    for v in values {
        let Some(d) = digits_only(v) else {
            continue;
        };
        n += 1;
        if is_national_id(&d) {
            nid += 1;
            if !scheme.accepts(&d) {
                exact += 1;
            }
        }
    }
    let flagged = exact > 0 || (n > 0 && 2 * nid >= n && n >= 4);
    ColumnVerdict {
        values: n,
        national_ids: nid,
        exact,
        flagged,
    }
}

/// Makes a valid synthetic national ID / Iqama from a 9-digit payload starting with 1 or 2
/// (test and simulation helper; never a real person's number).
pub fn with_national_check_digit(payload9: &str) -> Option<String> {
    if payload9.len() != 9 || !payload9.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    (0..10)
        .map(|c| format!("{payload9}{c}"))
        .find(|s| is_national_id(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luhn_variant_matches_spec_formula() {
        // Every payload has exactly one valid check digit.
        for p in ["100000000", "123456789", "287654321", "199999999"] {
            let ids: Vec<String> = (0..10)
                .map(|c| format!("{p}{c}"))
                .filter(|s| is_national_id(s))
                .collect();
            assert_eq!(ids.len(), 1, "{p}");
        }
        let id = with_national_check_digit("123456789").unwrap();
        // Spec: odd positions (1st, 3rd, …, 9th) doubled: 1→2, 3→6, 5→1+0, 7→1+4, 9→1+8.
        // 2+2+6+4+1+6+5+8+9 = 43 → check (10 − 3) mod 10 = 7.
        assert_eq!(id, "1234567897");
        assert_eq!(national_id_kind(&id), Some(NationalIdKind::Citizen));
        assert!(national_id_kind("3234567897").is_none(), "first digit 3");
        assert!(national_id_kind("123456789").is_none(), "9 digits");
    }

    #[test]
    fn classification_against_schemes() {
        let iau = StudentIdScheme::iau();
        let ksu = StudentIdScheme::ksu();
        let citizen = with_national_check_digit("112345678").unwrap();
        assert_eq!(
            classify_identifier(&citizen, &iau),
            IdentifierClass::NationalId(NationalIdKind::Citizen)
        );
        // An Iqama whose digits 2–3 are not an admission year: exact under IAU.
        let iqama = with_national_check_digit("299123456").unwrap();
        assert!(classify_identifier(&iqama, &iau).is_wrong_identifier());
        // An Iqama-shaped string the IAU scheme can produce: only a note.
        let shaped = with_national_check_digit("222000123").unwrap();
        assert_eq!(
            classify_identifier(&shaped, &iau),
            IdentifierClass::NationalIdShaped(NationalIdKind::Resident)
        );
        // Under a 9-digit scheme any national ID is exact.
        assert!(classify_identifier(&shaped, &ksu).is_wrong_identifier());
        assert_eq!(
            classify_identifier("0551234567", &iau),
            IdentifierClass::Phone
        );
        assert_eq!(
            classify_identifier("٠٥٥١٢٣٤٥٦٧", &iau),
            IdentifierClass::Phone
        );
        assert_eq!(mask_digits("12-34"), "••-••");
    }

    #[test]
    fn column_detection() {
        let iau = StudentIdScheme::iau();
        let nids: Vec<String> = (0..20)
            .map(|i| with_national_check_digit(&format!("1{:08}", i * 7919)).unwrap())
            .collect();
        let refs: Vec<&str> = nids.iter().map(String::as_str).collect();
        assert!(national_id_column(&refs, &iau).flagged);
        let students: Vec<String> = (0..20).map(|i| format!("22200{:05}", i * 37)).collect();
        let refs: Vec<&str> = students.iter().map(String::as_str).collect();
        let v = national_id_column(&refs, &iau);
        assert!(!v.flagged, "{v:?}");
    }
}
