//! Golden and oracle tests of every export on the hand-made fixture (no real data).
//!
//! Goldens live in `tests/golden/`; regenerate with `OMR_UPDATE_GOLDEN=1` only together with
//! evidence that the change is intended (docs/IMPLEMENTATION.md §4). Hand-computed values are
//! asserted separately so a regenerated golden cannot hide a wrong number.

mod fixture;

use fixture::*;
use omr_report::{
    AbetSummary, CourseInput, Headers, HtmlReport, ItemAnalysis, Language, Tp154, Tp154Labels,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn golden(name: &str, actual: &[u8]) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os("OMR_UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let want = std::fs::read(&path)
        .unwrap_or_else(|_| panic!("missing golden {name}; run with OMR_UPDATE_GOLDEN=1"));
    if want != actual {
        let a = String::from_utf8_lossy(actual);
        let w = String::from_utf8_lossy(&want);
        let line = a
            .lines()
            .zip(w.lines())
            .position(|(x, y)| x != y)
            .unwrap_or(0);
        panic!(
            "{name} differs from its golden at line {}:\n  got:  {:?}\n  want: {:?}",
            line + 1,
            a.lines().nth(line),
            w.lines().nth(line)
        );
    }
}

fn csv_rows(bytes: &[u8]) -> Vec<Vec<String>> {
    assert!(
        bytes.starts_with(b"\xEF\xBB\xBF"),
        "CSV must start with a UTF-8 BOM"
    );
    let mut r = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(&bytes[3..]);
    r.records()
        .map(|x| x.unwrap().iter().map(str::to_owned).collect())
        .collect()
}

fn row<'a>(rows: &'a [Vec<String>], id: &str) -> &'a Vec<String> {
    rows.iter().find(|r| r[0] == id).unwrap()
}

#[test]
fn req_a8_results_csv_letters_on_the_saudi_scale() {
    let f = fixture();
    let mut out = Vec::new();
    f.results.write_csv(&mut out).unwrap();
    golden("results_en.csv", &out);
    let text = String::from_utf8(out[3..].to_vec()).unwrap();
    // IDs are quoted text; names are formula-guarded; commas quoted.
    assert!(text.contains("\"220000001\",Sara Al-Harbi,\"101\",1,A,scored,10.00,10.00,100.00,A+,أ+,5.00,4.00,A,B,C,D,A\r\n"));
    assert!(text.contains("'@Noura Test"));
    assert!(text.contains("\"Omar Saleh, Jr.\""));
    let rows = csv_rows(&out);
    assert_eq!(
        rows[0][..13].join("|"),
        "Student ID|Name|Section|Booklet|Form|Status|Raw score|Maximum|Percent|Letter|Letter (Arabic)|GPA (5)|GPA (4)"
    );
    assert_eq!(rows[0][13..].join(" "), "Q1 Q2 Q3 Q4 Q5");
    let pick = |id: &str| {
        let r = row(&rows, id);
        (
            r[5].clone(),
            r[8].clone(),
            r[9].clone(),
            r[10].clone(),
            r[11].clone(),
            r[12].clone(),
        )
    };
    let s = |a: &str, b: &str, c: &str, d: &str, e: &str, f: &str| {
        (
            a.to_string(),
            b.to_string(),
            c.to_string(),
            d.to_string(),
            e.to_string(),
            f.to_string(),
        )
    };
    assert_eq!(
        pick("220000001"),
        s("scored", "100.00", "A+", "أ+", "5.00", "4.00")
    );
    assert_eq!(
        pick("220000002"),
        s("scored", "80.00", "B", "ب", "4.00", "3.00")
    );
    assert_eq!(
        pick("220000003"),
        s("scored", "80.00", "B", "ب", "4.00", "3.00")
    );
    assert_eq!(
        pick("220000004"),
        s("scored", "60.00", "D", "د", "2.00", "1.00")
    );
    assert_eq!(
        pick("220000005"),
        s("scored", "40.00", "F", "هـ", "1.00", "0.00")
    );
    assert_eq!(
        pick("220000006"),
        s("provisional", "80.00", "B", "ب", "4.00", "3.00")
    );
    assert_eq!(pick("220000007"), s("absent_excused", "", "", "", "", ""));
    // Master-space positional responses (form B is mapped back to master order).
    assert_eq!(row(&rows, "220000003")[13..].join(" "), "A B C D B");
    assert_eq!(row(&rows, "220000004")[13..].join("|"), "A|B|C||AC");
    assert_eq!(row(&rows, "220000006")[13..].join("|"), "A|?|C|D|A");
    assert_eq!(row(&rows, "220000007")[13..].join(""), "");
}

#[test]
fn results_csv_in_arabic() {
    let f = fixture();
    let mut t = f.results.clone();
    t.meta.language = Language::Ar;
    let mut out = Vec::new();
    t.write_csv(&mut out).unwrap();
    golden("results_ar.csv", &out);
    let rows = csv_rows(&out);
    assert_eq!(rows[0][0], "الرقم الجامعي");
    assert_eq!(rows[0][9], "التقدير");
}

#[test]
fn item_analysis_csv() {
    let f = fixture();
    let meta = meta();
    let ia = ItemAnalysis {
        meta: &meta,
        report: &f.analytics,
        show_key: false,
    };
    let mut items = Vec::new();
    ia.write_items_csv(&mut items).unwrap();
    golden("items.csv", &items);
    let mut tests = Vec::new();
    ia.write_tests_csv(&mut tests).unwrap();
    golden("tests.csv", &tests);
    let rows = csv_rows(&items);
    assert!(
        !rows[0].contains(&"Key".to_string()),
        "key hidden by default"
    );
    assert_eq!(rows.len(), 1 + 5);
    // p of Q1 = 5 of the 6 present students reached and got it right: S1–S5 and Lina all chose
    // A, so p = 6/6 = 1.000; Q3 = 5/6 (S5 chose A) = 0.833.
    let q = |l: &str| rows.iter().find(|r| r[0] == l).unwrap().clone();
    assert_eq!(q("Q1")[1], "6");
    assert_eq!(q("Q1")[3], "1.000");
    assert_eq!(q("Q3")[3], "0.833");
    // Shown on request.
    let ia = ItemAnalysis {
        show_key: true,
        ..ia
    };
    let mut out = Vec::new();
    ia.write_items_csv(&mut out).unwrap();
    let rows = csv_rows(&out);
    let k = rows[0].iter().position(|h| h == "Key").unwrap();
    let keys: Vec<&str> = rows[1..].iter().map(|r| r[k].as_str()).collect();
    assert_eq!(keys, vec!["A", "B", "C", "D", "A"]);
}

#[test]
fn flag_messages_naming_the_key_stay_hidden_with_the_key() {
    let f = fixture();
    let meta = meta();
    let mut report = f.analytics.clone();
    let flag = |rule, message: &str| omr_analytics::ItemFlag {
        item: 4,
        master: 4,
        label: "Q5".into(),
        rule,
        option: Some(1),
        message: message.into(),
        confidence: omr_analytics::Confidence::Normal,
        stability: None,
    };
    report.flags = vec![
        flag(
            omr_analytics::Rule::MiskeyedOption,
            "Probable mis-keyed option: A (r = -0.20) keyed, B (r = 0.31) not",
        ),
        flag(
            omr_analytics::Rule::TwoCorrect,
            "Possibly two correct answers: A and B",
        ),
        flag(
            omr_analytics::Rule::DistractorOutperformsKey,
            "Distractor outperforms key: B (p = 0.33, key p = 0.30)",
        ),
    ];
    let ia = ItemAnalysis {
        meta: &meta,
        report: &report,
        show_key: false,
    };
    let sheet = ia.flags_sheet();
    let msg = |s: &omr_report::Sheet, k: usize| match &s.rows[k].cells[7] {
        omr_report::Cell::Text(t) => t.clone(),
        other => panic!("{other:?}"),
    };
    for k in 0..2 {
        assert!(!msg(&sheet, k).contains("keyed, B") && !msg(&sheet, k).contains("A and B"));
        assert!(msg(&sheet, k).contains("shown only with the key"));
    }
    // A message that names only the distractor stays.
    assert!(msg(&sheet, 2).starts_with("Distractor outperforms key: B"));
    let shown = ItemAnalysis {
        show_key: true,
        ..ia
    }
    .flags_sheet();
    assert!(msg(&shown, 0).contains("A (r = -0.20) keyed"));
}

#[test]
fn req_a9_tp154_a1_and_b1_csv_with_bilingual_headers() {
    let f = fixture();
    let meta = meta();
    let labels = Tp154Labels::default();
    let tp = Tp154 {
        meta: &meta,
        labels: &labels,
        headers: Headers::Both,
    };
    let mut a1 = Vec::new();
    tp.write_a1_csv(&mut a1, &f.a1).unwrap();
    golden("tp154_a1.csv", &a1);
    let rows = csv_rows(&a1);
    assert_eq!(
        rows[0].join("|"),
        "Grade|A+|A|B+|B|C+|C|D+|D|F|Denied Entry|In Progress|Incomplete|Pass|Fail|Withdrawn"
    );
    assert_eq!(
        rows[1].join("|"),
        "التقدير|أ+|أ|ب+|ب|ج+|ج|د+|د|هـ|محروم|مستمر|غير مكتمل|ناجح|راسب|منسحب"
    );
    // 6 letters + DN + W = 8 listed: A+ 1, B 3, D 1, F 1, DN 1, W 1.
    assert_eq!(
        rows[2].join("|"),
        "Number of Students / عدد الطلاب|1|0|0|3|0|0|0|1|1|1|0|0|0|0|1"
    );
    assert_eq!(
        rows[3].join("|"),
        "Percentage / النسبة المئوية|12.5|0.0|0.0|37.5|0.0|0.0|0.0|12.5|12.5|12.5|0.0|0.0|0.0|0.0|12.5"
    );
    let mut b1 = Vec::new();
    tp.write_b1_csv(&mut b1, &f.b1).unwrap();
    golden("tp154_b1.csv", &b1);
    let rows = csv_rows(&b1);
    assert_eq!(rows[0][1], "Course Learning Outcomes");
    assert_eq!(rows[1][1], "نواتج تعلم المقرر");
    let numbers: Vec<&str> = rows[2..].iter().map(|r| r[0].as_str()).collect();
    assert_eq!(numbers, vec!["1.0", "1.1", "2.0", "2.1", "3.0", "3.1"]);
    assert_eq!(rows[2][1], "Knowledge and understanding / المعرفة والفهم");
    assert!(rows[3][1].starts_with("CLO1: "));
    assert_eq!(rows[3][2], "K1");
    assert_eq!(rows[3][3], "Quiz 1 Q1–2");
    assert_eq!(rows[3][4], "70.0%");
    // English-only and Arabic-only header variants.
    let tp_en = Tp154 {
        headers: Headers::English,
        ..tp.clone()
    };
    let mut out = Vec::new();
    tp_en.write_a1_csv(&mut out, &f.a1).unwrap();
    let rows = csv_rows(&out);
    assert_eq!(rows[1][0], "Number of Students");
}

#[test]
fn abet_criterion_3_summary_csv() {
    let f = fixture();
    let mut meta = meta();
    meta.title = "Computer Engineering — ABET Criterion 3 summary".into();
    let s = AbetSummary::build(
        meta,
        &f.framework,
        &[CourseInput {
            course: "CS 101",
            term: "1448/1",
            crosswalk: &f.crosswalk,
            attainment: &f.attainment,
            b1: Some(&f.b1),
        }],
        &f.so_values,
        omr_scoring::exact(7, 10),
        &BTreeMap::from([(
            "SO1".to_string(),
            "Add tracing practice in week 5".to_string(),
        )]),
        5,
    );
    let mut out = Vec::new();
    s.write_csv(&mut out).unwrap();
    golden("abet_c3.csv", &out);
    let rows = csv_rows(&out);
    // SO1 has two indicators (CLO1 via K1, CLO2 via S1); SO4 one (CLO3 via V1); SO6 one
    // (CLO2 via S1); SO2, SO3, SO5, SO7 are "not assessed".
    let so = |c: &str| rows.iter().filter(|r| r[0] == c).count();
    assert_eq!((so("SO1"), so("SO4"), so("SO6"), so("SO2")), (2, 1, 1, 1));
    let so2 = rows.iter().find(|r| r[0] == "SO2").unwrap();
    assert_eq!(so2[13], "not assessed");
    // N = 6 present students ≥ 5: not suppressed; no student identifiers anywhere.
    let text = String::from_utf8_lossy(&out);
    for id in ["220000001", "Sara", "خالد"] {
        assert!(!text.contains(id), "{id} leaked into the ABET export");
    }
    let so1 = rows.iter().find(|r| r[0] == "SO1").unwrap();
    assert_eq!(so1[18], "Add tracing practice in week 5");
}

#[test]
fn abet_small_groups_disclose_no_statistics() {
    let f = fixture();
    // Threshold 10 > N = 6: every indicator is a small group.
    let s = AbetSummary::build(
        meta(),
        &f.framework,
        &[CourseInput {
            course: "CS 101",
            term: "1448/1",
            crosswalk: &f.crosswalk,
            attainment: &f.attainment,
            b1: Some(&f.b1),
        }],
        &f.so_values,
        omr_scoring::exact(7, 10),
        &BTreeMap::new(),
        10,
    );
    let mut out = Vec::new();
    s.write_csv(&mut out).unwrap();
    let rows = csv_rows(&out);
    let assessed: Vec<_> = rows
        .iter()
        .skip(1)
        .filter(|r| r[13] != "not assessed")
        .collect();
    assert_eq!(assessed.len(), 4);
    for r in assessed {
        assert_eq!(r[8], "< 10");
        assert_eq!(r[9], "70.0", "the target is policy, not data");
        assert_eq!(
            (r[10].as_str(), r[11].as_str(), r[12].as_str()),
            ("", "", "")
        );
        assert_eq!(r[13], "suppressed (N < 10)");
        assert_eq!(r[14], "");
        // The SO roll-up rests only on small groups: suppressed as well.
        assert_eq!(
            (r[15].as_str(), r[17].as_str()),
            ("", "suppressed (N < 10)")
        );
    }
    // None of the small-group statistics of the unsuppressed export survive.
    let text = String::from_utf8_lossy(&out);
    for v in ["83.3", "66.7", "33.3", "43.6", "76.7", "yellow"] {
        assert!(
            !text.contains(v),
            "{v} disclosed for a group below the threshold"
        );
    }
}

#[test]
fn html_report_is_self_contained_and_labelled() {
    let f = fixture();
    let report = HtmlReport {
        results: &f.results,
        scale: &f.scale,
        test: f.analytics.tests.first(),
        items: Some(&f.analytics),
        show_key: false,
        clos: Some((&f.attainment, &f.clos)),
        open_exceptions: Some(f.run.exceptions.len()),
    };
    let mut out = Vec::new();
    report.write(&mut out).unwrap();
    golden("report.html", &out);
    let h = String::from_utf8(out).unwrap();
    assert!(h.starts_with("<!DOCTYPE html>\n<html lang=\"en\" dir=\"ltr\">"));
    // No external assets, no scripts.
    for bad in [
        "http://", "https://", "<script", " src=", "@import", "<link",
    ] {
        assert!(!h.contains(bad), "external or active content: {bad}");
    }
    // Classification label heads and closes the page.
    let first = h.find("Restricted / مقيد").unwrap();
    assert!(first < h.find("<h1").unwrap());
    assert!(h.rfind("Restricted / مقيد").unwrap() > h.find("<footer").unwrap());
    // Palette (light and dark), F hatched with the word "Fail", Arabic letters, dir=auto.
    for token in [
        "#08856A",
        "#16A384",
        "#B3261E",
        "#F5F0E6",
        "prefers-color-scheme:dark",
    ] {
        assert!(h.contains(token), "missing {token}");
    }
    assert!(h.contains("class=\"bar-fail\""));
    assert!(h.contains(">Fail</text>"));
    assert!(h.contains(">هـ</text>"));
    assert!(h.contains("<td dir=\"auto\">محمد العتيبي</td>"));
    assert!(
        h.contains("<td dir=\"auto\">@Noura Test</td>"),
        "HTML escapes, it does not formula-guard"
    );
    // Hand-computed statistics: mean 440/6, median 80, pass 5 of 6.
    assert!(h.contains(">73.33%<"));
    assert!(h.contains(">80.00%<"));
    assert!(h.contains(">83.3%<"));
    // Distribution: A+ 1, B 3, D 1, F 1.
    assert!(h.contains(
        "aria-label=\"Grade distribution: A+ 1, A 0, B+ 0, B 3, C+ 0, C 0, D+ 0, D 1, F 1\""
    ));
    // The key is hidden by default: no option bar uses the keyed colour.
    assert!(!h.contains("style=\"fill:var(--series-1)\""));
    // Deterministic.
    let mut again = Vec::new();
    report.write(&mut again).unwrap();
    assert_eq!(again, h.as_bytes());
}

#[test]
fn html_report_in_arabic_is_rtl() {
    let f = fixture();
    let mut t = f.results.clone();
    t.meta.language = Language::Ar;
    let report = HtmlReport {
        results: &t,
        scale: &f.scale,
        test: None,
        items: None,
        show_key: false,
        clos: None,
        open_exceptions: None,
    };
    let h = report.render();
    assert!(h.starts_with("<!DOCTYPE html>\n<html lang=\"ar\" dir=\"rtl\">"));
    assert!(h.contains("توزيع التقديرات"));
    assert!(h.contains(">راسب</text>"));
}
