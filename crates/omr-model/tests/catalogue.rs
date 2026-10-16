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
