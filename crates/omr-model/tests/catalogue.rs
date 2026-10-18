//! The checked-in catalogue must match Appendix A of the spec (single authority, §13.1).

use omr_model::catalogue::CATALOGUE;
use omr_model::exceptions::{Exception, ExceptionError, lookup};
use omr_model::{Scope, Severity};
use std::collections::BTreeSet;

#[test]
fn catalogue_matches_appendix_a() {
    let doc = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/TECHNICAL_DESIGN.md"
    ))
    .expect("spec present");
    let start = doc.find("## Appendix A").expect("Appendix A");
    let end = doc.find("## Appendix B").expect("Appendix B");
    let mut spec_codes = BTreeSet::new();
    for line in doc[start..end].lines() {
        let l = line.trim_start_matches('|').trim().trim_start_matches("**");
        let code: String = l.chars().take(7).collect();
        if code.len() == 7
            && ["CAP-", "REG-", "IDN-", "COL-", "MRK-", "SCO-", "SYS-"]
                .iter()
                .any(|p| code.starts_with(p))
            && code[4..].chars().all(|c| c.is_ascii_digit())
        {
            spec_codes.insert(code);
        }
    }
    let ours: BTreeSet<String> = CATALOGUE.iter().map(|d| d.code.to_owned()).collect();
    assert_eq!(
        ours, spec_codes,
        "regenerate with python3 scripts/gen_exception_catalogue.py"
    );
    assert_eq!(ours.len(), 115);
}

#[test]
fn every_entry_has_severity_and_scope() {
    for d in CATALOGUE {
        assert!(!d.severities.is_empty(), "{} has no severity", d.code);
        assert!(!d.scopes.is_empty(), "{} has no scope", d.code);
    }
}

#[test]
fn construction_is_checked_against_the_catalogue() {
    assert!(lookup("MRK-001").is_some());
    assert!(Exception::new("MRK-001", Severity::Error, Scope::Item).is_ok());
    assert_eq!(
        Exception::new("MRK-999", Severity::Error, Scope::Item),
        Err(ExceptionError::UnknownCode("MRK-999".into()))
    );
    assert!(matches!(
        Exception::new("MRK-001", Severity::Info, Scope::Item),
        Err(ExceptionError::BadSeverity(..))
    ));
}

/// Every `.rs` and `.slint` file under `crates/` (sorted, `target*` folders skipped).
fn workspace_sources() -> Vec<std::path::PathBuf> {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .expect("readable source folder")
            .map(|e| e.expect("entry").path())
            .collect();
        entries.sort();
        for p in entries {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if p.is_dir() {
                if !name.starts_with("target") && !name.starts_with('.') {
                    walk(&p, out);
                }
            } else if name.ends_with(".rs") || name.ends_with(".slint") {
                out.push(p);
            }
        }
    }
    let mut v = Vec::new();
    walk(
        std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/..")),
        &mut v,
    );
    v
}

/// Code-shaped tokens (`MRK-001`) with their byte offsets.
fn code_tokens(text: &str) -> Vec<(usize, &str)> {
    const FAMILIES: [&str; 7] = ["CAP-", "REG-", "IDN-", "COL-", "MRK-", "SCO-", "SYS-"];
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 7 <= b.len() {
        let word_start = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
        let word_end = i + 7 == b.len() || !(b[i + 7].is_ascii_alphanumeric() || b[i + 7] == b'_');
        if word_start
            && word_end
            && FAMILIES.iter().any(|f| b[i..].starts_with(f.as_bytes()))
            && b[i + 4..i + 7].iter().all(u8::is_ascii_digit)
        {
            out.push((i, &text[i..i + 7]));
            i += 7;
        } else {
            i += 1;
        }
    }
    out
}

/// The identifier after `prefix` (e.g. `Severity::`) when it comes next, skipping whitespace
/// and one comma.
fn literal_arg<'a>(rest: &'a str, prefix: &str) -> Option<&'a str> {
    let r = rest.trim_start().strip_prefix(',')?.trim_start();
    let r = r.strip_prefix(prefix)?;
    let end = r
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .unwrap_or(r.len());
    Some(&r[..end])
}

/// The exception constructor `before` ends with, if any: `Some(None)` for
/// `Exception::new`/`must` (scope passed), `Some(Some(scope))` for a helper that fixes it.
fn constructor(before: &str) -> Option<Option<&'static str>> {
    const CALLS: [(&str, Option<&str>); 9] = [
        ("Exception::new(", None),
        ("Exception::must(", None),
        ("exc::page(", Some("Page")),
        ("exc::item(", Some("Item")),
        ("exc::region(", Some("Region")),
        ("exc::batch(", Some("Batch")),
        ("exc::booklet(", Some("Booklet")),
        ("reg_exc(", Some("Page")),
        ("pe(", Some("Page")),
    ];
    CALLS.iter().find_map(|(name, scope)| {
        let head = before.strip_suffix(name)?;
        let boundary = head.chars().next_back().is_none_or(|c| {
            !(c.is_ascii_alphanumeric() || c == '_' || (c == ':' && *name == "pe("))
        });
        boundary.then_some(*scope)
    })
}

/// A10 (docs/IMPLEMENTATION.md §1): every exception code written anywhere in the workspace
/// sources (engines, app, CLI, shell, tests, comments) exists in Appendix A, and every
/// constructor call with a literal severity and scope (`Exception::new`/`must` and omr-recog's
/// `exc::*` helpers) uses a combination Appendix A allows. `Exception::must` also checks at run time; this catches a code on a rarely taken
/// path before it panics in the field. The only allowed unknown code is `MRK-999`, the
/// deliberate negative case of `construction_is_checked_against_the_catalogue`.
#[test]
fn req_a10_every_code_in_the_workspace_sources_is_in_appendix_a() {
    let known: BTreeSet<&str> = CATALOGUE.iter().map(|d| d.code).collect();
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut unknown: Vec<String> = Vec::new();
    let mut bad_combination: Vec<String> = Vec::new();
    let (mut files, mut tokens, mut literal_calls, mut dynamic_calls) = (0usize, 0usize, 0, 0);
    for path in workspace_sources() {
        let text = std::fs::read_to_string(&path).expect("UTF-8 source");
        files += 1;
        let rel = path
            .strip_prefix(concat!(env!("CARGO_MANIFEST_DIR"), "/.."))
            .unwrap_or(&path)
            .display()
            .to_string();
        for (at, code) in code_tokens(&text) {
            tokens += 1;
            let line = text[..at].lines().count().max(1);
            if !known.contains(code) {
                if code != "MRK-999" || !rel.ends_with("omr-model/tests/catalogue.rs") {
                    unknown.push(format!("{rel}:{line}: {code}"));
                }
                continue;
            }
            used.insert(code.to_owned());
            // A constructor call right before the literal: `Exception::new`/`must` (scope is the
            // next argument) or one of omr-recog's helpers (scope fixed by the helper).
            let Some(before) = text[..at].trim_end().strip_suffix('"') else {
                continue;
            };
            let Some(implied) = constructor(before.trim_end()) else {
                continue;
            };
            if rel.ends_with("omr-model/tests/catalogue.rs") {
                continue; // the deliberate negative cases above
            }
            let rest = &text[at + 8..];
            let sev = literal_arg(rest, "Severity::");
            let scope = match implied {
                Some(s) => Some(s),
                None => sev
                    .and_then(|s| rest.find(s).map(|k| &rest[k + s.len()..]))
                    .and_then(|r| literal_arg(r, "Scope::")),
            };
            let (Some(sev), Some(scope)) = (sev, scope) else {
                dynamic_calls += 1;
                continue;
            };
            literal_calls += 1;
            let def = CATALOGUE.iter().find(|d| d.code == code).expect("known");
            let sev_ok = def.severities.iter().any(|s| format!("{s:?}") == sev);
            let scope_ok = def.scopes.iter().any(|s| format!("{s:?}") == scope);
            if !(sev_ok && scope_ok) {
                bad_combination.push(format!("{rel}:{line}: {code} {sev} {scope}"));
            }
        }
    }
    eprintln!(
        "A10: {files} source files, {tokens} code tokens, {} distinct codes of {} in Appendix A, \
         {literal_calls} constructor calls with literal severity and scope checked, \
         {dynamic_calls} with computed arguments (checked at run time by Exception::must)",
        used.len(),
        known.len()
    );
    if let Ok(dir) = std::env::var("OMR_ACCEPT_OUT") {
        let report = serde_json::json!({
            "source_files": files, "code_tokens": tokens, "distinct_codes": used.len(),
            "catalogue_codes": known.len(), "literal_constructor_calls": literal_calls,
            "computed_constructor_calls": dynamic_calls,
            "unknown_codes": unknown.len(), "bad_severity_or_scope": bad_combination.len(),
        });
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(
            std::path::Path::new(&dir).join("a10-catalogue-scan.json"),
            serde_json::to_vec_pretty(&report).expect("json"),
        )
        .expect("write the A10 report");
    }
    assert!(files > 100, "the source walk found only {files} files");
    assert!(
        unknown.is_empty(),
        "codes missing from Appendix A (add them to the spec and run \
         python3 scripts/gen_exception_catalogue.py, or fix the code):\n{}",
        unknown.join("\n")
    );
    assert!(
        bad_combination.is_empty(),
        "severity or scope not allowed by Appendix A for that code (use one the catalogue \
         lists in crates/omr-model/src/catalogue.rs):\n{}",
        bad_combination.join("\n")
    );
}
