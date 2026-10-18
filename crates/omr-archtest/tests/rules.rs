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

/// The assertion macro invocations of `src` (`assert!`, `assert_eq!`, `assert_ne!`), each from
/// its name to its closing parenthesis, with string literals blanked.
fn assertions(src: &str) -> Vec<String> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(k) = src[i..].find("assert") {
        let start = i + k;
        let Some(open) = src[start..].find('(').map(|o| start + o) else {
            break;
        };
        if !src[start..open].ends_with('!') {
            i = start + 6;
            continue;
        }
        let (mut depth, mut j, mut text) = (0i32, open, String::new());
        let mut in_str = false;
        while j < b.len() {
            let c = b[j] as char;
            if in_str {
                if c == '\\' {
                    j += 2;
                    continue;
                }
                if c == '"' {
                    in_str = false;
                }
                text.push(' ');
            } else {
                match c {
                    '"' => in_str = true,
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                text.push(c);
                if depth == 0 {
                    break;
                }
            }
            j += 1;
        }
        out.push(text);
        i = j.max(start + 6);
    }
    out
}

/// An integer literal of at least 1 in `text` (not part of a name, a float or an exponent).
fn has_count_literal(text: &str) -> bool {
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() {
            let s = i;
            while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
                i += 1;
            }
            let before = s.checked_sub(1).map(|p| b[p] as char);
            let after = b.get(i).map(|c| *c as char);
            let exponent =
                matches!(before, Some('-' | '+')) && s >= 2 && matches!(b[s - 2], b'e' | b'E');
            let named = exponent
                || before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
            let float = matches!(after, Some('.' | 'e' | 'E' | 'f'));
            let value: u64 = text[s..i].replace('_', "").parse().unwrap_or(0);
            if !named && !float && value >= 1 {
                return true;
            }
        } else {
            i += 1;
        }
    }
    false
}

#[test]
fn head_to_head_figures_of_the_real_exams_stay_out_of_the_repository() {
    // Spec §3.5 (Q-08): what a parallel run with the incumbent found on the real exams leaves
    // the university only after legal review. The gated tests that compare with Remark may
    // assert only what holds for any exam; the counts of the real run belong in the restricted
    // expectations file (`OMR_REMARK_EXPECT`), outside the repository.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let kinds = [
        "NoRemarkRow",
        "NoBooklet",
        "DifferentId",
        "RemarkIdIsHandwritten",
        "RemarkIdNotRead",
        "SameIdAnswersDiffer",
        "DuplicateId",
    ];
    let mut scanned = 0;
    let mut bad = Vec::new();
    for c in std::fs::read_dir(root.join("crates")).unwrap() {
        let tests = c.unwrap().path().join("tests");
        let Ok(dir) = std::fs::read_dir(&tests) else {
            continue;
        };
        for f in dir {
            let p = f.unwrap().path();
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            if !(name.starts_with("real_") && name.ends_with(".rs")) {
                continue;
            }
            let src = std::fs::read_to_string(&p).unwrap();
            if !src.contains("compare_remark") {
                continue;
            }
            scanned += 1;
            for k in kinds {
                if src.contains(&format!("\"{k}\"")) || src.contains(&format!("IdentityKind::{k}"))
                {
                    bad.push(format!("{name}: names the identity difference {k}"));
                }
            }
            for a in assertions(&src) {
                if has_count_literal(&a) {
                    bad.push(format!(
                        "{name}: an assertion with a hard-coded count: {}",
                        a.split_whitespace().collect::<Vec<_>>().join(" ")
                    ));
                }
            }
        }
    }
    assert!(scanned >= 1, "the gated Remark test was not found");
    assert!(
        bad.is_empty(),
        "Q-08: head-to-head figures in a committed test. Fix: move them to the restricted \
         expectations (OMR_REMARK_EXPECT):\n{}",
        bad.join("\n")
    );
    // The scanner itself.
    assert!(has_count_literal("assert_eq!(x, 6, \" \")"));
    assert!(!has_count_literal(
        "assert_eq!(x.k1, 0, \" \"); (a - b).abs() < 5e-3; 100.0"
    ));
    assert_eq!(assertions("assert_eq!(a, \"b)\"); x").len(), 1);
}
