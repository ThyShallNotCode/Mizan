//! A small hand-made dataset (no real data): CS 101 quiz, 5 single-answer items worth 2
//! points each, keys A B C D A in master order, two forms (B prints the items in reverse
//! order), seven students.
//!
//! | Student | Form | Master responses | Correct | % | Letter |
//! |---|---|---|---|---|---|
//! | 220000001 Sara Al-Harbi | A | A B C D A | 5 | 100 | A+ |
//! | 220000002 محمد العتيبي | A | A B C D B | 4 | 80 | B |
//! | 220000003 @Noura Test | B | A B C D B | 4 | 80 | B |
//! | 220000004 Omar Saleh, Jr. | A | A B C · AC | 3 | 60 | D |
//! | 220000005 خالد | B | A B A A C | 2 | 40 | F |
//! | 220000006 Lina | A | A ? C D A | 4 + pending | 80 (provisional) | B |
//! | 220000007 (absent, excused) | A | — | — | — | — |
//!
//! Mean of the six percentages = 440/6 = 73.33; median = 80.00; 5 of 6 pass (83.3 %).
#![allow(dead_code)]

use omr_analytics::{AnalysisData, AnalyticsConfig, AnalyticsReport, analyze};
use omr_model::{FormCode, StudentKey};
use omr_outcomes::{A1Table, AttainmentPolicy};
use omr_outcomes::{
    AssessmentEvidence, AttainmentResult, B1Table, Clo, CloPloMap, CloSet, Crosswalk, CrosswalkRow,
    Domain, ItemCloMap, MappingMode, MappingSet, Offering, OutcomeFramework, RosterEntry,
    SectionInfo, Track, compute_attainment, crosswalk_rows, framework_rollup, plo_course_level,
    tp154_a1, tp154_b1,
};
use omr_report::{ReportMeta, ResultsTable, StudentInfo};
use omr_scoring::{
    Adjustments, BookletL1, Exact, Form, FormMap, GradeOutcome, GradeScale, KeyEntry, KeyVersion,
    L1Response, MasterItem, PolicyVersion, ResponseMatrix, ResponseSnapshot, RosterStatus,
    RoundingPolicy, ScoreRun, StatusCode, exact, prepare, pts, score_prepared,
};
use std::collections::{BTreeMap, BTreeSet};

pub struct Fixture {
    pub run: ScoreRun,
    pub matrix: ResponseMatrix,
    pub scale: GradeScale,
    pub results: ResultsTable,
    pub analytics: AnalyticsReport,
    pub clos: CloSet,
    pub attainment: AttainmentResult,
    pub b1: B1Table,
    pub a1: A1Table,
    pub crosswalk: Vec<CrosswalkRow>,
    pub so_values: BTreeMap<String, Exact>,
    pub framework: OutcomeFramework,
    pub policy: AttainmentPolicy,
}

pub fn meta() -> ReportMeta {
    let mut m = ReportMeta::new("CS 101 Quiz 1 results", "2026-10-15T09:00:00Z");
    m.title_ar = Some("نتائج الاختبار القصير 1".into());
    m.institution = Some("Example University".into());
    m.course_code = Some("CS 101".into());
    m.course_name = Some("Introduction to Computing".into());
    m.course_name_ar = Some("مقدمة في الحوسبة".into());
    m.term = Some("1448/1 (2026-08-30 – 2026-12-24)".into());
    m.scope = Some("Sections 101, 102".into());
    m.score_run_id = Some("9f2c4e1ab37d5560c0ffee0123456789".into());
    m.key_version = Some(1);
    m.policy_version = Some(1);
    m.roster_snapshot = Some("5ad1e0c3b2a19f8e7d6c5b4a39281706".into());
    m.engine_manifest = Some("e1e2e3e4e5e6e7e8e9eaebecedeeeff0".into());
    m.chain_head = Some("c4a1b2c3d4e5f60718293a4b5c6d7e8f".into());
    m
}

fn items() -> Vec<MasterItem> {
    (1..=5)
        .map(|i| MasterItem::sba(format!("Q{i}"), 4, pts(2, 1)))
        .collect()
}

fn forms() -> FormMap {
    let items = items();
    let a = Form::identity(FormCode(1), &items);
    let b = Form::permuted(
        FormCode(2),
        &[4, 3, 2, 1, 0],
        &[
            vec![0, 1, 2, 3],
            vec![0, 1, 2, 3],
            vec![0, 1, 2, 3],
            vec![0, 1, 2, 3],
            vec![0, 1, 2, 3],
        ],
    );
    FormMap {
        items,
        forms: vec![a, b],
        per_form_normalization: false,
    }
}

fn key() -> KeyVersion {
    KeyVersion::new(vec![
        KeyEntry::single(0, 4),
        KeyEntry::single(1, 4),
        KeyEntry::single(2, 4),
        KeyEntry::single(3, 4),
        KeyEntry::single(0, 4),
    ])
}

use L1Response::{Blank, Multi, Pending, Single};

fn l1() -> ResponseSnapshot {
    let a = FormCode(1);
    let b = FormCode(2);
    let mut lina = BookletL1::present(
        6,
        Some("220000006"),
        a,
        vec![Single(0), Pending, Single(2), Single(3), Single(0)],
    );
    lina.open_exceptions = BTreeSet::from([1]);
    ResponseSnapshot {
        booklets: vec![
            BookletL1::present(
                1,
                Some("220000001"),
                a,
                vec![Single(0), Single(1), Single(2), Single(3), Single(0)],
            ),
            BookletL1::present(
                2,
                Some("220000002"),
                a,
                vec![Single(0), Single(1), Single(2), Single(3), Single(1)],
            ),
            // Form B prints Q5 Q4 Q3 Q2 Q1.
            BookletL1::present(
                3,
                Some("220000003"),
                b,
                vec![Single(1), Single(3), Single(2), Single(1), Single(0)],
            ),
            BookletL1::present(
                4,
                Some("220000004"),
                a,
                vec![Single(0), Single(1), Single(2), Blank, Multi(vec![0, 2])],
            ),
            BookletL1::present(
                5,
                Some("220000005"),
                b,
                vec![Single(2), Single(0), Single(0), Single(1), Single(0)],
            ),
            lina,
            BookletL1::absent(7, Some("220000007"), a, true),
        ],
    }
}

pub fn students() -> BTreeMap<StudentKey, StudentInfo> {
    let s = |id: &str, name: &str, sec: &str| {
        (
            StudentKey(id.into()),
            StudentInfo {
                sis_id: None,
                name: Some(name.into()),
                section: Some(sec.into()),
            },
        )
    };
    BTreeMap::from([
        s("220000001", "Sara Al-Harbi", "101"),
        s("220000002", "محمد العتيبي", "101"),
        s("220000003", "@Noura Test", "102"),
        s("220000004", "Omar Saleh, Jr.", "102"),
        s("220000005", "خالد", "101"),
        s("220000006", "Lina", "102"),
        s("220000007", "Absent Student", "101"),
    ])
}

pub fn fixture() -> Fixture {
    let fm = forms();
    let prep = prepare(&l1(), &fm).unwrap();
    let run = score_prepared(
        &prep,
        &fm,
        &key(),
        &PolicyVersion::default(),
        &Adjustments::default(),
    )
    .unwrap();
    let matrix = prep.matrix.clone();
    let scale = GradeScale::saudi();
    let rounding = RoundingPolicy::default();
    let results =
        ResultsTable::from_run(meta(), &run, &matrix, &scale, &rounding, &students()).unwrap();
    let analytics = analyze(
        &AnalysisData::from_run(&run, &matrix),
        &AnalyticsConfig::default(),
    );
    // Outcomes: CLO1 (K) ← Q1, Q2; CLO2 (S) ← Q3, Q4; CLO3 (V) ← Q5.
    let clos = CloSet {
        course: "CS 101".into(),
        version: 1,
        clos: vec![
            Clo::new(
                "CLO1",
                "Explain basic computing concepts",
                Domain::Knowledge,
            ),
            Clo::new("CLO2", "Trace simple algorithms", Domain::Skills),
            Clo::new("CLO3", "Act with academic integrity", Domain::Values),
        ],
    };
    let mapping = MappingSet {
        version: 1,
        mode: MappingMode::Full,
        rows: vec![
            ItemCloMap::approved("quiz1", 0, "CLO1", pts(1, 1)),
            ItemCloMap::approved("quiz1", 1, "CLO1", pts(1, 1)),
            ItemCloMap::approved("quiz1", 2, "CLO2", pts(1, 1)),
            ItemCloMap::approved("quiz1", 3, "CLO2", pts(1, 1)),
            ItemCloMap::approved("quiz1", 4, "CLO3", pts(1, 1)),
        ],
    };
    let roster: Vec<RosterEntry> = students()
        .iter()
        .map(|(k, i)| RosterEntry {
            student: k.clone(),
            section: i.section.clone().unwrap_or_default(),
            status: RosterStatus::Enrolled,
            passing: Some(true),
        })
        .collect();
    let offering = Offering {
        course: "CS 101".into(),
        term: "1448/1".into(),
        credit_hours: 3,
        sections: vec![
            SectionInfo {
                id: "101".into(),
                campus: "main".into(),
                track: Track::Male,
            },
            SectionInfo {
                id: "102".into(),
                campus: "main".into(),
                track: Track::Female,
            },
        ],
        roster,
        assessments: vec![AssessmentEvidence::from_run(
            "quiz1",
            "Quiz 1",
            exact(1, 1),
            &run,
        )],
    };
    let policy = AttainmentPolicy::default();
    let attainment = compute_attainment(&offering, &clos, &mapping, &policy, &[]);
    let clo_plo = vec![
        CloPloMap::new("CLO1", "K1", 3),
        CloPloMap::new("CLO2", "S1", 2),
        CloPloMap::new("CLO3", "V1", 1),
    ];
    let b1 = tp154_b1(
        &offering,
        &clos,
        &mapping,
        &clo_plo,
        &attainment,
        &policy,
        &BTreeMap::new(),
    );
    let mut grades: Vec<GradeOutcome> = run
        .grades(&scale, &rounding)
        .into_iter()
        .flatten()
        .collect();
    grades.push(GradeOutcome::Status(StatusCode::DN));
    grades.push(GradeOutcome::Status(StatusCode::W));
    let a1 = tp154_a1(&grades, &scale);
    let crosswalk_links = vec![
        Crosswalk::new("K1", "SO1", 3),
        Crosswalk::new("S1", "SO1", 2),
        Crosswalk::new("S1", "SO6", 3),
        Crosswalk::new("V1", "SO4", 2),
    ];
    let crosswalk = crosswalk_rows(&clos, &clo_plo, &crosswalk_links);
    let clo_values: BTreeMap<String, Exact> = attainment
        .clos
        .iter()
        .filter_map(|c| c.value.map(|v| (c.clo.clone(), v)))
        .collect();
    let plo_values = plo_course_level(&clo_values, &clo_plo, false);
    let so_values = framework_rollup(&plo_values, &crosswalk_links);
    Fixture {
        run,
        matrix,
        scale,
        results,
        analytics,
        clos,
        attainment,
        b1,
        a1,
        crosswalk,
        so_values,
        framework: OutcomeFramework::abet_eac_2026(),
        policy,
    }
}

/// Reads one entry of a ZIP archive (central directory; stored or deflated).
pub fn unzip(zip: &[u8], name: &str) -> Option<Vec<u8>> {
    let u16le = |o: usize| u16::from_le_bytes([zip[o], zip[o + 1]]) as usize;
    let u32le =
        |o: usize| u32::from_le_bytes([zip[o], zip[o + 1], zip[o + 2], zip[o + 3]]) as usize;
    let eocd = (0..zip.len().saturating_sub(21))
        .rev()
        .find(|&o| zip[o..o + 4] == [0x50, 0x4b, 0x05, 0x06])?;
    let count = u16le(eocd + 10);
    let mut o = u32le(eocd + 16);
    for _ in 0..count {
        if zip[o..o + 4] != [0x50, 0x4b, 0x01, 0x02] {
            return None;
        }
        let method = u16le(o + 10);
        let csize = u32le(o + 20);
        let nlen = u16le(o + 28);
        let elen = u16le(o + 30);
        let clen = u16le(o + 32);
        let local = u32le(o + 42);
        let n = std::str::from_utf8(&zip[o + 46..o + 46 + nlen]).ok()?;
        if n == name {
            let data = local + 30 + u16le(local + 26) + u16le(local + 28);
            let raw = &zip[data..data + csize];
            return match method {
                0 => Some(raw.to_vec()),
                8 => miniz_oxide::inflate::decompress_to_vec(raw).ok(),
                _ => None,
            };
        }
        o += 46 + nlen + elen + clen;
    }
    None
}

pub fn unzip_str(zip: &[u8], name: &str) -> String {
    String::from_utf8(unzip(zip, name).unwrap_or_else(|| panic!("{name} missing"))).unwrap()
}
