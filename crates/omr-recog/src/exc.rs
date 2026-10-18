//! Exception constructors. Every exception goes through `omr_model::Exception::must`, which
//! checks code, severity and scope against Appendix A (a violation is a programming error).

use omr_model::{Exception, Scope, Severity, exceptions::Target};

/// A page-scoped exception.
pub(crate) fn page(code: &str, sev: Severity) -> Exception {
    Exception::must(code, sev, Scope::Page)
}

/// An item-scoped (question) exception targeting question `q` (0-based) on `page`.
pub(crate) fn item(code: &str, sev: Severity, page: Option<&str>, q: u16) -> Exception {
    Exception::must(code, sev, Scope::Item).with_target(Target {
        page: page.map(str::to_owned),
        question: Some(q),
        ..Target::default()
    })
}

/// A region-scoped exception (unreadable bubbles of one question or field).
pub(crate) fn region(
    code: &str,
    sev: Severity,
    page: Option<&str>,
    q: Option<u16>,
    bubble: Option<u8>,
) -> Exception {
    Exception::must(code, sev, Scope::Region).with_target(Target {
        page: page.map(str::to_owned),
        question: q,
        bubble,
        ..Target::default()
    })
}

/// A batch-scoped exception.
pub(crate) fn batch(code: &str, sev: Severity) -> Exception {
    Exception::must(code, sev, Scope::Batch)
}

/// A booklet-scoped exception.
pub(crate) fn booklet(code: &str, sev: Severity, page: Option<&str>) -> Exception {
    Exception::must(code, sev, Scope::Booklet).with_target(Target {
        page: page.map(str::to_owned),
        ..Target::default()
    })
}

/// Sets the page label on an exception's target.
pub(crate) fn on_page(mut e: Exception, page: Option<&str>) -> Exception {
    if e.target.page.is_none() {
        e.target.page = page.map(str::to_owned);
    }
    e
}
