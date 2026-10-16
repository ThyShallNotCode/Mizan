//! Fails the build on any architecture-rule violation (spec §4.3 R1, R2, R6, R8; R10 text).

use std::path::Path;

#[test]
fn architecture_rules_hold() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let ws = omr_archtest::workspace(&root);
    assert!(ws.len() >= 10, "workspace not found");
    let v = omr_archtest::violations(&ws);
    assert!(v.is_empty(), "architecture violations:\n{}", v.join("\n"));
}

#[test]
fn every_violation_message_carries_remediation_text() {
    // R10 meta-test: a synthetic graph that breaks R1, R2 and R8 must produce messages with
    // the rule ID and a "Fix:" clause.
    use omr_archtest::Krate;
    use std::collections::BTreeSet;
    let mk = |n: &str, d: &[&str]| Krate {
        name: n.into(),
        dir: ".".into(),
        deps: d.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>(),
        roots: vec![],
    };
    let ws = vec![
        mk("omr-recog", &["omr-scoring"]),
        mk("omr-scoring", &[]),
        mk("omr-app", &["slint", "omr-sim"]),
        mk("omr-sim", &[]),
    ];
    let v = omr_archtest::violations(&ws);
    for rule in ["R1", "R2", "R8"] {
        assert!(
            v.iter().any(|m| m.starts_with(rule) && m.contains("Fix:")),
            "{rule} missing in {v:?}"
        );
    }
}
