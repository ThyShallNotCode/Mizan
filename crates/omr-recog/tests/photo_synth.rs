//! Requirement A1 on simulated phone photos of the committed IAU blank form (synthetic marks,
//! truth by construction; no real data): perspective, rotation, anisotropic scale, 120–160 ppi
//! RGB JPEG, colour cast, vignetting and shadows, plus adversarial sheets moved by exactly one
//! answer row or option pitch in each of the eight directions. Every photo must be read
//! correctly or refused with a REG code: never a silent misread (§2, §8.4).

#[path = "support/photo_sim.rs"]
mod photo_sim;

use omr_recog::LegacyEngine;
use photo_sim::{Photo, Tally};
use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::SeedableRng;
use rayon::prelude::*;

/// The eight one-pitch directions (option pitch, answer row).
const SHIFTS: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (-1, -1),
    (1, -1),
    (-1, 1),
];

fn run(n: u32, seed: u64) -> Tally {
    let engine = LegacyEngine::iau_ccsit_v1().expect("engine");
    let results: Vec<_> = (0..n)
        .into_par_iter()
        .map(|i| {
            let mut rng =
                ChaCha8Rng::seed_from_u64(seed.wrapping_mul(1_000_003).wrapping_add(u64::from(i)));
            let truth = photo_sim::random_truth(&mut rng);
            let sheet = photo_sim::marked_sheet(&engine, &truth, &mut rng);
            let mut p = Photo::sample(&mut rng);
            // Every fifth photo is adversarial: the sheet moved by exactly one pitch.
            if i % 5 == 0 {
                p.pitch_shift = SHIFTS[(i / 5) as usize % SHIFTS.len()];
            }
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
fn req_a1_phone_photos_read_or_refused_never_silently_wrong() {
    let n: u32 = std::env::var("OMR_PHOTO_N")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let t = run(n, 20_261_017);
    eprintln!("{t:#?}");
    assert_eq!(t.photos, n);
    assert_eq!(t.silent, 0, "silent misreads: {t:#?}");
    assert_eq!(t.id_wrong, 0, "student number misread: {t:#?}");
    assert_eq!(t.version_wrong, 0, "version misread: {t:#?}");
    // The path works: most photos are read, not refused.
    assert!(t.refused * 10 <= t.photos, "too many refusals: {t:#?}");
    assert!(
        t.auto_accepted * 10 >= t.questions * 8,
        "too many reviews: {t:#?}"
    );
}
