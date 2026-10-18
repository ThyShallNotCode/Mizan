//! IAU real-data identity check (restricted data; never committed).
//!
//! Skips unless `OMR_IAU_REFERENCE` points at the adjudicated `reference.json` (143 pages:
//! bubbled student number with `_` for blank columns, and the handwritten "Student ID" line as
//! read by one or two readers, `?` for unreadable characters). Optional `OMR_IAU_OUT` (a
//! scratch folder) receives the aggregate report. Only counts are printed or written.
//!
//! Checks:
//! 1. every complete reference student number satisfies the provisional IAU scheme;
//! 2. per course, a roster built from those numbers imports cleanly and every page decodes to
//!    its own number (blank columns as one-column wildcards);
//! 3. the handwritten ID either agrees with the bubbles or is flagged `channel_conflict`;
//! 4. leave-one-out: a student missing from the roster is never matched (`not_on_roster`);
//! 5. national-ID patterns are never accepted as student keys (synthetic national IDs and
//!    Iqamas, roster columns, rosterless mode);
//! 6. in the batch assignment every complete page keeps its own number; IDN-005 can only be
//!    raised by a page whose grid is incomplete.

use omr_identity::national::with_national_check_digit;
use omr_identity::*;
use omr_model::{ColumnState, IdColumnRead, IdGridRead, LogitMilli, StudentKey};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

fn grid(p: &str) -> IdGridRead {
    IdGridRead {
        columns: p
            .chars()
            .enumerate()
            .map(|(i, ch)| {
                let (state, marked) = match ch.to_digit(10) {
                    Some(d) => (ColumnState::Single, vec![d as u8]),
                    None => (ColumnState::Blank, vec![]),
                };
                IdColumnRead {
                    column: i as u8,
                    logits: (0..10u8)
                        .map(|k| LogitMilli(if marked.contains(&k) { 9000 } else { -9000 }))
                        .collect(),
                    state,
                    marked,
                }
            })
            .collect(),
    }
}

/// Merges one or two readers' transcriptions: positions where they differ become `?`.
fn merged_handwriting(v: &[Value]) -> Option<String> {
    let reads: Vec<&str> = v.iter().filter_map(Value::as_str).collect();
    let first = *reads.first()?;
    let merged: String = match reads.get(1) {
        Some(second) if second.chars().count() == first.chars().count() => first
            .chars()
            .zip(second.chars())
            .map(|(a, b)| if a == b { a } else { '?' })
            .collect(),
        _ => first.to_owned(),
    };
    merged.chars().any(|c| c.is_ascii_digit()).then_some(merged)
}

fn course_of(tag: &str) -> &'static str {
    if tag.starts_with('B') {
        "course-1"
    } else {
        "course-2"
    }
}

#[test]
fn iau_reference_identity_check() {
    let Some(path) = std::env::var_os("OMR_IAU_REFERENCE").filter(|v| !v.is_empty()) else {
        eprintln!("skipped: set OMR_IAU_REFERENCE to run the IAU identity check");
        return;
    };
    let pages: Vec<Value> = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let scheme = StudentIdScheme::iau();
    let policy = IdentityPolicy::default();
    let prior = PriorContext::default();

    // 1. Scheme.
    let (mut complete, mut scheme_ok, mut partial, mut all_blank) = (0, 0, 0, 0);
    let mut nid_shaped_numbers = 0;
    let mut by_course: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
    for p in &pages {
        let sn = p["student_number"].as_str().unwrap();
        by_course
            .entry(course_of(p["tag"].as_str().unwrap()))
            .or_default()
            .push(p);
        if sn.bytes().all(|b| b.is_ascii_digit()) {
            complete += 1;
            if scheme.accepts(sn) {
                scheme_ok += 1;
            }
            if is_national_id(sn) {
                nid_shaped_numbers += 1;
                assert!(
                    matches!(
                        classify_identifier(sn, &scheme),
                        IdentifierClass::NationalIdShaped(_)
                    ),
                    "classification"
                );
            }
        } else if sn.bytes().all(|b| !b.is_ascii_digit()) {
            all_blank += 1;
        } else {
            partial += 1;
        }
    }
    assert!(
        scheme_ok == complete,
        "a complete student number fails the IAU scheme"
    );

    // 2–3. Per course: roster, decode, handwriting.
    let mut stats: BTreeMap<&str, usize> = BTreeMap::new();
    let mut bump = |k: &'static str| *stats.entry(k).or_default() += 1;
    let mut roster_sizes = Vec::new();
    let mut batch_dups = 0;
    let mut batch_not_on_roster = 0;
    let mut loo_not_on_roster = 0;
    let mut loo_writer_error_zone = 0;
    let mut loo_total = 0;
    for (course, ps) in &by_course {
        let keys: BTreeSet<String> = ps
            .iter()
            .map(|p| p["student_number"].as_str().unwrap().to_owned())
            .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
            .collect();
        let recs = |skip: Option<&str>| -> Vec<RosterRecord> {
            keys.iter()
                .filter(|k| Some(k.as_str()) != skip)
                .enumerate()
                .map(|(i, k)| RosterRecord {
                    student_key: k.clone(),
                    name: format!("Student {i}"),
                    ..Default::default()
                })
                .collect()
        };
        let (roster, report) =
            RosterSnapshot::build(recs(None), &scheme, &ImportOptions::default())
                .unwrap_or_else(|e| panic!("{course}: roster import blocked: {e}"));
        assert!(
            !report
                .issues
                .iter()
                .any(|i| matches!(i, ImportIssue::NationalIdColumn { .. }))
        );
        roster_sizes.push(json!({ "course": course, "students": roster.len(),
            "national_id_shaped_keys": report.national_id_shaped_keys }));

        let mut items = Vec::new();
        for (n, p) in ps.iter().enumerate() {
            let sn = p["student_number"].as_str().unwrap();
            let hw = merged_handwriting(p["student_id_handwritten"].as_array().unwrap());
            let ev = BookletEvidence {
                grid: Some(grid(sn)),
                typed: hw.as_ref().map(|t| TypedId {
                    text: t.clone(),
                    source: TypedSource::Ocr,
                }),
                ..Default::default()
            };
            let d = decode_booklet(&ev, &roster, &scheme, &prior, &policy);
            let complete_number = sn.bytes().all(|b| b.is_ascii_digit());
            if complete_number {
                assert!(
                    d.student.as_ref().map(|s| s.0.as_str()) == Some(sn),
                    "{course}: a complete page did not decode to its own number"
                );
                bump("complete_decoded_to_own_number");
            } else if sn.bytes().any(|b| b.is_ascii_digit()) {
                bump("partial_pages");
                if d.has(SubReason::IdBlank) {
                    bump("partial_flagged_id_blank");
                }
                if d.student.is_some() && d.candidates.first().is_some_and(|c| c.posterior > 0.99) {
                    bump("partial_resolved_by_wildcard_or_handwriting");
                }
            } else {
                bump("blank_grid_pages");
                if d.has(SubReason::IdBlank) {
                    bump("blank_grid_flagged_id_blank");
                }
            }
            match d.status {
                IdentityStatus::AutoAccepted => bump("status_auto_accepted"),
                IdentityStatus::NeedsConfirmation => bump("status_needs_confirmation"),
                IdentityStatus::Review => bump("status_review"),
            }
            // Handwriting: agree or flagged, never a silent disagreement.
            match &hw {
                None => bump("handwriting_unreadable_or_absent"),
                Some(t) => {
                    let consistent = complete_number
                        && t.chars().count() == sn.chars().count()
                        && t.chars().zip(sn.chars()).all(|(a, b)| a == '?' || a == b);
                    let flagged = d.has(SubReason::ChannelConflict);
                    if complete_number {
                        if consistent {
                            assert!(
                                !flagged,
                                "{course}: consistent handwriting must not be flagged"
                            );
                            bump("handwriting_agrees");
                        } else {
                            assert!(flagged, "{course}: disagreeing handwriting must be flagged");
                            bump("handwriting_disagrees_flagged");
                        }
                    } else if flagged {
                        bump("handwriting_flagged_on_partial_grid");
                    } else {
                        bump("handwriting_consistent_with_partial_grid");
                    }
                }
            }
            items.push(BookletIdentity {
                booklet: format!("{course}-{n:04}"),
                decision: d,
                confirmed: None,
            });
        }
        let out = assign_batch(&items, &roster, &prior, &policy);
        batch_dups += out.duplicates.len();
        // Every complete page keeps its own number; a duplicate can only come from a page whose
        // grid is incomplete (its own number is not in this roster built from complete grids).
        let incomplete: BTreeSet<String> = ps
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                !p["student_number"]
                    .as_str()
                    .unwrap()
                    .bytes()
                    .all(|b| b.is_ascii_digit())
            })
            .map(|(n, _)| format!("{course}-{n:04}"))
            .collect();
        for (b, p) in out.booklets.iter().zip(ps.iter()) {
            let sn = p["student_number"].as_str().unwrap();
            if sn.bytes().all(|x| x.is_ascii_digit()) {
                let own = StudentKey(sn.to_owned());
                // Never another student and never ∅; a second copy only in a tie with an
                // incomplete page claiming the same student (both then carry IDN-005).
                let ok = match &b.assigned {
                    Assigned::Student(k) => *k == own,
                    Assigned::Duplicate(k) => {
                        *k == own && out.duplicates.iter().any(|(d, _)| *d == own)
                    }
                    Assigned::NotOnRoster => false,
                };
                assert!(
                    ok,
                    "a complete page was assigned to something other than its own number"
                );
                if matches!(b.assigned, Assigned::Student(_)) {
                    bump("complete_pages_assigned_own_number");
                } else {
                    bump("complete_pages_tied_with_incomplete_claim");
                }
            }
        }
        for (_, e) in &out.duplicates {
            assert!(incomplete.iter().any(|b| e.detail.contains(b.as_str())));
            bump("duplicates_raised_by_incomplete_grid_pages");
        }
        batch_not_on_roster += out
            .booklets
            .iter()
            .filter(|b| b.assigned == Assigned::NotOnRoster)
            .count();

        // 4. Leave-one-out: remove each student from the roster; the page must never match.
        for k in &keys {
            let (r2, _) =
                RosterSnapshot::build(recs(Some(k)), &scheme, &ImportOptions::default()).unwrap();
            let d = decode_booklet(
                &BookletEvidence {
                    grid: Some(grid(k)),
                    ..Default::default()
                },
                &r2,
                &scheme,
                &prior,
                &policy,
            );
            loo_total += 1;
            assert!(
                d.student.is_none(),
                "{course}: a missing student was matched"
            );
            assert!(
                d.status == IdentityStatus::Review,
                "{course}: leave-one-out not in review"
            );
            if d.has(SubReason::NotOnRoster) {
                loo_not_on_roster += 1;
            }
            if d.has(SubReason::WriterError) {
                loo_writer_error_zone += 1;
            }
        }
    }

    // 5. National IDs are never student keys (synthetic numbers, never real ones).
    let (mut nid_refused, mut nid_total) = (0, 0);
    let course1 = by_course.values().next().unwrap();
    let keys1: Vec<String> = course1
        .iter()
        .map(|p| p["student_number"].as_str().unwrap().to_owned())
        .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let roster1 = RosterSnapshot::build(
        keys1
            .iter()
            .map(|k| RosterRecord {
                student_key: k.clone(),
                name: "S".into(),
                ..Default::default()
            })
            .collect(),
        &scheme,
        &ImportOptions::default(),
    )
    .unwrap()
    .0;
    for i in 0..pages.len() as u32 {
        let citizen =
            with_national_check_digit(&format!("1{:08}", (i * 104_729) % 100_000_000)).unwrap();
        let iqama =
            with_national_check_digit(&format!("29{:07}", (i * 7_919) % 10_000_000)).unwrap();
        for nid in [citizen, iqama] {
            nid_total += 1;
            let ev = BookletEvidence {
                grid: Some(grid(&nid)),
                ..Default::default()
            };
            let d = decode_booklet(&ev, &roster1, &scheme, &prior, &policy);
            let r = decode_rosterless(&ev, &scheme, &policy);
            let blocked = RosterSnapshot::build(
                vec![RosterRecord {
                    student_key: nid.clone(),
                    name: "X".into(),
                    ..Default::default()
                }],
                &scheme,
                &ImportOptions::default(),
            )
            .is_err();
            if d.student.is_none()
                && d.has(SubReason::WrongIdentifier)
                && r.student.is_none()
                && r.has(SubReason::WrongIdentifier)
                && blocked
            {
                nid_refused += 1;
            }
            assert!(
                !format!("{d:?}{r:?}").contains(nid.as_str()),
                "national ID digits must be masked"
            );
        }
    }
    assert!(
        nid_refused == nid_total,
        "a synthetic national ID was accepted"
    );
    // Rosterless mode accepts every complete real number as its own key.
    let rosterless_ok = pages
        .iter()
        .filter_map(|p| p["student_number"].as_str())
        .filter(|s| s.bytes().all(|b| b.is_ascii_digit()))
        .filter(|s| {
            decode_rosterless(
                &BookletEvidence {
                    grid: Some(grid(s)),
                    ..Default::default()
                },
                &scheme,
                &policy,
            )
            .student
            .is_some_and(|k| k == StudentKey((*s).to_owned()))
        })
        .count();
    assert!(
        rosterless_ok == complete,
        "rosterless mode refused a complete number"
    );

    let report = json!({
        "pages": pages.len(),
        "student_numbers_complete": complete,
        "student_numbers_pass_iau_scheme": scheme_ok,
        "student_numbers_partial_blank_columns": partial,
        "student_numbers_all_blank": all_blank,
        "complete_numbers_luhn_valid_national_id_shape": nid_shaped_numbers,
        "rosters": roster_sizes,
        "decode": stats,
        "batch_duplicates_idn005": batch_dups,
        "batch_not_on_roster": batch_not_on_roster,
        "leave_one_out": { "cases": loo_total, "not_on_roster": loo_not_on_roster,
            "writer_error_zone": loo_writer_error_zone, "matched_to_another_student": 0 },
        "synthetic_national_ids_refused": format!("{nid_refused}/{nid_total}"),
        "rosterless_accepts_complete_numbers": rosterless_ok,
    });
    eprintln!("{}", serde_json::to_string_pretty(&report).unwrap());
    if let Some(out) = std::env::var_os("OMR_IAU_OUT").filter(|v| !v.is_empty()) {
        let out = std::path::PathBuf::from(out);
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(
            out.join("omr_identity_real_iau.json"),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .unwrap();
    }
    assert!(
        loo_not_on_roster == loo_total,
        "leave-one-out without not_on_roster"
    );
}
