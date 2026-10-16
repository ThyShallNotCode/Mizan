#![allow(missing_docs)]
// A4 (docs/IMPLEMENTATION.md §1): a key change on 10,000 booklets × 100 items rescored in
// ≤ 50 ms (spec §10.1, §21.1). The timing assertion runs in release builds
// (`cargo test --release -p omr-scoring --test regrade`); debug builds only report it.

use omr_model::FormCode;
use omr_scoring::stats::SplitMix64;
use omr_scoring::*;
use std::time::{Duration, Instant};

const N: usize = 10_000;
const ITEMS: usize = 100;
const FORMS: usize = 4;

fn shuffle<T>(v: &mut [T], rng: &mut SplitMix64) {
    for i in (1..v.len()).rev() {
        let j = rng.below(i as u64 + 1) as usize;
        v.swap(i, j);
    }
}

/// 90 SBA items (5 options, M = 1) and 10 select-all items (5 options, M = 2), four forms
/// with shuffled items and options, and 10,000 simulated booklets.
fn fixture() -> (FormMap, KeyVersion, ResponseSnapshot) {
    let mut rng = SplitMix64::new(0xA4);
    let items: Vec<MasterItem> = (0..ITEMS)
        .map(|i| {
            if i % 10 == 9 {
                MasterItem::multi_select(format!("Q{}", i + 1), 5, SelectMode::SelectAll, pts(2, 1))
            } else {
                MasterItem::sba(format!("Q{}", i + 1), 5, pts(1, 1))
            }
        })
        .collect();
    let key = KeyVersion::new(
        (0..ITEMS)
            .map(|i| {
                if i % 10 == 9 {
                    KeyEntry::set(&[0, (1 + i % 4) as u8])
                } else {
                    KeyEntry::single(rng.below(5) as u8, 5)
                }
            })
            .collect(),
    );
    let mut forms = vec![Form::identity(FormCode(1), &items)];
    for f in 1..FORMS {
        let mut order: Vec<usize> = (0..ITEMS).collect();
        shuffle(&mut order, &mut rng);
        let maps: Vec<Vec<u8>> = (0..ITEMS)
            .map(|_| {
                let mut m: Vec<u8> = (0..5).collect();
                shuffle(&mut m, &mut rng);
                m
            })
            .collect();
        forms.push(Form::permuted(FormCode(f as u16 + 1), &order, &maps));
    }
    let fm = FormMap {
        items,
        forms,
        per_form_normalization: false,
    };
    let printed: Vec<Vec<Option<KeyEntry>>> = (0..FORMS).map(|f| key.printed_key(&fm, f)).collect();
    let mut booklets = Vec::with_capacity(N);
    for n in 0..N {
        let f = n % FORMS;
        let ability = 40 + rng.below(55); // percent chance of a correct answer
        let responses = printed[f]
            .iter()
            .map(|e| {
                let roll = rng.below(100);
                match e.as_ref().unwrap() {
                    KeyEntry::Set(m) => {
                        if roll < ability {
                            L1Response::Set(options_of(*m))
                        } else if roll < 97 {
                            L1Response::Set(options_of((rng.below(31) + 1) as u32))
                        } else {
                            L1Response::Blank
                        }
                    }
                    k => {
                        let kk = k.single_key().unwrap();
                        if roll < ability {
                            L1Response::Single(kk)
                        } else if roll < 97 {
                            L1Response::Single(rng.below(5) as u8)
                        } else if roll < 99 {
                            L1Response::Blank
                        } else {
                            L1Response::Multi(vec![kk, (kk + 1) % 5])
                        }
                    }
                }
            })
            .collect();
        booklets.push(BookletL1::present(
            n as u32 + 1,
            Some(&format!("S{n:05}")),
            FormCode(f as u16 + 1),
            responses,
        ));
    }
    (fm, key, ResponseSnapshot { booklets })
}

#[test]
fn req_a4_regrade_10k_x100() {
    let (forms, key, l1) = fixture();
    let policy = PolicyVersion::default();
    let adj = Adjustments::default();
    let scale = GradeScale::saudi();
    let rounding = RoundingPolicy::default();

    let t = Instant::now();
    let prep = prepare(&l1, &forms).unwrap();
    let prepare_time = t.elapsed();
    let base = score_prepared(&prep, &forms, &key, &policy, &adj).unwrap();
    assert_eq!(base.students.len(), N);

    // The key change: ten items re-keyed (five "change key to d", five "accept key ∪ {d}").
    let changes: Vec<(usize, KeyEntry)> = (0..10)
        .map(|k| {
            let i = k * 10 + 1;
            let o = key.entry(i).unwrap().single_key().unwrap();
            let d = (o + 1) % 5;
            if k % 2 == 0 {
                (i, KeyEntry::single(d, 5))
            } else {
                (i, KeyEntry::any_of(&[o, d], 5))
            }
        })
        .collect();
    let key2 = key.revise(changes, "KEY_ERROR");

    let mut best = Duration::MAX;
    let mut run = None;
    for _ in 0..5 {
        let t = Instant::now();
        let r = score_prepared(&prep, &forms, &key2, &policy, &adj).unwrap();
        let g = r.grades(&scale, &rounding);
        let dt = t.elapsed();
        assert_eq!(g.len(), N);
        best = best.min(dt);
        run = Some(r);
    }
    let run = run.unwrap();
    assert_ne!(run.run_id, base.run_id);
    let changed = run
        .students
        .iter()
        .zip(&base.students)
        .filter(|(a, b)| a.final_pct != b.final_pct)
        .count();
    assert!(changed > 0);
    eprintln!(
        "A4: prepare (L1 -> L2, once per L1 change) {:.1} ms; key-change regrade of {N} × \
         {ITEMS} (validate + compile + L3 + L4 + checks + run ID + L6) best of 5: {:.2} ms; \
         {changed} totals changed",
        prepare_time.as_secs_f64() * 1e3,
        best.as_secs_f64() * 1e3
    );
    if cfg!(debug_assertions) {
        eprintln!("A4: debug build, timing assertion skipped (run with --release)");
    } else {
        assert!(
            best <= Duration::from_millis(50),
            "A4 regrade budget exceeded: {best:?} > 50 ms"
        );
    }
}


#[test]
fn zz_profile_stages() {
    use omr_scoring::l3::{CompiledKey, score_matrix, mark_not_reached};
    use omr_scoring::l4::apply_l4;
    use omr_scoring::run::{row_shift_check, near_chance_check, RowShiftConfig, hash_json};
    let (forms, key, l1) = fixture();
    let policy = PolicyVersion::default();
    let adj = Adjustments::default();
    let prep = prepare(&l1, &forms).unwrap();
    for _ in 0..3 {
        let t = Instant::now();
        let mut f = forms.validate(); f.extend(key.validate(&forms)); f.extend(policy.lint(&forms));
        let t_val = t.elapsed();
        let t = Instant::now();
        let ck = CompiledKey::compile(&forms, &key, &policy, &prep.matrix.points).unwrap();
        let t_comp = t.elapsed();
        let t = Instant::now();
        let mut l3 = score_matrix(&prep.matrix, &ck);
        let _ = mark_not_reached(&mut l3, &prep.matrix, &forms, &policy);
        let t_l3 = t.elapsed();
        let ms: Vec<i64> = (0..forms.items.len()).map(|i| ck.max_scaled(i)).collect();
        let t = Instant::now();
        let l4 = apply_l4(&prep.matrix, &l3, &ms, &forms, &policy, &adj);
        let t_l4 = t.elapsed();
        let t = Instant::now();
        let _ = row_shift_check(&prep.matrix, &forms, &key, RowShiftConfig::default());
        let t_rs = t.elapsed();
        let t = Instant::now();
        let nc = near_chance_check(&prep.matrix, &l3, &forms, &key);
        let t_nc = t.elapsed();
        let t = Instant::now();
        let g = omr_scoring::run::ScoreRun { run_id: String::new(), key_version: 1, policy_version: 1, items: vec![], l3: l3.clone(), counted: l4.counted.clone(), students: l4.students.clone(), curve: None, exceptions: vec![] };
        let t_clone = t.elapsed();
        let t = Instant::now();
        let gg = g.grades(&GradeScale::saudi(), &RoundingPolicy::default());
        eprintln!("grades {:?} (clone {:?}) {}", t.elapsed(), t_clone, gg.len());
        let t = Instant::now();
        let _ = (hash_json(&forms).unwrap(), hash_json(&key).unwrap(), hash_json(&policy).unwrap(), hash_json(&adj).unwrap());
        let t_h = t.elapsed();
        let t = Instant::now();
        let t_l4b = t.elapsed();
        let _ = t_l4b;
        let _ = &l4;
        eprintln!("validate {:?} compile {:?} l3 {:?} l4 {:?} rowshift {:?} nearchance {:?} ({}) hash {:?}", t_val, t_comp, t_l3, t_l4, t_rs, t_nc, nc.len(), t_h);
    }
}
