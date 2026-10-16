//! A8 (docs/IMPLEMENTATION.md §1): scoring oracle tests with hand-computed expected values.
//! Every expected number is worked out in the comments next to it.

use omr_model::{FormCode, Severity};
use omr_scoring::course::{
    Component, ComponentMark, CoursePolicy, CourseStudent, Requirement, RosterStatus, SisPart,
    aggregate_course, boundary_list,
};
use omr_scoring::*;
use std::collections::{BTreeMap, BTreeSet};

const A: FormCode = FormCode(1);
const B: FormCode = FormCode(2);

fn e(n: i128) -> Exact {
    Exact::from_integer(n)
}

fn sba4(label: &str) -> MasterItem {
    MasterItem::sba(label, 4, pts(1, 1))
}

fn single(v: &[u8]) -> Vec<L1Response> {
    v.iter().map(|&o| L1Response::Single(o)).collect()
}

fn codes(run: &ScoreRun) -> Vec<&str> {
    run.exceptions.iter().map(|e| e.code.as_str()).collect()
}

/// Two forms of four SBA items; form B reorders items and reverses options.
fn two_forms() -> (FormMap, KeyVersion) {
    let items = vec![sba4("Q1"), sba4("Q2"), sba4("Q3"), sba4("Q4")];
    let form_a = Form::identity(A, &items);
    let rev = vec![3u8, 2, 1, 0];
    let form_b = Form::permuted(B, &[2, 0, 3, 1], &vec![rev; 4]);
    let forms = FormMap {
        items,
        forms: vec![form_a, form_b],
        per_form_normalization: false,
    };
    // Master key: Q1 = A, Q2 = B, Q3 = C, Q4 = D.
    let key = KeyVersion::new(vec![
        KeyEntry::single(0, 4),
        KeyEntry::single(1, 4),
        KeyEntry::single(2, 4),
        KeyEntry::single(3, 4),
    ]);
    (forms, key)
}

#[test]
fn req_a8_multi_form_mapping_oracle() {
    let (forms, key) = two_forms();
    assert!(forms.validate().is_empty());
    // Derived printed key of form B (σ reverses options, so printed ℓ = 3 − master o):
    // pos 1 = Q3 (master C=2) -> printed B; pos 2 = Q1 (A=0) -> D; pos 3 = Q4 (D=3) -> A;
    // pos 4 = Q2 (B=1) -> C. Printed key B = "B D A C".
    let pk = key.printed_key(&forms, 1);
    let letters: Vec<u8> = pk
        .iter()
        .map(|e| e.as_ref().unwrap().single_key().unwrap())
        .collect();
    assert_eq!(letters, vec![1, 3, 0, 2]);

    let l1 = ResponseSnapshot {
        booklets: vec![
            // S1, form A, all correct: A B C D -> 4/4.
            BookletL1::present(1, Some("S1"), A, single(&[0, 1, 2, 3])),
            // S2, form B, the printed key B D A C -> 4/4, same master cells as S1.
            BookletL1::present(2, Some("S2"), B, single(&[1, 3, 0, 2])),
            // S3, form B, A A A A: printed A maps to master D everywhere; only Q4 (pos 3)
            // is keyed D -> 1/4 = 25 %.
            BookletL1::present(3, Some("S3"), B, single(&[0, 0, 0, 0])),
        ],
    };
    let run = score(&l1, &forms, &key, &PolicyVersion::default(), &Adjustments::default())
        .unwrap();
    let prep = prepare(&l1, &forms).unwrap();
    for i in 0..4 {
        assert_eq!(prep.matrix.raw(0, i), prep.matrix.raw(1, i), "item {i}");
    }
    assert_eq!(run.students[0].final_pct, Some(e(100)));
    assert_eq!(run.students[1].final_pct, Some(e(100)));
    assert_eq!(run.students[2].final_pct, Some(e(25)));
    assert_eq!(run.item_score(2, 3), pts(1, 1)); // Q4
    assert_eq!(run.item_score(2, 0), pts(0, 1)); // Q1
    // S3's 1/4 is at chance under both forms (E₀ = 4·¼ = 1, SD₀ = √(4·¼·¾) = 0.866,
    // threshold 2.73; read as form A, "A A A A" also scores 1): SCO-005 near_chance only.
    assert_eq!(codes(&run), vec!["SCO-005"]);
    assert_eq!(run.exceptions[0].target.booklet, Some(3));
}

/// Items for the rule oracle: partial SBA, Pick-2, select-all, multi-accept SBA.
fn rule_items() -> (FormMap, KeyVersion) {
    let items = vec![
        MasterItem::sba("Q1", 5, pts(2, 1)),
        MasterItem::multi_select("Q2", 5, SelectMode::PickN(2), pts(3, 1)),
        MasterItem::multi_select("Q3", 5, SelectMode::SelectAll, pts(3, 1)),
        MasterItem::sba("Q4", 4, pts(1, 1)),
    ];
    let key = KeyVersion::new(vec![
        KeyEntry::single(2, 5).with_credit(3, pts(1, 2)), // C = 1, D = ½
        KeyEntry::set(&[0, 2]),                           // {A, C}
        KeyEntry::set(&[0, 1, 2]),                        // {A, B, C}
        KeyEntry::any_of(&[0, 1], 4),                     // A or B
    ]);
    (FormMap::single_form(items), key)
}

fn rule_snapshot() -> ResponseSnapshot {
    use L1Response::*;
    ResponseSnapshot {
        booklets: vec![
            BookletL1::present(
                1,
                Some("S1"),
                A,
                vec![Single(3), Set(vec![0, 1]), Set(vec![0, 1, 3]), Single(1)],
            ),
            BookletL1::present(
                2,
                Some("S2"),
                A,
                vec![Single(0), Set(vec![0, 2]), Set(vec![0, 1, 2]), Single(2)],
            ),
            BookletL1::present(3, Some("S3"), A, vec![Blank, Blank, Blank, Blank]),
            BookletL1::present(
                4,
                Some("S4"),
                A,
                vec![Multi(vec![2, 3]), Set(vec![0, 1, 2]), Set(vec![3, 4]), Single(0)],
            ),
        ],
    }
}

#[test]
fn req_a8_item_rules_oracle_default_policy() {
    let (forms, key) = rule_items();
    let run = score(
        &rule_snapshot(),
        &forms,
        &key,
        &PolicyVersion::default(),
        &Adjustments::default(),
    )
    .unwrap();
    // M = 2 + 3 + 3 + 1 = 9.
    // S1: Q1 D = ½ -> 2·½ = 1. Q2 {A,B}: TP 1, FP 1, t 2, PS1/t: TP+FP = 2 <= 2 -> ½ -> 1.5.
    //     Q3 {A,B,D}: TP 2, FP 1, t 3, m 5, plus/minus: 2/3 − 1/2 = 1/6 -> 0.5. Q4 B -> 1.
    //     Total 4/9 -> x% = 400/9 = 44.44…
    assert_eq!(run.item_score(0, 0), pts(1, 1));
    assert_eq!(run.item_score(0, 1), pts(3, 2));
    assert_eq!(run.item_score(0, 2), pts(1, 2));
    assert_eq!(run.item_score(0, 3), pts(1, 1));
    assert_eq!(run.students[0].raw, e(4));
    assert_eq!(run.students[0].final_pct, Some(exact(400, 9)));
    // S2: Q1 A wrong 0; Q2 exact 3; Q3 exact 3; Q4 C wrong 0 -> 6/9 = 200/3 %.
    assert_eq!(run.students[1].final_pct, Some(exact(200, 3)));
    // S3: all blank -> 0 %.
    assert_eq!(run.students[2].final_pct, Some(e(0)));
    assert_eq!(run.item_state(2, 1), ItemState::Blank);
    // S4: Q1 confirmed multi {C, D}: invalid = 0. Q2 {A,B,C}: 3 marks > t = 2 -> PS1/t 0.
    //     Q3 {D,E}: TP 0, FP 2 -> max(0, 0 − 2/2) = 0. Q4 A -> 1. Total 1/9 -> 100/9 %.
    assert_eq!(run.item_score(3, 0), pts(0, 1));
    assert_eq!(run.item_score(3, 1), pts(0, 1));
    assert_eq!(run.students[3].final_pct, Some(exact(100, 9)));
}

#[test]
fn req_a8_item_rules_oracle_formula_scoring() {
    let (forms, key) = rule_items();
    let policy = PolicyVersion {
        negative: NegativeMarking::Formula,
        multi_on_sba: MultiOnSba::PartialMaxOverCount,
        ..PolicyVersion::default()
    };
    let run = score(&rule_snapshot(), &forms, &key, &policy, &Adjustments::default()).unwrap();
    // Formula scoring: Q1 (a = 1, m = 5) wrong = −1/4 · M = −½; Q4 (a = 2, m = 4)
    // wrong = −2/2 · M = −1. Partial options keep their credit (Q1 D = ½ -> 1).
    // S1 unchanged: 4/9.
    assert_eq!(run.students[0].final_pct, Some(exact(400, 9)));
    // S2: −½ + 3 + 3 − 1 = 4.5 -> 4.5/9 = 50 %.
    assert_eq!(run.item_score(1, 0), pts(-1, 2));
    assert_eq!(run.item_score(1, 3), pts(-1, 1));
    assert_eq!(run.students[1].raw, exact(9, 2));
    assert_eq!(run.students[1].final_pct, Some(e(50)));
    // S3 blanks score 0, no penalty.
    assert_eq!(run.students[2].final_pct, Some(e(0)));
    // S4: Q1 multi {C, D} partial = max(1, ½)/2 = ½ -> 2·½ = 1; Q4 A -> 1. Total 2/9.
    assert_eq!(run.item_score(3, 0), pts(1, 1));
    assert_eq!(run.students[3].final_pct, Some(exact(200, 9)));
    // Blind guessing gains 0 in expectation: Σ_o credit over Q1's options =
    // 1 + ½ + 3·(−¼) ... the formula rule is defined for a = #full-credit options:
    // a·1 + (m − a)·(−a/(m − a)) = 0 on an unpartialled item (Q4: 2·1 + 2·(−1) = 0).
    let q4: Points = (0..4).map(|o| run_single(&forms, &key, &policy, 3, o)).sum();
    assert_eq!(q4, pts(0, 1));
}

fn run_single(forms: &FormMap, key: &KeyVersion, policy: &PolicyVersion, item: usize, o: u8) -> Points {
    use L1Response::*;
    let mut r = vec![Blank; forms.items.len()];
    r[item] = Single(o);
    let l1 = ResponseSnapshot {
        booklets: vec![BookletL1::present(1, None, A, r)],
    };
    score(&l1, forms, key, policy, &Adjustments::default())
        .unwrap()
        .item_score(0, item)
}

#[test]
fn req_a8_exact_rationals_point_three() {
    // Three items worth 0.3 and one worth 0.1 (M = 1.0). Three 0.3 items right:
    // 0.3 + 0.3 + 0.3 = 0.9 exactly -> 90 % -> A (in f64: 89.99999999999999 -> B+).
    let items = vec![
        MasterItem::sba("Q1", 4, parse_points("0.3").unwrap()),
        MasterItem::sba("Q2", 4, parse_points("0.3").unwrap()),
        MasterItem::sba("Q3", 4, parse_points("0.3").unwrap()),
        MasterItem::sba("Q4", 4, parse_points("0.1").unwrap()),
    ];
    let forms = FormMap::single_form(items);
    let key = KeyVersion::new((0..4).map(|_| KeyEntry::single(0, 4)).collect());
    let l1 = ResponseSnapshot {
        booklets: vec![BookletL1::present(1, None, A, single(&[0, 0, 0, 1]))],
    };
    let run = score(&l1, &forms, &key, &PolicyVersion::default(), &Adjustments::default())
        .unwrap();
    assert_eq!(run.students[0].final_pct, Some(e(90)));
    let unrounded = RoundingPolicy {
        letter_basis: LetterBasis::Unrounded,
        ..RoundingPolicy::default()
    };
    let g = run.grades(&GradeScale::saudi(), &unrounded);
    assert_eq!(g[0].as_ref().unwrap().code(), "A");
    let f64_sum: f64 = 0.3 + 0.3 + 0.3;
    assert!(f64_sum < 0.9, "the f64 path would have mis-graded this student");
}

#[test]
fn req_a8_saudi_grade_scale() {
    let s = GradeScale::saudi();
    assert!(s.validate().is_empty());
    let cases: [(Exact, &str, &str, Points, Points); 12] = [
        (e(100), "A+", "أ+", pts(5, 1), pts(4, 1)),
        (e(95), "A+", "أ+", pts(5, 1), pts(4, 1)),
        (exact(9499, 100), "A", "أ", pts(475, 100), pts(375, 100)),
        (e(90), "A", "أ", pts(475, 100), pts(375, 100)),
        (e(85), "B+", "ب+", pts(450, 100), pts(350, 100)),
        (e(80), "B", "ب", pts(4, 1), pts(3, 1)),
        (e(75), "C+", "ج+", pts(350, 100), pts(250, 100)),
        (e(70), "C", "ج", pts(3, 1), pts(2, 1)),
        (e(65), "D+", "د+", pts(250, 100), pts(150, 100)),
        (e(60), "D", "د", pts(2, 1), pts(1, 1)),
        (exact(5999, 100), "F", "هـ", pts(1, 1), pts(0, 1)),
        (e(-3), "F", "هـ", pts(1, 1), pts(0, 1)),
    ];
    for (x, l, ar, g5, g4) in cases {
        let b = s.band_for(&x).unwrap();
        assert_eq!((b.letter.as_str(), b.arabic.as_str(), b.gpa5, b.gpa4), (l, ar, g5, g4));
    }
    // Rounding before the letter (default letter_basis = Rounded, 2 dp, half away from
    // zero): 89.995 -> 90.00 -> A; unrounded it is B+.
    let p = RoundingPolicy::default();
    let x = exact(89_995, 1000);
    assert_eq!(s.band_for(&p.round(&x)).unwrap().letter, "A");
    assert_eq!(s.band_for(&x).unwrap().letter, "B+");
    // A gap or an overlap is SCO-029.
    let mut gap = s.clone();
    gap.bands[1].lo = Some(e(61)); // D starts at 61, F ends at 60: gap
    assert_eq!(gap.validate()[0].code, "SCO-029");
    let mut overlap = s.clone();
    overlap.bands[1].lo = Some(e(59));
    assert_eq!(overlap.validate()[0].code, "SCO-029");
    // DN counts as F on both GPA scales.
    assert_eq!(
        GradeOutcome::Status(StatusCode::DN).gpa5(),
        Some(pts(1, 1))
    );
    assert_eq!(GradeOutcome::Status(StatusCode::DN).gpa4(), Some(pts(0, 1)));
    assert_eq!(StatusCode::DN.a1_column(), Some(A1Column::DeniedEntry));
    assert_eq!(StatusCode::E.a1_column(), None);
}

fn course_policy() -> CoursePolicy {
    CoursePolicy::new(vec![
        Component {
            id: "quiz".into(),
            weight: pts(10, 1),
            sis_part: SisPart::Coursework,
            required: None,
        },
        Component {
            id: "mid".into(),
            weight: pts(30, 1),
            sis_part: SisPart::Coursework,
            required: None,
        },
        Component {
            id: "final".into(),
            weight: pts(60, 1),
            sis_part: SisPart::Final,
            required: None,
        },
    ])
}

fn student(id: &str, status: RosterStatus, marks: &[(&str, ComponentMark)]) -> CourseStudent {
    CourseStudent {
        student: omr_model::StudentKey(id.into()),
        status,
        marks: marks
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
        extra_credit_pp: Exact::default(),
    }
}

#[test]
fn req_a8_course_split_and_status_codes() {
    use ComponentMark::*;
    let policy = course_policy();
    let scale = GradeScale::saudi();
    let students = vec![
        // S1 (the §11.3 worked example): quiz 15/20 = 75, mid 26/40 = 65, final 64.
        // Coursework = 0.1·75 + 0.3·65 = 7.5 + 19.5 = 27.00; final = 0.6·64 = 38.40;
        // total 65.40 -> D+.
        student(
            "S1",
            RosterStatus::Enrolled,
            &[("quiz", Scored(e(75))), ("mid", Scored(e(65))), ("final", Scored(e(64)))],
        ),
        // S2 denied entry: coursework as recorded 0.1·80 + 0.3·70 = 29; no final; DN = F.
        student(
            "S2",
            RosterStatus::Denied,
            &[("quiz", Scored(e(80))), ("mid", Scored(e(70)))],
        ),
        // S3 withdrawn.
        student("S3", RosterStatus::Withdrawn, &[]),
        // S4 excused quiz: renormalize over 90: mid 30/90·100·0.9 ... W′ = 33⅓, 66⅔;
        // coursework = 33⅓·90/100 = 30, final = 66⅔·90/100 = 60 -> 90 -> A.
        student(
            "S4",
            RosterStatus::Enrolled,
            &[
                ("quiz", Absent { excused: true }),
                ("mid", Scored(e(90))),
                ("final", Scored(e(90))),
            ],
        ),
        // S5 unexcused absence from the final: 0 -> 10 + 30 + 0 = 40 -> F.
        student(
            "S5",
            RosterStatus::Enrolled,
            &[
                ("quiz", Scored(e(100))),
                ("mid", Scored(e(100))),
                ("final", Absent { excused: false }),
            ],
        ),
        // S6: final 83.325 -> 0.6·83.325 = 49.995 -> rounds to 50.00; coursework 40.00;
        // SIS sum 90.00 -> A (unrounded 89.995 would be B+): a boundary student.
        student(
            "S6",
            RosterStatus::Enrolled,
            &[
                ("quiz", Scored(e(100))),
                ("mid", Scored(e(100))),
                ("final", Scored(exact(83_325, 1000))),
            ],
        ),
        // S7: quiz and mid excused = 40 % > ⅓ -> IC.
        student(
            "S7",
            RosterStatus::Enrolled,
            &[
                ("quiz", Absent { excused: true }),
                ("mid", NotScorable),
                ("final", Scored(e(70))),
            ],
        ),
    ];
    let r = aggregate_course(&policy, &students, &scale).unwrap();
    assert_eq!(r[0].coursework, Some(e(27)));
    assert_eq!(r[0].final_mark, Some(exact(384, 10)));
    assert_eq!(r[0].total, Some(exact(654, 10)));
    assert_eq!(r[0].grade.code(), "D+");
    assert_eq!(r[1].grade, GradeOutcome::Status(StatusCode::DN));
    assert_eq!(r[1].coursework, Some(e(29)));
    assert_eq!(r[1].final_mark, None);
    assert_eq!(r[1].grade.gpa5(), Some(pts(1, 1)));
    assert_eq!(r[2].grade.code(), "W");
    assert_eq!(r[3].coursework, Some(e(30)));
    assert_eq!(r[3].final_mark, Some(e(60)));
    assert_eq!(r[3].grade.code(), "A");
    assert_eq!(r[4].total, Some(e(40)));
    assert_eq!(r[4].grade.code(), "F");
    assert_eq!(r[5].total, Some(exact(89_995, 1000)));
    assert_eq!(r[5].final_mark, Some(e(50)));
    assert_eq!(r[5].total_rounded, Some(e(90)));
    assert_eq!(r[5].grade.code(), "A");
    assert_eq!(r[6].grade.code(), "IC");
    // Boundary list (δ = 0.5 pp): S1 (65.4 vs 65), S4 (exactly 90) and S6 (89.995 vs 90).
    let bl = boundary_list(&r, &scale, &exact(1, 2));
    let listed: BTreeSet<&str> = bl
        .iter()
        .map(|x| x.detail.split(':').next().unwrap())
        .collect();
    assert_eq!(listed, BTreeSet::from(["student S1", "student S4", "student S6"]));
    assert!(bl.iter().all(|x| x.code == "SCO-017" && x.severity == Severity::Info));

    // Σ W_a = 99 -> SCO-010 (BLOCKER, Course scope): L5 refused.
    let mut bad = course_policy();
    bad.components[0].weight = pts(9, 1);
    let err = aggregate_course(&bad, &students, &scale).unwrap_err();
    assert_eq!(err[0].code, "SCO-010");

    // A required minimum on the final (≥ 50) caps the letter at D: S4-like student with
    // mid 100, quiz 100, final 45 -> 10 + 30 + 27 = 67 -> D+ capped to D.
    let mut req = course_policy();
    req.components[2].required = Some(Requirement {
        min_pct: e(50),
        letter_cap: "D".into(),
    });
    let s = student(
        "S8",
        RosterStatus::Enrolled,
        &[("quiz", Scored(e(100))), ("mid", Scored(e(100))), ("final", Scored(e(45)))],
    );
    let r = aggregate_course(&req, &[s.clone()], &scale).unwrap();
    assert_eq!(r[0].total, Some(e(67)));
    assert!(r[0].requirement_not_met);
    assert_eq!(r[0].grade.code(), "D");
    // Pass/fail course: 67 -> NP; 40 -> NF.
    let mut pf = course_policy();
    pf.pass_fail = true;
    let r = aggregate_course(&pf, &[s, students[4].clone()], &scale).unwrap();
    assert_eq!(r[0].grade.code(), "NP");
    assert_eq!(r[1].grade.code(), "NF");
}

#[test]
fn req_a8_publish_validation_codes() {
    let (forms, key) = two_forms();
    // Missing key entry -> SCO-003 BLOCKER, run refused.
    let mut missing = key.clone();
    missing.entries[2] = None;
    let l1 = ResponseSnapshot {
        booklets: vec![BookletL1::present(1, None, A, single(&[0, 1, 2, 3]))],
    };
    match score(&l1, &forms, &missing, &PolicyVersion::default(), &Adjustments::default()) {
        Err(ScoringError::Refused(ex)) => assert_eq!(ex[0].code, "SCO-003"),
        other => panic!("expected refusal, got {other:?}"),
    }
    // Blank keyed item (no option earns credit) -> SCO-003.
    let blank = key.revise([(0, KeyEntry::Credits { credits: vec![pts(0, 1); 4], multi_accept: false })], "test");
    assert!(blank.validate(&forms).iter().any(|e| e.code == "SCO-003"));
    // Key option >= m_i -> SCO-004.
    let wide = key.revise([(1, KeyEntry::single(0, 5))], "test");
    assert!(wide.validate(&forms).iter().any(|e| e.code == "SCO-004"));
    // σ not a bijection -> SCO-004; π_f not injective -> SCO-004.
    let mut bad = forms.clone();
    bad.forms[1].positions[0].option_map = vec![0, 0, 1, 2];
    assert!(bad.validate().iter().any(|e| e.code == "SCO-004"));
    let mut dup = forms.clone();
    dup.forms[1].positions[1].item = 2;
    assert!(dup.validate().iter().any(|e| e.code == "SCO-004"));
    // Unequal form totals without normalization -> SCO-006 (ERROR; run continues).
    let mut unequal = forms.clone();
    unequal.forms[1].positions.pop();
    let ex = unequal.validate();
    assert!(ex.iter().any(|e| e.code == "SCO-006" && e.severity == Severity::Error));
    // Two full-credit options without multi-accept -> SCO-027 WARN.
    let multi = key.revise(
        [(0, KeyEntry::single(0, 4).with_credit(1, pts(1, 1)))],
        "test",
    );
    assert!(multi.validate(&forms).iter().any(|e| e.code == "SCO-027"));
    // Run of 5 identical letters -> SCO-027.
    let items: Vec<MasterItem> = (0..6).map(|i| sba4(&format!("Q{i}"))).collect();
    let f6 = FormMap::single_form(items);
    let k6 = KeyVersion::new((0..6).map(|_| KeyEntry::single(2, 4)).collect());
    assert!(k6.validate(&f6).iter().any(|e| e.code == "SCO-027"));
    // Unequal M_i in a choice group -> SCO-004 `choice_group`.
    let mut items = vec![sba4("Q1"), sba4("Q2")];
    items[1].max_points = pts(2, 1);
    let fm = FormMap::single_form(items);
    let pol = PolicyVersion {
        choice_groups: vec![ChoiceGroup {
            name: "G".into(),
            items: vec![0, 1],
            count: 1,
            mode: ChoiceMode::Best,
        }],
        ..PolicyVersion::default()
    };
    let ex = pol.lint(&fm);
    assert_eq!(ex[0].code, "SCO-004");
    assert_eq!(ex[0].sub_reason.as_deref(), Some("choice_group"));
    // Negative marking with no floor -> SCO-019.
    let pol = PolicyVersion {
        negative: NegativeMarking::Formula,
        floors: Floors::NONE,
        ..PolicyVersion::default()
    };
    assert!(pol.lint(&fm).iter().any(|e| e.code == "SCO-019"));
}

#[test]
fn req_a8_unreadable_and_absent_exceptions() {
    use L1Response::*;
    let (forms, key) = two_forms();
    let mut covered = BookletL1::present(2, Some("S2"), A, vec![Single(0), Unreadable, Single(2), Single(3)]);
    covered.open_exceptions.insert(1);
    let mut absent_bubble = BookletL1::present(3, Some("S3"), A, single(&[0, 1, 0, 0]));
    absent_bubble.absent_bubble = true;
    let l1 = ResponseSnapshot {
        booklets: vec![
            // Unreadable with no open exception: SCO-008, item pending, provisional.
            BookletL1::present(1, Some("S1"), A, vec![Single(0), Unreadable, Single(2), Single(3)]),
            // Unreadable covered by an open MRK exception: pending, no SCO-008.
            covered,
            // Absent bubble but answers: SCO-031, scored (2/4), never auto-zero.
            absent_bubble,
            // On the absent list, no answers: Absent, not 0.
            BookletL1::absent(4, Some("S4"), A, true),
            BookletL1::absent(5, Some("S5"), A, false),
        ],
    };
    let run = score(&l1, &forms, &key, &PolicyVersion::default(), &Adjustments::default())
        .unwrap();
    let sco008: Vec<_> = run.exceptions.iter().filter(|e| e.code == "SCO-008").collect();
    assert_eq!(sco008.len(), 1);
    assert_eq!(sco008[0].target.booklet, Some(1));
    assert_eq!(sco008[0].target.question, Some(1));
    assert_eq!(run.item_state(0, 1), ItemState::Pending);
    assert_eq!(run.students[0].status, AssessmentStatus::Provisional);
    assert_eq!(run.students[1].status, AssessmentStatus::Provisional);
    let sco031: Vec<_> = run.exceptions.iter().filter(|e| e.code == "SCO-031").collect();
    assert_eq!(sco031.len(), 1);
    assert_eq!(run.students[2].final_pct, Some(e(50)));
    assert_eq!(run.students[3].status, AssessmentStatus::Absent { excused: true });
    assert_eq!(run.students[4].status, AssessmentStatus::Absent { excused: false });
    assert_eq!(run.students[4].final_pct, None);
    assert!(run.blocks_release());
}

#[test]
fn req_a8_l4_voids_bonus_choice_groups_overrides() {
    // Five SBA items (M = 1) and a bonus item Q6 (M = 1); key A everywhere.
    let items: Vec<MasterItem> = (1..=6).map(|i| sba4(&format!("Q{i}"))).collect();
    let forms = FormMap::single_form(items);
    let key = KeyVersion::new((0..6).map(|_| KeyEntry::single(0, 4)).collect());
    let mut policy = PolicyVersion::default();
    policy.bonus.insert(5);
    // S1: right, right, wrong, right, wrong, bonus right.
    // S2: everything right (incl. bonus).
    let l1 = ResponseSnapshot {
        booklets: vec![
            BookletL1::present(1, Some("S1"), A, single(&[0, 0, 1, 0, 1, 0])),
            BookletL1::present(2, Some("S2"), A, single(&[0, 0, 0, 0, 0, 0])),
        ],
    };
    // No adjustments: S1 = (3 + 1 bonus)/5 = 80 %; S2 = 6/5 = 120 % at step 3, capped to
    // 100 % at step 5 only.
    let run = score(&l1, &forms, &key, &policy, &Adjustments::default()).unwrap();
    assert_eq!(run.students[0].step3, Some(e(80)));
    assert_eq!(run.students[1].step3, Some(e(120)));
    assert_eq!(run.students[1].final_pct, Some(e(100)));
    // Void Q2 (drop): S1 counted Q1, Q3, Q4, Q5 -> max 4, raw 1 + 0 + 1 + 0 + bonus 1 = 3
    // -> 75 %.
    let drop = Adjustments {
        voids: vec![Void { item: 1, mode: VoidMode::Drop, reason: "KEY_AMBIGUOUS".into() }],
        ..Adjustments::default()
    };
    let run = score(&l1, &forms, &key, &policy, &drop).unwrap();
    assert_eq!(run.students[0].final_pct, Some(e(75)));
    assert_eq!(run.counted(0, 1), Counted::Voided);
    // Void Q3 with full credit to all: S1 raw 1 + 1 + 1 + 1 + 0 + 1 = 5 of 5 -> 100 %.
    let full = Adjustments {
        voids: vec![Void { item: 2, mode: VoidMode::FullCredit, reason: "PRINT_ERROR".into() }],
        ..Adjustments::default()
    };
    let run = score(&l1, &forms, &key, &policy, &full).unwrap();
    assert_eq!(run.students[0].final_pct, Some(e(100)));
    assert_eq!(run.counted(0, 2), Counted::VoidedCredited);
    // Choice group "best 1 of {Q4, Q5}" and Q3 excused for S1:
    // counted Q1, Q2, Q4 (Q5 unselected) -> max 3, raw 1 + 1 + 1 + bonus 1 = 4 -> 133⅓ %
    // at step 3, 100 % after the cap.
    let mut cg = policy.clone();
    cg.choice_groups.push(ChoiceGroup {
        name: "G".into(),
        items: vec![3, 4],
        count: 1,
        mode: ChoiceMode::Best,
    });
    let exc = Adjustments {
        excused: BTreeMap::from([(omr_model::BookletSerial(1), BTreeSet::from([2]))]),
        ..Adjustments::default()
    };
    let run = score(&l1, &forms, &key, &cg, &exc).unwrap();
    assert_eq!(run.students[0].step3, Some(exact(400, 3)));
    assert_eq!(run.counted(0, 4), Counted::Unselected);
    assert_eq!(run.counted(0, 2), Counted::Excused);
    // Frozen additive curve +5 pp, an override of −10 pp on S1 and hold-harmless at 90:
    // S1 80 -> 85 (step 5) -> 75 (step 7) -> max(75, 90) = 90 (step 8).
    let adj = Adjustments {
        curve: Some(Curve::Frozen(FrozenCurve::Linear { slope: e(1), intercept: e(5) })),
        overrides: vec![IndividualOverride {
            booklet: omr_model::BookletSerial(1),
            delta_pp: e(-10),
            reason: "MISCONDUCT".into(),
            approver: "chair".into(),
        }],
        hold_harmless: BTreeMap::from([(omr_model::BookletSerial(1), e(90))]),
        ..Adjustments::default()
    };
    let run = score(&l1, &forms, &key, &policy, &adj).unwrap();
    assert_eq!(run.students[0].step5, Some(e(85)));
    assert_eq!(run.students[0].final_pct, Some(e(90)));
    // A curve that lowers a score raises SCO-016 (no-decrease, step 6).
    let lower = Adjustments {
        curve: Some(Curve::Frozen(FrozenCurve::Linear { slope: e(1), intercept: e(-5) })),
        ..Adjustments::default()
    };
    let run = score(&l1, &forms, &key, &policy, &lower).unwrap();
    assert!(run.exceptions.iter().any(|x| x.code == "SCO-016"));
    // A fitted additive curve to μ_t = 95 on the eligible cohort {80, 120}: μ = 100,
    // shift −5 -> S1 75 < 80 -> SCO-016; the fitted numbers are frozen in the run.
    let fit = Adjustments {
        curve: Some(Curve::Fit(CurveSpec::AdditiveShift { target_mean: e(95) })),
        ..Adjustments::default()
    };
    let run = score(&l1, &forms, &key, &policy, &fit).unwrap();
    assert_eq!(run.curve, Some(FrozenCurve::Linear { slope: e(1), intercept: e(-5) }));
}

#[test]
fn req_a8_floors_and_penalties() {
    // Three 4-option items (M = 1) under formula scoring: wrong = −1/3.
    let items: Vec<MasterItem> = (1..=3).map(|i| sba4(&format!("Q{i}"))).collect();
    let forms = FormMap::single_form(items);
    let key = KeyVersion::new((0..3).map(|_| KeyEntry::single(0, 4)).collect());
    let l1 = ResponseSnapshot {
        booklets: vec![BookletL1::present(1, None, A, single(&[1, 1, 0]))],
    };
    // Raw = −1/3 − 1/3 + 1 = 1/3 -> x% = 100/9.
    let formula = PolicyVersion {
        negative: NegativeMarking::Formula,
        ..PolicyVersion::default()
    };
    let run = score(&l1, &forms, &key, &formula, &Adjustments::default()).unwrap();
    assert_eq!(run.students[0].raw, exact(1, 3));
    assert_eq!(run.students[0].final_pct, Some(exact(100, 9)));
    // All wrong: −1 -> total floor 0.
    let l1w = ResponseSnapshot {
        booklets: vec![BookletL1::present(1, None, A, single(&[1, 1, 1]))],
    };
    let run = score(&l1w, &forms, &key, &formula, &Adjustments::default()).unwrap();
    assert_eq!(run.students[0].raw, e(0));
    // A fixed penalty of ¼ with no floor: −¼·3 = −¾ -> −25 %, clamped to 0 at step 5.
    let pen = PolicyVersion {
        negative: NegativeMarking::Penalty { default: pts(1, 4), per_item: BTreeMap::new() },
        floors: Floors::NONE,
        ..PolicyVersion::default()
    };
    let run = score(&l1w, &forms, &key, &pen, &Adjustments::default()).unwrap();
    assert_eq!(run.students[0].step3, Some(e(-25)));
    assert_eq!(run.students[0].final_pct, Some(e(0)));
    assert!(run.exceptions.iter().any(|x| x.code == "SCO-019"));
}

#[test]
fn req_a8_not_reached_row_shift_near_chance() {
    // Eight SBA items, key A B C D A B C D, in one speeded section.
    let items: Vec<MasterItem> = (1..=8).map(|i| sba4(&format!("Q{i}"))).collect();
    let forms = FormMap::single_form(items);
    let key = KeyVersion::new((0..8).map(|i| KeyEntry::single((i % 4) as u8, 4)).collect());
    let policy = PolicyVersion {
        sections: vec![Section { name: "S".into(), items: (0..8).collect(), speeded: true }],
        ..PolicyVersion::default()
    };
    use L1Response::*;
    let l1 = ResponseSnapshot {
        booklets: vec![
            // Answers the first five correctly, then stops: 3 not reached (SCO-024).
            BookletL1::present(
                1,
                Some("S1"),
                A,
                vec![Single(0), Single(1), Single(2), Single(3), Single(0), Blank, Blank, Blank],
            ),
            // Every answer is the next item's key (shift +1): B C D A B C D A.
            // h₊₁ = 7 >= 5 and h₀ = 0 -> MRK-012. Score 0/8 -> also near chance.
            BookletL1::present(2, Some("S2"), A, single(&[1, 2, 3, 0, 1, 2, 3, 0])),
        ],
    };
    let run = score(&l1, &forms, &key, &policy, &Adjustments::default()).unwrap();
    assert_eq!(run.item_state(0, 5), ItemState::NotReached);
    assert_eq!(run.item_state(0, 7), ItemState::NotReached);
    assert_eq!(run.students[0].final_pct, Some(exact(500, 8)));
    let by = |c: &str| run.exceptions.iter().filter(|e| e.code == c).count();
    assert_eq!(by("SCO-024"), 1);
    assert_eq!(by("MRK-012"), 1);
    // E₀ = 8·¼ = 2; SD₀ = √(8·¼·¾) = 1.2247; threshold 4.449. S1 scores 5 (not flagged);
    // S2 scores 0 under the only form -> SCO-005 near_chance.
    let nc: Vec<_> = run.exceptions.iter().filter(|e| e.code == "SCO-005").collect();
    assert_eq!(nc.len(), 1);
    assert_eq!(nc[0].target.booklet, Some(2));
    assert_eq!(nc[0].sub_reason.as_deref(), Some("near_chance"));
}

#[test]
fn req_a8_key_revision_is_a_new_run_and_rescore_is_idempotent() {
    let (forms, key) = two_forms();
    let l1 = ResponseSnapshot {
        booklets: vec![
            BookletL1::present(1, Some("S1"), A, single(&[0, 1, 2, 2])),
            BookletL1::present(2, Some("S2"), A, single(&[0, 1, 2, 3])),
        ],
    };
    let pol = PolicyVersion::default();
    let adj = Adjustments::default();
    let r1 = score(&l1, &forms, &key, &pol, &adj).unwrap();
    let r2 = score(&l1, &forms, &key, &pol, &adj).unwrap();
    assert_eq!(r1, r2, "identical regrade must be a no-op");
    // Accept key ∪ {C} on Q4 (master D or C): S1 rises 75 -> 100, S2 unchanged.
    let k2 = key.revise([(3, KeyEntry::any_of(&[2, 3], 4))], "KEY_ERROR");
    assert_eq!(k2.version, 2);
    let r3 = score(&l1, &forms, &k2, &pol, &adj).unwrap();
    assert_ne!(r1.run_id, r3.run_id);
    assert_eq!(r1.students[0].final_pct, Some(e(75)));
    assert_eq!(r3.students[0].final_pct, Some(e(100)));
    assert_eq!(r3.students[1].final_pct, Some(e(100)));
    assert_eq!(r3.key_version, 2);
}

#[test]
fn req_a8_score_box_points_and_mrk013() {
    let items = vec![
        MasterItem::score_box("Essay", pts(10, 1)),
        MasterItem::sba("Q1", 4, pts(5, 1)),
    ];
    let forms = FormMap::single_form(items);
    let key = KeyVersion::new(vec![KeyEntry::Open, KeyEntry::single(0, 4)]);
    use L1Response::*;
    let l1 = ResponseSnapshot {
        booklets: vec![
            // 7.5/10 + 5/5 = 12.5/15 -> 250/3 %.
            BookletL1::present(1, None, A, vec![Points(parse_points("7.5").unwrap()), Single(0)]),
            // 11 > M = 10 -> MRK-013, item pending.
            BookletL1::present(2, None, A, vec![Points(pts(11, 1)), Single(0)]),
        ],
    };
    let run = score(&l1, &forms, &key, &PolicyVersion::default(), &Adjustments::default())
        .unwrap();
    assert_eq!(run.item_score(0, 0), pts(15, 2));
    assert_eq!(run.students[0].final_pct, Some(exact(250, 3)));
    assert!(run.exceptions.iter().any(|x| x.code == "MRK-013"));
    assert_eq!(run.students[1].status, AssessmentStatus::Provisional);
}
