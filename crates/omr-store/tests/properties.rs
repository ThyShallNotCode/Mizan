//! Property tests: JCS is a fixed point and round-trips; any payload edit anywhere in a
//! random chain is caught at exactly that event.

mod common;

use common::*;
use omr_store::jcs::{es_number, value_to_jcs};
use omr_store::{ChainError, NewEvent, StoreError};
use proptest::prelude::*;
use rusqlite::Connection;
use serde_json::{Map, Value, json};
use uuid::Uuid;

fn arb_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        (-(1i64 << 53)..(1i64 << 53)).prop_map(|i| json!(i)),
        "\\PC{0,12}".prop_map(Value::String),
    ];
    leaf.prop_recursive(3, 24, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..5).prop_map(Value::Array),
            prop::collection::btree_map("\\PC{0,6}", inner, 0..5)
                .prop_map(|m| Value::Object(m.into_iter().collect::<Map<_, _>>())),
        ]
    })
}

fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, p)| y.get(k).is_some_and(|q| same(p, q)))
        }
        _ => a == b,
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn jcs_is_a_fixed_point_and_round_trips(v in arb_json()) {
        let a = value_to_jcs(&v).unwrap();
        let back: Value = serde_json::from_str(&a).unwrap();
        prop_assert_eq!(value_to_jcs(&back).unwrap(), a.clone());
        prop_assert!(same(&back, &v), "{back} vs {v}");
    }

    /// ECMAScript number text re-reads (with a correctly rounding parser) to the same double.
    /// Event payloads refuse floats anyway (decimals are strings, §14.4).
    #[test]
    fn es_number_round_trips(f in any::<f64>().prop_filter("finite", |f| f.is_finite())) {
        let s = es_number(f).unwrap();
        let back: f64 = s.parse().unwrap();
        prop_assert!(back == f || (back == 0.0 && f == 0.0), "{f} -> {s} -> {back}");
        prop_assert!(!s.contains("e") || s.contains("e+") || s.contains("e-"));
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(12))]

    #[test]
    fn any_payload_edit_is_caught_at_its_event(
        payloads in prop::collection::vec(0u32..1000, 1..12),
        pick in any::<prop::sample::Index>(),
        delta in 1u32..50,
    ) {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("Exam.omrproj");
        let mut p = new_project(&root);
        for (k, v) in payloads.iter().enumerate() {
            p.append_event(&NewEvent::new("NoteAdded", "exam", "e", instructor(), json!({"k": k, "v": v})))
                .unwrap();
        }
        p.close().unwrap();
        let target = 2 + pick.index(payloads.len()) as i64; // seq 1 is ProjectCreated
        let c = Connection::open(root.join("project.db")).unwrap();
        c.execute_batch("DROP TRIGGER events_append_only_update").unwrap();
        let k = target - 2;
        let v = payloads[k as usize] + delta;
        c.execute(
            "UPDATE events SET payload = ?2 WHERE seq = ?1",
            rusqlite::params![target, format!("{{\"k\":{k},\"v\":{v}}}")],
        )
        .unwrap();
        let r = omr_store::verify_chain(&c, &Uuid::from_u128(PROJECT), None, &[]);
        let caught = matches!(
            r,
            Err(StoreError::Chain(ChainError::HashMismatch { seq })) if seq == target
        );
        prop_assert!(caught);
    }
}
