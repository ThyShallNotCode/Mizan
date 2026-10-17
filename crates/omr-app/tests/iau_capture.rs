//! Real-world capture cases, proven on synthetic sheets only (the committed blank IAU form with
//! synthetic pencil marks; `omr-sim` for own forms):
//! - C1 orientation (§8.3): IAU pages fed upside down (180°) or scanned in portrait (90°/270°),
//!   own-form pages at 90°/180°/270°, are turned upright before registration (REG-002 INFO);
//!   a page whose orientation cannot be decided is never read silently;
//! - C2 duplex blank backs (§7.5): blank and bleed-through-only pages between the sheets, in a
//!   PDF and as image files, are CAP-012 (INFO) and dropped: no booklet, no blocking item, no
//!   shift in collation;
//! - C3 mixed capture (§8.1): a 600-dpi bitonal PDF, 300-dpi gray JPEGs and 200-dpi PNGs of
//!   the same form in one exam, one baseline per capture slice, all read correctly.

mod common;

use common::*;
use omr_app::model::{PageStatus, ResponseStatus};
use omr_app::{ExamDefinition, ExamProject, FormKey, KeySource};
use omr_model::{Response, Severity};
use std::collections::BTreeMap;

const N: u16 = 29;

/// Key of form `v` (0 = A).
fn key(v: u16, q: u16) -> u8 {
    ((q * 3 + v) % 5) as u8
}

/// What sheet `k` answers: wrong on every (k + 2)-th question, blank on a few.
fn answer(k: u16, q: u16) -> Option<u8> {
    let v = k % 2;
    if (q + k) % 11 == 7 {
        None
    } else if q.is_multiple_of(k + 2) {
        Some((key(v, q) + 1) % 5)
    } else {
        Some(key(v, q))
    }
}

fn student(k: u16) -> String {
    format!("22103456{k:02}")
}

fn marks(k: u16) -> Marks {
    let answers: Vec<Option<u8>> = (0..N).map(|q| answer(k, q)).collect();
    Marks::new(&answers, &student(k), Some((k % 2) as u8))
}

fn definition(title: &str) -> ExamDefinition {
    legacy_def(
        title,
        N,
        &["A", "B"],
        KeySource::Provided {
            forms: (0..2u16)
                .map(|v| FormKey {
                    form: ["A", "B"][usize::from(v)].into(),
                    answers: (0..N)
                        .map(|q| char::from(b'A' + key(v, q)).to_string())
                        .collect(),
                })
                .collect(),
        },
    )
}

/// Every booklet is the sheet `sheet_of` names: its version, every auto-accepted answer and its
/// student number; nearly every answer auto-accepted.
fn check_booklets(p: &ExamProject, sheet_of: &dyn Fn(usize) -> u16) {
    let s = &p.session;
    for (bi, b) in s.booklets.iter().enumerate() {
        let k = sheet_of(bi);
        assert_eq!(
            s.definition.forms[b.form.expect("form")],
            ["A", "B"][usize::from(k % 2)],
            "sheet {k}"
        );
        let mut auto = 0;
        for (q, qs) in b.questions.iter().enumerate() {
            if qs.status == ResponseStatus::Auto {
                auto += 1;
                let want = answer(k, q as u16).map_or(Response::Blank, Response::Single);
                assert_eq!(qs.current, want, "sheet {k} q{q}");
            }
        }
        assert!(
            auto >= usize::from(N) - 1,
            "sheet {k}: {auto} auto-accepted"
        );
        assert_eq!(
            b.identity.student.as_deref(),
            Some(student(k).as_str()),
            "sheet {k}"
        );
    }
}

/// `sheet-KK…` file names carry the sheet number.
fn sheet_from_name(p: &ExamProject, bi: usize) -> u16 {
    source_of(p, bi)[6..8].parse().unwrap()
}

fn page_codes(p: &ExamProject, page: usize) -> Vec<String> {
    let pg = &p.session.pages[page];
    pg.read
        .iter()
        .flat_map(|r| r.exceptions.iter())
        .chain(pg.refusal.iter())
        .map(|e| e.code.clone())
        .collect()
}

#[test]
fn c1_legacy_pages_turned_a_quarter_or_upside_down_are_read_upright() {
    let dir = tempfile::tempdir().unwrap();
    let e = iau();
    let scans = dir.path().join("scans");
    // Sheet k is turned k quarter turns clockwise: upright, portrait (90°), upside down,
    // portrait the other way (270°).
    for k in 0..4u16 {
        let img = sheet(&e, &marks(k), 40 + u64::from(k)).rotated_quarter(k as u8);
        write(&scans, &format!("sheet-{k:02}-turn{k}.png"), &png(&img));
    }
    let def = definition("Orientation");
    let p = grade(&def, &[scans], &dir.path().join("out"), 2);
    let s = &p.session;
    assert_eq!(s.booklets.len(), 4);
    assert!(s.pages.iter().all(|pg| pg.status == PageStatus::Read));
    check_booklets(&p, &|bi| sheet_from_name(&p, bi));
    for (bi, b) in s.booklets.iter().enumerate() {
        let k = sheet_from_name(&p, bi);
        let codes = page_codes(&p, b.pages[0]);
        assert_eq!(
            codes.contains(&"REG-002".to_string()),
            k != 0,
            "sheet {k}: {codes:?}"
        );
        // The working page is landscape whatever the scan: crops show the registered page.
        let (w, h) = s.pages[b.pages[0]].working.unwrap();
        assert!(w > h, "sheet {k}: working page {w}×{h}");
    }
    // Nothing about orientation reaches the queue (REG-002 is INFO).
    assert!(p.queue(None).iter().all(|i| !i.code.starts_with("REG")));
}

#[test]
fn c1_legacy_orientation_that_cannot_be_decided_is_never_read_silently() {
    let dir = tempfile::tempdir().unwrap();
    let e = iau();
    let scans = dir.path().join("scans");
    // A sheet printed over its own upside-down copy correlates as well both ways round.
    let a = sheet(&e, &marks(0), 7);
    let b = a.rotated_quarter(2);
    let both =
        omr_imaging::GrayImage::from_fn(a.width(), a.height(), |x, y| a.get(x, y).min(b.get(x, y)));
    write(&scans, "sheet-00.png", &png(&both));
    write(&scans, "sheet-01.png", &png(&sheet(&e, &marks(1), 8)));
    let def = definition("Undecided");
    let p = grade(&def, &[scans], &dir.path().join("out"), 2);
    let s = &p.session;
    let bad = s
        .pages
        .iter()
        .position(|pg| s.sources[pg.source].name == "sheet-00.png")
        .unwrap();
    let pg = &s.pages[bad];
    // Either not attributed at all, or attributed with nothing auto-accepted; in both cases a
    // blocking item holds it in the queue.
    let auto = pg.booklet.map_or(0, |bi| {
        s.booklets[bi]
            .questions
            .iter()
            .filter(|q| q.status == ResponseStatus::Auto)
            .count()
    });
    assert_eq!(
        auto,
        0,
        "{:?} {:?}",
        page_codes(&p, bad),
        pg.read.as_ref().map(|r| r.registration.clone())
    );
    assert!(
        p.queue(None)
            .iter()
            .any(|i| i.page == Some(bad) && i.severity >= Severity::Error),
        "{:?}",
        page_codes(&p, bad)
    );
    assert!(page_codes(&p, bad).contains(&"REG-001".to_string()));
    assert!(s.release_blocked().is_some());
}

/// An own-form exam (two forms, roster) for the `omr-sim` corpora.
mod own {
    use omr_app::{
        ExamDefinition, FormKey, IdSchemeChoice, KeySource, RosterStudent, TemplateChoice,
    };
    use omr_forms::{ExamKey, Template, TemplateSpec};
    use omr_model::{CancelMode, ExamId};
    use omr_sim::{CorpusConfig, PageTruth};
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    pub const QUESTIONS: u16 = 20;
    pub const KEY: [u8; 32] = [0x5a; 32];

    pub fn spec() -> TemplateSpec {
        let mut s = TemplateSpec::standard(
            ExamId(uuid::Uuid::from_u128(
                0x0190_7777_5678_7abc_8def_0000_0000_0042,
            )),
            QUESTIONS,
            4,
            CancelMode::EraseOnly,
        );
        s.forms = 2;
        s.title = "Capture quiz".into();
        s
    }

    pub fn roster() -> Vec<String> {
        (0..8).map(|i| format!("4511{:05}", 100 + i * 7)).collect()
    }

    pub fn corpus(dir: &Path, sheets: u32, seed: u64) -> (PathBuf, BTreeMap<String, PageTruth>) {
        let t = Template::build(spec()).unwrap();
        let mut cfg = CorpusConfig::standard(sheets, seed);
        cfg.roster = roster();
        cfg.threads = 2;
        omr_sim::corpus::write_corpus(&t, &ExamKey::from_bytes(KEY), &cfg, dir).unwrap();
        let truth = omr_sim::corpus::read_truth(&dir.join("truth.jsonl"))
            .unwrap()
            .into_iter()
            .map(|p| {
                let name = Path::new(&p.file)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                (name, p)
            })
            .collect();
        (dir.join("images"), truth)
    }

    pub fn definition() -> ExamDefinition {
        let mut d = ExamDefinition::new(
            "Own-form capture",
            TemplateChoice::Generated(Box::new(spec())),
            QUESTIONS,
            4,
            vec!["A".into(), "B".into()],
            CancelMode::EraseOnly,
        );
        d.page_code_key_hex = Some(hex::encode(KEY));
        d.key = KeySource::Provided {
            forms: (0..2usize)
                .map(|f| FormKey {
                    form: ["A", "B"][f].into(),
                    answers: (0..usize::from(QUESTIONS))
                        .map(|q| char::from(b'A' + ((q * 3 + f) % 4) as u8).to_string())
                        .collect(),
                })
                .collect(),
        };
        d.roster = Some(
            roster()
                .into_iter()
                .enumerate()
                .map(|(i, id)| RosterStudent {
                    id,
                    name: format!("Student {i}"),
                    name_ar: String::new(),
                    section: "1".into(),
                    numeric_cell: false,
                })
                .collect(),
        );
        d.id_scheme = IdSchemeChoice::Digits(9);
        d.exam_id = d.derived_exam_id();
        d.validate().unwrap();
        d
    }
}

/// Own-form booklets agree with the simulator's truth (auto-accepted answers, form, student).
fn check_own(
    p: &ExamProject,
    truth: &BTreeMap<String, omr_sim::PageTruth>,
    name_of: &dyn Fn(&str) -> String,
) {
    let s = &p.session;
    for (bi, b) in s.booklets.iter().enumerate() {
        let t = &truth[&name_of(&source_of(p, bi))];
        assert_eq!(b.serial, t.booklet);
        assert_eq!(
            b.form,
            Some(usize::from(t.form - 1)),
            "booklet {}",
            b.serial
        );
        for (q, qs) in b.questions.iter().enumerate() {
            if qs.status == ResponseStatus::Auto
                && let omr_sim::Expected::Auto(r) = &t.questions[q].expected
            {
                assert_eq!(*r, qs.current, "booklet {} q{q}", b.serial);
            }
        }
        assert_eq!(b.identity.student.as_deref(), t.student_id.as_deref());
    }
}

#[test]
fn c1_own_form_pages_at_90_180_270_degrees_are_read() {
    let dir = tempfile::tempdir().unwrap();
    let (images, truth) = own::corpus(&dir.path().join("corpus"), 4, 71);
    let turned = dir.path().join("turned");
    let mut names: Vec<String> = truth.keys().cloned().collect();
    names.sort();
    for (k, name) in names.iter().enumerate() {
        let img = omr_imaging::decode(&std::fs::read(images.join(name)).unwrap())
            .unwrap()
            .image
            .rotated_quarter(k as u8);
        write(&turned, &format!("{name}.turn{k}.png"), &png(&img));
    }
    let p = grade(&own::definition(), &[turned], &dir.path().join("out"), 2);
    let s = &p.session;
    assert_eq!(s.booklets.len(), 4);
    assert!(s.pages.iter().all(|pg| pg.status == PageStatus::Read));
    check_own(&p, &truth, &|f| {
        f.split(".turn").next().unwrap().to_string()
    });
    for (pi, pg) in s.pages.iter().enumerate() {
        let k: u8 = s.sources[pg.source].name.rsplit("turn").next().unwrap()[..1]
            .parse()
            .unwrap();
        let codes = page_codes(&p, pi);
        assert_eq!(
            codes.contains(&"REG-002".to_string()),
            k != 0,
            "turn {k}: {codes:?}"
        );
    }
}

#[test]
fn c2_blank_backs_in_a_pdf_and_as_image_files_are_dropped_with_cap012() {
    let dir = tempfile::tempdir().unwrap();
    let e = iau();
    // A duplex PDF: each sheet followed by its back (blank, or bleed-through of the front).
    let fronts: Vec<_> = (0..3u16)
        .map(|k| sheet(&e, &marks(k), 60 + u64::from(k)))
        .collect();
    let mut pages = Vec::new();
    for (k, f) in fronts.iter().enumerate() {
        pages.push(PdfImage::jpeg(f, 200.0));
        let back = if k == 1 {
            bleed_through(f, 3)
        } else {
            blank_page(f.width(), f.height(), k as u64)
        };
        pages.push(PdfImage::jpeg(&back, 200.0));
    }
    let scans = dir.path().join("scans");
    write(&scans, "duplex.pdf", &pdf(&pages));
    // The same as image files, a blank back between two sheets (names sort front, back).
    for k in 3..5u16 {
        let f = sheet(&e, &marks(k), 60 + u64::from(k));
        let back = if k == 3 {
            bleed_through(&f, 9)
        } else {
            blank_page(f.width(), f.height(), 11)
        };
        write(&scans, &format!("sheet-{k:02}-a.png"), &png(&f));
        write(&scans, &format!("sheet-{k:02}-b.jpg"), &jpeg(&back, 85));
    }
    let def = definition("Duplex");
    let p = grade(&def, &[scans], &dir.path().join("out"), 3);
    let s = &p.session;
    let blank: Vec<usize> = (0..s.pages.len())
        .filter(|&i| s.pages[i].status == PageStatus::Blank)
        .collect();
    assert_eq!(blank.len(), 5, "three PDF backs and two image backs");
    for &i in &blank {
        let r = s.pages[i].refusal.as_ref().expect("CAP-012");
        assert_eq!((r.code.as_str(), r.severity), ("CAP-012", Severity::Info));
        assert_eq!(s.pages[i].booklet, None);
    }
    // Five sheets, five booklets, in the right order and read correctly; no orphan booklet.
    assert_eq!(s.booklets.len(), 5);
    let sheet_of = |bi: usize| {
        let pg = &s.pages[s.booklets[bi].pages[0]];
        let name = &s.sources[pg.source].name;
        if name == "duplex.pdf" {
            (pg.index / 2) as u16
        } else {
            name[6..8].parse().unwrap()
        }
    };
    check_booklets(&p, &sheet_of);
    // Blank backs never queue and never block release.
    assert!(p.queue(None).iter().all(|i| i.code != "CAP-012"));
    assert!(
        p.queue(None)
            .iter()
            .all(|i| i.page.is_none_or(|pg| !blank.contains(&pg))),
        "no item about a blank back"
    );
    let summary: serde_json::Value = serde_json::from_slice(
        &std::fs::read(dir.path().join("out").join("summary.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(summary["pages"]["blank"], 5);
}

#[test]
fn c2_own_form_blank_backs_do_not_shift_collation() {
    let dir = tempfile::tempdir().unwrap();
    let (images, truth) = own::corpus(&dir.path().join("corpus"), 3, 72);
    let scans = dir.path().join("scans");
    let mut names: Vec<String> = truth.keys().cloned().collect();
    names.sort();
    for (k, name) in names.iter().enumerate() {
        let bytes = std::fs::read(images.join(name)).unwrap();
        let img = omr_imaging::decode(&bytes).unwrap().image;
        write(&scans, &format!("{k:02}-a-{name}"), &bytes);
        write(
            &scans,
            &format!("{k:02}-b-back.png"),
            &png(&bleed_through(&img, k as u64)),
        );
    }
    let p = grade(&own::definition(), &[scans], &dir.path().join("out"), 2);
    let s = &p.session;
    assert_eq!(s.booklets.len(), 3);
    assert_eq!(
        s.pages
            .iter()
            .filter(|pg| pg.status == PageStatus::Blank)
            .count(),
        3
    );
    check_own(&p, &truth, &|f| f[5..].to_string());
    assert!(p.queue(None).iter().all(|i| i.code != "CAP-012"));
}

#[test]
fn c3_bitonal_pdf_gray_jpeg_and_png_in_one_exam_each_get_their_baseline() {
    let dir = tempfile::tempdir().unwrap();
    let e = iau();
    let scans = dir.path().join("scans");
    // Sheets 0–1: a 600-dpi bitonal PDF (two pages).
    let pages: Vec<PdfImage> = (0..2u16)
        .map(|k| {
            let s200 = sheet(&e, &marks(k), 80 + u64::from(k));
            PdfImage::bitonal(&bitonal(&rescale(&s200, 3, 1), 150), 600.0)
        })
        .collect();
    write(&scans, "sheet-00-bitonal600.pdf", &pdf(&pages));
    // Sheets 2–3: 300-dpi gray JPEG; sheets 4–5: 200-dpi PNG.
    for k in 2..4u16 {
        let img = rescale(&sheet(&e, &marks(k), 80 + u64::from(k)), 3, 2);
        write(
            &scans,
            &format!("sheet-{k:02}-gray300.jpg"),
            &jpeg(&img, 92),
        );
    }
    for k in 4..6u16 {
        let img = sheet(&e, &marks(k), 80 + u64::from(k));
        write(&scans, &format!("sheet-{k:02}-gray200.png"), &png(&img));
    }
    let def = definition("Mixed capture");
    let p = grade(
        &def,
        std::slice::from_ref(&scans),
        &dir.path().join("out"),
        3,
    );
    let s = &p.session;
    assert_eq!(s.booklets.len(), 6);
    assert!(s.pages.iter().all(|pg| pg.status == PageStatus::Read));
    let sheet_of = |bi: usize| {
        let pg = &s.pages[s.booklets[bi].pages[0]];
        let name = &s.sources[pg.source].name;
        name[6..8].parse::<u16>().unwrap() + pg.index as u16
    };
    check_booklets(&p, &sheet_of);
    // Capture class and source resolution per page; one slice (baseline) each.
    for pg in &s.pages {
        let name = &s.sources[pg.source].name;
        let (bitonal, dpi) = if name.contains("bitonal600") {
            (true, 600)
        } else if name.contains("gray300") {
            (false, 300)
        } else {
            (false, 200)
        };
        assert_eq!(pg.capture.unwrap().is_bitonal(), bitonal, "{name}");
        assert_eq!(pg.source_dpi, Some(dpi), "{name}");
    }
    // One baseline per slice: every page reads exactly as when its slice is graded alone, so
    // no capture class or resolution shifts another's β_p.
    for (k, pattern) in ["bitonal600", "gray300", "gray200"].iter().enumerate() {
        let alone = dir.path().join(format!("alone-{k}"));
        for f in std::fs::read_dir(&scans).unwrap() {
            let f = f.unwrap().path();
            if f.to_string_lossy().contains(pattern) {
                write(
                    &alone,
                    f.file_name().unwrap().to_str().unwrap(),
                    &std::fs::read(&f).unwrap(),
                );
            }
        }
        let q = grade(&def, &[alone], &dir.path().join(format!("out-{k}")), 1);
        assert_eq!(q.session.pages.len(), 2);
        for pg in &q.session.pages {
            let mixed = s.pages.iter().find(|m| m.page_id == pg.page_id).unwrap();
            assert_eq!(
                mixed.read, pg.read,
                "{pattern}: page reads differ in the mixed batch"
            );
        }
    }
    // The bitonal slice is announced once for the batch (CAP-003 `bitonal`, WARN).
    let cap003: Vec<_> = s
        .batch_exceptions
        .iter()
        .filter(|e| e.code == "CAP-003")
        .collect();
    assert_eq!(cap003.len(), 1);
    assert_eq!(cap003[0].severity, Severity::Warn);
}
