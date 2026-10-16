//! Roster snapshots (§9.2): records parsed by the caller (CSV/XLSX/SIS) become a validated,
//! normalised, hashed snapshot. Identity decisions store the snapshot hash so appeals replay.

use crate::national::{IdentifierClass, classify_identifier, mask_digits, national_id_column};
use crate::normalize::{name_tokens, normalize_id, normalize_name, token_edit_distance};
use crate::scheme::{DecodedId, StudentIdScheme};
use omr_model::StudentKey;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha384};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

/// Section track (§9.2): an organisational attribute of the section, not a demographic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Track {
    /// Male section.
    Male,
    /// Female section.
    Female,
    /// Mixed section.
    Mixed,
}

/// SIS enrolment status (§9.2, consumed by §9.6 and §10.6).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SisStatus {
    /// Regularly enrolled.
    Enrolled,
    /// DN: denied entry (attendance below 75 %); not expected at the final, never absent.
    DeniedEntry,
    /// W: withdrawn.
    Withdrawn,
    /// IC: incomplete.
    Incomplete,
    /// Any other SIS code, verbatim.
    Other(String),
}

/// One roster row as parsed by the caller (display values, unchanged).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RosterRecord {
    /// The student ID as text, exactly as imported.
    pub student_key: String,
    /// The key came from a numeric spreadsheet cell (Excel float coercion checks, §9.2 rule 1).
    pub key_from_numeric_cell: bool,
    /// Official name.
    pub name: String,
    /// Arabic name, when the SIS has both scripts.
    pub name_ar: Option<String>,
    /// English name.
    pub name_en: Option<String>,
    /// Section code.
    pub section: Option<String>,
    /// Campus (main or a branch code).
    pub campus: Option<String>,
    /// Section track.
    pub track: Option<Track>,
    /// SIS status.
    pub sis_status: Option<SisStatus>,
}

/// A validated roster row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RosterRow {
    /// Normalised key (§9.2 rule 3).
    pub key: StudentKey,
    /// The record as imported (display values).
    pub record: RosterRecord,
    /// Name matching key (§9.3).
    pub name_key: String,
    /// The key's digits (0–9 values) when the key is all digits, for ID-grid matching.
    pub digits: Option<Vec<u8>>,
    /// Decoded scheme fields, when the key satisfies the scheme.
    pub decoded: Option<DecodedId>,
}

/// Import options.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ImportOptions {
    /// Institution rule "IDs have fixed length N": shorter all-digit keys get their leading
    /// zeros restored (shown in the preview, §9.2 rule 2). `None` never pads.
    pub pad_to_length: Option<usize>,
}

/// One finding of the pre-commit validation report (§9.2 rule 4). Row numbers are 0-based.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImportIssue {
    /// Two rows normalise to the same key (blocks).
    DuplicateKey {
        /// The key.
        key: String,
        /// Rows.
        rows: Vec<usize>,
    },
    /// Empty key (blocks).
    EmptyKey {
        /// Row.
        row: usize,
    },
    /// `1.23E+15`-style text (blocks: precision loss).
    ScientificNotation {
        /// Row.
        row: usize,
    },
    /// `….0` artefact of a float cell (blocks).
    FloatArtifact {
        /// Row.
        row: usize,
    },
    /// Numeric cell with more than 15 significant digits (blocks: precision loss).
    PrecisionLoss {
        /// Row.
        row: usize,
    },
    /// A key that is a valid national ID/Iqama the scheme cannot produce (blocks; masked).
    NationalIdAsKey {
        /// Row.
        row: usize,
        /// Masked value.
        masked: String,
    },
    /// The key column looks like a national-ID column (blocks).
    NationalIdColumn {
        /// Valid national IDs among the keys.
        national_ids: usize,
        /// Keys examined.
        values: usize,
    },
    /// Integer-valued numeric cell converted to text (warns).
    NumericCellConverted {
        /// Row.
        row: usize,
    },
    /// Leading zeros restored under the fixed-length rule (warns).
    LeadingZerosRestored {
        /// Row.
        row: usize,
    },
    /// The key fails the student-ID scheme (warns).
    SchemeMismatch {
        /// Row.
        row: usize,
        /// Reason.
        reason: String,
    },
    /// Empty name (warns).
    EmptyName {
        /// Row.
        row: usize,
    },
    /// Keys of different lengths (warns): length → count.
    MixedLengths {
        /// Histogram.
        lengths: BTreeMap<usize, usize>,
    },
}

impl ImportIssue {
    /// True for issues that block the import.
    pub fn is_blocking(&self) -> bool {
        matches!(
            self,
            ImportIssue::DuplicateKey { .. }
                | ImportIssue::EmptyKey { .. }
                | ImportIssue::ScientificNotation { .. }
                | ImportIssue::FloatArtifact { .. }
                | ImportIssue::PrecisionLoss { .. }
                | ImportIssue::NationalIdAsKey { .. }
                | ImportIssue::NationalIdColumn { .. }
        )
    }
}

/// The validation report shown before commit.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ImportReport {
    /// Rows received.
    pub rows: usize,
    /// All findings, blocking first, then by row.
    pub issues: Vec<ImportIssue>,
    /// Namesake groups (identical normalised names, or token edit distance ≤ 1), for §9.7.
    pub namesake_groups: Vec<Vec<StudentKey>>,
    /// Keys that are national-ID shaped although the scheme produces them (fusion note only).
    pub national_id_shaped_keys: usize,
}

impl ImportReport {
    /// True when any issue blocks the import.
    pub fn is_blocked(&self) -> bool {
        self.issues.iter().any(ImportIssue::is_blocking)
    }
}

/// Import failure.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum RosterError {
    /// The validation report has blocking issues.
    #[error("roster import blocked: {} blocking issue(s)", .0.issues.iter().filter(|i| i.is_blocking()).count())]
    Blocked(ImportReport),
}

/// A versioned roster snapshot (§9.2 "Snapshots"): rows sorted by key, hashed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RosterSnapshot {
    rows: Vec<RosterRow>,
    index: BTreeMap<String, usize>,
    scheme: String,
    scheme_version: u32,
    sha384: String,
    year_counts: BTreeMap<u16, u32>,
    namesakes: Vec<Vec<StudentKey>>,
}

/// Differences between two snapshots (IDN-004 `roster_diff` review input).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RosterDiff {
    /// Keys only in the newer snapshot.
    pub added: Vec<StudentKey>,
    /// Keys only in the older snapshot.
    pub removed: Vec<StudentKey>,
    /// Keys whose record changed.
    pub changed: Vec<StudentKey>,
}

impl RosterDiff {
    /// True when nothing changed.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

fn is_scientific(s: &str) -> bool {
    let t = s.trim();
    let Some((mant, exp)) = t.split_once(['e', 'E']) else {
        return false;
    };
    let exp = exp.trim_start_matches(['+', '-']);
    !mant.is_empty()
        && !exp.is_empty()
        && exp.bytes().all(|b| b.is_ascii_digit())
        && mant.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && mant.bytes().any(|b| b.is_ascii_digit())
}

fn is_float_artifact(s: &str) -> bool {
    let t = s.trim();
    match t.split_once('.') {
        Some((int, frac)) => {
            !int.is_empty()
                && int.bytes().all(|b| b.is_ascii_digit())
                && !frac.is_empty()
                && frac.bytes().all(|b| b == b'0')
        }
        None => false,
    }
}

impl RosterSnapshot {
    /// Validates and normalises records into a snapshot. Blocking issues (§9.2 rules 1–4)
    /// return [`RosterError::Blocked`] with the full report; otherwise the report carries the
    /// warnings and namesake groups.
    pub fn build(
        records: Vec<RosterRecord>,
        scheme: &StudentIdScheme,
        opts: &ImportOptions,
    ) -> Result<(Self, ImportReport), RosterError> {
        let mut issues = Vec::new();
        let mut keyed: Vec<(String, usize)> = Vec::with_capacity(records.len());
        let mut nid_shaped = 0;
        for (row, r) in records.iter().enumerate() {
            let raw = r.student_key.trim();
            if is_scientific(raw) {
                issues.push(ImportIssue::ScientificNotation { row });
                continue;
            }
            if is_float_artifact(raw) {
                issues.push(ImportIssue::FloatArtifact { row });
                continue;
            }
            let mut key = normalize_id(raw);
            if key.is_empty() {
                issues.push(ImportIssue::EmptyKey { row });
                continue;
            }
            let all_digits = key.bytes().all(|b| b.is_ascii_digit());
            if r.key_from_numeric_cell {
                if !all_digits || key.trim_start_matches('0').len() > 15 {
                    issues.push(ImportIssue::PrecisionLoss { row });
                    continue;
                }
                issues.push(ImportIssue::NumericCellConverted { row });
            }
            if let Some(n) = opts.pad_to_length
                && all_digits
                && key.len() < n
            {
                key = format!("{key:0>n$}");
                issues.push(ImportIssue::LeadingZerosRestored { row });
            }
            match classify_identifier(&key, scheme) {
                IdentifierClass::NationalId(_) => {
                    issues.push(ImportIssue::NationalIdAsKey {
                        row,
                        masked: mask_digits(&key),
                    });
                    continue;
                }
                IdentifierClass::NationalIdShaped(_) => nid_shaped += 1,
                _ => {}
            }
            if let Err(e) = scheme.validate(&key) {
                issues.push(ImportIssue::SchemeMismatch {
                    row,
                    reason: e.to_string(),
                });
            }
            if r.name.trim().is_empty() {
                issues.push(ImportIssue::EmptyName { row });
            }
            keyed.push((key, row));
        }
        // Column-level national-ID check over all keys.
        let keys: Vec<&str> = keyed.iter().map(|(k, _)| k.as_str()).collect();
        let verdict = national_id_column(&keys, scheme);
        if verdict.flagged && verdict.exact == 0 {
            issues.push(ImportIssue::NationalIdColumn {
                national_ids: verdict.national_ids,
                values: verdict.values,
            });
        }
        // Duplicates.
        let mut by_key: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for (k, row) in &keyed {
            by_key.entry(k.as_str()).or_default().push(*row);
        }
        for (k, rows) in &by_key {
            if rows.len() > 1 {
                issues.push(ImportIssue::DuplicateKey {
                    key: (*k).to_owned(),
                    rows: rows.clone(),
                });
            }
        }
        let mut lengths: BTreeMap<usize, usize> = BTreeMap::new();
        for (k, _) in &keyed {
            *lengths.entry(k.chars().count()).or_default() += 1;
        }
        if lengths.len() > 1 {
            issues.push(ImportIssue::MixedLengths { lengths });
        }
        issues.sort_by_key(|i| (!i.is_blocking(), issue_row(i)));

        let mut rows: Vec<RosterRow> = keyed
            .iter()
            .map(|(k, row)| {
                let rec = records[*row].clone();
                let digits = k
                    .bytes()
                    .all(|b| b.is_ascii_digit())
                    .then(|| k.bytes().map(|b| b - b'0').collect());
                RosterRow {
                    key: StudentKey(k.clone()),
                    name_key: normalize_name(&rec.name),
                    decoded: scheme.validate(k).ok(),
                    digits,
                    record: rec,
                }
            })
            .collect();
        rows.sort_by(|a, b| a.key.cmp(&b.key));
        rows.dedup_by(|a, b| a.key == b.key);
        let namesakes = namesake_groups(&rows);
        let mut report = ImportReport {
            rows: records.len(),
            issues,
            namesake_groups: namesakes.clone(),
            national_id_shaped_keys: nid_shaped,
        };
        if report.is_blocked() {
            report.namesake_groups.clear();
            return Err(RosterError::Blocked(report));
        }
        let index = rows
            .iter()
            .enumerate()
            .map(|(i, r)| (r.key.0.clone(), i))
            .collect();
        let mut year_counts = BTreeMap::new();
        for r in &rows {
            if let Some(y) = r.decoded.as_ref().and_then(DecodedId::year) {
                *year_counts.entry(y).or_insert(0u32) += 1;
            }
        }
        let sha384 = snapshot_hash(&rows, scheme);
        Ok((
            Self {
                rows,
                index,
                scheme: scheme.name().to_owned(),
                scheme_version: scheme.spec().version,
                sha384,
                year_counts,
                namesakes,
            },
            report,
        ))
    }

    /// Rows sorted by key.
    pub fn rows(&self) -> &[RosterRow] {
        &self.rows
    }

    /// Number of students.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Looks a (raw or normalised) key up.
    pub fn get(&self, key: &str) -> Option<&RosterRow> {
        self.index.get(&normalize_id(key)).map(|&i| &self.rows[i])
    }

    /// Row index of a normalised key.
    pub(crate) fn position(&self, key: &str) -> Option<usize> {
        self.index.get(key).copied()
    }

    /// SHA-384 (hex) of the canonical JSON (RFC 8785 subset: sorted keys, no whitespace) of the
    /// normalised rows sorted by key plus the scheme name and version.
    pub fn sha384(&self) -> &str {
        &self.sha384
    }

    /// Admission-year histogram (for the ∅ prior, §9.5).
    pub fn year_counts(&self) -> &BTreeMap<u16, u32> {
        &self.year_counts
    }

    /// Namesake groups recorded at import.
    pub fn namesake_groups(&self) -> &[Vec<StudentKey>] {
        &self.namesakes
    }

    /// Scheme name and version pinned by this snapshot.
    pub fn scheme(&self) -> (&str, u32) {
        (&self.scheme, self.scheme_version)
    }

    /// Differences from `self` (older) to `newer`.
    pub fn diff(&self, newer: &RosterSnapshot) -> RosterDiff {
        let mut d = RosterDiff::default();
        for r in &newer.rows {
            match self.get(&r.key.0) {
                None => d.added.push(r.key.clone()),
                Some(old) if old.record != r.record => d.changed.push(r.key.clone()),
                _ => {}
            }
        }
        for r in &self.rows {
            if newer.get(&r.key.0).is_none() {
                d.removed.push(r.key.clone());
            }
        }
        d
    }
}

fn issue_row(i: &ImportIssue) -> usize {
    match i {
        ImportIssue::EmptyKey { row }
        | ImportIssue::ScientificNotation { row }
        | ImportIssue::FloatArtifact { row }
        | ImportIssue::PrecisionLoss { row }
        | ImportIssue::NationalIdAsKey { row, .. }
        | ImportIssue::NumericCellConverted { row }
        | ImportIssue::LeadingZerosRestored { row }
        | ImportIssue::SchemeMismatch { row, .. }
        | ImportIssue::EmptyName { row } => *row,
        ImportIssue::DuplicateKey { rows, .. } => rows.first().copied().unwrap_or(0),
        ImportIssue::NationalIdColumn { .. } | ImportIssue::MixedLengths { .. } => usize::MAX,
    }
}

fn snapshot_hash(rows: &[RosterRow], scheme: &StudentIdScheme) -> String {
    let opt = |v: &Option<String>| v.as_ref().map_or(Value::Null, |s| Value::String(s.clone()));
    let rows_json: Vec<Value> = rows
        .iter()
        .map(|r| {
            let mut m = Map::new();
            m.insert("campus".into(), opt(&r.record.campus));
            m.insert("key".into(), Value::String(r.key.0.clone()));
            m.insert("name".into(), Value::String(r.record.name.clone()));
            m.insert("name_ar".into(), opt(&r.record.name_ar));
            m.insert("name_en".into(), opt(&r.record.name_en));
            m.insert("section".into(), opt(&r.record.section));
            m.insert(
                "sis_status".into(),
                r.record.sis_status.as_ref().map_or(Value::Null, |s| {
                    serde_json::to_value(s).unwrap_or(Value::Null)
                }),
            );
            m.insert(
                "track".into(),
                r.record.track.map_or(Value::Null, |t| {
                    serde_json::to_value(t).unwrap_or(Value::Null)
                }),
            );
            Value::Object(m)
        })
        .collect();
    let doc = json!({
        "rows": rows_json,
        "scheme": scheme.name(),
        "scheme_version": scheme.spec().version,
    });
    let bytes = serde_json::to_vec(&doc).unwrap_or_default();
    hex::encode(Sha384::digest(&bytes))
}

/// Namesake groups: identical normalised names, or token edit distance ≤ 1 (§9.2 rule 4).
/// Candidate pairs come from a symmetric-deletion index, then are verified exactly.
fn namesake_groups(rows: &[RosterRow]) -> Vec<Vec<StudentKey>> {
    let tokens: Vec<Vec<String>> = rows.iter().map(|r| name_tokens(&r.name_key)).collect();
    let mut index: BTreeMap<Vec<String>, Vec<usize>> = BTreeMap::new();
    for (i, t) in tokens.iter().enumerate() {
        if t.is_empty() {
            continue;
        }
        index.entry(t.clone()).or_default().push(i);
        if t.len() > 1 {
            for d in 0..t.len() {
                let mut v = t.clone();
                v.remove(d);
                index.entry(v).or_default().push(i);
            }
        }
    }
    // Union-find over verified pairs.
    let mut parent: Vec<usize> = (0..rows.len()).collect();
    fn find(p: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while p[r] != r {
            r = p[r];
        }
        let mut y = x;
        while p[y] != r {
            let n = p[y];
            p[y] = r;
            y = n;
        }
        r
    }
    for members in index.values() {
        let uniq: BTreeSet<usize> = members.iter().copied().collect();
        let v: Vec<usize> = uniq.into_iter().collect();
        for a in 0..v.len() {
            for b in a + 1..v.len() {
                let (i, j) = (v[a], v[b]);
                if token_edit_distance(&tokens[i], &tokens[j]) <= 1 {
                    let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                    if ri != rj {
                        parent[ri.max(rj)] = ri.min(rj);
                    }
                }
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<StudentKey>> = BTreeMap::new();
    for (i, row) in rows.iter().enumerate() {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(row.key.clone());
    }
    groups.into_values().filter(|g| g.len() > 1).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::national::with_national_check_digit;

    fn rec(key: &str, name: &str) -> RosterRecord {
        RosterRecord {
            student_key: key.into(),
            name: name.into(),
            ..Default::default()
        }
    }

    #[test]
    fn build_normalises_and_hashes_deterministically() {
        let s = StudentIdScheme::iau();
        let records = vec![
            rec("٢٢٢٠٠٠١٢٣٤", "Ali Hassan"),
            rec("2210000001", "Sara Noor"),
            rec(" 2200000002 ", "Omar Ali"),
        ];
        let (snap, report) =
            RosterSnapshot::build(records.clone(), &s, &ImportOptions::default()).unwrap();
        assert!(!report.is_blocked());
        assert_eq!(snap.len(), 3);
        assert!(snap.get("2220001234").is_some());
        assert_eq!(snap.rows()[0].key.0, "2200000002", "sorted by key");
        assert_eq!(snap.year_counts().get(&2022), Some(&1));
        // Same rows in another order → same hash.
        let mut rev = records;
        rev.reverse();
        let (snap2, _) = RosterSnapshot::build(rev, &s, &ImportOptions::default()).unwrap();
        assert_eq!(snap.sha384(), snap2.sha384());
        assert_eq!(snap.sha384().len(), 96);
    }

    #[test]
    fn blocking_rules() {
        let s = StudentIdScheme::iau();
        let nid = with_national_check_digit("112345678").unwrap();
        let bad = vec![
            rec("2.22E+09", "A"),
            rec("2220001234.0", "B"),
            rec("", "C"),
            rec("2220001111", "D"),
            rec("2220001111", "E"),
            rec(&nid, "F"),
        ];
        let Err(RosterError::Blocked(r)) =
            RosterSnapshot::build(bad, &s, &ImportOptions::default())
        else {
            panic!("must block")
        };
        let kinds: Vec<&ImportIssue> = r.issues.iter().filter(|i| i.is_blocking()).collect();
        assert!(
            kinds
                .iter()
                .any(|i| matches!(i, ImportIssue::ScientificNotation { row: 0 }))
        );
        assert!(
            kinds
                .iter()
                .any(|i| matches!(i, ImportIssue::FloatArtifact { row: 1 }))
        );
        assert!(
            kinds
                .iter()
                .any(|i| matches!(i, ImportIssue::EmptyKey { row: 2 }))
        );
        assert!(
            kinds
                .iter()
                .any(|i| matches!(i, ImportIssue::DuplicateKey { .. }))
        );
        let masked = kinds.iter().find_map(|i| match i {
            ImportIssue::NationalIdAsKey { masked, .. } => Some(masked.clone()),
            _ => None,
        });
        assert_eq!(
            masked.as_deref(),
            Some("••••••••••"),
            "national IDs are masked"
        );
        // Numeric cells: > 15 significant digits block; integers convert with a warning.
        let mut big = rec("1234567890123456", "G");
        big.key_from_numeric_cell = true;
        assert!(RosterSnapshot::build(vec![big], &s, &ImportOptions::default()).is_err());
        let mut ok = rec("2220000009", "H");
        ok.key_from_numeric_cell = true;
        let (_, r) = RosterSnapshot::build(vec![ok], &s, &ImportOptions::default()).unwrap();
        assert!(
            r.issues
                .contains(&ImportIssue::NumericCellConverted { row: 0 })
        );
    }

    #[test]
    fn national_id_column_blocks_and_padding_restores_zeros() {
        let s = StudentIdScheme::digits(10, None);
        // A column of national IDs that the permissive scheme could produce: column-level flag.
        let recs: Vec<RosterRecord> = (0..8)
            .map(|i| {
                rec(
                    &with_national_check_digit(&format!("1{:08}", 1000 + i * 31)).unwrap(),
                    "X",
                )
            })
            .collect();
        // Under DIGITS10 these are "shaped", not exact; the column rule still blocks.
        let Err(RosterError::Blocked(r)) =
            RosterSnapshot::build(recs, &s, &ImportOptions::default())
        else {
            panic!("national-ID column must block")
        };
        assert!(
            r.issues
                .iter()
                .any(|i| matches!(i, ImportIssue::NationalIdColumn { .. }))
        );

        let opts = ImportOptions {
            pad_to_length: Some(9),
        };
        let (snap, r) = RosterSnapshot::build(
            vec![rec("1234567", "A"), rec("433000001", "B")],
            &StudentIdScheme::ksu(),
            &opts,
        )
        .unwrap();
        assert!(snap.get("001234567").is_some());
        assert!(
            r.issues
                .contains(&ImportIssue::LeadingZerosRestored { row: 0 })
        );
    }

    #[test]
    fn namesakes_and_diff() {
        let s = StudentIdScheme::iau();
        let (a, r) = RosterSnapshot::build(
            vec![
                rec("2220000001", "محمد أحمد العتيبي"),
                rec("2220000002", "محمد احمد العتيبي"),
                rec("2220000003", "Sara Noor"),
                rec("2220000004", "Sara Noor Ali"),
                rec("2220000005", "Khalid Omar"),
            ],
            &s,
            &ImportOptions::default(),
        )
        .unwrap();
        assert_eq!(r.namesake_groups.len(), 2, "{:?}", r.namesake_groups);
        let (b, _) = RosterSnapshot::build(
            vec![
                rec("2220000001", "محمد أحمد العتيبي"),
                rec("2220000003", "Sara N."),
                rec("2220000009", "New Student"),
            ],
            &s,
            &ImportOptions::default(),
        )
        .unwrap();
        let d = a.diff(&b);
        assert_eq!(d.added, vec![StudentKey("2220000009".into())]);
        assert_eq!(d.changed, vec![StudentKey("2220000003".into())]);
        assert_eq!(d.removed.len(), 3);
    }
}
