//! Adversarial verification of the legacy-form registration (synthetic data only: the committed
//! blank IAU form with synthetic marks, truth by construction). Every page must be read
//! correctly or refused with a REG code — never read rows or options off (§2, §8.4):
//! - phone photos turned upside down, skewed by 11–13°, very dark or overexposed, at 100 ppi,
//!   10–14 % anisotropic, moved by one or two pitches in the frame, and key sheets (student number
//!   all 0 or all 9, every question marked) and a real-looking number that is mostly zeros;
//! - sheets damaged so that part of the page moved against the rest by whole rows or options: a
//!   fold (or a torn and taped sheet) hiding one, two or three rows, an ADF feed stall repeating
//!   one or two rows' worth of scan lines, and on scanner pages a fold or stall across the
//!   options. Before the row and column checks a two-row stall or fold verified on the part
//!   that still matched and read every row below it two rows off, all auto-accepted.
//!
//! `OMR_PHOTO_ADV_N` (default 6) sets the pages per case.

#[path = "support/photo_sim.rs"]
mod photo_sim;

use omr_forms::legacy::LegacyField;
use omr_imaging::GrayImage;
use omr_recog::LegacyEngine;
use photo_sim::{Photo, Tally, Truth};
use rand::RngExt;
use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::SeedableRng;
use rayon::prelude::*;

const SHIFTS: [(i32, i32); 10] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (-1, -1),
    (1, -1),
    (-1, 1),
    (0, 2),
    (0, -2),
];

fn n_per_case() -> u32 {
    std::env::var("OMR_PHOTO_ADV_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6)
}

/// One adversarial photo condition.
fn tweak(profile: &str, i: u32, p: &mut Photo, t: &mut Truth, rng: &mut ChaCha8Rng) {
    let sign = if i.is_multiple_of(2) { 1.0 } else { -1.0 };
    match profile {
        "rot180" => p.rotation_deg += 180.0,
        "skew12" => p.rotation_deg = sign * rng.random_range(11.0..=13.0),
        "aniso12" => p.anisotropy = sign * rng.random_range(0.10..=0.14),
        "dark" => p.exposure = Some(rng.random_range(0.25..=0.45)),
        "bright" => p.exposure = Some(rng.random_range(1.3..=1.8)),
        "ppi100" => p.ppi = rng.random_range(95.0..=105.0),
        "shift" => p.pitch_shift = SHIFTS[i as usize % SHIFTS.len()],
        "key0" | "key9" => {
            let d = if profile == "key0" { 0 } else { 9 };
            t.student = vec![d; 10];
            for a in &mut t.answers {
                *a = Some(rng.random_range(0..5u8));
            }
            p.pitch_shift = SHIFTS[i as usize % SHIFTS.len()];
            if i.is_multiple_of(3) {
                p.tight = true;
                p.vignette = 0.0;
                p.shadow = None;
            }
        }
        "mostly0" => {
            t.student = vec![0; 10];
            t.student[0] = 2;
            t.student[9] = rng.random_range(1..10u8);
            p.pitch_shift = SHIFTS[i as usize % SHIFTS.len()];
        }
        _ => {}
    }
}

fn photos(profile: &str, n: u32, seed: u64) -> Tally {
    let engine = LegacyEngine::iau_ccsit_v1().expect("engine");
    let results: Vec<_> = (0..n)
        .into_par_iter()
        .map(|i| {
            let mut rng =
                ChaCha8Rng::seed_from_u64(seed.wrapping_mul(1_000_003).wrapping_add(u64::from(i)));
            let mut truth = photo_sim::random_truth(&mut rng);
            let mut p = Photo::sample(&mut rng);
            tweak(profile, i, &mut p, &mut truth, &mut rng);
            let sheet = photo_sim::marked_sheet(&engine, &truth, &mut rng);
            let (jpeg, dpi) = photo_sim::render(&engine, &sheet, &p, u64::from(i) ^ seed);
            // Production classification only (C3): the capture class omr-app gives the file.
            let (img, kind) = photo_sim::to_working(&jpeg, dpi);
            (photo_sim::read(&engine, &img, kind), truth)
        })
        .collect();
    let mut t = Tally::default();
    for (r, truth) in &results {
        t.add(r, truth);
    }
    t
}

#[test]
fn req_a1_adversarial_photos_are_read_or_refused_never_silently_wrong() {
    let n = n_per_case();
    for profile in [
        "rot180", "skew12", "aniso12", "dark", "bright", "ppi100", "shift", "key0", "key9",
        "mostly0",
    ] {
        let t = photos(profile, n, 77_001);
        eprintln!(
            "{profile}: photos {} refused {} auto-accepted {} silent {} reviews {}",
            t.photos, t.refused, t.auto_accepted, t.silent, t.review
        );
        assert_eq!(t.photos, n);
        assert_eq!(t.silent, 0, "{profile}: {t:#?}");
        assert_eq!(t.id_wrong, 0, "{profile}: {t:#?}");
        assert_eq!(t.version_wrong, 0, "{profile}: {t:#?}");
    }
}

/// Damage at template height (or width, `vertical`) `at`: `band > 0` hides `band` px of paper
/// (a fold or a taped tear: everything beyond moves back), `band < 0` repeats the line at `at`
/// for `-band` px (an ADF feed stall: everything beyond moves on).
fn damaged(sheet: &GrayImage, at: usize, band: i64, vertical: bool) -> GrayImage {
    let (w, h) = (sheet.width(), sheet.height());
    GrayImage::from_fn(w, h, |x, y| {
        let (u, lim) = if vertical { (x, w) } else { (y, h) };
        let src = if u < at {
            Some(u)
        } else if band > 0 {
            let v = u + band as usize;
            (v < lim).then_some(v)
        } else if u < at + (-band) as usize {
            Some(at)
        } else {
            Some(u - (-band) as usize)
        };
        match src {
            Some(v) if vertical => sheet.get(v, y),
            Some(v) => sheet.get(x, v),
            None => 255,
        }
    })
}

/// `n` damaged sheets, the damage line spread over the first answer block; on a scanner page
/// (bitonal, 200 dpi) or as a phone photo.
fn damaged_pages(band: i64, vertical: bool, scanner: bool, n: u32) -> Tally {
    let engine = LegacyEngine::iau_ccsit_v1().expect("engine");
    let first_block: Vec<omr_model::PointPx> = engine
        .template()
        .bubbles()
        .iter()
        .filter(|b| b.field == LegacyField::Answers && b.group < 20)
        .map(|b| b.centre)
        .collect();
    let y0 = first_block
        .iter()
        .map(|p| p.y)
        .fold(f64::INFINITY, f64::min);
    let y1 = first_block
        .iter()
        .map(|p| p.y)
        .fold(f64::NEG_INFINITY, f64::max);
    let x0 = first_block
        .iter()
        .map(|p| p.x)
        .fold(f64::INFINITY, f64::min);
    let results: Vec<_> = (0..n)
        .into_par_iter()
        .map(|i| {
            let mut rng = ChaCha8Rng::seed_from_u64(4242 + u64::from(i));
            let truth = photo_sim::random_truth(&mut rng);
            let sheet = photo_sim::marked_sheet(&engine, &truth, &mut rng);
            let frac = 0.15 + 0.7 * f64::from(i) / f64::from(n.max(2) - 1);
            let at = if vertical {
                (x0 + 75.0) as usize
            } else {
                (y0 + frac * (y1 - y0) + 20.0) as usize
            };
            let page = damaged(&sheet, at, band, vertical);
            if scanner {
                let img = GrayImage::from_fn(page.width() + 30, page.height() + 20, |x, y| {
                    let inside =
                        x >= 17 && y >= 11 && x - 17 < page.width() && y - 11 < page.height();
                    let v = if inside {
                        page.get(x - 17, y - 11)
                    } else {
                        255
                    };
                    if v < 150 { 0 } else { 255 }
                });
                let kind = omr_imaging::classify_capture(&img);
                return (photo_sim::read(&engine, &img, kind), truth);
            }
            let p = Photo::sample(&mut rng);
            let (jpeg, dpi) = photo_sim::render(&engine, &page, &p, u64::from(i));
            let (img, kind) = photo_sim::to_working(&jpeg, dpi);
            (photo_sim::read(&engine, &img, kind), truth)
        })
        .collect();
    let mut t = Tally::default();
    for (r, truth) in &results {
        t.add(r, truth);
    }
    t
}

#[test]
fn req_a1_folded_or_stalled_sheets_are_refused_never_read_rows_off() {
    let n = n_per_case();
    let row = 42; // one answer-row pitch at 200 dpi (41.6 px)
    let option = 101; // two option pitches (50.4 px each)
    let cases: [(&str, i64, bool, bool); 10] = [
        ("scanner: fold hiding 2 rows", 2 * row, false, true),
        ("scanner: fold hiding 3 rows", 3 * row, false, true),
        ("scanner: feed stall of 1 row", -row, false, true),
        ("scanner: feed stall of 2 rows", -2 * row, false, true),
        ("scanner: fold hiding 2 options", option, true, true),
        ("scanner: feed stall of 2 options", -option, true, true),
        ("photo: fold hiding 1 row", row, false, false),
        ("photo: fold hiding 2 rows", 2 * row, false, false),
        ("photo: fold hiding 3 rows", 3 * row, false, false),
        ("photo: feed stall of 2 rows", -2 * row, false, false),
    ];
    for (name, band, vertical, scanner) in cases {
        let t = damaged_pages(band, vertical, scanner, n);
        eprintln!(
            "{name}: pages {} refused {} auto-accepted {} silent {}",
            t.photos, t.refused, t.auto_accepted, t.silent
        );
        assert_eq!(t.silent, 0, "{name}: {t:#?}");
        assert_eq!(t.id_wrong, 0, "{name}: {t:#?}");
        assert_eq!(t.version_wrong, 0, "{name}: {t:#?}");
    }
}
