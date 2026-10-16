//! `omr-archtest`: architecture fitness rules over `cargo metadata` (spec §4.3).
//!
//! Implemented rules (each failure prints R10 remediation text: rule ID, offending edge, fix):
//! - **R1** only `shell-*` crates may depend on Slint, Qt or cxx-qt;
//! - **R2** key-blindness: recognition crates never reach scoring/outcome/analytics/store/report/AI crates;
//! - **R6** every crate root carries `#![forbid(unsafe_code)]` (no audited exceptions yet);
//! - **R8** test-only crates (`omr-sim`, fakes) are never normal dependencies of shipped crates.
#![forbid(unsafe_code)]

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

/// One workspace crate: name, manifest directory and its normal (non-dev) dependency names.
#[derive(Debug, Clone)]
pub struct Krate {
    /// Package name.
    pub name: String,
    /// Directory containing Cargo.toml.
    pub dir: PathBuf,
    /// Normal and build dependency names (dev-dependencies excluded).
    pub deps: BTreeSet<String>,
    /// Paths of the crate's lib/bin targets.
    pub roots: Vec<PathBuf>,
}

/// Reads the workspace graph with `cargo metadata --no-deps`.
pub fn workspace(root: &Path) -> Vec<Krate> {
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ])
        .current_dir(root)
        .output()
        .expect("run cargo metadata");
    assert!(
        out.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("metadata json");
    v["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .map(|p| {
            let manifest = PathBuf::from(p["manifest_path"].as_str().unwrap());
            Krate {
                name: p["name"].as_str().unwrap().to_owned(),
                dir: manifest.parent().unwrap().to_owned(),
                deps: p["dependencies"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|d| d["kind"].as_str() != Some("dev"))
                    .map(|d| d["name"].as_str().unwrap().to_owned())
                    .collect(),
                roots: p["targets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|t| {
                        t["kind"].as_array().unwrap().iter().any(|k| {
                            matches!(
                                k.as_str(),
                                Some("lib" | "bin" | "rlib" | "cdylib" | "staticlib")
                            )
                        })
                    })
                    .map(|t| PathBuf::from(t["src_path"].as_str().unwrap()))
                    .collect(),
            }
        })
        .collect()
}

/// Transitive closure of workspace-internal normal dependencies of `name`.
pub fn closure(ws: &[Krate], name: &str) -> BTreeSet<String> {
    let by: BTreeMap<&str, &Krate> = ws.iter().map(|k| (k.name.as_str(), k)).collect();
    let mut seen = BTreeSet::new();
    let mut stack = vec![name.to_owned()];
    while let Some(n) = stack.pop() {
        if let Some(k) = by.get(n.as_str()) {
            for d in &k.deps {
                if seen.insert(d.clone()) {
                    stack.push(d.clone());
                }
            }
        }
    }
    seen
}

/// Recognition-side crates bound by R2.
pub const KEY_BLIND: &[&str] = &["omr-imaging", "omr-forms", "omr-recog", "omr-identity"];
/// Crates that hold or derive from keys, policies or scores (forbidden to R2 crates).
pub const KEY_AWARE: &[&str] = &[
    "omr-scoring",
    "omr-outcomes",
    "omr-analytics",
    "omr-store",
    "omr-report",
    "omr-integrations",
    "omr-ai",
    "omr-app",
];
/// GUI toolkits restricted by R1.
pub const GUI_TOOLKITS: &[&str] = &["slint", "cxx-qt", "cxx-qt-lib", "qt"];
/// R6 audited allowlist: crates allowed `#![deny(unsafe_code)]` instead of `forbid`, each with the
/// reason. The only `allow(unsafe_code)` in such a crate must sit directly on the module that includes
/// generated code.
pub const R6_AUDITED: &[(&str, &str)] =
    &[("shell-slint", "Slint-generated code expands allow(unsafe_code); scoped to `mod ui` only")];
/// Test-only crates restricted by R8.
pub const TEST_ONLY: &[&str] = &[
    "omr-sim",
    "escl-sim",
    "omr-fakes",
    "i-slint-backend-testing",
];

/// Checks every rule; returns violation messages (empty = pass).
pub fn violations(ws: &[Krate]) -> Vec<String> {
    let mut v = Vec::new();
    for k in ws {
        let cl = closure(ws, &k.name);
        // R1
        if !k.name.starts_with("shell-") {
            for t in GUI_TOOLKITS {
                if k.deps.contains(*t) || cl.contains(*t) {
                    v.push(format!(
                        "R1 violated: `{}` depends on GUI toolkit `{t}`. Fix: move the code that needs `{t}` into a `shell-*` crate and expose a view-model from `omr-app` instead.",
                        k.name
                    ));
                }
            }
            if cl.iter().any(|c| c.starts_with("shell-")) {
                v.push(format!("R1 violated: `{}` depends on a shell crate. Fix: shells depend on `omr-app`, never the reverse.", k.name));
            }
        }
        // R2
        if KEY_BLIND.contains(&k.name.as_str()) {
            for bad in KEY_AWARE {
                if cl.contains(*bad) {
                    v.push(format!(
                        "R2 (key-blindness) violated: `{}` reaches `{bad}`. Fix: recognition receives a key-free `BatchContext` from `omr-app`; move the key-dependent logic into `omr-scoring` or `omr-app`.",
                        k.name
                    ));
                }
            }
        }
        // R8
        if !TEST_ONLY.contains(&k.name.as_str()) && k.name != "omr-archtest" {
            for t in TEST_ONLY {
                if k.deps.contains(*t) {
                    v.push(format!(
                        "R8 violated: shipped crate `{}` has a normal dependency on test-only `{t}`. Fix: make it a [dev-dependencies] entry or move the code into a test/bench target.",
                        k.name
                    ));
                }
            }
        }
        // R6
        let audited = R6_AUDITED.iter().any(|(n, _)| *n == k.name);
        for root in &k.roots {
            let src = std::fs::read_to_string(root).unwrap_or_default();
            if audited && src.contains("#![deny(unsafe_code)]") {
                let allows: Vec<usize> =
                    src.lines().enumerate().filter(|(_, l)| l.contains("allow(unsafe_code)")).map(|(i, _)| i).collect();
                let lines: Vec<&str> = src.lines().collect();
                let ok = allows.iter().all(|&i| {
                    lines.iter().skip(i + 1).take(3).any(|l| l.trim_start().starts_with("mod ")) &&
                        src.contains("slint::include_modules!()")
                });
                if !ok || allows.len() > 1 {
                    v.push(format!(
                        "R6 violated: audited crate `{}` has `allow(unsafe_code)` outside the generated-code module. Fix: keep exactly one `#[allow(unsafe_code)]` directly on `mod ui {{ slint::include_modules!(); }}`.",
                        k.name
                    ));
                }
                continue;
            }
            if !src.contains("#![forbid(unsafe_code)]") {
                v.push(format!(
                    "R6 violated: `{}` root {} lacks `#![forbid(unsafe_code)]`. Fix: add it; unsafe code needs an audited allowlist entry (§4.3 R6).",
                    k.name,
                    root.display()
                ));
            }
        }
    }
    v
}
