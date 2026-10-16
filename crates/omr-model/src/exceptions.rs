//! The exception model (spec §13.1) and the Appendix A catalogue lookup.

use crate::catalogue::CATALOGUE;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Exception severity (§13.1). Release is blocked while any BLOCKER or ERROR is open in scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    /// Informational; never blocks.
    Info,
    /// Warning; never blocks.
    Warn,
    /// Must be resolved before release of its scope.
    Error,
    /// Stops processing of its scope until resolved.
    Blocker,
}

impl Severity {
    /// True for ERROR and BLOCKER.
    pub fn blocks_release(self) -> bool {
        matches!(self, Severity::Error | Severity::Blocker)
    }
}

/// What an open exception blocks (§13.1 scope enum).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Scope {
    /// One captured page.
    Page,
    /// A region of a page.
    Region,
    /// One question.
    Item,
    /// One booklet (all pages of one copy).
    Booklet,
    /// One student.
    Student,
    /// One scan batch.
    Batch,
    /// The whole exam.
    Exam,
    /// The course offering (blocks L5 publication).
    Course,
    /// An outcome report (blocks accreditation exports).
    OutcomeReport,
    /// One document only.
    Document,
}

/// A catalogue entry generated from Appendix A.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExceptionDef {
    /// The code, e.g. `MRK-001`.
    pub code: &'static str,
    /// Short case title.
    pub title: &'static str,
    /// Severities Appendix A allows for this code (per branch).
    pub severities: &'static [Severity],
    /// Scopes Appendix A allows for this code.
    pub scopes: &'static [Scope],
    /// Owning spec section(s).
    pub owner: &'static str,
}

/// Looks a code up in the Appendix A catalogue.
pub fn lookup(code: &str) -> Option<&'static ExceptionDef> {
    CATALOGUE.iter().find(|d| d.code == code)
}

/// Errors constructing an exception that Appendix A does not allow.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ExceptionError {
    /// The code is not in Appendix A.
    #[error("exception code {0} is not defined in Appendix A")]
    UnknownCode(String),
    /// The severity is not allowed for this code.
    #[error("severity {1:?} is not allowed for {0}")]
    BadSeverity(String, Severity),
    /// The scope is not allowed for this code.
    #[error("scope {1:?} is not allowed for {0}")]
    BadScope(String, Scope),
}

/// Where an exception applies; all fields optional.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Target {
    /// Captured page or source file identifier.
    pub page: Option<String>,
    /// Booklet serial.
    pub booklet: Option<u32>,
    /// 0-based question position.
    pub question: Option<u16>,
    /// 0-based bubble/option index.
    pub bubble: Option<u8>,
}

/// One raised exception (§13.1).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Exception {
    /// Appendix A code.
    pub code: String,
    /// Severity for this branch.
    pub severity: Severity,
    /// Scope it blocks.
    pub scope: Scope,
    /// Closed sub-reason per code (snake_case, e.g. `resolved_erasure`).
    pub sub_reason: Option<String>,
    /// Where it applies.
    pub target: Target,
    /// Human-readable detail for the reviewer.
    pub detail: String,
}

impl Exception {
    /// Builds an exception, checking code, severity and scope against Appendix A.
    pub fn new(code: &str, severity: Severity, scope: Scope) -> Result<Self, ExceptionError> {
        let def = lookup(code).ok_or_else(|| ExceptionError::UnknownCode(code.to_owned()))?;
        if !def.severities.contains(&severity) {
            return Err(ExceptionError::BadSeverity(code.to_owned(), severity));
        }
        if !def.scopes.contains(&scope) {
            return Err(ExceptionError::BadScope(code.to_owned(), scope));
        }
        Ok(Self {
            code: code.to_owned(),
            severity,
            scope,
            sub_reason: None,
            target: Target::default(),
            detail: String::new(),
        })
    }

    /// Like [`Exception::new`] but panics on a catalogue violation (a programming error).
    #[track_caller]
    pub fn must(code: &str, severity: Severity, scope: Scope) -> Self {
        match Self::new(code, severity, scope) {
            Ok(e) => e,
            Err(err) => panic!("{err}"),
        }
    }

    /// Sets the sub-reason.
    pub fn with_sub_reason(mut self, s: impl Into<String>) -> Self {
        self.sub_reason = Some(s.into());
        self
    }

    /// Sets the target.
    pub fn with_target(mut self, t: Target) -> Self {
        self.target = t;
        self
    }

    /// Sets the human-readable detail.
    pub fn with_detail(mut self, d: impl Into<String>) -> Self {
        self.detail = d.into();
        self
    }
}
