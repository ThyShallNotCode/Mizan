//! Acceptance item A6 (docs/IMPLEMENTATION.md): roster match from the bubbled ID, national
//! ID/Iqama detection, duplicates and not-on-roster, with the correct IDN-004 sub-reasons and
//! IDN-005. Synthetic rosters only (no real identities).

use omr_identity::national::with_national_check_digit;
use omr_identity::*;
use omr_model::{ColumnState, IdColumnRead, IdGridRead, LogitMilli, Severity, StudentKey};
use proptest::prelude::*;
use std::collections::BTreeSet;

/// Grid from a pattern: digit = single mark, `_` blank, `?` unreadable, `[38]` double mark.
fn grid(p: &str) -> IdGridRead {
    let mut cols = Vec::new();
    let mut it = p.chars().peekable();
    while let Some(ch) = it.next() {
        let i = cols.len() as u8;
        let (state, marked) = match ch {
            '_' => (ColumnState::Blank, vec![]),
            '?' => (ColumnState::Unreadable, vec![]),
            '[' => {
                let mut m = Vec::new();
                for c in it.by_ref() {
                    if c == ']' {
                        break;
                    }
                    m.push(c.to_digit(10).unwrap() as u8);
                }
                (ColumnState::Multi, m)
            }
            d => (ColumnState::Single, vec![d.to_digit(10).unwrap() as u8]),
        };
        let logits = (0..10u8)
            .map(|k| LogitMilli(if marked.contains(&k) { 9000 } else { -9000 }))
            .collect();
        cols.push(IdColumnRead {
            column: i,
            state,
            marked,
            logits,
        });
    }
    IdGridRead { columns: cols }
}

fn rec(key: &str) -> RosterRecord {
    RosterRecord {
        student_key: key.into(),
        name: format!("Student {key}"),
        section: Some("001".into()),
        ..Default::default()
    }
}

/// 60 IAU-format IDs sharing prefixes, like a real class.
fn ids() -> Vec<String> {
    (0..60u32)
        .map(|i| {
            format!(
                "22{}0{:06}",
                if i % 3 == 0 { 1 } else { 2 },
                (i * 7919 + 1234) % 1_000_000
            )
        })
        .collect()
}

fn roster_of(keys: &[String]) -> RosterSnapshot {
    let (r, _) = RosterSnapshot::build(
        keys.iter().map(|k| rec(k)).collect(),
        &StudentIdScheme::iau(),
        &ImportOptions::default(),
    )
    .unwrap();
    r
}

fn decode(ev: &BookletEvidence, r: &RosterSnapshot) -> IdentityDecision {
    decode_booklet(
        ev,
        r,
        &StudentIdScheme::iau(),
        &PriorContext::default(),
        &IdentityPolicy::default(),
    )
}

fn ev_grid(p: &str) -> BookletEvidence {
    BookletEvidence {
        grid: Some(grid(p)),
        ..Default::default()
    }
}

fn subs(d: &IdentityDecision) -> BTreeSet<&'static str> {
    d.exceptions
        .iter()
        .filter(|e| e.code == "IDN-004")
        .map(|e| match e.sub_reason.as_deref() {
            Some(s) => SubReason_str(s),
            None => "",
        })
        .collect()
}

#[allow(non_snake_case)]
fn SubReason_str(s: &str) -> &'static str {
    // Leak-free static mapping for set comparisons.
    for r in [
        SubReason::IdBlank,
        SubReason::IdAmbiguous,
        SubReason::NotOnRoster,
        SubReason::LowPosterior,
        SubReason::LowMargin,
        SubReason::ChannelConflict,
        SubReason::WriterError,
        SubReason::WrongIdentifier,
        SubReason::WrongField,
        SubReason::Namesake,
        SubReason::AssignmentChanged,
        SubReason::AssignedToAbsent,
        SubReason::RosterDiff,
        SubReason::CrossSection,
    ] {
        if r.as_str() == s {
            return r.as_str();
        }
    }
    panic!("unknown sub-reason {s}")
}

fn severity(d: &IdentityDecision, sub: &str) -> Option<Severity> {
    d.exceptions
        .iter()
        .find(|e| e.sub_reason.as_deref() == Some(sub))
        .map(|e| e.severity)
}

#[test]
fn req_a6_exact_roster_match_from_bubbled_id() {
    let keys = ids();
    let r = roster_of(&keys);
    for k in keys.iter().take(10) {
        let d = decode(&ev_grid(k), &r);
        assert_eq!(d.student.as_ref().unwrap().0, *k);
        assert_eq!(d.candidates[0].relation, ReadRelation::Exact);
        assert!(d.exceptions.is_empty(), "{:?}", d.exceptions);
        // One channel: a would-be auto-accept that needs contact-sheet confirmation (§9.7).
        assert_eq!(d.status, IdentityStatus::NeedsConfirmation);
        assert!(d.logit_q.unwrap() > 7 * 1024);
        assert!(d.gap_q.unwrap().abs() < 64);
    }
}

#[test]
fn req_a6_second_channel_page_code_auto_accepts_and_conflict_is_idn006() {
    let keys = ids();
    let r = roster_of(&keys);
    let mut ev = ev_grid(&keys[5]);
    ev.bound_student = Some(StudentKey(keys[5].clone()));
    let d = decode(&ev, &r);
    assert_eq!(d.status, IdentityStatus::AutoAccepted);
    assert!(d.exceptions.is_empty());
    // Binding to another student: hard conflict.
    ev.bound_student = Some(StudentKey(keys[6].clone()));
    let d = decode(&ev, &r);
    assert_eq!(d.status, IdentityStatus::Review);
    assert!(d.exceptions.iter().any(|e| e.code == "IDN-006"));
}

#[test]
fn req_a6_blank_multi_unreadable_columns_are_one_column_wildcards() {
    let keys = ids();
    let r = roster_of(&keys);
    let k = &keys[7];
    for (pos, mark) in [(9usize, '_'), (6, '?')] {
        let mut p: Vec<char> = k.chars().collect();
        p[pos] = mark;
        let p: String = p.into_iter().collect();
        let d = decode(&ev_grid(&p), &r);
        assert_eq!(d.student.as_ref().unwrap().0, *k, "{p}");
        assert!(matches!(
            d.candidates[0].relation,
            ReadRelation::WildcardMatch { .. }
        ));
        let sub = if mark == '_' {
            "id_blank"
        } else {
            "id_ambiguous"
        };
        assert_eq!(
            severity(&d, sub),
            Some(Severity::Error),
            "single channel → ERROR"
        );
        assert_eq!(d.status, IdentityStatus::Review);
        // With the page-code channel the same booklet auto-accepts and the sub-reason is INFO.
        let mut ev = ev_grid(&p);
        ev.bound_student = Some(StudentKey(k.clone()));
        let d = decode(&ev, &r);
        assert_eq!(d.status, IdentityStatus::AutoAccepted, "{:?}", d.findings);
        assert_eq!(severity(&d, sub), Some(Severity::Info));
    }
    // A double mark that two roster IDs satisfy: ranked candidates, never auto-decided.
    let a = "2220001131".to_string();
    let b = "2220001181".to_string();
    let r2 = roster_of(&[a.clone(), b.clone(), "2210009999".into()]);
    let d = decode(&ev_grid("22200011[38]1"), &r2);
    let top: BTreeSet<String> = d
        .candidates
        .iter()
        .take(2)
        .map(|c| c.student.0.clone())
        .collect();
    assert_eq!(top, BTreeSet::from([a, b]));
    assert!((d.candidates[0].posterior - 0.5).abs() < 0.05);
    assert_eq!(d.status, IdentityStatus::Review);
    assert!(subs(&d).contains("id_ambiguous") && subs(&d).contains("low_posterior"));
}

#[test]
fn req_a6_never_snaps_to_the_nearest_roster_entry() {
    let keys = ids();
    let r = roster_of(&keys);
    let k = keys[3].clone();
    // One substitution, one adjacent transposition, one dropped digit (writer errors).
    let mut sub: Vec<u8> = k.bytes().collect();
    sub[8] = if sub[8] == b'9' { b'0' } else { sub[8] + 1 };
    let sub = String::from_utf8(sub).unwrap();
    let mut tr: Vec<u8> = k.bytes().collect();
    let i = (0..9).rev().find(|&i| tr[i] != tr[i + 1]).unwrap();
    tr.swap(i, i + 1);
    let tr = String::from_utf8(tr).unwrap();
    for read in [sub, tr] {
        if keys.contains(&read) {
            continue;
        }
        let d = decode(&ev_grid(&read), &r);
        assert_eq!(d.status, IdentityStatus::Review, "{read}");
        assert!(d.student.is_none(), "never snapped: {read}");
        assert_eq!(severity(&d, "not_on_roster"), Some(Severity::Error));
        assert_eq!(severity(&d, "writer_error"), Some(Severity::Error));
        assert!(
            d.notes
                .iter()
                .any(|n| matches!(n, IdentityNote::WriterErrorHypothesis(_)))
        );
        // A second channel supporting the intended student lets it proceed (§9.5 gap test).
        let mut ev = ev_grid(&read);
        ev.bound_student = Some(StudentKey(k.clone()));
        let d = decode(&ev, &r);
        assert_eq!(d.student.as_ref().unwrap().0, k);
        assert!(!subs(&d).contains("not_on_roster"));
        assert!(subs(&d).contains("writer_error"));
    }
    // A read far from every roster ID.
    let d = decode(&ev_grid("2299999999"), &r);
    assert!(d.student.is_none());
    assert_eq!(subs(&d), BTreeSet::from(["not_on_roster"]));
    assert!(nats(d.gap_q.unwrap()) >= 12.0);
}

#[test]
fn req_a6_national_id_and_iqama_detection() {
    let keys = ids();
    let r = roster_of(&keys);
    let citizen = with_national_check_digit("110203040").unwrap();
    let iqama = with_national_check_digit("298765432").unwrap(); // "98" is no admission year
    for nid in [&citizen, &iqama] {
        let d = decode(&ev_grid(nid), &r);
        assert_eq!(severity(&d, "wrong_identifier"), Some(Severity::Error));
        assert!(d.student.is_none());
        assert!(d.read.masked);
        let shown = format!("{:?}{:?}", d.read, d.exceptions);
        assert!(!shown.contains(nid.as_str()), "digits must be masked");
        assert!(matches!(d.channels[0], ChannelUse::Neutralized(_)));
        // Written on the ID line instead of the bubbles: the grid still identifies the student.
        let ev = BookletEvidence {
            grid: Some(grid(&keys[2])),
            typed: Some(TypedId {
                text: nid.clone(),
                source: TypedSource::Ocr,
            }),
            ..Default::default()
        };
        let d = decode(&ev, &r);
        assert_eq!(d.student.as_ref().unwrap().0, keys[2]);
        assert!(subs(&d).contains("wrong_identifier"));
        assert!(!format!("{:?}", d.exceptions).contains(nid.as_str()));
    }
    // Rosters never take a national ID as the key.
    let blocked = RosterSnapshot::build(
        vec![rec(&keys[0]), rec(&citizen)],
        &StudentIdScheme::iau(),
        &ImportOptions::default(),
    );
    assert!(matches!(blocked, Err(RosterError::Blocked(_))));
    // An IAU number that happens to pass the Luhn check is a note, not an exception.
    let shaped = with_national_check_digit("222000555").unwrap();
    assert!(StudentIdScheme::iau().accepts(&shaped));
    let r2 = roster_of(&[shaped.clone(), keys[0].clone()]);
    let d = decode(&ev_grid(&shaped), &r2);
    assert_eq!(d.student.as_ref().unwrap().0, shaped);
    assert!(d.notes.contains(&IdentityNote::NationalIdShaped));
    assert!(d.exceptions.is_empty());
    // Phone numbers are wrong identifiers too.
    let d = decode(&ev_grid("0551234567"), &r);
    assert!(subs(&d).contains("wrong_identifier"));
}

#[test]
fn req_a6_typed_channel_agreement_raises_confidence_and_conflict_is_flagged() {
    let keys = ids();
    let r = roster_of(&keys);
    let k = &keys[11];
    let alone = decode(&ev_grid(k), &r);
    let typed = |text: &str| BookletEvidence {
        grid: Some(grid(k)),
        typed: Some(TypedId {
            text: text.into(),
            source: TypedSource::Ocr,
        }),
        ..Default::default()
    };
    let agree = decode(&typed(k), &r);
    // The logit is capped by the ∅ prior odds (an unlisted student with this exact ID explains
    // both channels equally), so agreement shows as separation from competing students.
    assert!(agree.logit_q >= alone.logit_q);
    assert!(
        agree.candidates[1].log_posterior_q < alone.candidates[1].log_posterior_q - 10 * 1024,
        "agreement pushes competing students further down"
    );
    assert!(agree.notes.contains(&IdentityNote::ChannelsAgree));
    assert!(agree.exceptions.is_empty());
    // Arabic-Indic digits and an unreadable character still agree.
    let arabic: String = k
        .chars()
        .enumerate()
        .map(|(i, c)| {
            if i == 4 {
                '?'
            } else {
                char::from_u32(0x0660 + c.to_digit(10).unwrap()).unwrap()
            }
        })
        .collect();
    let d = decode(&typed(&arabic), &r);
    assert!(d.exceptions.is_empty(), "{:?}", d.exceptions);
    // A different written digit: IDN-004 channel_conflict (ERROR without a third channel).
    let mut other: Vec<u8> = k.bytes().collect();
    other[9] = if other[9] == b'0' { b'1' } else { b'0' };
    let d = decode(&typed(std::str::from_utf8(&other).unwrap()), &r);
    assert_eq!(severity(&d, "channel_conflict"), Some(Severity::Error));
    // Letters in the ID line.
    let d = decode(&typed("22A0001234"), &r);
    assert!(subs(&d).contains("wrong_field"));
}

#[test]
fn req_a6_typed_string_resolves_a_blank_column() {
    let keys = vec!["2220001111".to_string(), "2220001121".to_string()];
    let r = roster_of(&keys);
    let grid_only = decode(&ev_grid("22200011_1"), &r);
    assert!(grid_only.candidates[0].posterior < 0.6);
    let ev = BookletEvidence {
        grid: Some(grid("22200011_1")),
        typed: Some(TypedId {
            text: "2220001121".into(),
            source: TypedSource::Typed,
        }),
        ..Default::default()
    };
    let d = decode(&ev, &r);
    assert_eq!(d.student.as_ref().unwrap().0, "2220001121");
    assert!(d.candidates[0].posterior > 0.99);
}

#[test]
fn req_a6_rosterless_mode() {
    let s = StudentIdScheme::iau();
    let p = IdentityPolicy::default();
    let d = decode_rosterless(&ev_grid("2220001234"), &s, &p);
    assert_eq!(d.status, IdentityStatus::AutoAccepted);
    assert_eq!(d.student.as_ref().unwrap().0, "2220001234");
    let d = decode_rosterless(&ev_grid("22200012_4"), &s, &p);
    assert!(d.student.is_none() && subs(&d).contains("id_blank"));
    let d = decode_rosterless(&ev_grid("22200012[34]4"), &s, &p);
    assert!(d.student.is_none() && subs(&d).contains("id_ambiguous"));
    let nid = with_national_check_digit("101010101").unwrap();
    let d = decode_rosterless(&ev_grid(&nid), &s, &p);
    assert!(d.student.is_none() && subs(&d).contains("wrong_identifier"));
    assert!(!format!("{:?}", d).contains(nid.as_str()));
    let d = decode_rosterless(&ev_grid("2990001234"), &s, &p);
    assert!(
        d.student.is_none(),
        "fails the scheme's admission-year field"
    );
    // Check-digit schemes reject a mistyped digit.
    let luhn = StudentIdScheme::digits(8, Some(CheckDigit::Luhn));
    assert!(
        decode_rosterless(&ev_grid("79927398"), &luhn, &p)
            .student
            .is_none()
            || luhn.accepts("79927398")
    );
    // Second channel must agree.
    let ev = BookletEvidence {
        grid: Some(grid("2220001234")),
        typed: Some(TypedId {
            text: "2220001239".into(),
            source: TypedSource::Typed,
        }),
        ..Default::default()
    };
    let d = decode_rosterless(&ev, &s, &p);
    assert!(subs(&d).contains("channel_conflict") && d.student.is_none());
    // Duplicates across booklets.
    let items: Vec<BookletIdentity> = ["b1", "b2", "b3"]
        .iter()
        .zip(["2220001234", "2220005678", "2220001234"])
        .map(|(b, k)| BookletIdentity {
            booklet: (*b).into(),
            decision: decode_rosterless(&ev_grid(k), &s, &p),
            confirmed: None,
        })
        .collect();
    let out = assign_rosterless(&items);
    assert_eq!(out.duplicates.len(), 1);
    assert_eq!(out.duplicates[0].1.code, "IDN-005");
    assert_eq!(
        out.booklets[2].assigned,
        Assigned::Duplicate(StudentKey("2220001234".into()))
    );
    assert_eq!(out.booklets[0].status, IdentityStatus::Review);
    assert_eq!(out.booklets[1].status, IdentityStatus::AutoAccepted);
}

fn booklet(id: &str, ev: &BookletEvidence, r: &RosterSnapshot) -> BookletIdentity {
    BookletIdentity {
        booklet: id.into(),
        decision: decode(ev, r),
        confirmed: None,
    }
}

#[test]
fn req_a6_batch_duplicates_and_not_on_roster() {
    let keys = ids();
    let r = roster_of(&keys);
    let items = vec![
        booklet("01", &ev_grid(&keys[0]), &r),
        booklet("02", &ev_grid(&keys[1]), &r),
        booklet("03", &ev_grid(&keys[0]), &r), // the same student again
        booklet("04", &ev_grid("2299999999"), &r), // not on the roster
    ];
    let out = assign_batch(
        &items,
        &r,
        &PriorContext::default(),
        &IdentityPolicy::default(),
    );
    assert_eq!(out.duplicates.len(), 1);
    let (who, e) = &out.duplicates[0];
    assert_eq!(who.0, keys[0]);
    assert_eq!(
        (e.code.as_str(), e.severity, e.scope),
        ("IDN-005", Severity::Error, omr_model::Scope::Student)
    );
    let by: std::collections::BTreeMap<&str, &BookletAssignment> = out
        .booklets
        .iter()
        .map(|b| (b.booklet.as_str(), b))
        .collect();
    assert_eq!(
        by["01"].assigned,
        Assigned::Student(StudentKey(keys[0].clone()))
    );
    assert_eq!(
        by["03"].assigned,
        Assigned::Duplicate(StudentKey(keys[0].clone()))
    );
    assert_eq!(
        by["01"].status,
        IdentityStatus::Review,
        "both copies go to review"
    );
    assert_eq!(by["02"].status, IdentityStatus::NeedsConfirmation);
    assert_eq!(by["04"].assigned, Assigned::NotOnRoster);
    assert!(
        by["04"]
            .decision
            .exceptions
            .iter()
            .any(|e| e.sub_reason.as_deref() == Some("not_on_roster"))
    );
    // Batch close: everyone else is absent.
    assert_eq!(out.absent.len(), keys.len() - 2);
}

#[test]
fn req_a6_confirmed_pairs_stay_fixed_and_absent_dn_rules() {
    let keys = ids();
    let mut recs: Vec<RosterRecord> = keys.iter().map(|k| rec(k)).collect();
    recs[4].sis_status = Some(SisStatus::DeniedEntry);
    recs[5].sis_status = Some(SisStatus::Withdrawn);
    let (r, _) =
        RosterSnapshot::build(recs, &StudentIdScheme::iau(), &ImportOptions::default()).unwrap();
    let mut first = booklet("01", &ev_grid(&keys[0]), &r);
    first.confirmed = Some(StudentKey(keys[0].clone()));
    let items = vec![
        first,
        booklet("02", &ev_grid(&keys[0]), &r), // competes with a fixed pair
        booklet("03", &ev_grid(&keys[4]), &r), // DN student wrote the exam
    ];
    let out = assign_batch(
        &items,
        &r,
        &PriorContext::default(),
        &IdentityPolicy::default(),
    );
    assert_eq!(
        out.booklets[0].assigned,
        Assigned::Student(StudentKey(keys[0].clone()))
    );
    assert_ne!(
        out.booklets[1].assigned,
        Assigned::Student(StudentKey(keys[0].clone()))
    );
    assert_eq!(out.duplicates.len(), 1, "IDN-005 against the fixed pair");
    assert!(
        out.booklets[2]
            .decision
            .exceptions
            .iter()
            .any(|e| e.sub_reason.as_deref() == Some("assigned_to_absent"))
    );
    assert_eq!(out.denied_entry, vec![StudentKey(keys[4].clone())]);
    assert_eq!(out.withdrawn, vec![StudentKey(keys[5].clone())]);
    assert!(
        !out.absent.contains(&StudentKey(keys[4].clone())),
        "DN is never absent"
    );
}

#[test]
fn req_a6_assignment_changed_when_the_batch_moves_a_booklet() {
    // Booklet 01 is certain of A; booklet 02 is ambiguous between A and B (blank column).
    let a = "2220001111".to_string();
    let b = "2220001121".to_string();
    let r = roster_of(&[a.clone(), b.clone(), "2210000007".into()]);
    let items = vec![
        booklet("01", &ev_grid(&a), &r),
        booklet("02", &ev_grid("22200011_1"), &r),
    ];
    let out = assign_batch(
        &items,
        &r,
        &PriorContext::default(),
        &IdentityPolicy::default(),
    );
    assert_eq!(
        out.booklets[0].assigned,
        Assigned::Student(StudentKey(a.clone()))
    );
    assert_eq!(
        out.booklets[1].assigned,
        Assigned::Student(StudentKey(b.clone()))
    );
    if out.booklets[1].decision.student.as_ref() == Some(&StudentKey(a)) {
        assert!(
            out.booklets[1]
                .decision
                .exceptions
                .iter()
                .any(|e| e.sub_reason.as_deref() == Some("assignment_changed"))
        );
    }
    assert!(out.duplicates.is_empty());
}

#[test]
fn req_a6_batch_outcome_is_invariant_to_booklet_order() {
    let keys = ids();
    let r = roster_of(&keys);
    let mut items: Vec<BookletIdentity> = (0..12)
        .map(|i| {
            let p = match i % 4 {
                0 => keys[i].clone(),
                1 => {
                    let mut s: Vec<char> = keys[i].chars().collect();
                    s[9] = '_';
                    s.into_iter().collect()
                }
                2 => keys[0].clone(),
                _ => "2299999999".into(),
            };
            booklet(&format!("{i:02}"), &ev_grid(&p), &r)
        })
        .collect();
    let a = serde_json::to_string(&assign_batch(
        &items,
        &r,
        &PriorContext::default(),
        &IdentityPolicy::default(),
    ))
    .unwrap();
    items.reverse();
    items.swap(1, 7);
    let b = serde_json::to_string(&assign_batch(
        &items,
        &r,
        &PriorContext::default(),
        &IdentityPolicy::default(),
    ))
    .unwrap();
    assert_eq!(a, b);
}

#[test]
fn req_a6_blocked_scoring_with_a_typed_string_is_conservative() {
    // With channel B only the best rows are scored exactly and the rest is bounded: the
    // posterior can only be understated, never overstated, and the best student is the same.
    let keys = ids();
    let r = roster_of(&keys);
    for (p, t) in [
        ("22200012_4", "2220001234"),
        (keys[9].as_str(), keys[9].as_str()),
    ] {
        let ev = BookletEvidence {
            grid: Some(grid(p)),
            typed: Some(TypedId {
                text: t.into(),
                source: TypedSource::Ocr,
            }),
            ..Default::default()
        };
        let small = IdentityPolicy {
            block_k: 2,
            ..Default::default()
        };
        let big = IdentityPolicy {
            block_k: 10_000,
            ..Default::default()
        };
        let s = StudentIdScheme::iau();
        let a = decode_booklet(&ev, &r, &s, &PriorContext::default(), &small);
        let b = decode_booklet(&ev, &r, &s, &PriorContext::default(), &big);
        assert_eq!(a.student, b.student);
        assert!(
            a.logit_q.unwrap() <= b.logit_q.unwrap() + 1,
            "{:?} vs {:?}",
            a.logit_q,
            b.logit_q
        );
    }
}

#[test]
fn req_a6_roster_change_after_scanning_is_flagged() {
    let keys = ids();
    let old = roster_of(&keys[..10]);
    let read = keys[12].clone(); // not on the old roster
    let before = decode(&ev_grid(&read), &old);
    assert!(before.student.is_none());
    let new = roster_of(&keys[..20]); // the student was added later
    let mut after = decode(&ev_grid(&read), &new);
    assert_eq!(after.student.as_ref().unwrap().0, read);
    assert!(mark_roster_diff(&before, &mut after));
    assert_eq!(severity(&after, "roster_diff"), Some(Severity::Error));
    assert_eq!(after.status, IdentityStatus::Review);
    let d = old.diff(&new);
    assert_eq!(d.added.len(), 10);
}

#[test]
fn req_a10_identity_exceptions_are_catalogued() {
    let allowed = [
        "id_blank",
        "id_ambiguous",
        "not_on_roster",
        "low_posterior",
        "low_margin",
        "channel_conflict",
        "writer_error",
        "wrong_identifier",
        "wrong_field",
        "namesake",
        "assignment_changed",
        "assigned_to_absent",
        "roster_diff",
        "cross_section",
    ];
    let keys = ids();
    let r = roster_of(&keys);
    for p in [
        "2220001234",
        "22200012_4",
        "2299999999",
        "22[12]0001234",
        "??????????",
    ] {
        let d = decode(&ev_grid(p), &r);
        for e in &d.exceptions {
            assert!(omr_model::exceptions::lookup(&e.code).is_some());
            if e.code == "IDN-004" {
                assert!(allowed.contains(&e.sub_reason.as_deref().unwrap()));
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, .. ProptestConfig::default() })]

    /// A clean read of a roster ID resolves to it; a clean read of any other ID is never
    /// auto-accepted or proposed for confirmation (no nearest-neighbour acceptance).
    #[test]
    fn req_a6_never_nearest_neighbour_property(seed in any::<u64>(), pick in 0usize..60, digit in 0usize..10, delta in 1u8..10) {
        let keys = ids();
        let r = roster_of(&keys);
        let k = &keys[pick];
        let d = decode(&ev_grid(k), &r);
        prop_assert_eq!(d.student.as_ref().map(|s| s.0.as_str()), Some(k.as_str()));
        let mut v: Vec<u8> = k.bytes().collect();
        v[digit] = b'0' + ((v[digit] - b'0' + delta) % 10);
        let read = String::from_utf8(v).unwrap();
        let _ = seed;
        if !keys.contains(&read) {
            let d = decode(&ev_grid(&read), &r);
            prop_assert_eq!(d.status, IdentityStatus::Review);
            prop_assert!(d.student.is_none() || d.exceptions.iter().any(|e| e.severity == Severity::Error));
        }
    }
}
