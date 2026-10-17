//! Fractional working resolutions on the legacy path (§8.2): the committed 200-dpi IAU blank
//! form rendered at 300 and 400 dpi (8-bit gray, slightly rotated and shifted), synthetic pencil
//! marks painted at the true bubble centres, encoded as JPEG / PNG and decoded by
//! `omr-imaging`. The engine must bring the page to 200 dpi by exact area coverage (2/3, 1/2),
//! register it and read every answer, the student number and the version. Before the
//! fractional resample a 300-dpi page (ratio 1.5) kept its native size and failed registration.
//! No real data is used.

use image::ImageEncoder;
use omr_forms::legacy::LegacyField;
use omr_imaging::{
    Affine, CaptureKind, GrayImage, SplitMix64, Warp, bilinear, classify_capture, decode,
};
use omr_model::{PointPx, Response};
use omr_recog::{LegacyEngine, PageRequest};

/// A capture at `dpi`: template px → page px is a similarity (scale dpi/200, `angle`, shift).
fn capture_map(dpi: f64, angle: f64) -> Affine {
    let s = dpi / 200.0;
    let (c, si) = (angle.cos() * s, angle.sin() * s);
    Affine {
        m: [c, -si, 21.0 * s, si, c, 14.0 * s],
    }
}

/// Renders the blank form at `dpi` and paints `fills` (template bubble indices) as pencil.
fn render(engine: &LegacyEngine, dpi: f64, fills: &[usize], seed: u64) -> GrayImage {
    let t = engine.template();
    let (tw, th) = t.frame_size();
    let s = dpi / 200.0;
    let (w, h) = (
        (f64::from(tw) * s + 40.0 * s) as usize,
        (f64::from(th) * s + 30.0 * s) as usize,
    );
    let fwd = capture_map(dpi, 0.0105);
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
    let r = t.bubble_radius_px() * s * 0.82;
    for &j in fills {
        let c = fwd.map(t.bubbles()[j].centre);
        let (x0, x1) = ((c.x - r).floor() as isize, (c.x + r).ceil() as isize);
        let (y0, y1) = ((c.y - r).floor() as isize, (c.y + r).ceil() as isize);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (dx, dy) = (x as f64 + 0.5 - c.x, y as f64 + 0.5 - c.y);
                if dx * dx + dy * dy > r * r || x < 0 || y < 0 {
                    continue;
                }
                let (xu, yu) = (x as usize, y as usize);
                if xu >= w || yu >= h {
                    continue;
                }
                // Graphite: 25–75 gray with grain, darkening the print multiplicatively.
                let grain = (rng.next_u64() % 51) as u32;
                let ink = 25 + grain;
                let v = u32::from(img.get(xu, yu)) * ink / 255;
                img.set(xu, yu, v as u8);
            }
        }
    }
    img
}

fn index(engine: &LegacyEngine, field: LegacyField, group: u16, option: u8) -> usize {
    engine
        .template()
        .bubbles()
        .iter()
        .position(|b| b.field == field && b.group == group && b.option == option)
        .expect("bubble")
}

struct Truth {
    answers: Vec<Option<u8>>,
    id: &'static str,
    version: u8,
}

fn sheet(engine: &LegacyEngine, n: u16, salt: u16) -> (Vec<usize>, Truth) {
    let mut fills = Vec::new();
    let mut answers = Vec::new();
    for q in 0..n {
        let a = if (q + salt) % 11 == 7 {
            None
        } else {
            Some(((q * 3 + salt) % 5) as u8)
        };
        if let Some(o) = a {
            fills.push(index(engine, LegacyField::Answers, q, o));
        }
        answers.push(a);
    }
    let id = "2210345678";
    for (c, ch) in id.bytes().enumerate() {
        fills.push(index(
            engine,
            LegacyField::StudentNumber,
            c as u16,
            ch - b'0',
        ));
    }
    let version = (salt % 2) as u8;
    fills.push(index(engine, LegacyField::Version, 0, version));
    (
        fills,
        Truth {
            answers,
            id,
            version,
        },
    )
}

fn encode_jpeg(img: &GrayImage, quality: u8) -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
        .write_image(
            img.as_raw(),
            img.width() as u32,
            img.height() as u32,
            image::ExtendedColorType::L8,
        )
        .expect("jpeg");
    out
}

fn encode_png(img: &GrayImage) -> Vec<u8> {
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

fn check(engine: &LegacyEngine, bytes: &[u8], truth: &Truth, n: u16, ratio: (u32, u32)) {
    let decoded = decode(bytes).expect("decode");
    assert_eq!(classify_capture(&decoded.image), CaptureKind::Gray);
    let (w, h) = (decoded.image.width(), decoded.image.height());
    assert_eq!(engine.working_ratio(w, h), ratio, "{w}x{h}");
    let work = engine.to_working_resolution(&decoded.image);
    let m = engine.measure_page(&work);
    assert!(m.registered(), "{:?}", m.registration_exceptions);
    assert!(m.registration.matched >= 600, "{:?}", m.registration);
    assert!(m.registration.rms_px < 1.0, "{:?}", m.registration);
    let req = PageRequest {
        n_questions: n,
        high_stakes: false,
        capture: decoded.capture,
        page: Some("synthetic-300".into()),
    };
    let r = engine.decide(&m, &engine.prior_baseline(), &req);
    for (q, (got, want)) in r.questions.iter().zip(&truth.answers).enumerate() {
        let want = want.map_or(Response::Blank, Response::Single);
        assert_eq!(got.response, want, "q{q}: {got:?}");
        assert!(got.auto_accepted, "q{q}: {got:?}");
    }
    assert_eq!(r.student_number.text(), truth.id);
    assert_eq!(
        r.student_number.read.exact_digits().as_deref(),
        Some(truth.id)
    );
    let labels = engine.template().choice_labels(LegacyField::Version);
    assert_eq!(
        r.version.value(),
        Some(labels[usize::from(truth.version)].as_str())
    );
}

#[test]
fn req_a1_iau_form_at_300_dpi_gray_jpeg_and_png_registers_and_reads() {
    let engine = LegacyEngine::iau_ccsit_v1().unwrap();
    let n = 29;
    let (fills, truth) = sheet(&engine, n, 3);
    let img = render(&engine, 300.0, &fills, 7);
    check(&engine, &encode_jpeg(&img, 90), &truth, n, (2, 3));
    check(&engine, &encode_png(&img), &truth, n, (2, 3));
}

#[test]
fn iau_form_at_400_dpi_gray_reads_with_ratio_one_half() {
    let engine = LegacyEngine::iau_ccsit_v1().unwrap();
    let n = 28;
    let (fills, truth) = sheet(&engine, n, 4);
    let img = render(&engine, 400.0, &fills, 9);
    check(&engine, &encode_jpeg(&img, 92), &truth, n, (1, 2));
}

#[test]
fn working_ratio_snaps_page_sizes_to_scanner_resolutions() {
    let engine = LegacyEngine::iau_ccsit_v1().unwrap();
    let (tw, th) = engine.template().frame_size();
    let (tw, th) = (tw as usize, th as usize);
    // Auto-cropped 600-dpi pages (the real IAU range) stay on the 3 × 3 box.
    assert_eq!(engine.working_ratio(7050, 4962), (1, 3));
    assert_eq!(engine.working_ratio(7229, 5044), (1, 3));
    assert_eq!(engine.working_factor(7229, 5044), 3);
    assert_eq!(engine.working_ratio(tw, th), (1, 1));
    assert_eq!(engine.working_ratio(tw * 3 / 2 - 30, th * 3 / 2), (2, 3));
    assert_eq!(engine.working_ratio(tw * 2, th * 2 + 20), (1, 2));
    assert_eq!(engine.estimated_dpi(tw * 6 / 5, th * 6 / 5), 240);
    assert_eq!(engine.working_ratio(tw * 6 / 5, th * 6 / 5), (5, 6));
    // 600 dpi through the fractional path is the old integer reduction, bit for bit.
    let img = GrayImage::from_fn(90, 60, |x, y| ((x * 7 + y * 13) % 256) as u8);
    assert_eq!(
        omr_imaging::resample_area(&img, 1, 3),
        omr_imaging::reduce_area(&img, 3)
    );
}
