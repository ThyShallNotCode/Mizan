//! omr-sim integration tests: determinism, ground truth consistent with pixels, page codes
//! still machine-readable after a standard capture, and a corpus that is identical at any
//! thread count.

use image::GrayImage;
use omr_forms::pagecode::decode_verified;
use omr_forms::{ExamKey, Template, TemplateSpec};
use omr_model::{CancelMode, ExamId, MarkClass, PageNo};
use omr_sim::corpus::sheet_script;
use omr_sim::profile::apply_h;
use omr_sim::{
    CorpusConfig, Expected, ImageFormat, MarkRates, PageTruth, Profile, SimPage, Simulator,
    read_truth, write_corpus,
};

fn template(questions: u16) -> Template {
    let mut s = TemplateSpec::standard(
        ExamId(uuid::Uuid::from_u128(
            0x0190_1111_2222_7333_8444_5555_6666_7777,
        )),
        questions,
        5,
        CancelMode::EraseOnly,
    );
    s.forms = 3;
    Template::build(s).expect("template")
}

fn key() -> ExamKey {
    ExamKey::from_bytes([0x24; 32])
}

fn cfg(sheets: u32, seed: u64, profile: Profile) -> CorpusConfig {
    CorpusConfig {
        sheets,
        profile,
        seed,
        dpi: 200,
        format: ImageFormat::Jpeg,
        roster: (0..50)
            .map(|i| format!("44{:07}", 3_000 + i * 11))
            .collect(),
        rates: match profile {
            Profile::Clean => MarkRates::clean(),
            Profile::Standard => MarkRates::standard(),
            Profile::Hard => MarkRates::hard(),
        },
        serial_start: 100,
        threads: 2,
    }
}

fn simulate(t: &Template, k: &ExamKey, c: &CorpusConfig, index: u32) -> Vec<SimPage> {
    let sim = Simulator::new(t, k, c.dpi);
    let order: Vec<usize> = (0..c.roster.len()).collect();
    let (script, mut rng) = sheet_script(t, c, &order, index).expect("script");
    sim.simulate(&script, c.profile, &mut rng)
        .expect("simulate")
}

/// Normalised darkness D = (B − I)/(B − K) over the inner 60 % of a bubble, with B the local
/// paper level (90th percentile of a surrounding ring) and K the capture's toner black.
fn darkness(img: &GrayImage, truth: &PageTruth, cx: f64, cy: f64, r: f64) -> f64 {
    let h = &truth.mm_to_px;
    let (u, v) = apply_h(h, cx, cy);
    let (u2, _) = apply_h(h, cx + r, cy);
    let rp = (u2 - u).abs().max(1.0);
    let (w, hgt) = img.dimensions();
    let mut inner = Vec::new();
    let mut ring = Vec::new();
    let reach = (2.6 * rp).ceil() as i64;
    for dy in -reach..=reach {
        for dx in -reach..=reach {
            let (x, y) = (u as i64 + dx, v as i64 + dy);
            if x < 0 || y < 0 || x >= i64::from(w) || y >= i64::from(hgt) {
                continue;
            }
            let d = ((x as f64 + 0.5 - u).powi(2) + (y as f64 + 0.5 - v).powi(2)).sqrt();
            let p = f64::from(img.get_pixel(x as u32, y as u32).0[0]);
            if d <= 0.6 * rp {
                inner.push(p);
            } else if d >= 1.5 * rp && d <= 2.5 * rp {
                ring.push(p);
            }
        }
    }
    ring.sort_by(|a, b| a.total_cmp(b));
    let b = ring[(ring.len() * 9) / 10];
    let k = truth.capture.black;
    let i = inner.iter().sum::<f64>() / inner.len() as f64;
    ((b - i) / (b - k)).clamp(0.0, 1.0)
}

#[test]
fn deterministic_by_seed() {
    let t = template(40);
    let k = key();
    let c = cfg(1, 7, Profile::Standard);
    let a = simulate(&t, &k, &c, 3);
    let b = simulate(&t, &k, &c, 3);
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(&b) {
        assert_eq!(x.image, y.image);
        assert_eq!(x.truth, y.truth);
    }
    let other = simulate(&t, &k, &cfg(1, 8, Profile::Standard), 3);
    assert_ne!(a[0].image, other[0].image);
    assert_ne!(
        simulate(&t, &k, &c, 4)[0].image,
        a[0].image,
        "sheets differ by index"
    );
}

#[test]
fn truth_is_consistent_with_the_captured_pixels() {
    let t = template(100);
    let k = key();
    let c = cfg(4, 11, Profile::Standard);
    let (mut filled, mut empty, mut light, mut residue) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for i in 0..c.sheets {
        for p in simulate(&t, &k, &c, i) {
            for q in &p.truth.questions {
                for b in t.question_bubbles(q.question) {
                    let cm = b.centre_mm();
                    let d = darkness(&p.image, &p.truth, cm.x, cm.y, b.radius_mm());
                    match q.classes[usize::from(b.option)] {
                        MarkClass::Filled => filled.push(d),
                        MarkClass::Empty => empty.push(d),
                        MarkClass::LightFill => light.push(d),
                        MarkClass::ErasedResidue => residue.push(d),
                        _ => {}
                    }
                }
            }
            // The bubbled ID digits are dark, the others are not.
            let g = t.page(PageNo(p.truth.page)).unwrap();
            for b in &g.id_bubbles {
                let cm = b.centre_mm();
                let d = darkness(&p.image, &p.truth, cm.x, cm.y, b.radius_mm());
                let marked = p.truth.id_digits.get(usize::from(b.column)) == Some(&b.digit);
                if marked {
                    assert!(
                        d > 0.3,
                        "ID column {} digit {} too light: {d:.2}",
                        b.column,
                        b.digit
                    );
                } else {
                    assert!(
                        d < 0.25,
                        "ID column {} digit {} too dark: {d:.2}",
                        b.column,
                        b.digit
                    );
                }
            }
        }
    }
    let min_f = filled.iter().copied().fold(1.0, f64::min);
    let max_e = empty.iter().copied().fold(0.0, f64::max);
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
    eprintln!(
        "D: filled n={} min {min_f:.3} mean {:.3}; empty n={} max {max_e:.3} mean {:.3}; light n={} mean {:.3}; residue n={} mean {:.3}",
        filled.len(),
        mean(&filled),
        empty.len(),
        mean(&empty),
        light.len(),
        mean(&light),
        residue.len(),
        mean(&residue)
    );
    assert!(filled.len() > 300 && empty.len() > 1_200);
    assert!(min_f > 0.30, "lightest filled bubble D = {min_f:.3}");
    assert!(max_e < 0.22, "darkest empty bubble D = {max_e:.3}");
    if !light.is_empty() {
        assert!(mean(&light) > mean(&empty) && mean(&light) < mean(&filled));
    }
    if !residue.is_empty() {
        assert!(mean(&residue) < mean(&filled));
    }
}

#[test]
fn page_codes_decode_after_a_standard_capture() {
    let t = template(60);
    let k = key();
    let c = cfg(3, 5, Profile::Standard);
    for i in 0..c.sheets {
        for p in simulate(&t, &k, &c, i) {
            let g = t.page(PageNo(p.truth.page)).unwrap();
            for slot in &g.codes {
                let r = slot.rect.inflate(3_000).to_mm();
                let corners = [
                    (r.x, r.y),
                    (r.x + r.w, r.y),
                    (r.x, r.y + r.h),
                    (r.x + r.w, r.y + r.h),
                ]
                .map(|(x, y)| apply_h(&p.truth.mm_to_px, x, y));
                let x0 = corners
                    .iter()
                    .map(|c| c.0)
                    .fold(f64::MAX, f64::min)
                    .max(0.0) as u32;
                let y0 = corners
                    .iter()
                    .map(|c| c.1)
                    .fold(f64::MAX, f64::min)
                    .max(0.0) as u32;
                let x1 = corners.iter().map(|c| c.0).fold(0.0, f64::max).ceil() as u32;
                let y1 = corners.iter().map(|c| c.1).fold(0.0, f64::max).ceil() as u32;
                let (w, h) = p.image.dimensions();
                let crop = image::imageops::crop_imm(
                    &p.image,
                    x0,
                    y0,
                    (x1 - x0).min(w - x0),
                    (y1 - y0).min(h - y0),
                )
                .to_image();
                let (cw, ch) = crop.dimensions();
                let res = rxing::helpers::detect_in_luma(
                    crop.into_raw(),
                    cw,
                    ch,
                    Some(rxing::BarcodeFormat::QR_CODE),
                )
                .unwrap_or_else(|e| panic!("sheet {i} copy {}: {e:?}", slot.copy));
                assert_eq!(res.getText(), p.truth.payload);
                let code = decode_verified(res.getText(), &k).expect("MAC-valid");
                assert_eq!(code.serial.0, p.truth.booklet);
                assert_eq!(code.form.0, p.truth.form);
            }
        }
    }
}

#[test]
fn corpus_is_identical_at_any_thread_count() {
    let t = template(30);
    let k = key();
    let dir1 = tempfile::tempdir().unwrap();
    let dir2 = tempfile::tempdir().unwrap();
    let mut c = cfg(5, 99, Profile::Standard);
    c.threads = 1;
    let s1 = write_corpus(&t, &k, &c, dir1.path()).unwrap();
    c.threads = 3;
    let s2 = write_corpus(&t, &k, &c, dir2.path()).unwrap();
    assert_eq!(s1, s2);
    for f in ["truth.jsonl", "manifest.json"] {
        let a = std::fs::read(dir1.path().join(f)).unwrap();
        let b = std::fs::read(dir2.path().join(f)).unwrap();
        assert_eq!(a, b, "{f}");
    }
    let truth = read_truth(&dir1.path().join("truth.jsonl")).unwrap();
    assert_eq!(truth.len(), 5);
    for (i, tr) in truth.iter().enumerate() {
        assert_eq!(tr.sheet, i as u64);
        let a = std::fs::read(dir1.path().join(&tr.file)).unwrap();
        let b = std::fs::read(dir2.path().join(&tr.file)).unwrap();
        assert_eq!(a, b, "{}", tr.file);
        assert_eq!(&a[..2], &[0xFF, 0xD8], "JPEG");
        let id = tr.student_id.clone().unwrap();
        assert!(c.roster.contains(&id), "{id} from the roster");
        assert_eq!(tr.id_digits.len(), 9);
        assert_eq!(tr.booklet, 100 + i as u32);
        assert!((1..=3).contains(&tr.form));
        assert_eq!(tr.questions.len(), 30);
    }
    let m: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir1.path().join("manifest.json")).unwrap()).unwrap();
    assert_eq!(m["template_sha384"], t.hash_hex());
    assert_eq!(m["seed"], 99);
    assert_eq!(m["dpi"], 200);
    assert_eq!(m["profile"], "Standard");
}

#[test]
fn roster_ids_must_match_the_grid() {
    let t = template(10);
    let k = key();
    let dir = tempfile::tempdir().unwrap();
    let mut c = cfg(1, 1, Profile::Clean);
    c.roster = vec!["12345".into()];
    assert!(matches!(
        write_corpus(&t, &k, &c, dir.path()),
        Err(omr_sim::SimError::Roster(_))
    ));
}

#[test]
fn hard_profile_and_multi_page_booklets() {
    let t = template(240);
    assert!(t.page_count() >= 2);
    let k = key();
    let c = cfg(1, 3, Profile::Hard);
    let pages = simulate(&t, &k, &c, 0);
    assert_eq!(pages.len(), usize::from(t.page_count()));
    let mut seen = Vec::new();
    for p in &pages {
        seen.extend(p.truth.questions.iter().map(|q| q.question));
        if p.truth.page == 1 {
            assert_eq!(p.truth.id_digits.len(), 9);
        } else {
            assert!(p.truth.id_digits.is_empty());
        }
        let (w, h) = p.image.dimensions();
        assert_eq!((w, h), p.truth.capture.frame_px());
        assert!(p.truth.capture.rotation_deg.abs() <= 4.0);
    }
    assert_eq!(seen, (0..240).collect::<Vec<_>>());
    // Hard marks produce some ambiguity, and every auto expectation matches the intent.
    let qs: Vec<_> = pages
        .iter()
        .flat_map(|p| p.truth.questions.iter())
        .collect();
    for q in &qs {
        if let Expected::Auto(r) = &q.expected {
            assert_eq!(r, &q.intended);
        }
    }
}
