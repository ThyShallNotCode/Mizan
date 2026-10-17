//! Adversarial verification of the whole pipeline (synthetic data only: the committed blank IAU
//! form with synthetic pencil marks, and `omr-sim` own forms). Every hostile input must give
//! its Appendix A code, never a panic and never a silent wrong score:
//! - broken files: zero-byte, truncated, encrypted, mislabelled (CAP-001, each naming its file
//!   and what to do); a multi-page TIFF gives every sheet, a damaged one the rest as CAP-001;
//! - pages turned 90/180/270° (image files and PDF `/Rotate`) read upright; mirrored ones and
//!   pages that are not this exam's sheet, or of a form it does not declare, or cannot be read
//!   are held (REG-001/REG-003/IDN-002);
//! - scan resolution: 150 (CAP-002 WARN), 300 and 400 dpi read; below 100 dpi refused (CAP-002
//!   ERROR);
//! - rescans and duplicated pages: COL-003 when the marks agree, IDN-005 when they differ,
//!   CAP-011 for a byte-identical file;
//! - every bubble filled, no marks at all; keys with a void and an any-of entry;
//! - a review action, a key change and undo, an appended batch and undo, a group decision
//!   refused part way: all rescore (incremental = full rebuild) and stay audited;
//! - determinism with all of the above across thread counts and input orders;
//! - a 500-page PDF: memory bounded and throughput held (`#[ignore]`, release).

mod common;

use common::*;
use omr_app::model::{PageStatus, ResponseStatus};
use omr_app::review::{Lane, ReviewReason};
use omr_app::{
    Action, Decision, ExamDefinition, ExamProject, FormKey, IdSchemeChoice, ItemKind, KeySource,
    RosterStudent, TemplateChoice,
};
use omr_forms::legacy::LegacyField;
use omr_forms::{ExamKey, Template, TemplateSpec};
use omr_imaging::GrayImage;
use omr_model::{CancelMode, ExamId, Response, Severity};
use omr_scoring::AssessmentStatus;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const N: u16 = 29;

fn key(v: u16, q: u16) -> u8 {
    ((q * 3 + v) % 5) as u8
}

/// Sheet `k`'s answers: wrong on every (k + 2)-th question, blank on a few.
fn answers(k: u16) -> Vec<Option<u8>> {
    let v = k % 2;
    (0..N)
        .map(|q| {
            if (q + k) % 11 == 7 {
                None
            } else if q.is_multiple_of(k + 2) {
                Some((key(v, q) + 1) % 5)
            } else {
                Some(key(v, q))
            }
        })
        .collect()
}

fn student(k: u16) -> String {
    format!("22103458{k:02}")
}

fn marks(k: u16) -> Marks {
    Marks::new(&answers(k), &student(k), Some((k % 2) as u8))
}

/// Sheet `k` at 200 dpi.
fn page(k: u16) -> GrayImage {
    sheet(&iau(), &marks(k), 300 + u64::from(k))
}

/// The hand count of sheet `k` under the provided key.
fn hand(k: u16) -> String {
    (0..N)
        .filter(|&q| answers(k)[usize::from(q)] == Some(key(k % 2, q)))
        .count()
        .to_string()
}

fn provided_key() -> KeySource {
    KeySource::Provided {
        forms: (0..2u16)
            .map(|v| FormKey {
                form: ["A", "B"][usize::from(v)].into(),
                answers: (0..N)
                    .map(|q| char::from(b'A' + key(v, q)).to_string())
                    .collect(),
            })
            .collect(),
    }
}

fn definition() -> ExamDefinition {
    legacy_def("Adversarial", N, &["A", "B"], provided_key())
}

/// The sheet number of booklet `bi` (files are named `<anything>-s<k>…`).
fn sheet_no(p: &ExamProject, bi: usize) -> u16 {
    let name = source_of(p, bi);
    let at = name.find("-s").expect("sheet number in the file name") + 2;
    name[at..at + 2].parse().unwrap()
}

/// (status, raw) of booklet `bi`.
fn result(p: &ExamProject, bi: usize) -> (AssessmentStatus, String) {
    let sc = p.scoring.as_ref().unwrap();
    let r = &sc.run.as_ref().unwrap().students[sc.row_of[bi].expect("scored")];
    (r.status, r.raw.to_string())
}

/// Every booklet's auto-accepted answers are the sheet's truth, and every booklet read without
/// review scores its hand count.
fn check_scores(p: &ExamProject) {
    for bi in 0..p.session.booklets.len() {
        let k = sheet_no(p, bi);
        let b = &p.session.booklets[bi];
        // (The read is masked when the number is national-ID shaped; the student is not.)
        assert_eq!(b.student(), Some(student(k).as_str()), "sheet {k}");
        for (q, qs) in b.questions.iter().enumerate() {
            if qs.status == ResponseStatus::Auto {
                let truth = answers(k)[q].map_or(Response::Blank, Response::Single);
                assert_eq!(qs.current, truth, "sheet {k} q{}: silent error", q + 1);
            }
        }
        if b.questions.iter().all(|q| q.status == ResponseStatus::Auto) {
            assert_eq!(
                result(p, bi),
                (AssessmentStatus::Scored, hand(k)),
                "sheet {k}"
            );
        }
    }
}

/// The page read from file `name` (the first one when the file has several).
fn page_of<'a>(p: &'a ExamProject, name: &str) -> &'a omr_app::model::PageState {
    let s = &p.session;
    s.pages
        .iter()
        .find(|pg| s.sources[pg.source].name.split(" (unreadable").next() == Some(name))
        .unwrap_or_else(|| panic!("no page from {name}"))
}

/// Page-level codes of the page read from file `name`.
fn codes_of(p: &ExamProject, name: &str) -> Vec<(String, Severity)> {
    let pg = page_of(p, name);
    pg.read
        .iter()
        .flat_map(|r| r.exceptions.iter())
        .chain(pg.refusal.iter())
        .map(|e| (e.code.clone(), e.severity))
        .collect()
}

/// The open Capture-lane item of the page read from file `name`.
fn capture_item<'a>(p: &'a ExamProject, name: &str) -> &'a omr_app::ReviewItem {
    let s = &p.session;
    p.queue(Some(Lane::Capture))
        .into_iter()
        .find(|i| {
            i.page.is_some_and(|pg| {
                s.sources[s.pages[pg].source]
                    .name
                    .split(" (unreadable")
                    .next()
                    == Some(name)
            })
        })
        .unwrap_or_else(|| panic!("{name}: no Capture item"))
}

/// The incremental scoring state equals a full rebuild from the session (A4 invariant).
fn incremental_equals_rebuild(p: &ExamProject) {
    let full = omr_app::score::Scoring::build(&p.session).unwrap().unwrap();
    let rows = |r: &omr_scoring::ScoreRun| -> Vec<String> {
        r.students
            .iter()
            .map(|s| format!("{:?} {} {:?}", s.status, s.raw, s.student))
            .collect()
    };
    assert_eq!(
        rows(p.scoring.as_ref().unwrap().run.as_ref().unwrap()),
        rows(full.run.as_ref().unwrap())
    );
}

/// `pdf` with an `/Encrypt` dictionary whose user password is not empty: the file opens only
/// with a password.
fn encrypted(pdf: &[u8]) -> Vec<u8> {
    let at = pdf.windows(7).rposition(|w| w == b"trailer").unwrap();
    let trailer = String::from_utf8(pdf[at..].to_vec()).unwrap().replace(
        "/Root 1 0 R >>",
        "/Root 1 0 R /Encrypt << /Filter /Standard /V 1 /R 2 \
         /O <0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20> \
         /U <2122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f40> /P -4 >> \
         /ID [<00112233445566778899aabbccddeeff><00112233445566778899aabbccddeeff>] >>",
    );
    let mut out = pdf[..at].to_vec();
    out.extend_from_slice(trailer.as_bytes());
    out
}

/// A minimal multi-page TIFF (8-bit gray, uncompressed, one strip per page).
fn tiff(pages: &[GrayImage]) -> Vec<u8> {
    let mut out = b"II*\0\0\0\0\0".to_vec();
    let mut link = 4usize;
    for img in pages {
        let data = out.len();
        out.extend_from_slice(img.as_raw());
        if out.len() % 2 == 1 {
            out.push(0);
        }
        let ifd = out.len();
        out[link..link + 4].copy_from_slice(&(ifd as u32).to_le_bytes());
        let tags: [(u16, u16, u32); 8] = [
            (256, 4, img.width() as u32),
            (257, 4, img.height() as u32),
            (258, 3, 8),
            (259, 3, 1),
            (262, 3, 1),
            (273, 4, data as u32),
            (278, 4, img.height() as u32),
            (279, 4, (img.width() * img.height()) as u32),
        ];
        out.extend_from_slice(&(tags.len() as u16).to_le_bytes());
        for (tag, ty, v) in tags {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&ty.to_le_bytes());
            out.extend_from_slice(&1u32.to_le_bytes());
            if ty == 3 {
                out.extend_from_slice(&(v as u16).to_le_bytes());
                out.extend_from_slice(&[0, 0]);
            } else {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        link = out.len();
        out.extend_from_slice(&0u32.to_le_bytes());
    }
    out
}

fn to_gray(i: &image::GrayImage) -> GrayImage {
    GrayImage::from_raw(i.width() as usize, i.height() as usize, i.as_raw().clone()).unwrap()
}

fn rng(seed: u64) -> rand_chacha::ChaCha8Rng {
    <rand_chacha::ChaCha8Rng as rand::SeedableRng>::seed_from_u64(seed)
}

#[test]
fn broken_files_are_cap001_naming_the_file_and_the_fix() {
    let dir = tempfile::tempdir().unwrap();
    let scans = dir.path().join("scans");
    write(&scans, "a-s00.png", &png(&page(0)));
    let pdf1 = pdf(&[PdfImage::jpeg(&page(1), 200.0)]);
    write(&scans, "a-s01.pdf", &pdf1);
    // Three different empty files: three lost scans, never "duplicates" of each other.
    for name in ["empty.pdf", "empty.jpg", "empty.png"] {
        write(&scans, name, b"");
    }
    write(&scans, "cut.pdf", &pdf1[..pdf1.len() / 2]);
    // A JPEG cut anywhere, even just before its end-of-image marker, is never decoded: the
    // decoder would fill the missing rows with gray.
    let j = jpeg(&page(2), 85);
    for (name, cut) in [
        ("cut-30.jpg", j.len() * 3 / 10),
        ("cut-90.jpg", j.len() * 9 / 10),
        ("cut-end.jpg", j.len() - 2),
    ] {
        write(&scans, name, &j[..cut]);
    }
    let p3 = png(&page(3));
    write(&scans, "cut.png", &p3[..p3.len() * 9 / 10]);
    write(&scans, "locked.pdf", &encrypted(&pdf1));
    write(
        &scans,
        "pdf-named.jpg",
        &pdf(&[PdfImage::jpeg(&page(4), 200.0)]),
    );
    let p = grade(&definition(), &[scans], &dir.path().join("out"), 2);
    // The two good sheets are graded exactly.
    assert_eq!(p.session.booklets.len(), 2);
    check_scores(&p);
    let cases = [
        ("empty.pdf", "zero-byte"),
        ("empty.jpg", "zero-byte"),
        ("empty.png", "zero-byte"),
        ("cut.pdf", "cut short"),
        ("cut-30.jpg", "truncated JPEG"),
        ("cut-90.jpg", "truncated JPEG"),
        ("cut-end.jpg", "truncated JPEG"),
        ("cut.png", "end of file"),
        ("locked.pdf", "password"),
        ("pdf-named.jpg", "unsupported image"),
    ];
    for (name, why) in cases {
        assert_eq!(page_of(&p, name).status, PageStatus::Refused, "{name}");
        assert_eq!(
            codes_of(&p, name),
            vec![("CAP-001".to_string(), Severity::Blocker)],
            "{name}"
        );
        let it = capture_item(&p, name);
        assert!(it.detail.starts_with(name), "{name}: {}", it.detail);
        assert!(it.detail.contains(why), "{name}: {}", it.detail);
        assert!(
            it.detail.contains("rescan"),
            "{name}: no remedy: {}",
            it.detail
        );
    }
    assert_eq!(p.queue(Some(Lane::Capture)).len(), cases.len());
    assert!(p.session.release_blocked().is_some());
    p.store().unwrap().verify().unwrap();
}

#[test]
fn every_page_of_a_multi_page_tiff_is_graded_and_a_damaged_rest_is_cap001() {
    let dir = tempfile::tempdir().unwrap();
    let scans = dir.path().join("scans");
    let pages: Vec<GrayImage> = (0..3).map(page).collect();
    let t = tiff(&pages);
    write(&scans, "batch.tif", &t);
    let p = grade(&definition(), &[scans], &dir.path().join("out"), 2);
    assert_eq!(p.session.pages.len(), 3);
    assert_eq!(p.session.booklets.len(), 3);
    for bi in 0..3 {
        let st = p.session.booklets[bi].student().map(str::to_string);
        let k = (0..3)
            .find(|&k| Some(student(k)) == st)
            .expect("one of the three sheets");
        assert_eq!(result(&p, bi), (AssessmentStatus::Scored, hand(k)));
    }
    assert!(p.session.release_blocked().is_none());

    // The page chain cut inside the third page's directory: two sheets graded, the rest of the
    // file one CAP-001 page that says what was lost.
    let tp = omr_imaging::tiff_pages(&t).unwrap();
    let damaged = dir.path().join("damaged");
    write(&damaged, "batch.tif", &t[..tp.offsets[2] as usize + 6]);
    let p = grade(&definition(), &[damaged], &dir.path().join("out2"), 2);
    assert_eq!(p.session.booklets.len(), 2);
    let it = capture_item(&p, "batch.tif");
    assert_eq!(
        (it.code.as_str(), it.severity),
        ("CAP-001", Severity::Blocker)
    );
    assert!(it.detail.contains("after page 2"), "{}", it.detail);
}

/// `pdf` (from `common::pdf`) with `/Rotate deg` on every page and its cross-reference table
/// rebuilt.
fn rotated(pdf: &[u8], deg: u32) -> Vec<u8> {
    let body = &pdf[..pdf.windows(5).rposition(|w| w == b"xref\n").unwrap()];
    let mut out = Vec::with_capacity(pdf.len() + 64);
    let needle = b"/Type /Page /Parent";
    let mut i = 0;
    while let Some(k) = body[i..].windows(needle.len()).position(|w| w == needle) {
        out.extend_from_slice(&body[i..i + k]);
        out.extend_from_slice(format!("/Type /Page /Rotate {deg} /Parent").as_bytes());
        i += k + needle.len();
    }
    out.extend_from_slice(&body[i..]);
    let mut offsets = Vec::new();
    for n in 1.. {
        let tag = format!("\n{n} 0 obj\n");
        match out.windows(tag.len()).position(|w| w == tag.as_bytes()) {
            Some(at) => offsets.push(at + 1),
            None => break,
        }
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
    );
    for o in &offsets {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    out
}

#[test]
fn turned_pages_read_upright_and_mirrored_ones_are_never_read() {
    let dir = tempfile::tempdir().unwrap();
    let scans = dir.path().join("scans");
    // A scanner PDF whose pages carry /Rotate (upright images shown turned), and image files
    // turned 90°, 180° and 270°.
    for (k, deg) in [(0u16, 90u32), (1, 180), (2, 270)] {
        let img = page(k).rotated_quarter(((4 - deg / 90) % 4) as u8);
        write(
            &scans,
            &format!("a-s{k:02}-rotate{deg}.pdf"),
            &rotated(&pdf(&[PdfImage::jpeg(&img, 200.0)]), deg),
        );
    }
    for k in 3..6u16 {
        let turned = page(k).rotated_quarter((k - 2) as u8);
        write(&scans, &format!("b-s{k:02}-turned.png"), &png(&turned));
    }
    // Mirrored (a sheet scanned through the back light) and flipped top to bottom.
    let m = page(6);
    let (w, h) = (m.width(), m.height());
    let mirrored = GrayImage::from_fn(w, h, |x, y| m.get(w - 1 - x, y));
    let flipped = GrayImage::from_fn(w, h, |x, y| m.get(x, h - 1 - y));
    write(&scans, "mirrored.png", &png(&mirrored));
    write(&scans, "flipped.png", &png(&flipped));
    let p = grade(&definition(), &[scans], &dir.path().join("out"), 2);
    assert_eq!(p.session.booklets.len(), 6);
    check_scores(&p);
    for b in &p.session.booklets {
        assert!(b.questions.iter().all(|q| q.status == ResponseStatus::Auto));
    }
    for name in ["mirrored.png", "flipped.png"] {
        let pg = page_of(&p, name);
        assert_eq!(
            (pg.status, pg.booklet),
            (PageStatus::Unattributed, None),
            "{name}"
        );
        let it = p
            .queue(None)
            .into_iter()
            .find(|i| {
                i.page
                    .is_some_and(|x| p.session.pages[x].page_id == pg.page_id)
            })
            .unwrap_or_else(|| panic!("{name}: not queued"));
        assert!(it.severity >= Severity::Error, "{name}: {}", it.code);
        assert!(it.detail.starts_with(name), "{}", it.detail);
    }
}

#[test]
fn pages_that_are_not_this_exams_sheet_are_held_never_graded() {
    let dir = tempfile::tempdir().unwrap();
    let scans = dir.path().join("scans");
    let good = page(0);
    write(&scans, "a-s00.png", &png(&good));
    let (w, h) = (good.width(), good.height());
    let mut noise = omr_imaging::SplitMix64::new(9);
    write(
        &scans,
        "noise.png",
        &png(&GrayImage::from_fn(w, h, |_, _| {
            (noise.next_u64() % 256) as u8
        })),
    );
    write(&scans, "black.png", &png(&blank_page(w, h, 3).inverted()));
    let letter = GrayImage::from_fn(1654, 2339, |x, y| {
        let text = (y / 30) % 2 == 0 && (x / 9) % 3 != 0;
        if text && (150..1500).contains(&x) && (200..2100).contains(&y) {
            40
        } else {
            245
        }
    });
    write(&scans, "letter.png", &png(&letter));
    // Every bubble of the sheet filled: the rings registration needs are gone.
    let e = iau();
    let all: Vec<usize> = (0..e.template().bubbles().len()).collect();
    write(&scans, "all-filled.png", &png(&render(&e, &all, 2)));
    // A generated (own-form) sheet in this legacy exam.
    let t = Template::build(own_spec()).unwrap();
    let key = ExamKey::from_bytes(OWN_KEY);
    let sim = omr_sim::Simulator::new(&t, &key, 200);
    let mut r = rng(3);
    let s = omr_sim::SheetScript::random(&t, 1, 9, None, &omr_sim::MarkRates::clean(), &mut r);
    let own = sim
        .simulate(&s, omr_sim::Profile::Standard, &mut r)
        .unwrap();
    write(&scans, "own-form.png", &png(&to_gray(&own[0].image)));
    let p = grade(&definition(), &[scans], &dir.path().join("out"), 2);
    assert_eq!(p.session.booklets.len(), 1);
    check_scores(&p);
    for name in [
        "noise.png",
        "black.png",
        "letter.png",
        "all-filled.png",
        "own-form.png",
    ] {
        assert_eq!(page_of(&p, name).status, PageStatus::Unattributed, "{name}");
        let it = capture_item(&p, name);
        assert!(
            matches!(it.code.as_str(), "REG-001" | "IDN-002") && it.severity == Severity::Blocker,
            "{name}: {} {:?}",
            it.code,
            it.severity
        );
        assert!(it.detail.starts_with(name), "{}", it.detail);
        assert!(it.detail.contains("not exam material"), "{}", it.detail);
    }
    assert!(p.session.release_blocked().is_some());

    // The other way round: an IAU sheet in an own-form exam.
    let own_dir = dir.path().join("own");
    write(&own_dir, "iau.png", &png(&good));
    let opts = options(&dir.path().join("own-out"), 2);
    let p = ExamProject::grade(&own_definition(), &[own_dir], &opts, &|_| {}).unwrap();
    assert!(p.session.booklets.is_empty());
    let it = capture_item(&p, "iau.png");
    assert!(
        matches!(it.code.as_str(), "REG-001" | "IDN-002"),
        "{}",
        it.code
    );
}

#[test]
fn a_sheet_of_a_form_the_exam_does_not_declare_is_held_with_idn002() {
    // The sheets were printed in forms A, B and C; the exam declares A and B only (and, in a
    // second run, A only). A form-C sheet is never scored under another form's key, nor offered
    // for one: it is an unknown form (IDN-002), held with the fix.
    let dir = tempfile::tempdir().unwrap();
    let mut spec = own_spec();
    spec.forms = 3;
    let t = Template::build(spec.clone()).unwrap();
    let key = ExamKey::from_bytes(OWN_KEY);
    let mut cfg = omr_sim::CorpusConfig::standard(9, 5);
    cfg.roster = own_roster()[..9].to_vec();
    cfg.threads = 2;
    let corpus = dir.path().join("corpus");
    omr_sim::corpus::write_corpus(&t, &key, &cfg, &corpus).unwrap();
    let forms: BTreeMap<u32, u16> = omr_sim::corpus::read_truth(&corpus.join("truth.jsonl"))
        .unwrap()
        .into_iter()
        .map(|p| (p.booklet, p.form))
        .collect();
    for declared in [vec!["A", "B"], vec!["A"]] {
        let mut def = own_definition();
        def.template = TemplateChoice::Generated(Box::new(spec.clone()));
        def.forms = declared.iter().map(|f| (*f).to_string()).collect();
        def.key = KeySource::Provided {
            forms: declared
                .iter()
                .map(|f| FormKey {
                    form: (*f).into(),
                    answers: vec!["A".into(); usize::from(OWN_Q)],
                })
                .collect(),
        };
        def.exam_id = def.derived_exam_id();
        def.validate().unwrap();
        let out = dir.path().join(format!("out-{}", declared.len()));
        let p =
            ExamProject::grade(&def, &[corpus.join("images")], &options(&out, 2), &|_| {}).unwrap();
        let in_exam = |f: u16| usize::from(f) <= declared.len();
        let graded: Vec<u32> = p.session.booklets.iter().map(|b| b.serial).collect();
        let want: Vec<u32> = forms
            .iter()
            .filter(|(_, f)| in_exam(**f))
            .map(|(s, _)| *s)
            .collect();
        let mut got = graded.clone();
        got.sort_unstable();
        assert_eq!(got, want, "forms {declared:?}");
        let held: Vec<_> = p
            .queue(Some(Lane::Capture))
            .into_iter()
            .filter(|i| i.code == "IDN-002")
            .collect();
        assert_eq!(held.len(), forms.len() - want.len(), "forms {declared:?}");
        for it in held {
            assert_eq!(it.severity, Severity::Blocker);
            assert!(it.detail.contains("does not declare"), "{}", it.detail);
        }
        assert!(
            p.session
                .booklets
                .iter()
                .all(|b| b.exceptions.iter().all(|e| e.code != "SCO-005"))
        );
        assert!(p.session.release_blocked().is_some());
    }
}

#[test]
fn scans_at_150_300_400_dpi_read_and_below_100_dpi_are_refused_with_cap002() {
    let dir = tempfile::tempdir().unwrap();
    let scans = dir.path().join("scans");
    // Legacy IAU sheets as JPEG files at 150, 300 and 400 dpi, and at 75 dpi.
    write(
        &scans,
        "a-s00-150dpi.jpg",
        &jpeg(&rescale(&page(0), 3, 4), 90),
    );
    write(
        &scans,
        "a-s01-300dpi.jpg",
        &jpeg(&rescale(&page(1), 3, 2), 90),
    );
    write(
        &scans,
        "a-s02-400dpi.jpg",
        &jpeg(&rescale(&page(2), 2, 1), 90),
    );
    write(
        &scans,
        "a-s03-75dpi.jpg",
        &jpeg(&rescale(&page(3), 3, 8), 90),
    );
    let p = grade(&definition(), &[scans], &dir.path().join("out"), 2);
    assert_eq!(p.session.booklets.len(), 3);
    check_scores(&p);
    let auto =
        |b: &omr_app::model::Booklet| b.questions.iter().all(|q| q.status == ResponseStatus::Auto);
    assert!(p.session.booklets.iter().all(auto));
    assert_eq!(
        codes_of(&p, "a-s00-150dpi.jpg"),
        vec![("CAP-002".to_string(), Severity::Warn)]
    );
    assert!(codes_of(&p, "a-s01-300dpi.jpg").is_empty());
    assert!(codes_of(&p, "a-s02-400dpi.jpg").is_empty());
    assert_eq!(page_of(&p, "a-s03-75dpi.jpg").status, PageStatus::Refused);
    let it = capture_item(&p, "a-s03-75dpi.jpg");
    assert_eq!(
        (it.code.as_str(), it.severity),
        ("CAP-002", Severity::Error)
    );
    assert!(it.detail.contains("rescan"), "{}", it.detail);
    // The WARN is recorded, not queued.
    assert_eq!(p.queue(None).len(), 1);

    // Own forms at 150, 300 and 400 dpi (omr-sim, truth by construction) and at 75 dpi.
    let t = Template::build(own_spec()).unwrap();
    let key = ExamKey::from_bytes(OWN_KEY);
    let mut truth = BTreeMap::new();
    let mut inputs = Vec::new();
    for (i, dpi) in [150u32, 300, 400].into_iter().enumerate() {
        let d = dir.path().join(format!("own{dpi}"));
        let mut cfg = omr_sim::CorpusConfig::standard(4, 77 + u64::from(dpi));
        cfg.roster = own_roster()[i * 4..i * 4 + 4].to_vec();
        cfg.dpi = dpi;
        cfg.serial_start = 1 + 10 * i as u32;
        cfg.threads = 2;
        omr_sim::corpus::write_corpus(&t, &key, &cfg, &d).unwrap();
        for pt in omr_sim::corpus::read_truth(&d.join("truth.jsonl")).unwrap() {
            truth.insert(pt.booklet, pt);
        }
        inputs.push(d.join("images"));
    }
    let low = dir.path().join("own75");
    let sim = omr_sim::Simulator::new(&t, &key, 200);
    let mut r = rng(5);
    let s = omr_sim::SheetScript::random(
        &t,
        1,
        40,
        Some(own_roster()[20].clone()),
        &omr_sim::MarkRates::clean(),
        &mut r,
    );
    let pg = sim
        .simulate(&s, omr_sim::Profile::Standard, &mut r)
        .unwrap();
    write(
        &low,
        "own-75dpi.png",
        &png(&rescale(&to_gray(&pg[0].image), 3, 8)),
    );
    inputs.push(low);
    let opts = options(&dir.path().join("own-out"), 2);
    let p = ExamProject::grade(&own_definition(), &inputs, &opts, &|_| {}).unwrap();
    assert_eq!(p.session.booklets.len(), 12);
    let (mut auto, mut silent) = (0, 0);
    for b in &p.session.booklets {
        for qt in &truth[&b.serial].questions {
            let qs = &b.questions[usize::from(qt.question)];
            if qs.status == ResponseStatus::Auto {
                auto += 1;
                if !matches!(&qt.expected, omr_sim::Expected::Auto(r) if *r == qs.current) {
                    silent += 1;
                }
            }
        }
    }
    assert_eq!(
        silent, 0,
        "silent errors among {auto} auto-accepted answers"
    );
    assert!(auto > 12 * 30, "{auto} auto-accepted");
    let it = capture_item(&p, "own-75dpi.png");
    assert_eq!(
        (it.code.as_str(), it.severity),
        ("CAP-002", Severity::Error)
    );
}

#[test]
fn rescans_and_duplicated_pages_are_col003_when_marks_agree() {
    let dir = tempfile::tempdir().unwrap();
    let e = iau();
    let scans = dir.path().join("scans");
    write(&scans, "a-s00.png", &png(&page(0)));
    // Sheet 1 scanned twice (other noise and position; the marks agree).
    write(&scans, "a-s01.png", &png(&page(1)));
    write(&scans, "b-s01-rescan.png", &png(&sheet(&e, &marks(1), 901)));
    // Sheet 2 twice inside one PDF (the feeder took it twice), and sheet 3 once.
    write(
        &scans,
        "c-s02.pdf",
        &pdf(&[
            PdfImage::jpeg(&page(2), 200.0),
            PdfImage::jpeg(&page(2), 200.0),
            PdfImage::jpeg(&page(3), 200.0),
        ]),
    );
    // A byte-identical copy of a file.
    write(&scans, "d-s00-copy.png", &png(&page(0)));
    // Another sheet bubbling sheet 4's number with other marks: not a rescan.
    write(&scans, "e-s04.png", &png(&page(4)));
    let mut other = Marks::new(&answers(6), &student(4), Some(0));
    other.answers[0] = vec![(key(0, 0) + 2) % 5];
    write(&scans, "f-s04-other.png", &png(&sheet(&e, &other, 77)));
    // A rescan whose other copy carries a light extra mark (an abstention): the marks still
    // agree, and the clean copy (fewer abstentions) is graded although it comes second.
    let mut light = marks(5);
    let o = (answers(5)[3].unwrap() + 2) % 5;
    light.light.push(bubble(&e, LegacyField::Answers, 3, o));
    write(&scans, "g-s05-light.png", &png(&sheet(&e, &light, 55)));
    write(&scans, "h-s05.png", &png(&page(5)));
    let p = grade(&definition(), &[scans], &dir.path().join("out"), 2);
    let s = &p.session;
    // Sheets 0, 1, 2, 3 and 5 once each; sheet 4's number on two booklets.
    assert_eq!(s.booklets.len(), 7);
    let superseded = |name: &str| {
        let pg = page_of(&p, name);
        let col003 = codes_of(&p, name).last() == Some(&("COL-003".to_string(), Severity::Info));
        match (pg.status, pg.booklet) {
            (PageStatus::Duplicate, None) if col003 => true,
            (PageStatus::Read, Some(_)) if !col003 => false,
            other => panic!("{name}: {other:?}"),
        }
    };
    // One copy of sheet 1 is graded (the first in content order: both read cleanly).
    assert!(superseded("a-s01.png") != superseded("b-s01-rescan.png"));
    // Sheet 5: the copy with the abstention is the one superseded.
    assert!(superseded("g-s05-light.png"));
    assert!(!superseded("h-s05.png"));
    let in_pdf: Vec<PageStatus> = s
        .pages
        .iter()
        .filter(|pg| s.sources[pg.source].name == "c-s02.pdf")
        .map(|pg| pg.status)
        .collect();
    assert_eq!(
        in_pdf
            .iter()
            .filter(|&&x| x == PageStatus::Duplicate)
            .count(),
        1
    );
    assert_eq!(
        codes_of(&p, "d-s00-copy.png"),
        vec![("CAP-011".to_string(), Severity::Info)]
    );
    // Only sheet 4's two booklets reach the queue (IDN-005 on both): none of the rescans do.
    let q = p.queue(None);
    assert_eq!(q.len(), 2, "{q:?}");
    assert!(q.iter().all(|i| i.code == "IDN-005"));
    for (bi, b) in s.booklets.iter().enumerate() {
        let name = source_of(&p, bi);
        if name.contains("-s04") {
            continue;
        }
        // (The PDF holds two sheets: the student number says which.)
        let k = (0..6)
            .find(|&k| b.student() == Some(student(k).as_str()))
            .unwrap();
        assert_eq!(
            result(&p, bi),
            (AssessmentStatus::Scored, hand(k)),
            "{name}"
        );
    }
    // The superseded copies are in the store with their link.
    let st = p.store().unwrap();
    st.verify().unwrap();
    let rescans = st
        .exceptions(None)
        .unwrap()
        .into_iter()
        .filter(|e| e.exception.code == "COL-003")
        .count();
    assert_eq!(rescans, 3);
}

#[test]
fn every_bubble_filled_or_none_marked_is_never_credited() {
    let dir = tempfile::tempdir().unwrap();
    let scans = dir.path().join("scans");
    write(&scans, "a-s00.png", &png(&page(0)));
    // The ID and version only: every answer blank (held, never scored 0).
    let m = Marks::new(&vec![None; usize::from(N)], &student(1), Some(1));
    write(&scans, "a-s01-noanswers.png", &png(&sheet(&iau(), &m, 7)));
    // No mark at all.
    write(
        &scans,
        "a-s02-nothing.png",
        &png(&sheet(&iau(), &Marks::default(), 8)),
    );
    let p = grade(&definition(), &[scans], &dir.path().join("out"), 2);
    assert_eq!(p.session.booklets.len(), 3);
    for name in ["a-s01-noanswers.png", "a-s02-nothing.png"] {
        let bi = (0..3).find(|&b| source_of(&p, b) == name).unwrap();
        assert_eq!(p.session.booklets[bi].hold.as_deref(), Some("MRK-015"));
        assert_eq!(result(&p, bi).0, AssessmentStatus::Provisional);
        assert!(
            p.queue(None)
                .iter()
                .any(|i| i.booklet == Some(bi) && i.code == "MRK-015")
        );
    }

    // Own form: every bubble filled reads as multiple marks, never credited.
    let t = Template::build(own_spec()).unwrap();
    let key = ExamKey::from_bytes(OWN_KEY);
    let sim = omr_sim::Simulator::new(&t, &key, 200);
    let mut r = rng(5);
    let all: Vec<Response> = (0..OWN_Q)
        .map(|_| Response::Multi(vec![0, 1, 2, 3]))
        .collect();
    let s = omr_sim::SheetScript::from_responses(
        &t,
        1,
        31,
        Some(own_roster()[30].clone()),
        &all,
        &omr_sim::MarkRates::clean(),
        &mut r,
    );
    let pg = sim
        .simulate(&s, omr_sim::Profile::Standard, &mut r)
        .unwrap();
    let own = dir.path().join("own");
    write(&own, "all.png", &png(&to_gray(&pg[0].image)));
    let opts = options(&dir.path().join("own-out"), 2);
    let p = ExamProject::grade(&own_definition(), &[own], &opts, &|_| {}).unwrap();
    assert_eq!(p.session.booklets.len(), 1);
    for qs in &p.session.booklets[0].questions {
        assert!(
            qs.status == ResponseStatus::Pending
                || matches!(qs.current, Response::Multi(ref v) if v.len() >= 2),
            "{:?} {:?}",
            qs.status,
            qs.current
        );
    }
    let sc = p.scoring.as_ref().unwrap();
    assert_eq!(sc.run.as_ref().unwrap().students[0].raw.to_string(), "0");
}

#[test]
fn a_void_and_an_any_of_key_entry_score_as_declared() {
    let dir = tempfile::tempdir().unwrap();
    let e = iau();
    let scans = dir.path().join("scans");
    // Question 1 is voided, question 2 accepts A or C. The sheets answer question 2 with A, C,
    // B, A and C together, and nothing.
    let q2: [Vec<u8>; 5] = [vec![0], vec![2], vec![1], vec![0, 2], vec![]];
    for (k, a) in q2.iter().enumerate() {
        let mut m = marks(k as u16);
        m.answers[1] = a.clone();
        write(
            &scans,
            &format!("a-s{k:02}.png"),
            &png(&sheet(&e, &m, 40 + k as u64)),
        );
    }
    let mut def = definition();
    if let KeySource::Provided { forms } = &mut def.key {
        for f in forms {
            f.answers[0] = "*".into();
            f.answers[1] = "A|C".into();
        }
    }
    def.validate().unwrap();
    let p = grade(&def, &[scans], &dir.path().join("out"), 2);
    assert_eq!(p.session.booklets.len(), 5);
    for bi in 0..5 {
        let k = sheet_no(&p, bi);
        let rest = (2..N)
            .filter(|&q| answers(k)[usize::from(q)] == Some(key(k % 2, q)))
            .count();
        // A or C alone is credited; B, both A and C, or nothing is not.
        let credit = usize::from(k <= 1);
        let sc = p.scoring.as_ref().unwrap();
        let st = &sc.run.as_ref().unwrap().students[sc.row_of[bi].unwrap()];
        assert_eq!(st.raw.to_string(), (rest + credit).to_string(), "sheet {k}");
        assert_eq!(st.max.to_string(), (N - 1).to_string(), "void dropped");
        if k == 3 {
            assert_eq!(
                p.session.booklets[bi].questions[1].recognized.response,
                Response::Multi(vec![0, 2])
            );
        }
    }
}

#[test]
fn an_incomplete_key_never_reads_as_releasable() {
    let dir = tempfile::tempdir().unwrap();
    let scans = dir.path().join("scans");
    for k in 0..3u16 {
        write(&scans, &format!("a-s{k:02}.png"), &png(&page(k)));
    }
    // Question 4 of form B left empty in the key file.
    let mut def = definition();
    if let KeySource::Provided { forms } = &mut def.key {
        forms[1].answers[3] = String::new();
    }
    def.validate().unwrap();
    let out = dir.path().join("out");
    let mut p = grade(&def, &[scans], &out, 2);
    let why = p.session.release_blocked().expect("release blocked");
    assert!(why.contains("SCO-003") && why.contains("B-Q4"), "{why}");
    // Filling it in releases the exam and scores every sheet by hand.
    p.set_key(1, 4, &char::from(b'A' + key(1, 3)).to_string())
        .unwrap();
    // The new score run's mis-key check (§12.2) reads "computing…" until it has run.
    assert!(p.session.release_blocked().unwrap().contains("computing"));
    p.judge_flags().unwrap();
    assert_eq!(p.session.release_blocked(), None);
    check_scores(&p);
    // Clearing an entry again blocks again.
    p.set_key(0, 1, "").unwrap();
    let why = p.session.release_blocked().expect("release blocked");
    assert!(why.contains("A-Q1"), "{why}");
    p.export(&out).unwrap();
    p.store().unwrap().verify().unwrap();
}

/// Resolves `id` with the sheet's true answer, giving a reason when that overrides the
/// engine's proposal.
fn resolve_with_truth(p: &mut ExamProject, id: u64, truth: Decision) {
    let reason = p
        .overrides_engine(id, &truth)
        .then(|| ReviewReason::code("ERASURE_RESIDUE"));
    p.apply_with_reason(
        id,
        Action::Resolve {
            decision: truth,
            accepted_suggestion: false,
            note: String::new(),
        },
        reason,
    )
    .unwrap();
}

/// Sheet `k` with a light second mark on question 4 (a review item) when `light`.
fn write_sheet(dir: &Path, k: u16, light: bool) {
    let e = iau();
    let mut m = marks(k);
    if light {
        let o = (answers(k)[3].unwrap() + 2) % 5;
        m.light.push(bubble(&e, LegacyField::Answers, 3, o));
    }
    write(
        dir,
        &format!("a-s{k:02}.png"),
        &png(&sheet(&e, &m, 300 + u64::from(k))),
    );
}

#[test]
fn review_then_key_change_then_undo_rescore_and_stay_audited() {
    let dir = tempfile::tempdir().unwrap();
    let scans = dir.path().join("scans");
    for k in 0..4u16 {
        write_sheet(&scans, k, k == 1);
    }
    let mut p = grade(&definition(), &[scans], &dir.path().join("out"), 2);
    let (id, bi, q) = p
        .queue(None)
        .iter()
        .find(|i| i.kind == ItemKind::Mark)
        .map(|i| (i.id, i.booklet.unwrap(), usize::from(i.question.unwrap())))
        .unwrap();
    assert_eq!(sheet_no(&p, bi), 1);
    incremental_equals_rebuild(&p);
    assert_eq!(result(&p, bi).0, AssessmentStatus::Provisional);
    resolve_with_truth(&mut p, id, Decision::Option(answers(1)[3].unwrap()));
    incremental_equals_rebuild(&p);
    assert_eq!(result(&p, bi), (AssessmentStatus::Scored, hand(1)));
    // The key of that question now accepts every option: the reviewed answer is credited.
    let f = p.session.booklets[bi].form.unwrap();
    p.set_key(f, q as u32 + 1, "A|B|C|D|E").unwrap();
    incremental_equals_rebuild(&p);
    assert_ne!(result(&p, bi).1, hand(1));
    // Undo reverts the review decision (not the key): the answer is pending again and the
    // total provisional, under the changed key; the item is open again.
    assert_eq!(p.undo().unwrap(), Some(id));
    incremental_equals_rebuild(&p);
    let qs = &p.session.booklets[bi].questions[q];
    assert_eq!(qs.status, ResponseStatus::Pending);
    assert_eq!(qs.current, qs.recognized.response);
    assert_eq!(result(&p, bi).0, AssessmentStatus::Provisional);
    assert!(p.queue(None).iter().any(|i| i.id == id));
    // Voiding the question settles the booklet without a decision.
    p.set_key(f, q as u32 + 1, "*").unwrap();
    incremental_equals_rebuild(&p);
    assert_eq!(result(&p, bi).0, AssessmentStatus::Scored);
    // Audited in order, and the chain verifies.
    let events: Vec<String> = p
        .store()
        .unwrap()
        .events()
        .unwrap()
        .into_iter()
        .map(|e| e.event_type)
        .skip_while(|t| t != "ResponseCorrected")
        .collect();
    let mut at = 0;
    for want in [
        "ResponseCorrected",
        "ExceptionResolved",
        "KeyVersionPublished",
        "KeyEntryChanged",
        "ExceptionReopened",
        "ReviewActionUndone",
        "KeyVersionPublished",
        "KeyEntryChanged",
    ] {
        at += events[at..]
            .iter()
            .position(|t| t == want)
            .unwrap_or_else(|| panic!("{want} missing after {at} in {events:?}"))
            + 1;
    }
    p.store().unwrap().verify().unwrap();
    // A reopened project holds the same state.
    let root = p.root().unwrap();
    let (booklets, key) = (p.session.booklets.clone(), p.session.key.clone());
    p.close().unwrap();
    let q = ExamProject::open(&root, "principal:test").unwrap();
    assert_eq!((&q.session.booklets, &q.session.key), (&booklets, &key));
    incremental_equals_rebuild(&q);
}

#[test]
fn undo_after_appending_a_batch_stops_at_the_append_and_rescores() {
    let dir = tempfile::tempdir().unwrap();
    let (day1, day2) = (dir.path().join("day1"), dir.path().join("day2"));
    for k in 0..6u16 {
        write_sheet(if k < 3 { &day1 } else { &day2 }, k, k == 1 || k == 5);
    }
    let out = dir.path().join("out");
    let mut p = grade(&definition(), &[day1], &out, 2);
    let item = |p: &ExamProject, k: u16| {
        p.queue(None)
            .iter()
            .find(|i| i.kind == ItemKind::Mark && sheet_no(p, i.booklet.unwrap()) == k)
            .map(|i| i.id)
            .unwrap()
    };
    let id1 = item(&p, 1);
    resolve_with_truth(&mut p, id1, Decision::Option(answers(1)[3].unwrap()));
    let r = p.add_scans(&[day2], &options(&out, 2), &|_| {}).unwrap();
    assert_eq!((r.booklets_after, r.carried_over, r.conflicts), (6, 1, 0));
    incremental_equals_rebuild(&p);
    let before = p.session.booklets.clone();
    // The only decision so far was carried over the append: undo refuses (exit 6) and changes
    // nothing.
    let err = p.undo().unwrap_err();
    assert_eq!(err.exit_code(), 6);
    assert!(err.to_string().contains("appended"), "{err}");
    assert_eq!(p.session.booklets, before);
    // A decision taken after the append undoes normally; then undo stops again.
    let id5 = item(&p, 5);
    resolve_with_truth(&mut p, id5, Decision::Option(answers(5)[3].unwrap()));
    incremental_equals_rebuild(&p);
    assert_eq!(p.undo().unwrap(), Some(id5));
    incremental_equals_rebuild(&p);
    assert_eq!(p.session.booklets, before);
    assert!(p.undo().is_err());
    check_scores(&p);
    p.store().unwrap().verify().unwrap();
    let root = p.root().unwrap();
    p.close().unwrap();
    let q = ExamProject::open(&root, "principal:test").unwrap();
    assert_eq!(q.session.booklets, before);
    incremental_equals_rebuild(&q);
}

#[test]
fn a_cleaner_rescan_in_a_later_batch_never_silently_replaces_a_reviewers_decision() {
    // Day 1: sheet 1 has a light extra mark on question 4 and the reviewer decides that
    // question (here: for the light option, not what the sheet's clean rescan reads). Day 2
    // brings a clean rescan of sheet 1: it supersedes the day-1 copy (fewer abstentions), and
    // its reading contradicts the reviewer, so the human value is kept and SYS-032 is queued.
    let dir = tempfile::tempdir().unwrap();
    let (day1, day2) = (dir.path().join("day1"), dir.path().join("day2"));
    for k in 0..3u16 {
        write_sheet(&day1, k, k == 1);
    }
    write(
        &day2,
        "b-s01-rescan.png",
        &png(&sheet(&iau(), &marks(1), 909)),
    );
    write_sheet(&day2, 3, false);
    let out = dir.path().join("out");
    let mut p = grade(&definition(), &[day1], &out, 2);
    let id = p
        .queue(None)
        .iter()
        .find(|i| i.kind == ItemKind::Mark)
        .map(|i| i.id)
        .unwrap();
    let light = (answers(1)[3].unwrap() + 2) % 5;
    resolve_with_truth(&mut p, id, Decision::Option(light));
    let r = p.add_scans(&[day2], &options(&out, 2), &|_| {}).unwrap();
    assert_eq!(r.conflicts, 1, "{r:?}");
    let bi = p
        .session
        .booklets
        .iter()
        .position(|b| b.student() == Some(student(1).as_str()))
        .unwrap();
    assert_eq!(source_of(&p, bi), "b-s01-rescan.png");
    let qs = &p.session.booklets[bi].questions[3];
    assert_eq!(
        (qs.status, &qs.current),
        (ResponseStatus::Pending, &Response::Single(light))
    );
    assert!(
        p.queue(Some(Lane::Marks))
            .iter()
            .any(|i| i.code == "SYS-032" && i.booklet == Some(bi))
    );
    assert_eq!(result(&p, bi).0, AssessmentStatus::Provisional);
    incremental_equals_rebuild(&p);
    p.store().unwrap().verify().unwrap();
}

#[test]
fn a_group_decision_refused_part_way_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let scans = dir.path().join("scans");
    // Two sheets with their last ID column left blank: one identity group (IDN-004 id_blank).
    for k in [0u16, 1] {
        let mut m = marks(k);
        m.id[9].clear();
        write(
            &scans,
            &format!("a-s{k:02}.png"),
            &png(&sheet(&iau(), &m, 60 + u64::from(k))),
        );
    }
    let mut def = definition();
    def.roster = Some(
        (0..4)
            .map(|k| RosterStudent {
                id: student(k),
                name: format!("Student {k}"),
                name_ar: String::new(),
                section: "101".into(),
                numeric_cell: false,
            })
            .collect(),
    );
    def.validate().unwrap();
    let mut p = grade(&def, &[scans], &dir.path().join("out"), 2);
    let ids: Vec<u64> = p.queue(Some(Lane::Identity)).iter().map(|i| i.id).collect();
    assert_eq!(ids.len(), 2);
    let booklets = p.session.booklets.clone();
    let history = p.session.review.history.len();
    // One student for the whole group: the second booklet refuses (IDN-005), so the first is
    // reverted too.
    let err = p
        .apply_with_reason(
            ids[0],
            Action::ApplyToGroup {
                decision: Decision::Student(student(3)),
            },
            Some(ReviewReason::code("ID_CORRECTION")),
        )
        .unwrap_err();
    assert_eq!(err.exit_code(), 6);
    assert!(err.to_string().contains("nothing changed"), "{err}");
    assert_eq!(p.session.booklets, booklets);
    assert_eq!(p.queue(Some(Lane::Identity)).len(), 2);
    assert!(p.session.review.history[history..].iter().all(|r| r.undone));
    incremental_equals_rebuild(&p);
    p.store().unwrap().verify().unwrap();
    // One by one, each is accepted.
    for id in ids {
        let bi = p.session.review.item(id).unwrap().booklet.unwrap();
        let d = Decision::Student(student(sheet_no(&p, bi)));
        let reason = p
            .overrides_engine(id, &d)
            .then(|| ReviewReason::code("ID_CORRECTION"));
        p.apply_with_reason(
            id,
            Action::Resolve {
                decision: d,
                accepted_suggestion: false,
                note: String::new(),
            },
            reason,
        )
        .unwrap();
    }
    assert!(p.queue(Some(Lane::Identity)).is_empty());
}

/// Decision files that must be byte-identical across runs (A5).
const DECISION_FILES: [&str; 6] = [
    "results.csv",
    "item_analysis.csv",
    "test_stats.csv",
    "booklets.json",
    "review_queue.json",
    "key.csv",
];

#[test]
fn hostile_batches_are_deterministic_across_threads_and_input_order() {
    let dir = tempfile::tempdir().unwrap();
    let scans = dir.path().join("scans");
    let e = iau();
    for k in 0..5u16 {
        write(&scans, &format!("a-s{k:02}.png"), &png(&page(k)));
    }
    write(&scans, "b-s01-rescan.png", &png(&sheet(&e, &marks(1), 901)));
    write(&scans, "c-s00-copy.png", &png(&page(0)));
    write(&scans, "d-empty.pdf", b"");
    write(&scans, "d-empty.jpg", b"");
    let j = jpeg(&page(5), 85);
    write(&scans, "e-cut.jpg", &j[..j.len() * 9 / 10]);
    write(&scans, "f-s06-batch.tif", &tiff(&[page(6), page(7)]));
    write(&scans, "g-blank.png", &png(&blank_page(1800, 1300, 4)));
    write(
        &scans,
        "h-s08-150dpi.jpg",
        &jpeg(&rescale(&page(8), 3, 4), 90),
    );
    write(&scans, "i-75dpi.jpg", &jpeg(&rescale(&page(9), 3, 8), 90));
    let mut files: Vec<PathBuf> = std::fs::read_dir(&scans)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    let run = |name: &str, threads: usize, inputs: &[PathBuf]| -> (ExamProject, PathBuf) {
        let out = dir.path().join(name);
        (grade(&definition(), inputs, &out, threads), out)
    };
    let (a, da) = run("t1", 1, &files);
    // Sheets 0–4, 6, 7 (TIFF) and 8; the rescan, the copy, the blank page and the four
    // refused files are not booklets.
    assert_eq!(a.session.booklets.len(), 8);
    let mut reversed = files.clone();
    reversed.reverse();
    let mut shuffled = files.clone();
    let mut r = omr_imaging::SplitMix64::new(42);
    for i in (1..shuffled.len()).rev() {
        shuffled.swap(i, (r.next_u64() % (i as u64 + 1)) as usize);
    }
    for (name, threads, inputs) in [
        ("t3", 3, &files),
        ("t2-reversed", 2, &reversed),
        ("t4-shuffled", 4, &shuffled),
    ] {
        let (b, db) = run(name, threads, inputs);
        assert_eq!(a.session.pages, b.session.pages, "{name}");
        assert_eq!(a.session.booklets, b.session.booklets, "{name}");
        for f in DECISION_FILES {
            assert!(
                std::fs::read(da.join(f)).unwrap() == std::fs::read(db.join(f)).unwrap(),
                "{name}: {f} differs"
            );
        }
    }
}

// ---- Own-form fixtures.

const OWN_Q: u16 = 40;
const OWN_KEY: [u8; 32] = [0x6b; 32];

fn own_spec() -> TemplateSpec {
    let mut s = TemplateSpec::standard(
        ExamId(uuid::Uuid::from_u128(
            0x0190_aaaa_bbbb_7ccc_8ddd_0000_0000_0099,
        )),
        OWN_Q,
        4,
        CancelMode::EraseOnly,
    );
    s.forms = 2;
    s.title = "Adversarial own form".into();
    s
}

fn own_roster() -> Vec<String> {
    (0..40).map(|i| format!("4210{:05}", 100 + i * 7)).collect()
}

fn own_definition() -> ExamDefinition {
    let mut d = ExamDefinition::new(
        "Adversarial own form",
        TemplateChoice::Generated(Box::new(own_spec())),
        OWN_Q,
        4,
        vec!["A".into(), "B".into()],
        CancelMode::EraseOnly,
    );
    d.page_code_key_hex = Some(hex::encode(OWN_KEY));
    d.key = KeySource::Provided {
        forms: ["A", "B"]
            .iter()
            .map(|f| FormKey {
                form: (*f).into(),
                answers: (0..OWN_Q)
                    .map(|q| ["A", "B", "C", "D"][usize::from(q % 4)].to_string())
                    .collect(),
            })
            .collect(),
    };
    d.roster = Some(
        own_roster()
            .into_iter()
            .map(|id| RosterStudent {
                id,
                name: "Student".into(),
                name_ar: String::new(),
                section: "101".into(),
                numeric_cell: false,
            })
            .collect(),
    );
    d.id_scheme = IdSchemeChoice::Digits(9);
    d.exam_id = d.derived_exam_id();
    d.validate().unwrap();
    d
}

/// Peak resident memory of this process in KiB (Linux `/proc/self/status`), after resetting
/// the high-water mark when `reset` (writing 5 to `clear_refs`).
fn peak_rss_kib(reset: bool) -> Option<u64> {
    if reset {
        std::fs::write("/proc/self/clear_refs", "5").ok()?;
    }
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

/// A 500-page scanner PDF (`omr-sim` own forms, 200-dpi JPEG pages) graded through the whole
/// application at 4 threads, store and exports included: decoded pages are not retained (the
/// peak memory grows far less than one decoded page per page), throughput holds (≥ the A2
/// floor in release) and nothing is read silently wrong. `scripts/acceptance.sh` runs it in
/// release, alone.
#[test]
#[ignore = "500-page corpus: release build, run by scripts/acceptance.sh"]
fn req_a2_a_500_page_pdf_keeps_memory_bounded_and_throughput() {
    let dir = tempfile::tempdir().unwrap();
    let t = Template::build(own_spec()).unwrap();
    let key = ExamKey::from_bytes(OWN_KEY);
    let corpus = dir.path().join("corpus");
    let mut cfg = omr_sim::CorpusConfig::standard(500, 20_261_017);
    cfg.roster = (0..500)
        .map(|i| format!("43{:07}", 1_000 + i * 13))
        .collect();
    omr_sim::corpus::write_corpus(&t, &key, &cfg, &corpus).unwrap();
    let truth: BTreeMap<u32, omr_sim::PageTruth> =
        omr_sim::corpus::read_truth(&corpus.join("truth.jsonl"))
            .unwrap()
            .into_iter()
            .map(|p| (p.booklet, p))
            .collect();
    // One PDF holding every page's JPEG as it is (DCTDecode, not re-encoded).
    let mut files: Vec<PathBuf> = std::fs::read_dir(corpus.join("images"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    let pages: Vec<PdfImage> = files
        .iter()
        .map(|f| {
            let (w, h) = image::image_dimensions(f).unwrap();
            PdfImage {
                width: w as usize,
                height: h as usize,
                bits: 8,
                data: std::fs::read(f).unwrap(),
                filter: Some("DCTDecode"),
                dpi: 200.0,
                components: 1,
            }
        })
        .collect();
    let bytes = pdf(&pages);
    drop(pages);
    let scans = dir.path().join("scans");
    write(&scans, "batch-500.pdf", &bytes);
    let pdf_mib = bytes.len() as f64 / f64::from(1u32 << 20);
    drop(bytes);
    std::fs::remove_dir_all(corpus.join("images")).unwrap();

    let mut def = own_definition();
    def.roster = Some(
        cfg.roster
            .iter()
            .map(|id| RosterStudent {
                id: id.clone(),
                name: "Student".into(),
                name_ar: String::new(),
                section: "101".into(),
                numeric_cell: false,
            })
            .collect(),
    );
    def.validate().unwrap();
    let base = peak_rss_kib(true);
    let t0 = std::time::Instant::now();
    let out = dir.path().join("out");
    let mut p = ExamProject::grade(&def, &[scans], &options(&out, 4), &|_| {}).unwrap();
    let grade_s = t0.elapsed().as_secs_f64();
    p.export(&out).unwrap();
    let wall_s = t0.elapsed().as_secs_f64();
    let peak = peak_rss_kib(false);
    let s = &p.session;
    assert_eq!((s.pages.len(), s.booklets.len()), (500, 500));
    let (mut auto, mut silent, mut total) = (0usize, 0usize, 0usize);
    for b in &s.booklets {
        for qt in &truth[&b.serial].questions {
            total += 1;
            let qs = &b.questions[usize::from(qt.question)];
            if qs.status == ResponseStatus::Auto {
                auto += 1;
                if !matches!(&qt.expected, omr_sim::Expected::Auto(r) if *r == qs.current) {
                    silent += 1;
                }
            }
        }
    }
    let whole_pps = 500.0 / grade_s;
    // A decoded 200-dpi A4 page is about 3.9 MB: retaining them would cost about 1.9 GB.
    let growth_mib = base
        .zip(peak)
        .map(|(b, p)| p.saturating_sub(b) as f64 / 1024.0);
    let v = serde_json::json!({
        "pages": 500,
        "pdf_mib": pdf_mib,
        "threads": 4,
        "grade_s": grade_s,
        "grade_and_export_s": wall_s,
        "page_stage_pages_per_s": 500.0 / (s.timings.page_stage_ms / 1e3),
        "whole_app_pages_per_s": whole_pps,
        "peak_rss_mib": peak.map(|k| k as f64 / 1024.0),
        "peak_rss_growth_mib": growth_mib,
        "questions": total,
        "auto_accepted": auto,
        "silent_errors": silent,
        "release_build": !cfg!(debug_assertions),
    });
    println!("{v}");
    if let Ok(dir) = std::env::var("OMR_ACCEPT_OUT") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            Path::new(&dir).join("a2-large-pdf-500.json"),
            serde_json::to_vec_pretty(&v).unwrap(),
        )
        .unwrap();
    }
    assert_eq!(silent, 0);
    if let Some(g) = growth_mib {
        assert!(g < 1024.0, "peak memory grew by {g:.0} MiB");
    }
    if !cfg!(debug_assertions) {
        assert!(whole_pps >= 40.0, "{whole_pps:.1} pages/s");
    }
}
