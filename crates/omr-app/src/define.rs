//! The exam definition from what an instructor gives (§6): the sheet, the question count and
//! the key, roster and CLO files. One set of rules for `omr-desktop --grade` and the desktop's
//! New-exam form, so both read the files as `omr grade` does (any encoding, delimiter or
//! layout; leading zeros restored; duplicate or damaged IDs refused with the rows to fix).
//! The file reads live here (rule R4), never in the shell.

use crate::AppError;
use crate::exam::{
    self, ExamDefinition, FormKey, IdSchemeChoice, KeySheetRule, KeySource, MultiReviewPolicy,
    RosterStudent, SetupField, TemplateChoice,
};
use crate::remark::RemarkImport;
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
#[derive(Debug, Clone, PartialEq)]
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
    /// No key: infer a provisional one from the answers (not releasable until replaced).
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
    /// `erase_only`, `cross_cancels` or `fill_cancels_check` (default: the IAU sheet's
    /// `erase_only`; an own sheet's printed convention, from its spec).
    pub cancel_mode: Option<String>,
    /// `never`, `always` or `high_stakes`.
    pub multi_review: Option<String>,
    /// High-stakes exam.
    pub high_stakes: bool,
    /// `iau`, `pnu`, `ksu` or `digits:N` (default: the sheet's ID grid).
    pub id_scheme: Option<String>,
    /// How key sheets are told apart from the students' (markers, detection in the stack;
    /// default: [`KeySheetRule::default`]), as `omr grade --key-sheet-marker` sets it.
    pub key_sheets: Option<KeySheetRule>,
    /// The key file's version labels read as the exam's versions (`1` → `A`, or a key
    /// version given to another letter), as the instructor answered the New-exam proposal.
    pub key_versions: Vec<(String, String)>,
    /// How the key file's headerless rows of one version each are read: their first cell is
    /// the version (`Some(true)`) or the answer to question 1 (`Some(false)`), as the New-exam
    /// proposal showed them; `None` decides it from the question count, as `omr grade` does
    /// ([`exam::read_key_file_rows`]).
    pub key_rows_labelled: Option<bool>,
    /// A Remark import (§3.5): its keys, exact points per version and learning objectives
    /// are the exam's (never with a key file, an inferred key or a CLO file).
    pub remark: Option<Box<RemarkImport>>,
    /// Where each value came from (the New-exam proposal's fields, counts only), recorded with
    /// the exam's setup ([`ExamDefinition::provenance`]).
    pub provenance: Vec<SetupField>,
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
            key_sheets: None,
            key_versions: Vec::new(),
            key_rows_labelled: None,
            remark: None,
            provenance: Vec::new(),
        }
    }
}

/// What a sheet prints: the bounds of every exam on it and its own conventions.
#[derive(Debug, Clone, PartialEq)]
pub struct PrintedSheet {
    /// The template.
    pub template: TemplateChoice,
    /// Questions printed (100 on the IAU sheet).
    pub questions: u16,
    /// Options per question printed (A–E on the IAU sheet).
    pub options: u8,
    /// Versions printed (bubbles A–D on the IAU sheet; an own sheet's forms).
    pub versions: Vec<String>,
    /// Its marking convention (§6.5): `erase_only` on the IAU sheet, an own sheet's spec's;
    /// `None` when the spec sets none (lint L14).
    pub cancel_mode: Option<CancelMode>,
    /// Columns of its student-number grid (0: none).
    pub id_digits: usize,
    /// An own sheet's page-code key (hex).
    pub key_hex: Option<String>,
}

/// Reads what `sheet` prints: a bundled template by name, or an own sheet's spec file.
pub fn printed(sheet: &Sheet) -> Result<PrintedSheet, AppError> {
    match sheet {
        Sheet::Template(t) => {
            if !exam::LEGACY_TEMPLATES.iter().any(|(n, _)| n == t) {
                return Err(AppError::Definition(format!(
                    "unknown legacy template `{t}`; known: {} (or give an own sheet's spec file)",
                    exam::LEGACY_TEMPLATES
                        .iter()
                        .map(|(n, _)| *n)
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
            Ok(PrintedSheet {
                template: TemplateChoice::Legacy(t.clone()),
                questions: 100,
                options: 5,
                versions: ["A", "B", "C", "D"].map(String::from).to_vec(),
                cancel_mode: Some(CancelMode::EraseOnly),
                id_digits: 10,
                key_hex: None,
            })
        }
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
            Ok(PrintedSheet {
                questions: spec.questions,
                options: spec.options.max(),
                versions: (0..spec.forms)
                    .map(|f| char::from(b'A' + f.min(25) as u8).to_string())
                    .collect(),
                cancel_mode: spec.cancel_mode,
                id_digits: usize::from(spec.id_digits),
                template: choice,
                key_hex: key,
            })
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

/// The validated definition, with its warnings: answer-key rows beyond the question count
/// (never graded, named so a count typed too low is seen) and the roster import's (student
/// numbers masked: they reach logs and the status line).
pub fn define(i: &Inputs) -> Result<(ExamDefinition, Vec<String>), AppError> {
    let sheet = printed(&i.sheet)?;
    let (template, options, key_hex) = (sheet.template, sheet.options, sheet.key_hex);
    // `omr grade`'s default for the IAU sheet stays A,B (changing the CLI's defaults is
    // deferred); the desktop's New exam always gives the versions its proposal found.
    let default_forms = match &template {
        TemplateChoice::Legacy(_) => vec!["A".to_string(), "B".to_string()],
        TemplateChoice::Generated(_) => sheet.versions,
    };
    let key_bytes = i
        .key
        .as_deref()
        .map(|p| read(p, "answer key"))
        .transpose()?;
    // A version label of the key file as the exam's version, and back.
    let as_version = |l: String| {
        i.key_versions
            .iter()
            .find(|(from, _)| *from == l)
            .map_or(l, |(_, to)| to.clone())
    };
    let remark_forms = i.remark.as_ref().map(|r| r.forms());
    let forms: Vec<String> = i
        .forms
        .clone()
        .filter(|f| !f.is_empty())
        .or_else(|| remark_forms.clone())
        .or_else(|| {
            key_bytes
                .as_deref()
                .and_then(exam::key_file_forms)
                .map(|f| f.into_iter().map(as_version).collect())
        })
        .unwrap_or(default_forms)
        .into_iter()
        .map(|f| f.trim().to_uppercase())
        .collect();
    let typed = match i.cancel_mode.as_deref() {
        None => None,
        Some("erase_only") => Some(CancelMode::EraseOnly),
        Some("cross_cancels") => Some(CancelMode::CrossCancels),
        Some("fill_cancels_check") => Some(CancelMode::FillCancelsCheck),
        Some(o) => {
            return Err(AppError::Definition(format!(
                "cancel mode {o}: use erase_only, cross_cancels or fill_cancels_check"
            )));
        }
    };
    // The marking convention is printed on the sheet (§6.5): the IAU sheet's is erase_only, an
    // own sheet's is its spec's (a typed one must agree, `ExamDefinition::validate`). A spec
    // without one cannot have printed sheets (lint L14): refused, never guessed.
    let cancel = match &template {
        TemplateChoice::Generated(spec) => match (spec.cancel_mode, typed) {
            (Some(printed), t) => t.unwrap_or(printed),
            (None, _) => {
                let path = match &i.sheet {
                    Sheet::Spec(p) => p.display().to_string(),
                    Sheet::Template(t) => t.clone(),
                };
                return Err(AppError::Definition(format!(
                    "{path}: the sheet spec sets no cancel_mode, so the marking convention \
                     printed on the sheets is unknown (lint L14); set `cancel_mode` in the spec \
                     to the one the sheets print (erase_only, cross_cancels or \
                     fill_cancels_check)"
                )));
            }
        },
        TemplateChoice::Legacy(_) => typed.unwrap_or(CancelMode::EraseOnly),
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
    // Every new exam holds a sheet bubbling a version it does not declare (§10.3 backstop).
    d.hold_undeclared_versions = true;
    if let Some(rule) = &i.key_sheets {
        d.key_sheets = rule.clone();
    }
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
    let mut warnings = Vec::new();
    match (&key_bytes, i.infer_key, &i.remark) {
        (Some(_), true, _) => {
            return Err(AppError::Definition(
                "give an answer key file or infer the key, not both".into(),
            ));
        }
        (_, _, Some(imp)) => {
            if key_bytes.is_some() || i.infer_key {
                return Err(AppError::Definition(
                    "the key comes from the Remark import: give no key file and infer no key"
                        .into(),
                ));
            }
            if imp.questions != d.questions {
                return Err(AppError::Definition(format!(
                    "the Remark export has {} questions, not {}",
                    imp.questions, d.questions
                )));
            }
            if remark_forms.as_ref() != Some(&d.forms) {
                return Err(AppError::Definition(format!(
                    "the Remark export gives versions {}, not {}",
                    imp.forms().join(", "),
                    d.forms.join(", ")
                )));
            }
            if i.clo.is_some() && imp.versions.iter().any(|v| !v.groups.is_empty()) {
                return Err(AppError::Definition(
                    "the learning objectives come from the Remark import: give no CLO file".into(),
                ));
            }
            d.key = KeySource::Provided {
                forms: imp.form_keys(),
            };
            d.set_key_points(&imp.key_points());
            d.outcomes = imp.outcome_setup();
        }
        (Some(bytes), false, None) => {
            // Read under the file's own labels, then named as the exam's versions.
            let labels: Vec<String> = d
                .forms
                .iter()
                .map(|f| {
                    i.key_versions
                        .iter()
                        .find(|(_, to)| to == f)
                        .map_or_else(|| f.clone(), |(from, _)| from.clone())
                })
                .collect();
            let mut k = exam::read_key_file_rows(
                bytes,
                &labels,
                d.questions,
                d.options,
                i.key_rows_labelled,
            )?;
            for key in &mut k.forms {
                key.form = as_version(std::mem::take(&mut key.form));
            }
            k.points.per_form = std::mem::take(&mut k.points.per_form)
                .into_iter()
                .map(|(f, p)| (as_version(f), p))
                .collect();
            warnings.extend(k.ignored_warning(d.questions));
            d.key = KeySource::Provided { forms: k.forms };
            d.set_key_points(&k.points);
        }
        (None, true, None) => d.key = KeySource::Inferred,
        (None, false, None) => {}
    }
    if let Some(r) = &i.roster {
        warnings.extend(
            d.import_roster(read_roster_path(r)?)?
                .iter()
                .map(|w| crate::export::mask_numbers(w)),
        );
    }
    if let Some(c) = &i.clo {
        d.outcomes = Some(exam::read_clo_csv(&read(c, "CLO mapping")?, d.questions)?);
    }
    d.provenance = i.provenance.clone();
    d.exam_id = d.derived_exam_id();
    d.validate()?;
    Ok((d, warnings))
}

/// The roster rows of a CSV or `.xlsx` file (before the exam's scheme repairs them).
pub fn read_roster_path(r: &Path) -> Result<Vec<RosterStudent>, AppError> {
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
    if ext == "xlsx" {
        exam::read_roster_xlsx(&bytes)
    } else {
        exam::read_roster_csv(&bytes)
    }
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

/// [`read_key_path`] with every points column the file has: one `points` column and/or one
/// per version (`points_A`, `points_B`), as `omr key replace` reads them.
pub fn read_key_path_full(path: &Path, def: &ExamDefinition) -> Result<exam::KeyFile, AppError> {
    exam::read_key_file_full(
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
