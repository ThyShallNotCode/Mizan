//! The instructors' key sheets of the two real IAU exams (exam-confidential): loaded from the
//! PDFs in `OMR_IAU_PDF_DIR` the way `omr-app` ingests them, read by the key-blind engine and
//! compared with the verified keys in `OMR_IAU_KEYS` (`key_cs522.csv`, `key_arti503.csv`,
//! format `question,A,B,points`). Only match counts leave this module: never a key letter, a
//! read answer or an image.
//!
//! - CS 522: one PDF with two phone photos (RGB JPEG ≈ 142 ppi): page 1 version A with the
//!   student number bubbled all 0, page 2 version B with all 9; 29 questions.
//! - ARTI 503: two 600-dpi bitonal scans (versions A and B, student number all 0); 28 questions.

#![allow(dead_code)]

use omr_imaging::{CaptureKind, GrayImage, decode};
use omr_model::Response;
use omr_pdf::{ExtractOptions, PageOutcome, PageRaster, PdfDocument};
use omr_recog::{LegacyEngine, PageMeasurement, PageRequest};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The working resolution `omr-app` brings every legacy page to.
const WORKING_DPI: u32 = 200;

/// One decoded page at working resolution.
pub struct KeyPage {
    pub image: GrayImage,
    pub capture: CaptureKind,
}

/// One key-sheet file and what it must read as.
pub struct KeyFile {
    pub label: &'static str,
    pub suffix: &'static str,
    pub key_csv: &'static str,
    pub n_questions: u16,
    /// Per page: (version label, student-number digit repeated in every column).
    pub expect: &'static [(&'static str, char)],
}

/// The three key-sheet files.
pub const FILES: [KeyFile; 3] = [
    KeyFile {
        label: "cs522_photos",
        suffix: "Key_-_A__B.pdf",
        key_csv: "key_cs522.csv",
        n_questions: 29,
        expect: &[("A", '0'), ("B", '9')],
    },
    KeyFile {
        label: "arti503_A_scan",
        suffix: "ARTI-503_Final_A.pdf",
        key_csv: "key_arti503.csv",
        n_questions: 28,
        expect: &[("A", '0')],
    },
    KeyFile {
        label: "arti503_B_scan",
        suffix: "ARTI-503_Final_B.pdf",
        key_csv: "key_arti503.csv",
        n_questions: 28,
        expect: &[("B", '0')],
    },
];

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

/// Finds the file in `dir` whose name ends with `suffix`.
pub fn find(dir: &Path, suffix: &str) -> Option<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(suffix))
        })
        .collect();
    v.sort();
    v.into_iter().next()
}

/// Decodes every page of a PDF and brings it to the working resolution, as `omr-app` does
/// (`ingest::decode_page` + `Recognizer::to_working`).
pub fn load_pdf(engine: &LegacyEngine, path: &Path) -> Vec<KeyPage> {
    let bytes = std::fs::read(path).expect("read key PDF");
    let doc = PdfDocument::open(bytes, ExtractOptions::default()).expect("open key PDF");
    let mut out = Vec::new();
    for i in 0..doc.page_count() {
        let PageOutcome::Extracted(p) = doc.extract_page(i) else {
            panic!("page {} refused", i + 1);
        };
        let src_dpi = p.nominal_dpi();
        let (image, capture, known) = match &p.raster {
            PageRaster::Bilevel(_) | PageRaster::Gray(_) => {
                let bilevel = matches!(p.raster, PageRaster::Bilevel(_));
                let g = p.working_gray(WORKING_DPI).unwrap().unwrap();
                let (w, h) = (g.width() as usize, g.height() as usize);
                let image = GrayImage::from_raw(w, h, g.into_raw()).unwrap();
                let s = src_dpi.unwrap_or(WORKING_DPI);
                let capture = if bilevel {
                    let f = (f64::from(s) / f64::from(WORKING_DPI)).round();
                    if f >= 2.0 {
                        CaptureKind::ReducedBitonal { factor: f as u8 }
                    } else {
                        CaptureKind::Bitonal
                    }
                } else {
                    omr_imaging::classify_capture(&image)
                };
                (image, capture, Some(s.min(WORKING_DPI)))
            }
            PageRaster::Jpeg(j) => {
                let d = decode(&j.data).expect("JPEG");
                let mut img = d.image;
                if j.invert {
                    img = img.inverted();
                }
                let (w, h) = (img.width() as u32, img.height() as u32);
                let g8 = omr_pdf::GrayImage8::from_raw(w, h, img.into_raw()).unwrap();
                let g8 = g8.oriented(p.placement.orientation);
                let (w, h) = (g8.width() as usize, g8.height() as usize);
                let image = GrayImage::from_raw(w, h, g8.into_raw()).unwrap();
                let capture = omr_imaging::classify_capture(&image);
                (image, capture, src_dpi)
            }
        };
        // `Recognizer::to_working` (legacy): quarter turn when the aspect disagrees, then the
        // trusted placement dpi (100–1200) or the page-size estimate.
        let (tw, th) = engine.template().frame_size();
        let image =
            if (tw > th) != (image.width() > image.height()) && image.width() != image.height() {
                image.rotated_quarter(1)
            } else {
                image
            };
        let known = known.filter(|d| (100..=1200).contains(d));
        let (num, den) = match known {
            Some(s) => {
                let g = gcd(WORKING_DPI, s);
                (WORKING_DPI / g, s / g)
            }
            None => engine.working_ratio(image.width(), image.height()),
        };
        let image = if (num, den) == (1, 1) {
            image
        } else {
            omr_imaging::resample_area(&image, num, den)
        };
        out.push(KeyPage { image, capture });
    }
    out
}

/// Reads a key CSV (`question,A,B,points`): per version label, the option index per question.
pub fn read_key(path: &Path) -> Vec<(String, Vec<u8>)> {
    let text = std::fs::read_to_string(path).expect("key CSV");
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header: Vec<String> = lines
        .next()
        .expect("header")
        .split(',')
        .map(|s| s.trim().to_owned())
        .collect();
    let mut cols: Vec<(String, Vec<u8>)> = header[1..header.len() - 1]
        .iter()
        .map(|h| (h.clone(), Vec::new()))
        .collect();
    for l in lines {
        let f: Vec<&str> = l.split(',').map(str::trim).collect();
        for (k, c) in cols.iter_mut().enumerate() {
            let b = f[1 + k].as_bytes()[0];
            c.1.push(b - b'A');
        }
    }
    cols
}

/// Evaluates one key file: per page, the aggregate agreement with the verified key.
pub fn evaluate_file(engine: &LegacyEngine, pdf_dir: &Path, key_dir: &Path, f: &KeyFile) -> Value {
    let Some(path) = find(pdf_dir, f.suffix) else {
        return json!({ "file": f.label, "missing": true });
    };
    let key = read_key(&key_dir.join(f.key_csv));
    let pages = load_pdf(engine, &path);
    let ms: Vec<PageMeasurement> = pages
        .iter()
        .map(|p| engine.measure_page(&p.image))
        .collect();
    let reg: Vec<&PageMeasurement> = ms.iter().filter(|m| m.registered()).collect();
    let baseline = if reg.is_empty() {
        engine.prior_baseline()
    } else {
        engine.batch_baseline(&reg)
    };
    let mut out = Vec::new();
    for (i, (p, m)) in pages.iter().zip(&ms).enumerate() {
        let (want_version, want_digit) = f.expect[i.min(f.expect.len() - 1)];
        let req = PageRequest {
            n_questions: f.n_questions,
            high_stakes: false,
            capture: p.capture,
            page: Some(format!("{}-{}", f.label, i + 1)),
        };
        let r = engine.decide(m, &baseline, &req);
        let answers = &key
            .iter()
            .find(|(v, _)| v == want_version)
            .expect("key version")
            .1;
        let (mut equal, mut auto, mut auto_wrong, mut review, mut unreadable) = (0, 0, 0, 0, 0);
        for q in &r.questions {
            let truth = answers[usize::from(q.question)];
            let ok = q.response == Response::Single(truth);
            equal += u32::from(ok);
            if q.auto_accepted {
                auto += 1;
                auto_wrong += u32::from(!ok);
            } else {
                review += 1;
            }
            unreadable += u32::from(matches!(q.response, Response::Unreadable { .. }));
        }
        let id = r.student_number.text();
        let id_pattern: String = std::iter::repeat_n(want_digit, id.chars().count()).collect();
        let codes: Vec<String> = r
            .exceptions
            .iter()
            .filter(|e| e.code.starts_with("REG"))
            .map(|e| {
                format!(
                    "{} {:?} {}",
                    e.code,
                    e.severity,
                    e.sub_reason.clone().unwrap_or_default()
                )
            })
            .collect();
        out.push(json!({
            "page": i + 1,
            "capture": format!("{:?}", p.capture),
            "size": [p.image.width(), p.image.height()],
            "registered": m.registered(),
            "matched": m.registration.matched,
            "rms_px": m.registration.rms_px,
            "scale": m.registration.scale,
            "coarse_ncc": m.registration.coarse_ncc,
            "alias_margin": m.registration.alias_margin,
            "method": format!("{:?}", m.registration.method),
            "verify_margin": m.registration.verify_margin,
            "verify_tests": m.registration.verify_tests,
            "illumination_cv": m.registration.illumination_cv,
            "reg_exceptions": codes,
            "version_ok": r.version.value() == Some(want_version),
            "version_confident": r.version.confident,
            "id_ok": id == id_pattern,
            "id_confident": r.student_number.confident(),
            "questions": r.questions.len(),
            "equal_key": equal,
            "auto_accepted": auto,
            "auto_accepted_wrong": auto_wrong,
            "review": review,
            "unreadable": unreadable,
        }));
    }
    json!({ "file": f.label, "pages": out })
}

/// Runs every key file; `None` when the environment is not set.
pub fn evaluate() -> Option<Value> {
    let pdf_dir = PathBuf::from(std::env::var_os("OMR_IAU_PDF_DIR")?);
    let key_dir = PathBuf::from(std::env::var_os("OMR_IAU_KEYS")?);
    let mut cfg = omr_recog::LegacyConfig::bitonal_v1();
    if std::env::var_os("OMR_KEY_FORCE_CAMERA").is_some() {
        // Diagnostics: an unreachable coarse alias margin fails the scanner path, so every page
        // is registered on the camera path.
        cfg.registration.alias_min_margin_milli = 2_000;
    }
    let engine = LegacyEngine::new(
        omr_forms::legacy::LegacyTemplate::iau_ccsit_v1(),
        omr_forms::legacy::IAU_CCSIT_V1_BLANK_PNG,
        cfg,
    )
    .expect("engine");
    Some(Value::Array(
        FILES
            .iter()
            .map(|f| evaluate_file(&engine, &pdf_dir, &key_dir, f))
            .collect(),
    ))
}
