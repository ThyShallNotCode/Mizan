//! The exam definition from what an instructor gives (§6): the sheet, the question count and
//! the key, roster and CLO files. One set of rules for `omr-desktop --grade` and the desktop's
//! New-exam form, so both read the files as `omr grade` does (any encoding, delimiter or
//! layout; leading zeros restored; duplicate or damaged IDs refused with the rows to fix).
//! The file reads live here (rule R4), never in the shell.

use crate::AppError;
use crate::exam::{
    self, ExamDefinition, FormKey, IdSchemeChoice, KeySource, MultiReviewPolicy, TemplateChoice,
};
use omr_model::CancelMode;
use std::path::{Path, PathBuf};

/// The printed sheet: a bundled legacy template or an own sheet's spec file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sheet {
    /// A bundled template ID (`iau-ccsit-v1`).
    Template(String),
    /// The spec file `omr sheet render` printed the sheets from.
    Spec(PathBuf),
}

impl Sheet {
    /// A template ID, or a spec file when the text names a `.json` file or a path.
    pub fn parse(text: &str) -> Self {
        let t = text.trim();
        if t.to_ascii_lowercase().ends_with(".json") || t.contains(['/', '\\']) {
            Sheet::Spec(PathBuf::from(t))
        } else {
            Sheet::Template(t.to_string())
        }
    }
}

/// What the instructor gives for a new exam; `None` takes the sheet's default.
#[derive(Debug, Clone)]
pub struct Inputs {
    /// The printed sheet.
    pub sheet: Sheet,
    /// Questions this exam uses.
    pub questions: u16,
    /// Options per question (default: the sheet's).
    pub options: Option<u8>,
    /// Form letters (default: the key file's forms, else the sheet's).
    pub forms: Option<Vec<String>>,
    /// The answer key file (CSV or Excel-saved CSV).
    pub key: Option<PathBuf>,
    /// No key: infer a provisional one from the answers (never releasable).
    pub infer_key: bool,
    /// The roster (CSV or .xlsx).
    pub roster: Option<PathBuf>,
    /// The CLO mapping (CSV).
    pub clo: Option<PathBuf>,
    /// Exam title (default "Exam").
    pub title: Option<String>,
    /// Course code.
    pub course: Option<String>,
    /// Term.
    pub term: Option<String>,
    /// `erase_only`, `cross_cancels` or `fill_cancels_check`.
    pub cancel_mode: Option<String>,
    /// `never`, `always` or `high_stakes`.
    pub multi_review: Option<String>,
    /// High-stakes exam.
    pub high_stakes: bool,
    /// `iau`, `pnu`, `ksu` or `digits:N` (default: the sheet's ID grid).
    pub id_scheme: Option<String>,
}

impl Inputs {
    /// Inputs with every optional field at its default.
    pub fn new(sheet: Sheet, questions: u16) -> Self {
        Self {
            sheet,
            questions,
            options: None,
            forms: None,
            key: None,
            infer_key: false,
            roster: None,
            clo: None,
            title: None,
            course: None,
            term: None,
            cancel_mode: None,
            multi_review: None,
            high_stakes: false,
            id_scheme: None,
        }
    }
}

fn read(p: &Path, what: &str) -> Result<Vec<u8>, AppError> {
    std::fs::read(p).map_err(|e| {
        AppError::Input(format!(
            "{} ({what}): {e}; check the path of the file",
            p.display()
        ))
    })
}

/// The validated definition, with the roster import's warnings (student numbers masked:
/// they reach logs and the status line).
pub fn define(i: &Inputs) -> Result<(ExamDefinition, Vec<String>), AppError> {
    let (template, options, default_forms, key_hex) = match &i.sheet {
        Sheet::Template(t) => (
            TemplateChoice::Legacy(t.clone()),
            5,
            vec!["A".to_string(), "B".to_string()],
            None,
        ),
        Sheet::Spec(p) => {
            let v: serde_json::Value =
                serde_json::from_slice(&read(p, "sheet spec")?).map_err(|e| {
                    AppError::Definition(format!(
                        "{}: not JSON ({e}); give the spec file `omr sheet render` printed from",
                        p.display()
                    ))
                })?;
            let (t, key) = match v.get("template") {
                Some(t) => (
                    t.clone(),
                    v.get("exam_key_hex")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                ),
                None => (v, None),
            };
            let choice: TemplateChoice =
                serde_json::from_value(serde_json::json!({ "generated": t })).map_err(|e| {
                    AppError::Definition(format!(
                        "{}: not a sheet spec ({e}); `omr templates --example-spec` prints one",
                        p.display()
                    ))
                })?;
            let TemplateChoice::Generated(spec) = &choice else {
                return Err(AppError::Internal(
                    "a spec file is a generated sheet".into(),
                ));
            };
            let forms = (0..spec.forms)
                .map(|f| char::from(b'A' + f.min(25) as u8).to_string())
                .collect();
            let opts = spec.options.max();
            (choice, opts, forms, key)
        }
    };
    let key_bytes = i
        .key
        .as_deref()
        .map(|p| read(p, "answer key"))
        .transpose()?;
    let forms = i
        .forms
        .clone()
        .filter(|f| !f.is_empty())
        .or_else(|| key_bytes.as_deref().and_then(exam::key_file_forms))
        .unwrap_or(default_forms)
        .into_iter()
        .map(|f| f.trim().to_uppercase())
        .collect();
    let cancel = match i.cancel_mode.as_deref().unwrap_or("erase_only") {
        "erase_only" => CancelMode::EraseOnly,
        "cross_cancels" => CancelMode::CrossCancels,
        "fill_cancels_check" => CancelMode::FillCancelsCheck,
        o => {
            return Err(AppError::Definition(format!(
                "cancel mode {o}: use erase_only, cross_cancels or fill_cancels_check"
            )));
        }
    };
    let mut d = ExamDefinition::new(
        i.title
            .as_deref()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or("Exam"),
        template,
        i.questions,
        i.options.unwrap_or(options),
        forms,
        cancel,
    );
    d.course_code = i.course.clone().unwrap_or_default();
    d.term = i.term.clone().unwrap_or_default();
    d.page_code_key_hex = key_hex;
    d.high_stakes = i.high_stakes;
    d.multi_review = match i.multi_review.as_deref().unwrap_or("never") {
        "never" => MultiReviewPolicy::Never,
        "always" => MultiReviewPolicy::Always,
        "high_stakes" => MultiReviewPolicy::HighStakes,
        o => {
            return Err(AppError::Definition(format!(
                "multi-review {o}: use never, always or high_stakes"
            )));
        }
    };
    // the default is the scheme of the sheet's ID grid (`ExamDefinition::new`)
    if let Some(s) = i.id_scheme.as_deref() {
        d.id_scheme = match s {
            "iau" => IdSchemeChoice::Iau,
            "pnu" => IdSchemeChoice::Pnu,
            "ksu" => IdSchemeChoice::Ksu,
            s => match s.strip_prefix("digits:").and_then(|n| n.parse().ok()) {
                Some(n) => IdSchemeChoice::Digits(n),
                None => {
                    return Err(AppError::Definition(format!(
                        "ID scheme {s}: use iau, pnu, ksu or digits:N"
                    )));
                }
            },
        };
    }
    match (&key_bytes, i.infer_key) {
        (Some(_), true) => {
            return Err(AppError::Definition(
                "give an answer key file or infer the key, not both".into(),
            ));
        }
        (Some(bytes), false) => {
            let (forms, points) = exam::read_key_file(bytes, &d.forms, d.questions, d.options)?;
            d.key = KeySource::Provided { forms };
            if let Some(p) = points {
                d.points = p;
            }
        }
        (None, true) => d.key = KeySource::Inferred,
        (None, false) => {}
    }
    let mut warnings = Vec::new();
    if let Some(r) = &i.roster {
        let ext = r
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext == "xlsm" || ext == "xls" {
            return Err(AppError::Input(format!(
                "{}: .{ext} workbooks are not read; save the roster as .xlsx or CSV UTF-8",
                r.display()
            )));
        }
        let bytes = read(r, "roster")?;
        let rows = if ext == "xlsx" {
            exam::read_roster_xlsx(&bytes)
        } else {
            exam::read_roster_csv(&bytes)
        }?;
        warnings = d
            .import_roster(rows)?
            .iter()
            .map(|w| crate::export::mask_numbers(w))
            .collect();
    }
    if let Some(c) = &i.clo {
        d.outcomes = Some(exam::read_clo_csv(&read(c, "CLO mapping")?, d.questions)?);
    }
    d.exam_id = d.derived_exam_id();
    d.validate()?;
    Ok((d, warnings))
}

/// A replacement answer key read from `path` for the exam `def` (the same reading as the key
/// file of a new exam).
pub fn read_key_path(
    path: &Path,
    def: &ExamDefinition,
) -> Result<(Vec<FormKey>, Option<Vec<String>>), AppError> {
    exam::read_key_file(
        &read(path, "answer key")?,
        &def.forms,
        def.questions,
        def.options,
    )
}

/// A new folder below `base` named after the exam's course and title (`cs-522-midterm`,
/// then `cs-522-midterm-2`, …), never one that already holds a project.
pub fn new_exam_dir(base: &Path, def: &ExamDefinition) -> PathBuf {
    let slug: String = format!("{} {}", def.course_code, def.title)
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.is_empty() { "exam".into() } else { slug };
    let mut dir = base.join(&slug);
    let mut n = 2;
    while dir.exists() {
        dir = base.join(format!("{slug}-{n}"));
        n += 1;
    }
    dir
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sheet_text_names_a_template_or_a_spec_file() {
        assert_eq!(
            Sheet::parse(" iau-ccsit-v1 "),
            Sheet::Template("iau-ccsit-v1".into())
        );
        assert_eq!(
            Sheet::parse("sheets.json"),
            Sheet::Spec("sheets.json".into())
        );
        assert_eq!(Sheet::parse("~/x/spec"), Sheet::Spec("~/x/spec".into()));
    }

    #[test]
    fn new_exam_folders_never_reuse_an_existing_folder() {
        let dir = tempfile::tempdir().unwrap();
        let mut d = ExamDefinition::new(
            "Midterm 1",
            TemplateChoice::Legacy("iau-ccsit-v1".into()),
            20,
            5,
            vec!["A".into()],
            CancelMode::EraseOnly,
        );
        d.course_code = "CS 522".into();
        let a = new_exam_dir(dir.path(), &d);
        assert_eq!(a, dir.path().join("cs-522-midterm-1"));
        std::fs::create_dir(&a).unwrap();
        assert_eq!(
            new_exam_dir(dir.path(), &d),
            dir.path().join("cs-522-midterm-1-2")
        );
    }

    #[test]
    fn a_missing_key_file_names_the_file_and_what_to_do() {
        let mut i = Inputs::new(Sheet::Template("iau-ccsit-v1".into()), 20);
        i.key = Some("/nonexistent/key.csv".into());
        let e = define(&i).unwrap_err().to_string();
        assert!(
            e.contains("answer key") && e.contains("check the path"),
            "{e}"
        );
    }
}
