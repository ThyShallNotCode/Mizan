//! Row ticks (§6.3, §8.4 Stage C control points): an ADF feed-speed bulge that a homography
//! cannot model (bubbles up to 2.5 px off mid-page) is absorbed by the per-row tick
//! correction. Synthetic sheets only.

use omr_forms::render::px_per_mm;
use omr_forms::{ExamKey, Template, TemplateSpec};
use omr_imaging::{GrayImage, bilinear};
use omr_model::{BookletSerial, CancelMode, ExamId, FormCode, PageNo};
use omr_recog::ownform::{CodeRegistry, OwnFormConfig, OwnFormEngine, OwnPageRequest};
use omr_sim::{MarkRates, SheetScript, Simulator};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;

const DPI: u32 = 200;
const BULGE: f64 = 2.5;

fn template() -> Template {
    Template::build(TemplateSpec::standard(
        ExamId(uuid::Uuid::from_u128(
            0x0190_0000_0000_7000_8000_0000_0000_0c09,
        )),
        100,
        5,
        CancelMode::EraseOnly,
    ))
    .expect("template")
}

/// Forward map of the simulated feed: y grows by `BULGE · sin(π t)` between the tag rows
/// (t = 0 at the top tags, 1 at the bottom tags), so the tags themselves do not move.
fn forward(y: f64, y0: f64, y1: f64) -> f64 {
    let t = ((y - y0) / (y1 - y0)).clamp(0.0, 1.0);
    y + BULGE * (std::f64::consts::PI * t).sin()
}

#[test]
fn row_ticks_absorb_a_feed_speed_bulge() {
    let t = template();
    let k = ExamKey::from_bytes([0x21; 32]);
    let sim = Simulator::new(&t, &k, DPI);
    let mut rng = ChaCha8Rng::seed_from_u64(5);
    let script = SheetScript::random(
        &t,
        1,
        3,
        Some("221000777".into()),
        &MarkRates::clean(),
        &mut rng,
    );
    let mut img = sim
        .clean_page(FormCode(1), PageNo(1), BookletSerial(3))
        .expect("render");
    sim.paint_page(&mut img, &script, PageNo(1), &mut rng)
        .expect("paint");
    let src = GrayImage::from_raw(
        img.width() as usize,
        img.height() as usize,
        img.as_raw().clone(),
    )
    .unwrap();
    let ppm = px_per_mm(DPI as f32);
    let g = t.page(PageNo(1)).unwrap();
    let y0 = g.tags[0].centre_mm().y * ppm;
    let y1 = g.tags[3].centre_mm().y * ppm;
    // Resample: each output row shows the input row whose forward image it is.
    let warped = GrayImage::from_fn(src.width(), src.height(), |x, yo| {
        let target = yo as f64 + 0.5;
        let mut y = target;
        for _ in 0..20 {
            y = target - (forward(y, y0, y1) - y);
        }
        bilinear(&src, x as f64 + 0.5, y, 255)
    });

    let reg = CodeRegistry::for_template(&t, k.clone());
    let req = OwnPageRequest {
        device_dpi: Some(DPI),
        ..OwnPageRequest::default()
    };
    let truth = |q: u16, o: usize| {
        let b = &t.question_bubbles(q)[o];
        let c = b.centre_mm();
        (c.x * ppm, forward(c.y * ppm, y0, y1))
    };
    let worst = |cfg: OwnFormConfig| {
        let e = OwnFormEngine::new(t.clone(), cfg);
        let m = e.measure_page(&warped, &req, &reg);
        let mut i = 0;
        let mut worst: f64 = 0.0;
        for q in g.questions() {
            for o in 0..t.question_bubbles(q).len() {
                let (x, y) = truth(q, o);
                let c = m.bubbles[i].centre;
                worst = worst.max((c.x - x).hypot(c.y - y));
                i += 1;
            }
        }
        let r = e.decide(&m, &e.batch_baseline(&[]));
        (worst, m, r)
    };
    let mut off = OwnFormConfig::gray_v1();
    off.ticks.apply = false;
    let (w_off, m_off, _) = worst(off);
    let (w_on, m_on, r_on) = worst(OwnFormConfig::gray_v1());
    let ticks = m_on.ticks.as_ref().unwrap();
    eprintln!(
        "worst bubble-centre error: {w_off:.2} px (Stage A only), {w_on:.2} px (with ticks); \
         tick median residual {:.0} µm",
        ticks.median_um
    );
    assert!(ticks.applied, "{ticks:?}");
    assert!(!m_off.ticks.as_ref().unwrap().applied);
    assert!(ticks.found >= ticks.expected - 2, "{ticks:?}");
    assert!(w_off > 1.5, "homography alone: {w_off}");
    assert!(w_on < 0.6, "with ticks: {w_on}");
    for q in &r_on.questions {
        if q.auto_accepted {
            assert_eq!(
                q.response,
                script.questions[usize::from(q.question)].intended
            );
        }
    }
    assert!(r_on.questions.iter().filter(|q| q.auto_accepted).count() >= 98);
}
