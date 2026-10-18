//! Requirement A5 / §8.11 on the own-form path: the page stage gives byte-identical
//! measurements on 1, 2 and 4 threads (the tag detector runs its internal `rayon` loops on the
//! caller's pool), and the batch stage gives identical decisions whatever the page order.
//! Synthetic sheets only.

#[path = "support/ownform_eval.rs"]
mod ownform_eval;

use omr_recog::ownform::{CodeRegistry, OwnPageMeasurement, OwnPageRequest};
use omr_sim::{CorpusConfig, Profile, Simulator};
use ownform_eval::{capture_sheet, engine, key, template};
use rayon::prelude::*;

#[test]
fn req_a5_ownform_identical_across_threads_and_page_orders() {
    let t = template(100, 5, 2);
    let k = key();
    let reg = CodeRegistry::for_template(&t, k.clone());
    let e = engine(&t, &k, 200, omr_recog::ownform::OwnFormConfig::gray_v1());
    let sim = Simulator::new(&t, &k, 200);
    let mut ccfg = CorpusConfig::standard(6, 9_105);
    ccfg.profile = Profile::Hard;
    ccfg.rates = omr_sim::MarkRates::hard();
    let pages: Vec<omr_imaging::GrayImage> = (0..6)
        .flat_map(|i| capture_sheet(&sim, &t, &ccfg, i))
        .map(|p| omr_imaging::decode(&p.jpeg).expect("decode").image)
        .collect();
    let req = OwnPageRequest {
        device_dpi: Some(200),
        ..OwnPageRequest::default()
    };
    let measure_all = |threads: usize| -> Vec<OwnPageMeasurement> {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .expect("pool");
        pool.install(|| {
            pages
                .par_iter()
                .map(|p| e.measure_page(p, &req, &reg))
                .collect()
        })
    };
    let json = |v: &[OwnPageMeasurement]| serde_json::to_string(v).expect("json");
    let one = measure_all(1);
    let reference = json(&one);
    for threads in [2, 4] {
        assert_eq!(json(&measure_all(threads)), reference, "{threads} threads");
    }

    // Batch stage: same baseline and decisions for any page order.
    let refs: Vec<&OwnPageMeasurement> = one.iter().collect();
    let b = e.batch_baseline(&refs);
    let mut shuffled = refs.clone();
    shuffled.reverse();
    shuffled.swap(0, 3);
    let b2 = e.batch_baseline(&shuffled);
    assert_eq!(b, b2);
    let d1: Vec<_> = one.iter().map(|m| e.decide(m, &b)).collect();
    let d2: Vec<_> = one.iter().map(|m| e.decide(m, &b2)).collect();
    assert_eq!(
        serde_json::to_string(&d1).unwrap(),
        serde_json::to_string(&d2).unwrap()
    );
    assert!(
        d1.iter()
            .all(|r| r.questions.len() == 100 && r.registration.is_some())
    );
}
