//! The legacy IAU path through the whole app without real data: the committed blank form
//! (layout only, no PII) rendered at 200 dpi with synthetic pencil marks, written as PNG files
//! and graded end to end (ingest → legacy recognition → bubbled version → rosterless identity
//! → scoring → exports). Checks the version (form) wiring, the question count, scores against a
//! hand computation, and the inferred key of a one-form exam whose version bubbles are blank.

use image::ImageEncoder;
use omr_app::model::{FormSource, KeyCell};
use omr_app::{
    Action, Decision, ExamDefinition, ExamProject, FormKey, GradeOptions, ItemKind, KeySource,
    TemplateChoice,
};
use omr_forms::legacy::LegacyField;
use omr_imaging::{Affine, GrayImage, SplitMix64, Warp, bilinear};
use omr_model::{CancelMode, PointPx, Response};
use omr_recog::LegacyEngine;
use std::path::{Path, PathBuf};

const N: u16 = 29;
const SHEETS: u16 = 6;

/// Key of form `v` (0 = A): option per question.
fn key(v: u16, q: u16) -> u8 {
    ((q * 3 + v) % 5) as u8
}

/// What sheet `k` answers (form `v`): wrong on every (k + 2)-th question, blank on a few.
fn answer(k: u16, v: u16, q: u16) -> Option<u8> {
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

fn index(engine: &LegacyEngine, field: LegacyField, group: u16, option: u8) -> usize {
    engine
        .template()
        .bubbles()
        .iter()
        .position(|b| b.field == field && b.group == group && b.option == option)
        .expect("bubble")
}

/// The blank form at 200 dpi (slightly rotated and shifted) with pencil fills.
fn render(engine: &LegacyEngine, fills: &[usize], seed: u64) -> GrayImage {
    let t = engine.template();
    let (tw, th) = t.frame_size();
    let (w, h) = (tw as usize + 40, th as usize + 30);
    let angle: f64 = 0.006 * (seed % 3) as f64 - 0.004;
    let fwd = Affine {
        m: [
            angle.cos(),
            -angle.sin(),
            21.0,
            angle.sin(),
            angle.cos(),
            14.0,
        ],
    };
    let inv = fwd.inverse().expect("invertible");
    let blank = engine.blank();
    let mut img = GrayImage::from_fn(w, h, |x, y| {
        let p = inv.map(PointPx {
            x: x as f64 + 0.5,
            y: y as f64 + 0.5,
        });
        bilinear(blank, p.x, p.y, 255)
    });
    let mut rng = SplitMix64::new(seed);
    let r = t.bubble_radius_px() * 0.82;
    for &j in fills {
        let c = fwd.map(t.bubbles()[j].centre);
        for y in (c.y - r).floor() as isize..=(c.y + r).ceil() as isize {
            for x in (c.x - r).floor() as isize..=(c.x + r).ceil() as isize {
                let (dx, dy) = (x as f64 + 0.5 - c.x, y as f64 + 0.5 - c.y);
                if dx * dx + dy * dy > r * r || x < 0 || y < 0 {
                    continue;
                }
                let (xu, yu) = (x as usize, y as usize);
                if xu >= w || yu >= h {
                    continue;
                }
                let ink = 25 + (rng.next_u64() % 51) as u32;
                let v = u32::from(img.get(xu, yu)) * ink / 255;
                img.set(xu, yu, v as u8);
            }
        }
    }
    img
}

fn png(img: &GrayImage) -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(
            img.as_raw(),
            img.width() as u32,
            img.height() as u32,
            image::ExtendedColorType::L8,
        )
        .expect("png");
    out
}

/// Writes the sheets; `version(k)` is the bubbled version (None: left blank).
fn write_sheets(dir: &Path, version: &dyn Fn(u16) -> Option<u16>) -> PathBuf {
    write_sheets_as(dir, version, &student)
}

/// [`write_sheets`] with the bubbled student number of each sheet.
fn write_sheets_as(
    dir: &Path,
    version: &dyn Fn(u16) -> Option<u16>,
    id_of: &dyn Fn(u16) -> String,
) -> PathBuf {
    let engine = LegacyEngine::iau_ccsit_v1().unwrap();
    std::fs::create_dir_all(dir).unwrap();
    for k in 0..SHEETS {
        let v = version(k).unwrap_or(0);
        let mut fills = Vec::new();
        for q in 0..N {
            if let Some(o) = answer(k, v, q) {
                fills.push(index(&engine, LegacyField::Answers, q, o));
            }
        }
        for (c, ch) in id_of(k).bytes().enumerate() {
            fills.push(index(
                &engine,
                LegacyField::StudentNumber,
                c as u16,
                ch - b'0',
            ));
        }
        if let Some(v) = version(k) {
            fills.push(index(&engine, LegacyField::Version, 0, v as u8));
        }
        let img = render(&engine, &fills, 100 + u64::from(k));
        std::fs::write(dir.join(format!("sheet-{k:02}.png")), png(&img)).unwrap();
    }
    dir.to_path_buf()
}

fn definition(forms: &[&str], key: KeySource) -> ExamDefinition {
    let mut d = ExamDefinition::new(
        "Legacy quiz",
        TemplateChoice::Legacy("iau-ccsit-v1".into()),
        N,
        5,
        forms.iter().map(|f| (*f).to_string()).collect(),
        CancelMode::EraseOnly,
    );
    d.key = key;
    d.validate().unwrap();
    d
}

fn grade(def: &ExamDefinition, input: &Path, out: &Path) -> ExamProject {
    let opts = GradeOptions {
        threads: 3,
        project_dir: Some(out.join("project.omrproj")),
        snapshot_utc: Some("2026-10-17T10:00:00Z".into()),
        operator: "principal:test".into(),
        max_pages: None,
        cancel: Default::default(),
    };
    let mut p = ExamProject::grade(def, &[input.to_path_buf()], &opts, &|_| {}).unwrap();
    p.export(out).unwrap();
    p
}

fn sheet_of(p: &ExamProject, bi: usize) -> u16 {
    let b = &p.session.booklets[bi];
    let name = &p.session.sources[p.session.pages[b.pages[0]].source].name;
    name[6..8].parse().unwrap()
}

#[test]
fn legacy_sheets_with_bubbled_versions_grade_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    // The definition lists B before A (as a key file may): forms map by the bubbled letter.
    let forms = ["B", "A"];
    let def = definition(
        &forms,
        KeySource::Provided {
            forms: forms
                .iter()
                .map(|f| FormKey {
                    form: (*f).into(),
                    answers: (0..N)
                        .map(|q| {
                            char::from(b'A' + key(u16::from(f.as_bytes()[0] - b'A'), q)).to_string()
                        })
                        .collect(),
                })
                .collect(),
        },
    );
    let input = write_sheets(&dir.path().join("scans"), &|k| Some(k % 2));
    let out = dir.path().join("out");
    let mut p = grade(&def, &input, &out);
    assert_eq!(p.session.booklets.len(), usize::from(SHEETS));
    for bi in 0..p.session.booklets.len() {
        let k = sheet_of(&p, bi);
        let b = &p.session.booklets[bi];
        let printed = if k.is_multiple_of(2) { "A" } else { "B" };
        assert_eq!(b.form_source, FormSource::Bubbled, "sheet {k}");
        assert_eq!(
            p.session.definition.forms[b.form.unwrap()],
            printed,
            "sheet {k}"
        );
        assert_eq!(b.questions.len(), usize::from(N));
        for (q, qs) in b.questions.iter().enumerate() {
            let want = answer(k, k % 2, q as u16).map_or(Response::Blank, Response::Single);
            if qs.status == omr_app::model::ResponseStatus::Auto {
                assert_eq!(qs.current, want, "sheet {k} q{q}");
            }
        }
        assert_eq!(b.identity.student.as_deref(), Some(student(k).as_str()));
    }
    // Resolve whatever is open with the truth, then compare the scores with a hand count.
    let open: Vec<(u64, ItemKind, Option<usize>, Option<u16>)> = p
        .queue(None)
        .iter()
        .map(|i| (i.id, i.kind, i.booklet, i.question))
        .collect();
    for (id, kind, b, q) in open {
        let d = match kind {
            ItemKind::Mark => {
                let k = sheet_of(&p, b.unwrap());
                answer(k, k % 2, q.unwrap()).map_or(Decision::Blank, Decision::Option)
            }
            ItemKind::Identity => Decision::Student(student(sheet_of(&p, b.unwrap()))),
            ItemKind::Form => {
                let k = sheet_of(&p, b.unwrap());
                let letter = if k.is_multiple_of(2) { "A" } else { "B" };
                Decision::Form(forms.iter().position(|f| *f == letter).unwrap() as u8)
            }
            ItemKind::Notice => Decision::Acknowledge,
        };
        p.apply(
            id,
            Action::Resolve {
                decision: d,
                accepted_suggestion: false,
                note: String::new(),
            },
        )
        .unwrap();
    }
    assert_eq!(p.session.release_blocked(), None);
    let sc = p.scoring.as_ref().unwrap();
    let run = sc.run.as_ref().unwrap();
    for bi in 0..p.session.booklets.len() {
        let k = sheet_of(&p, bi);
        let correct = (0..N)
            .filter(|&q| answer(k, k % 2, q) == Some(key(k % 2, q)))
            .count();
        let row = sc.row_of[bi].unwrap();
        assert_eq!(
            run.students[row].raw.to_string(),
            correct.to_string(),
            "sheet {k}"
        );
    }
    p.close().unwrap();
}

#[test]
fn a_one_form_exam_infers_its_key_from_every_booklet() {
    let dir = tempfile::tempdir().unwrap();
    // One version only: nobody bubbles it.
    let input = write_sheets(&dir.path().join("scans"), &|_| None);
    let def = definition(&["A"], KeySource::Inferred);
    let p = grade(&def, &input, &dir.path().join("out"));
    let s = &p.session;
    assert_eq!(s.booklets.len(), usize::from(SHEETS));
    let k = s.key.as_ref().unwrap();
    assert!(k.inferred);
    for q in 0..N {
        // The modal answer over all six sheets (ties: the lower option); never voided for
        // lack of booklets that declare the one form.
        let mut counts = [0u32; 5];
        for sheet in 0..SHEETS {
            if let Some(o) = answer(sheet, 0, q) {
                counts[usize::from(o)] += 1;
            }
        }
        let best = *counts.iter().max().unwrap();
        let mode = counts.iter().position(|&c| c == best).unwrap() as u8;
        assert_eq!(
            k.forms[0][usize::from(q)],
            KeyCell::Options(vec![mode]),
            "q{q}"
        );
    }
    let run = p.scoring.as_ref().unwrap().run.as_ref().unwrap();
    assert!(
        run.students
            .iter()
            .all(|r| r.status != omr_scoring::AssessmentStatus::NotScorable)
    );
    assert!(s.release_blocked().unwrap().contains("INFERRED"));
}

#[test]
fn a_review_decision_never_gives_one_student_two_booklets() {
    let dir = tempfile::tempdir().unwrap();
    // Sheets 0 and 1 carry the same student number (IDN-005 on both).
    let input = write_sheets_as(&dir.path().join("scans"), &|k| Some(k % 2), &|k| {
        student(k.max(1))
    });
    let def = definition(&["A", "B"], KeySource::Inferred);
    let mut p = grade(&def, &input, &dir.path().join("out"));
    let dup: Vec<u64> = p
        .queue(Some(omr_app::Lane::Identity))
        .iter()
        .filter(|i| i.code == "IDN-005")
        .map(|i| i.id)
        .collect();
    assert_eq!(dup.len(), 2, "both booklets flagged");
    let resolve = |p: &mut ExamProject, id: u64, d: Decision| {
        p.apply(
            id,
            Action::Resolve {
                decision: d,
                accepted_suggestion: false,
                note: String::new(),
            },
        )
    };
    // The first booklet keeps the student; the second cannot take it too.
    resolve(&mut p, dup[0], Decision::Student(student(1))).unwrap();
    let err = resolve(&mut p, dup[1], Decision::Student(student(1))).unwrap_err();
    assert_eq!(err.exit_code(), 6, "{err}");
    // Another student than the engine proposed: an override, with its reason (§13.4).
    p.apply_with_reason(
        dup[1],
        Action::Resolve {
            decision: Decision::Student(student(0)),
            accepted_suggestion: false,
            note: String::new(),
        },
        Some(omr_app::review::ReviewReason::code("DUPLICATE_RESOLVED")),
    )
    .unwrap();
    let students: Vec<Option<String>> = p
        .session
        .booklets
        .iter()
        .map(|b| b.identity.student.clone())
        .collect();
    let mut uniq = students.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(uniq.len(), students.len());
}
