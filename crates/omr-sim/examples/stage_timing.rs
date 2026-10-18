//! Per-stage timing of one simulated page (single thread):
//! `cargo run --release -p omr-sim --example stage_timing -- [dpi]`.
use omr_forms::{ExamKey, Template, TemplateSpec};
use omr_model::{BookletSerial, CancelMode, ExamId, FormCode, PageNo};
use omr_sim::capture::{capture, encode_jpeg};
use omr_sim::{CaptureParams, MarkRates, Profile, SheetScript, Simulator};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dpi: u32 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(200);
    let t = Template::build(TemplateSpec::standard(
        ExamId(uuid::Uuid::from_u128(3)),
        100,
        5,
        CancelMode::EraseOnly,
    ))?;
    let key = ExamKey::from_bytes([1; 32]);
    let sim = Simulator::new(&t, &key, dpi);
    let mut rng = ChaCha8Rng::seed_from_u64(1);
    let n: u32 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);
    let (mut a, mut b, mut c, mut d) = (0.0, 0.0, 0.0, 0.0);
    let t0 = Instant::now();
    sim.clean_page(FormCode(1), PageNo(1), BookletSerial(0))?;
    println!(
        "static raster (once): {:.1} ms",
        t0.elapsed().as_secs_f64() * 1e3
    );
    for i in 0..n {
        let s = SheetScript::random(
            &t,
            1,
            i,
            Some("441234567".into()),
            &MarkRates::standard(),
            &mut rng,
        );
        let t1 = Instant::now();
        let mut img = sim.clean_page(FormCode(1), PageNo(1), BookletSerial(i))?;
        let t2 = Instant::now();
        sim.paint_page(&mut img, &s, PageNo(1), &mut rng)?;
        let t3 = Instant::now();
        let p = CaptureParams::sample(Profile::Standard, dpi, &mut rng);
        let cap = capture(&img, &p, &mut rng);
        let t4 = Instant::now();
        let _ = encode_jpeg(&cap.image, p.jpeg_quality, dpi)?;
        let t5 = Instant::now();
        a += (t2 - t1).as_secs_f64();
        b += (t3 - t2).as_secs_f64();
        c += (t4 - t3).as_secs_f64();
        d += (t5 - t4).as_secs_f64();
    }
    // Isolate capture sub-stages.
    let img = sim.clean_page(FormCode(1), PageNo(1), BookletSerial(1))?;
    let mut p = CaptureParams::sample(Profile::Standard, dpi, &mut rng);
    p.blur_sigma = 0.0;
    p.noise_sigma = 0.0;
    let t6 = Instant::now();
    for _ in 0..5 {
        let _ = capture(&img, &p, &mut rng);
    }
    let warp = t6.elapsed().as_secs_f64() / 5.0;
    p.blur_sigma = 1.0;
    let t7 = Instant::now();
    for _ in 0..5 {
        let _ = capture(&img, &p, &mut rng);
    }
    let with_blur = t7.elapsed().as_secs_f64() / 5.0;
    println!(
        "warp+tone+quantise {:.1} ms; blur(σ=1) adds {:.1} ms",
        warp * 1e3,
        (with_blur - warp) * 1e3
    );
    let f = 1e3 / f64::from(n);
    println!(
        "per page @ {dpi} dpi: clean+codes {:.1} ms, marks {:.1} ms, capture {:.1} ms, jpeg {:.1} ms",
        a * f,
        b * f,
        c * f,
        d * f
    );
    Ok(())
}
