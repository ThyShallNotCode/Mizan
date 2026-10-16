//! Report errors; a failed render raises SYS-029 (ERROR, Document scope, §17.1).

use omr_model::{Exception, Scope, Severity};
use thiserror::Error;

/// Why a report could not be written.
#[derive(Debug, Error)]
pub enum ReportError {
    /// The destination writer failed.
    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),
    /// The XLSX writer failed.
    #[error("xlsx: {0}")]
    Xlsx(#[from] rust_xlsxwriter::XlsxError),
    /// Inconsistent input facts.
    #[error("invalid report input: {0}")]
    Invalid(String),
}

impl ReportError {
    /// SYS-029 (ERROR, Document): only this document and the deliveries needing it wait.
    pub fn exception(&self, document: &str) -> Exception {
        Exception::must("SYS-029", Severity::Error, Scope::Document)
            .with_detail(format!("{document}: {self}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_failure_is_sys_029() {
        let e = ReportError::Invalid("x".into()).exception("results.xlsx");
        assert_eq!((e.code.as_str(), e.scope), ("SYS-029", Scope::Document));
    }
}
