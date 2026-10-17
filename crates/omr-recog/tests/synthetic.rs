//! Synthetic regression tests on the committed IAU blank form: the form is warped by a known
//! quadratic map (scale 0.95, 1.2° rotation, a few px of non-linear stretch), marks are painted at
//! the true bubble centres, and the engine must recover the geometry and the decisions.
//! Marks: solid fills, stippled light pencil, erasure residue, erased letters, multis, ID digits.
//! No real data is used.

use omr_forms::legacy::{LegacyField, LegacyTemplate};
use omr_imaging::{GrayImage, QuadWarp, SplitMix64, Warp, bilinear, classify_capture};
use omr_model::{ColumnState, PointPx, Response, Severity};
use omr_recog::{LegacyEngine, PageRequest};

const W: usize = 2380;
const H: usize = 1680;

/// Page px → template px (the inverse of the simulated capture).
fn page_to_template() -> QuadWarp {
    let a: f64 = 1.0 / 0.95;
    let th: f64 = 0.02094; // 1.2°
    QuadWarp {
        centre: PointPx {
            x: 1190.0,
            y: 840.0,
        },
        norm: 1000.0,
        cx: [
            1188.0,
            1000.0 * a * th.cos(),
            1000.0 * a * th.sin(),
            3.0,
            -1.5,
            2.0,
        ],
        cy: [
            825.0,
            -1000.0 * a * th.sin(),
            1000.0 * a * th.cos(),
            -2.0,
            2.5,
            1.0,
        ],
        den: [0.0; 2],
    }
}

/// Newton inverse of `g` at `t` (template px → page px).
fn invert(g: &QuadWarp, t: PointPx) -> PointPx {
    let mut p = PointPx {
        x: 1190.0,
        y: 840.0,
    };
    for _ in 0..30 {
        let q = g.map(p);
        let j = g.jacobian(p);
        let (ex, ey) = (t.x - q.x, t.y - q.y);
        let det = j[0][0] * j[1][1] - j[0][1] * j[1][0];
        p.x += (j[1][1] * ex - j[0][1] * ey) / det;
        p.y += (-j[1][0] * ex + j[0][0] * ey) / det;
    }
    p
}

fn render(blank: &GrayImage) -> GrayImage {
    let g = page_to_template();
    GrayImage::from_fn(W, H, |x, y| {
        let t = g.map(PointPx {
            x: x as f64 + 0.5,
            y: y as f64 + 0.5,
        });
        bilinear(blank, t.x, t.y, 255)
    })
}

#[derive(Clone, Copy)]
enum Mark {
    Solid,
    Stipple(f64),
    Residue,
    Erased,
}

struct Painter {
    truth: Vec<PointPx>,
    radius: f64,
    rng: SplitMix64,
}

impl Painter {
    fn new(t: &LegacyTemplate) -> Self {
        let g = page_to_template();
        Self {
            truth: t.bubbles().iter().map(|b| invert(&g, b.centre)).collect(),
            radius: t.bubble_radius_px() * 0.95,
            rng: SplitMix64::new(42),
        }
    }

    fn paint(&mut self, img: &mut GrayImage, j: usize, mark: Mark) {
        let c = self.truth[j];
        let r = self.radius
            * if matches!(mark, Mark::Erased) {
                0.6
            } else {
                0.85
            };
        let (x0, x1) = ((c.x - r).floor() as isize, (c.x + r).ceil() as isize);
        let (y0, y1) = ((c.y - r).floor() as isize, (c.y + r).ceil() as isize);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (dx, dy) = (x as f64 + 0.5 - c.x, y as f64 + 0.5 - c.y);
                if dx * dx + dy * dy > r * r || x < 0 || y < 0 || x as usize >= W || y as usize >= H
                {
                    continue;
                }
                let u = (self.rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
                let v = match mark {
                    Mark::Solid => Some(20),
                    Mark::Stipple(p) => (u < p).then_some(0),
                    Mark::Residue => (u < 0.16).then_some(0),
                    Mark::Erased => Some(255),
                };
                if let Some(v) = v {
                    img.set(x as usize, y as usize, v);
                }
            }
        }
    }
}

fn index(t: &LegacyTemplate, field: LegacyField, group: u16, option: u8) -> usize {
    t.bubbles()
        .iter()
        .position(|b| b.field == field && b.group == group && b.option == option)
        .expect("bubble")
}

/// The solid-fill page: answers, ID digits, version, exam, gender.
fn solid_page(engine: &LegacyEngine) -> (GrayImage, Painter) {
    let t = engine.template();
    let mut img = render(engine.blank());
    let mut p = Painter::new(t);
    let q = |q: u16, o: u8| index(t, LegacyField::Answers, q, o);
    p.paint(&mut img, q(0, 1), Mark::Solid); // q0: B
    // q1 blank
    p.paint(&mut img, q(2, 3), Mark::Solid); // q2: D with residue on A
    p.paint(&mut img, q(2, 0), Mark::Residue);
    p.paint(&mut img, q(3, 0), Mark::Solid); // q3: A + C
    p.paint(&mut img, q(3, 2), Mark::Solid);
    p.paint(&mut img, q(4, 4), Mark::Stipple(0.40)); // q4: light stipple alone on a solid page
    p.paint(&mut img, q(5, 2), Mark::Solid); // q5: C, with B's letter erased
    p.paint(&mut img, q(5, 1), Mark::Erased);
    for k in 6..20u16 {
        p.paint(&mut img, q(k, (k % 5) as u8), Mark::Solid);
    }
    for (c, d) in [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 0].iter().enumerate() {
        p.paint(
            &mut img,
            index(t, LegacyField::StudentNumber, c as u16, *d),
            Mark::Solid,
        );
    }
    for (c, d) in [0u8, 1, 2].iter().enumerate() {
        p.paint(
            &mut img,
            index(t, LegacyField::Section, c as u16, *d),
            Mark::Solid,
        );
    }
    p.paint(&mut img, index(t, LegacyField::Version, 0, 1), Mark::Solid);
    p.paint(&mut img, index(t, LegacyField::Exam, 0, 4), Mark::Solid);
    p.paint(&mut img, index(t, LegacyField::Gender, 0, 1), Mark::Solid);
    (img, p)
}

fn request(n: u16, img: &GrayImage) -> PageRequest {
    PageRequest {
        n_questions: n,
        high_stakes: false,
        capture: classify_capture(img),
        page: Some("synthetic".into()),
    }
}

#[test]
fn registration_recovers_a_known_quadratic_warp() {
    let engine = LegacyEngine::iau_ccsit_v1().unwrap();
    let (img, painter) = solid_page(&engine);
    let m = engine.measure_page(&img);
    assert!(m.registered(), "{:?}", m.registration_exceptions);
    let q = &m.registration;
    assert!(q.matched >= 600, "{q:?}");
    assert!(q.rms_px < 0.8, "{q:?}");
    assert!((q.scale - 0.95).abs() < 0.01, "{q:?}");
    assert!((q.rotation_rad - 0.02094).abs() < 0.003, "{q:?}");
    assert!(q.alias_margin > 0.05, "{q:?}");
    let warp = m.warp.unwrap();
    let mut worst = 0.0f64;
    for (b, truth) in engine.template().bubbles().iter().zip(&painter.truth) {
        let e = warp.map(b.centre);
        worst = worst.max(((e.x - truth.x).powi(2) + (e.y - truth.y).powi(2)).sqrt());
    }
    assert!(worst < 1.0, "worst predicted-centre error {worst:.2} px");
    assert!(m.bubbles.iter().all(|b| b.readable()));
}

#[test]
fn req_a1_synthetic_decisions_follow_the_brief() {
    let engine = LegacyEngine::iau_ccsit_v1().unwrap();
    let (img, _) = solid_page(&engine);
    let res = engine.recognize_single(&img, &request(20, &img));
    let q = &res.questions;
    assert_eq!(
        (&q[0].response, q[0].auto_accepted),
        (&Response::Single(1), true)
    );
    assert_eq!(
        (&q[1].response, q[1].auto_accepted),
        (&Response::Blank, true)
    );
    assert_eq!(
        (&q[2].response, q[2].auto_accepted),
        (&Response::Single(3), true)
    );
    assert!(
        q[2].exceptions
            .iter()
            .any(|e| e.code == "MRK-003" && e.severity == Severity::Info),
        "{:?}",
        q[2]
    );
    assert_eq!(
        (&q[3].response, q[3].auto_accepted),
        (&Response::Multi(vec![0, 2]), true)
    );
    assert!(q[3].exceptions.iter().any(|e| e.code == "MRK-002"));
    // A lone stipple on a solid-fill page is never silently blank: review, proposed E.
    assert!(!q[4].auto_accepted);
    assert_eq!(q[4].response, Response::Single(4));
    assert!(q[4].exceptions.iter().any(|e| e.code == "MRK-001"));
    // An erased letter reads below the baseline: a plain single, no residue log.
    assert_eq!(
        (&q[5].response, q[5].auto_accepted),
        (&Response::Single(2), true)
    );
    assert!(q[5].exceptions.is_empty(), "{:?}", q[5]);
    for (k, qk) in q.iter().enumerate().take(20).skip(6) {
        assert_eq!(qk.response, Response::Single((k % 5) as u8), "q{k}");
        assert!(qk.auto_accepted, "q{k}");
    }
    assert_eq!(res.student_number.text(), "1234567890");
    assert!(res.student_number.confident());
    assert_eq!(
        res.student_number.read.exact_digits().as_deref(),
        Some("1234567890")
    );
    assert_eq!(res.section.text(), "012");
    assert_eq!(res.version.value(), Some("B"));
    assert_eq!(res.exam.value(), Some("Final"));
    assert_eq!(res.gender.value(), Some("Female"));
    assert!(res.exceptions.iter().all(|e| e.code != "MRK-015"));
}

#[test]
fn unconfident_id_digits_and_double_choices_are_never_confident() {
    let engine = LegacyEngine::iau_ccsit_v1().unwrap();
    let t = engine.template();
    let mut img = render(engine.blank());
    let mut p = Painter::new(t);
    for k in 0..20u16 {
        p.paint(
            &mut img,
            index(t, LegacyField::Answers, k, (k % 5) as u8),
            Mark::Solid,
        );
    }
    // Student number 12?4567890: column 2 holds only a light stipple on a solid-fill page.
    for (c, d) in [1u8, 2, 3, 4, 5, 6, 7, 8, 9, 0].iter().enumerate() {
        let mark = if c == 2 {
            Mark::Stipple(0.40)
        } else {
            Mark::Solid
        };
        p.paint(
            &mut img,
            index(t, LegacyField::StudentNumber, c as u16, *d),
            mark,
        );
    }
    // Two version bubbles filled.
    p.paint(&mut img, index(t, LegacyField::Version, 0, 0), Mark::Solid);
    p.paint(&mut img, index(t, LegacyField::Version, 0, 1), Mark::Solid);
    let res = engine.recognize_single(&img, &request(20, &img));
    let sn = &res.student_number;
    let col = &sn.read.columns[2];
    assert_ne!(col.state, ColumnState::Single, "{col:?}");
    assert!(sn.review[2] && !sn.confident());
    assert_eq!(
        sn.read.exact_digits(),
        None,
        "a proposed digit is never exact"
    );
    assert_eq!(sn.text(), "12?4567890");
    assert!(sn.review.iter().enumerate().all(|(c, &r)| r == (c == 2)));
    assert_eq!(res.version.response, Response::Multi(vec![0, 1]));
    assert!(!res.version.confident);
    assert_eq!(res.version.value(), None);
    assert_eq!(res.version.text(), None);
}

#[test]
fn light_pencil_page_accepts_stippled_marks() {
    let engine = LegacyEngine::iau_ccsit_v1().unwrap();
    let t = engine.template();
    let mut img = render(engine.blank());
    let mut p = Painter::new(t);
    for k in 0..20u16 {
        p.paint(
            &mut img,
            index(t, LegacyField::Answers, k, ((k * 3) % 5) as u8),
            Mark::Stipple(0.45),
        );
    }
    let res = engine.recognize_single(&img, &request(20, &img));
    assert!(res.level.as_f64() < 0.6, "level {:?}", res.level);
    for (k, q) in res.questions.iter().enumerate() {
        assert_eq!(q.response, Response::Single(((k * 3) % 5) as u8), "q{k}");
        assert!(q.auto_accepted, "q{k}: {q:?}");
    }
}

#[test]
fn req_a10_cut_off_bubbles_are_unreadable_never_blank() {
    let engine = LegacyEngine::iau_ccsit_v1().unwrap();
    let (img, painter) = solid_page(&engine);
    let cut_w = 1900;
    let cut = img.crop(0, 0, cut_w, H, 255);
    let res = engine.recognize_single(&cut, &request(100, &cut));
    let t = engine.template();
    let mut unreadable = 0;
    for qo in &res.questions {
        let outside = t.question(qo.question).iter().enumerate().any(|(o, _)| {
            let j = index(t, LegacyField::Answers, qo.question, o as u8);
            painter.truth[j].x + painter.radius >= cut_w as f64
        });
        if outside {
            unreadable += 1;
            assert!(
                qo.response.is_unreadable(),
                "q{} {:?}",
                qo.question,
                qo.response
            );
            assert!(!qo.auto_accepted);
            assert!(
                qo.exceptions
                    .iter()
                    .any(|e| e.code == "CAP-009" && e.severity == Severity::Error)
            );
        } else {
            assert!(!qo.response.is_unreadable(), "q{}", qo.question);
        }
    }
    assert!(unreadable >= 15, "{unreadable}");
    // Readable questions still decode.
    assert_eq!(res.questions[0].response, Response::Single(1));
    assert!(res.questions[0].auto_accepted);
}

#[test]
fn req_a10_failed_registration_is_unreadable_never_blank() {
    let engine = LegacyEngine::iau_ccsit_v1().unwrap();
    let white = GrayImage::new(W, H, 255);
    let res = engine.recognize_single(&white, &request(29, &white));
    assert!(
        res.exceptions
            .iter()
            .any(|e| e.code == "REG-001" && e.severity == Severity::Blocker)
    );
    for q in &res.questions {
        assert!(q.response.is_unreadable() && !q.auto_accepted);
    }
    assert!(
        res.student_number
            .read
            .columns
            .iter()
            .all(|c| c.state == ColumnState::Unreadable)
    );
    assert!(res.student_number.text().chars().all(|c| c == '?'));
    assert!(res.version.value().is_none());
}

#[test]
fn req_a5_page_stage_is_deterministic() {
    let engine = LegacyEngine::iau_ccsit_v1().unwrap();
    let (img, _) = solid_page(&engine);
    let a = engine.measure_page(&img);
    let b = engine.measure_page(&img);
    assert_eq!(a.bubbles, b.bubbles);
    assert_eq!(a.warp, b.warp);
    assert_eq!(a.registration, b.registration);
    let req = request(20, &img);
    let base = engine.batch_baseline(&[&a, &b]);
    assert_eq!(
        engine.decide(&a, &base, &req),
        engine.decide(&b, &base, &req)
    );
}

#[test]
fn blank_booklet_raises_mrk015() {
    let engine = LegacyEngine::iau_ccsit_v1().unwrap();
    let img = render(engine.blank());
    let res = engine.recognize_single(&img, &request(29, &img));
    assert!(
        res.questions
            .iter()
            .all(|q| q.response == Response::Blank && q.auto_accepted)
    );
    assert!(
        res.exceptions
            .iter()
            .any(|e| e.code == "MRK-015" && e.severity == Severity::Error)
    );
}

#[test]
fn upside_down_page_is_rotated_and_logged() {
    let engine = LegacyEngine::iau_ccsit_v1().unwrap();
    let (img, _) = solid_page(&engine);
    let flipped = GrayImage::from_fn(W, H, |x, y| img.get(W - 1 - x, H - 1 - y));
    let res = engine.recognize_single(&flipped, &request(20, &flipped));
    assert!(
        res.exceptions
            .iter()
            .any(|e| e.code == "REG-002" && e.severity == Severity::Info)
    );
    assert!(
        res.exceptions.iter().all(|e| e.code != "REG-004"),
        "{:?}",
        res.exceptions
    );
    assert_eq!(res.questions[0].response, Response::Single(1));
    assert_eq!(res.student_number.text(), "1234567890");
}

#[test]
fn req_a1_bitonal_600dpi_page_is_reduced_and_flagged() {
    let engine = LegacyEngine::iau_ccsit_v1().unwrap();
    let (img, _) = solid_page(&engine);
    // Simulate a 600-dpi bitonal scan: each 200-dpi pixel becomes a 3 × 3 block with
    // round(9 · darkness) black dots in a fixed order.
    const ORDER: [usize; 9] = [4, 0, 8, 2, 6, 1, 7, 3, 5];
    let big = GrayImage::from_fn(3 * W, 3 * H, |x, y| {
        let v = img.get(x / 3, y / 3);
        let k = ((255 - u32::from(v)) * 9 + 127) / 255;
        let slot = ORDER
            .iter()
            .position(|&s| s == (y % 3) * 3 + x % 3)
            .unwrap() as u32;
        if slot < k { 0 } else { 255 }
    });
    assert_eq!(classify_capture(&big), omr_imaging::CaptureKind::Bitonal);
    assert_eq!(engine.working_factor(big.width(), big.height()), 3);
    let work = engine.to_working_resolution(&big);
    let req = PageRequest {
        capture: classify_capture(&big),
        ..request(20, &work)
    };
    let res = engine.recognize_single(&work, &req);
    assert!(res.exceptions.iter().any(|e| e.code == "CAP-003"
        && e.sub_reason.as_deref() == Some("bitonal")
        && e.severity == Severity::Warn));
    assert_eq!(res.questions[0].response, Response::Single(1));
    assert!(res.questions[0].auto_accepted);
    assert_eq!(res.questions[3].response, Response::Multi(vec![0, 2]));
    assert_eq!(res.student_number.text(), "1234567890");
}
