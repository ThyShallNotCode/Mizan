//! The exam definition (spec §4.1 stage 1, §6.10, §10.1, §10.3, §11.2): which sheet, how many
//! questions, forms, the per-exam marking convention, keys, roster and learning outcomes.
//! Persisted in the project store (template revision, form map, key version, roster snapshot,
//! CLO set, mapping set, attainment policy) and in the session snapshot.

use crate::AppError;
use omr_forms::TemplateSpec;
use omr_model::{CancelMode, ExamId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Current schema of [`ExamDefinition`].
pub const DEFINITION_SCHEMA: u32 = 1;

/// The committed legacy templates (§6.10).
pub const LEGACY_TEMPLATES: &[(&str, &str)] = &[(
    "iau-ccsit-v1",
    "IAU CCSIT answer sheet (landscape A4, 100 × A–E, 10-digit student number, version A–D; no fiducials)",
)];

/// Which answer sheet the exam uses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateChoice {
    /// A sheet generated from the template model (§6.1): tags, page code, ID grid.
    Generated(Box<TemplateSpec>),
    /// A committed legacy template by name, e.g. `iau-ccsit-v1` (§6.10).
    Legacy(String),
}

impl TemplateChoice {
    /// Short identifier for displays, e.g. `iau-ccsit-v1` or `gen:100x5`.
    pub fn id(&self) -> String {
        match self {
            TemplateChoice::Legacy(n) => n.clone(),
            TemplateChoice::Generated(s) => format!("gen:{}q-{}f", s.questions, s.forms),
        }
    }
}

/// When confident multiple marks go to review (§8.8 `multi_review`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MultiReviewPolicy {
    /// Always review.
    Always,
    /// Review in high-stakes mode only.
    HighStakes,
    /// Auto-score confident multis (MRK-002 INFO).
    #[default]
    Never,
}

impl MultiReviewPolicy {
    /// The recognition setting.
    pub fn to_recog(self) -> omr_recog::MultiReview {
        match self {
            MultiReviewPolicy::Always => omr_recog::MultiReview::Always,
            MultiReviewPolicy::HighStakes => omr_recog::MultiReview::HighStakes,
            MultiReviewPolicy::Never => omr_recog::MultiReview::Never,
        }
    }
}

/// One form's printed key: one entry per question (§10.1 sources).
///
/// Entries: `"B"`; several accepted answers `"A|C"` (also `"A/C"`, `"A;C"` or `"A,C"`);
/// `"*"` or `"VOID"` voids the item (dropped from the counted set); `""` is missing (SCO-003).
/// Arabic labels (أ ب ج د هـ) and 1-based digits are accepted too.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormKey {
    /// Form (version) letter, e.g. `"A"`.
    pub form: String,
    /// Entries per question.
    pub answers: Vec<String>,
}

/// Where the answer key comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum KeySource {
    /// No key: recognition, identity and review only; no score run.
    #[default]
    None,
    /// Keys entered or imported per form.
    Provided {
        /// One key per form.
        forms: Vec<FormKey>,
    },
    /// **Explicit opt-in**: the key of each form is the modal confident response per question
    /// (§10.3 inference without a key). Every output is marked INFERRED and the run can never
    /// be released (SCO-003 BLOCKER, sub-reason `inferred_key`).
    Inferred,
}

/// The student-ID scheme (§9.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum IdSchemeChoice {
    /// IAU: `2 YY NNNNNNN` (provisional).
    #[default]
    Iau,
    /// PNU.
    Pnu,
    /// KSU.
    Ksu,
    /// Any `n` digits, no check digit.
    Digits(usize),
}

impl IdSchemeChoice {
    /// The identity scheme.
    pub fn scheme(&self) -> omr_identity::StudentIdScheme {
        match self {
            IdSchemeChoice::Iau => omr_identity::StudentIdScheme::iau(),
            IdSchemeChoice::Pnu => omr_identity::StudentIdScheme::pnu(),
            IdSchemeChoice::Ksu => omr_identity::StudentIdScheme::ksu(),
            IdSchemeChoice::Digits(n) => omr_identity::StudentIdScheme::digits(*n, None),
        }
    }
}

/// One roster row (§9.2). Never a national ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RosterStudent {
    /// University student ID as text.
    pub id: String,
    /// Name as the SIS writes it.
    pub name: String,
    /// Arabic name.
    #[serde(default)]
    pub name_ar: String,
    /// Section code.
    #[serde(default)]
    pub section: String,
    /// The ID came from a numeric spreadsheet cell.
    #[serde(default)]
    pub numeric_cell: bool,
}

/// NQF domain letter K / S / V.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DomainLetter {
    /// Knowledge and understanding.
    K,
    /// Skills.
    S,
    /// Values, autonomy and responsibility.
    V,
}

impl DomainLetter {
    /// The outcomes domain.
    pub fn domain(self) -> omr_outcomes::Domain {
        match self {
            DomainLetter::K => omr_outcomes::Domain::Knowledge,
            DomainLetter::S => omr_outcomes::Domain::Skills,
            DomainLetter::V => omr_outcomes::Domain::Values,
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_uppercase().as_str() {
            "K" | "KNOWLEDGE" => Some(DomainLetter::K),
            "S" | "SKILLS" | "SKILL" => Some(DomainLetter::S),
            "V" | "VALUES" | "VALUE" => Some(DomainLetter::V),
            _ => None,
        }
    }
}

/// A course learning outcome.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloDefinition {
    /// e.g. `CLO1`.
    pub code: String,
    /// English text.
    pub title: String,
    /// Arabic text.
    #[serde(default)]
    pub title_ar: String,
    /// Domain.
    pub domain: DomainLetter,
    /// Related PLO code(s).
    #[serde(default)]
    pub plo: String,
}

/// One item → CLO mapping cell (§11.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloMapping {
    /// 1-based question number (printed position on every form).
    pub question: u16,
    /// CLO code.
    pub clo: String,
    /// Weight as an exact decimal or fraction string ("1", "0.5", "1/3").
    pub weight: String,
    /// Free bilingual note.
    #[serde(default)]
    pub note: String,
}

/// Attainment policy template (§11.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PolicyChoice {
    /// The engine default.
    #[default]
    Default,
    /// SEU template.
    Seu,
    /// UoM template.
    Uom,
    /// Taif per-question template.
    Taif,
    /// ABET FCAR template.
    AbetFcar,
}

impl PolicyChoice {
    /// The policy object.
    pub fn policy(self) -> omr_outcomes::AttainmentPolicy {
        use omr_outcomes::AttainmentPolicy as P;
        match self {
            PolicyChoice::Default => P::default(),
            PolicyChoice::Seu => P::seu(),
            PolicyChoice::Uom => P::uom(),
            PolicyChoice::Taif => P::taif(),
            PolicyChoice::AbetFcar => P::abet_fcar(),
        }
    }
}

/// Learning-outcome setup of the exam.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct OutcomeSetup {
    /// CLOs.
    pub clos: Vec<CloDefinition>,
    /// Item → CLO mapping.
    pub mapping: Vec<CloMapping>,
    /// Split mode (weights of a row sum to 1) instead of full mode.
    #[serde(default)]
    pub split: bool,
    /// Attainment policy.
    #[serde(default)]
    pub policy: PolicyChoice,
}

/// The exam definition. `cancel_mode` is mandatory (no default, §6.5, lint L14).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExamDefinition {
    /// Schema version.
    pub schema: u32,
    /// Exam (assessment) ID.
    pub exam_id: ExamId,
    /// Display title, e.g. "Final exam".
    pub title: String,
    /// Course code, e.g. "CS 522".
    #[serde(default)]
    pub course_code: String,
    /// Course title.
    #[serde(default)]
    pub course_title: String,
    /// Arabic course title.
    #[serde(default)]
    pub course_title_ar: String,
    /// Term label as the university writes it.
    #[serde(default)]
    pub term: String,
    /// ISO exam date.
    #[serde(default)]
    pub date: String,
    /// The answer sheet.
    pub template: TemplateChoice,
    /// Questions used by this exam (the sheet may print more).
    pub questions: u16,
    /// Options per question.
    pub options: u8,
    /// Form (version) letters in order; form code k + 1 is `forms[k]`.
    pub forms: Vec<String>,
    /// Marking convention (mandatory).
    pub cancel_mode: CancelMode,
    /// Confident-multi review policy.
    #[serde(default)]
    pub multi_review: MultiReviewPolicy,
    /// High-stakes mode (§8.8: erasure rule review-only, L_q = 9.2 nat).
    #[serde(default)]
    pub high_stakes: bool,
    /// Points per question (exact decimal strings); empty = 1 each.
    #[serde(default)]
    pub points: Vec<String>,
    /// Answer keys.
    #[serde(default)]
    pub key: KeySource,
    /// Roster (none = rosterless candidate-number identity).
    #[serde(default)]
    pub roster: Option<Vec<RosterStudent>>,
    /// Student-ID scheme.
    #[serde(default)]
    pub id_scheme: IdSchemeChoice,
    /// Learning outcomes.
    #[serde(default)]
    pub outcomes: Option<OutcomeSetup>,
    /// Page-code secret K_exam of a generated sheet (hex, 32 bytes). A secret (§6.4): never
    /// printed in reports or event payloads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_code_key_hex: Option<String>,
}

impl ExamDefinition {
    /// A definition with defaults for everything but the essentials.
    pub fn new(
        title: &str,
        template: TemplateChoice,
        questions: u16,
        options: u8,
        forms: Vec<String>,
        cancel_mode: CancelMode,
    ) -> Self {
        let mut d = Self {
            schema: DEFINITION_SCHEMA,
            exam_id: ExamId(uuid::Uuid::nil()),
            title: title.into(),
            course_code: String::new(),
            course_title: String::new(),
            course_title_ar: String::new(),
            term: String::new(),
            date: String::new(),
            template,
            questions,
            options,
            forms,
            cancel_mode,
            multi_review: MultiReviewPolicy::default(),
            high_stakes: false,
            points: Vec::new(),
            key: KeySource::None,
            roster: None,
            id_scheme: IdSchemeChoice::default(),
            outcomes: None,
            page_code_key_hex: None,
        };
        d.exam_id = d.derived_exam_id();
        d
    }

    /// A deterministic exam ID from the title, course and template (so repeated runs of the
    /// same definition produce identical outputs).
    pub fn derived_exam_id(&self) -> ExamId {
        let seed = format!(
            "omr-exam|{}|{}|{}|{}|{}",
            self.title,
            self.course_code,
            self.term,
            self.template.id(),
            self.questions
        );
        let h = omr_store::jcs::sha384(seed.as_bytes());
        let mut b = [0u8; 16];
        b.copy_from_slice(&h[..16]);
        b[6] = (b[6] & 0x0f) | 0x80; // version 8 (custom)
        b[8] = (b[8] & 0x3f) | 0x80; // RFC 4122 variant
        ExamId(uuid::Uuid::from_bytes(b))
    }

    /// Validates the definition (clear messages for the CLI and the setup screen).
    pub fn validate(&self) -> Result<(), AppError> {
        let bad = |m: String| Err(AppError::Definition(m));
        if self.questions == 0 {
            return bad("the exam needs at least one question".into());
        }
        if !(2..=10).contains(&self.options) {
            return bad(format!(
                "{} options per question: 2–10 supported",
                self.options
            ));
        }
        if self.forms.is_empty() {
            return bad("at least one form (version) letter is needed".into());
        }
        let uniq: BTreeSet<&String> = self.forms.iter().collect();
        if uniq.len() != self.forms.len() {
            return bad(format!("form letters repeat: {:?}", self.forms));
        }
        match &self.template {
            TemplateChoice::Legacy(name) => {
                if !LEGACY_TEMPLATES.iter().any(|(n, _)| n == name) {
                    return bad(format!(
                        "unknown legacy template `{name}`; known: {}",
                        LEGACY_TEMPLATES
                            .iter()
                            .map(|(n, _)| *n)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                if self.questions > 100 {
                    return bad(format!(
                        "{name} prints 100 questions; {} requested",
                        self.questions
                    ));
                }
                if self.options > 5 {
                    return bad(format!("{name} prints options A–E"));
                }
                if self.cancel_mode != CancelMode::EraseOnly {
                    return bad(format!(
                        "{name}: the legacy engine supports cancel_mode erase_only only"
                    ));
                }
                if self.forms.len() > 4 {
                    return bad(format!("{name} prints versions A–D only"));
                }
                if let Some(f) = self
                    .forms
                    .iter()
                    .find(|f| !["A", "B", "C", "D"].contains(&f.as_str()))
                {
                    return bad(format!(
                        "{name} prints versions A–D; form `{f}` cannot be bubbled"
                    ));
                }
            }
            TemplateChoice::Generated(spec) => {
                if spec.questions < self.questions {
                    return bad(format!(
                        "the sheet prints {} questions; {} requested",
                        spec.questions, self.questions
                    ));
                }
                if spec.cancel_mode != Some(self.cancel_mode) {
                    return bad(format!(
                        "the sheet prints cancel mode {:?}, the exam declares {:?}",
                        spec.cancel_mode, self.cancel_mode
                    ));
                }
                if usize::from(spec.forms) < self.forms.len() {
                    return bad(format!(
                        "the sheet has {} forms; {} declared",
                        spec.forms,
                        self.forms.len()
                    ));
                }
                // Page codes name the printed form letter; every declared form must be one.
                let printed: Vec<String> = (1..=spec.forms)
                    .map(|f| omr_model::FormCode(f).letter().to_string())
                    .collect();
                if let Some(f) = self.forms.iter().find(|f| !printed.contains(f)) {
                    return bad(format!(
                        "form `{f}` is not printed on this sheet (forms {})",
                        printed.join(", ")
                    ));
                }
                if spec.options.max() != self.options {
                    return bad(format!(
                        "the sheet prints {} options per question; the exam declares {}",
                        spec.options.max(),
                        self.options
                    ));
                }
                if self.page_code_key_hex.is_none() {
                    return bad(
                        "a generated sheet needs its page-code key (exam key hex) to verify pages"
                            .into(),
                    );
                }
                self.page_code_key()?;
            }
        }
        if let IdSchemeChoice::Digits(n) = self.id_scheme
            && !(1..=20).contains(&n)
        {
            return bad(format!("a student-ID scheme of {n} digits: 1–20 supported"));
        }
        if !self.points.is_empty() && self.points.len() != usize::from(self.questions) {
            return bad(format!(
                "{} point values for {} questions",
                self.points.len(),
                self.questions
            ));
        }
        for p in &self.points {
            omr_scoring::parse_points(p)
                .map_err(|e| AppError::Definition(format!("points `{p}`: {e}")))?;
        }
        if let KeySource::Provided { forms } = &self.key {
            if let Some(f) = self
                .forms
                .iter()
                .find(|f| !forms.iter().any(|k| &k.form == *f))
            {
                return bad(format!("no key for form {f}"));
            }
            for k in forms {
                if !self.forms.contains(&k.form) {
                    return bad(format!("key for unknown form `{}`", k.form));
                }
                if k.answers.len() != usize::from(self.questions) {
                    return bad(format!(
                        "key of form {} has {} entries for {} questions",
                        k.form,
                        k.answers.len(),
                        self.questions
                    ));
                }
                for (q, a) in k.answers.iter().enumerate() {
                    parse_key_entry(a, self.options).map_err(|m| {
                        AppError::Definition(format!("key {} Q{}: {m}", k.form, q + 1))
                    })?;
                }
            }
        }
        if let Some(o) = &self.outcomes {
            let codes: BTreeSet<&str> = o.clos.iter().map(|c| c.code.as_str()).collect();
            for m in &o.mapping {
                if !codes.contains(m.clo.as_str()) {
                    return bad(format!("mapping to unknown CLO `{}`", m.clo));
                }
                if m.question == 0 || m.question > self.questions {
                    return bad(format!("mapping of question {} out of range", m.question));
                }
                omr_scoring::parse_points(&m.weight)
                    .map_err(|e| AppError::Definition(format!("weight `{}`: {e}", m.weight)))?;
            }
        }
        Ok(())
    }

    /// The page-code key of a generated sheet.
    pub fn page_code_key(&self) -> Result<[u8; 32], AppError> {
        let h = self
            .page_code_key_hex
            .as_deref()
            .ok_or_else(|| AppError::Definition("no page-code key".into()))?;
        let v = hex::decode(h.trim())
            .map_err(|e| AppError::Definition(format!("page-code key is not hex: {e}")))?;
        <[u8; 32]>::try_from(v.as_slice())
            .map_err(|_| AppError::Definition("page-code key must be 32 bytes (64 hex)".into()))
    }

    /// True when the key is inferred from the sheets (never releasable).
    pub fn key_inferred(&self) -> bool {
        matches!(self.key, KeySource::Inferred)
    }

    /// Option labels of the sheet.
    pub fn option_labels(&self) -> Vec<String> {
        match &self.template {
            TemplateChoice::Legacy(_) => ["A", "B", "C", "D", "E"]
                .iter()
                .take(usize::from(self.options))
                .map(|s| s.to_string())
                .collect(),
            TemplateChoice::Generated(spec) => {
                let arabic = ["أ", "ب", "ج", "د", "هـ", "و", "ز", "ح", "ط", "ي"];
                (0..self.options)
                    .map(|o| match spec.labels {
                        omr_forms::LabelSet::Arabic => arabic[usize::from(o)].to_string(),
                        omr_forms::LabelSet::Numeric => (o + 1).to_string(),
                        _ => char::from(b'A' + o).to_string(),
                    })
                    .collect()
            }
        }
    }
}

/// A parsed key entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParsedKey {
    /// Accepted options (one or several), 0-based.
    Options(Vec<u8>),
    /// Voided item.
    Void,
    /// No entry (SCO-003).
    Missing,
}

/// Parses one option label: Latin letters, Arabic letters or 1-based digits.
pub fn parse_option(s: &str, options: u8) -> Option<u8> {
    let t = s.trim();
    let arabic = ["أ", "ب", "ج", "د", "هـ", "و", "ز", "ح", "ط", "ي"];
    let o = if let Some(i) = arabic
        .iter()
        .position(|a| *a == t || (*a == "هـ" && t == "ه"))
    {
        i as u8
    } else if t.len() == 1 && t.as_bytes()[0].is_ascii_alphabetic() {
        t.to_ascii_uppercase().as_bytes()[0] - b'A'
    } else if let Ok(n) = t.parse::<u8>() {
        n.checked_sub(1)?
    } else {
        return None;
    };
    (o < options).then_some(o)
}

/// Parses a key entry (`"B"`, `"A|C"`, `"*"`, `""`).
pub fn parse_key_entry(s: &str, options: u8) -> Result<ParsedKey, String> {
    let t = s.trim();
    if t.is_empty() {
        return Ok(ParsedKey::Missing);
    }
    if t == "*" || t.eq_ignore_ascii_case("void") {
        return Ok(ParsedKey::Void);
    }
    let mut v = Vec::new();
    for part in t
        .split(['|', '/', ';', ',', '+', ' '])
        .filter(|p| !p.is_empty())
    {
        let o = parse_option(part, options)
            .ok_or_else(|| format!("`{part}` is not one of the {options} options"))?;
        if !v.contains(&o) {
            v.push(o);
        }
    }
    if v.is_empty() {
        return Ok(ParsedKey::Missing);
    }
    v.sort_unstable();
    Ok(ParsedKey::Options(v))
}

fn norm_header(h: &str) -> String {
    h.trim()
        .trim_start_matches('\u{feff}')
        .to_lowercase()
        .replace(['_', '-'], " ")
}

/// Reads a key file (CSV). Two layouts:
/// - long: header `question,A,B[,points]`, one row per question;
/// - wide: header `form,Q1,Q2,…` (or `version,…`), one row per form.
///
/// Returns the per-form keys (in `forms` order) and the points column if present.
pub fn read_key_csv(
    bytes: &[u8],
    forms: &[String],
    questions: u16,
) -> Result<(Vec<FormKey>, Option<Vec<String>>), AppError> {
    let mut r = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(bytes);
    let headers: Vec<String> = r
        .headers()
        .map_err(|e| AppError::Input(format!("key CSV: {e}")))?
        .iter()
        .map(norm_header)
        .collect();
    let rows: Vec<Vec<String>> = r
        .records()
        .map(|rec| {
            rec.map(|x| x.iter().map(|c| c.trim().to_string()).collect())
                .map_err(|e| AppError::Input(format!("key CSV: {e}")))
        })
        .collect::<Result<_, _>>()?;
    let first = headers.first().cloned().unwrap_or_default();
    if first == "form" || first == "version" {
        let mut out = Vec::new();
        for row in &rows {
            let form = row.first().cloned().unwrap_or_default().to_uppercase();
            let answers: Vec<String> = (0..usize::from(questions))
                .map(|q| row.get(q + 1).cloned().unwrap_or_default())
                .collect();
            out.push(FormKey { form, answers });
        }
        return Ok((order_keys(out, forms)?, None));
    }
    if first != "question" && first != "q" && first != "item" {
        return Err(AppError::Input(format!(
            "key CSV: first column must be `question` (long layout) or `form` (wide layout), got `{first}`"
        )));
    }
    let mut cols: Vec<(String, usize)> = Vec::new();
    let mut points_col = None;
    for (i, h) in headers.iter().enumerate().skip(1) {
        if h == "points" || h == "marks" {
            points_col = Some(i);
        } else if h == "key" || h == "answer" {
            cols.push(("*".into(), i)); // one key for every form
        } else {
            cols.push((h.to_uppercase(), i));
        }
    }
    let mut keys: BTreeMap<String, Vec<String>> = cols
        .iter()
        .map(|(f, _)| (f.clone(), vec![String::new(); usize::from(questions)]))
        .collect();
    let mut points = points_col.map(|_| vec!["1".to_string(); usize::from(questions)]);
    for row in &rows {
        let qs = row.first().map(String::as_str).unwrap_or("");
        let q: usize = qs
            .trim_start_matches(['Q', 'q'])
            .parse()
            .map_err(|_| AppError::Input(format!("key CSV: bad question `{qs}`")))?;
        if q == 0 || q > usize::from(questions) {
            continue; // the sheet may hold more questions than the exam uses
        }
        for (f, i) in &cols {
            if let Some(v) = row.get(*i)
                && let Some(k) = keys.get_mut(f)
            {
                k[q - 1] = v.clone();
            }
        }
        if let (Some(pc), Some(p)) = (points_col, points.as_mut())
            && let Some(v) = row.get(pc).filter(|v| !v.is_empty())
        {
            p[q - 1] = v.clone();
        }
    }
    let out = keys
        .into_iter()
        .map(|(form, answers)| FormKey { form, answers })
        .collect();
    Ok((order_keys(out, forms)?, points))
}

fn order_keys(keys: Vec<FormKey>, forms: &[String]) -> Result<Vec<FormKey>, AppError> {
    // A single unlabeled key applies to every form (identical forms).
    if keys.len() == 1 && !forms.contains(&keys[0].form) {
        return Ok(forms
            .iter()
            .map(|f| FormKey {
                form: f.clone(),
                answers: keys[0].answers.clone(),
            })
            .collect());
    }
    let mut out = Vec::new();
    for f in forms {
        match keys.iter().find(|k| &k.form == f) {
            Some(k) => out.push(k.clone()),
            None => {
                return Err(AppError::Input(format!(
                    "key file has no key for form {f} (found: {})",
                    keys.iter()
                        .map(|k| k.form.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        }
    }
    Ok(out)
}

/// Header aliases of the roster columns.
fn roster_column(h: &str) -> Option<&'static str> {
    let h = norm_header(h);
    match h.as_str() {
        "id"
        | "student id"
        | "studentid"
        | "student number"
        | "student no"
        | "university id"
        | "sis id"
        | "الرقم الجامعي"
        | "رقم الطالب" => Some("id"),
        "name" | "student name" | "full name" | "name en" | "english name" | "الاسم" => {
            Some("name")
        }
        "name ar" | "arabic name" | "الاسم بالعربي" | "الاسم العربي" => {
            Some("name_ar")
        }
        "section" | "sec" | "الشعبة" => Some("section"),
        _ => None,
    }
}

fn roster_from_rows(rows: Vec<Vec<(String, bool)>>) -> Result<Vec<RosterStudent>, AppError> {
    let mut it = rows.into_iter();
    let header = it
        .next()
        .ok_or_else(|| AppError::Input("roster: empty file".into()))?;
    let cols: Vec<Option<&'static str>> = header.iter().map(|(h, _)| roster_column(h)).collect();
    let find = |name: &str| cols.iter().position(|c| *c == Some(name));
    let id = find("id").ok_or_else(|| {
        AppError::Input("roster: no student-ID column (e.g. `student_id`)".into())
    })?;
    let name = find("name");
    let name_ar = find("name_ar");
    let section = find("section");
    let mut out = Vec::new();
    for row in it {
        let get = |i: Option<usize>| {
            i.and_then(|i| row.get(i))
                .map(|(v, _)| v.trim().to_string())
                .unwrap_or_default()
        };
        let Some((idv, numeric)) = row.get(id) else {
            continue;
        };
        if idv.trim().is_empty() {
            continue;
        }
        out.push(RosterStudent {
            id: idv.trim().to_string(),
            name: get(name),
            name_ar: get(name_ar),
            section: get(section),
            numeric_cell: *numeric,
        });
    }
    Ok(out)
}

/// Reads a roster from CSV bytes (header row with aliases, see [`roster_column`]).
pub fn read_roster_csv(bytes: &[u8]) -> Result<Vec<RosterStudent>, AppError> {
    let mut r = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(bytes);
    let rows = r
        .records()
        .map(|rec| {
            rec.map(|x| x.iter().map(|c| (c.to_string(), false)).collect())
                .map_err(|e| AppError::Input(format!("roster CSV: {e}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    roster_from_rows(rows)
}

/// Reads a roster from the first sheet of an XLSX workbook (calamine). Numeric ID cells are
/// written without a decimal part and flagged (§9.2 rule 1).
pub fn read_roster_xlsx(bytes: &[u8]) -> Result<Vec<RosterStudent>, AppError> {
    use calamine::{Data, Reader, Xlsx};
    let mut wb: Xlsx<_> = Xlsx::new(std::io::Cursor::new(bytes.to_vec()))
        .map_err(|e| AppError::Input(format!("roster XLSX: {e}")))?;
    let name = wb
        .sheet_names()
        .first()
        .cloned()
        .ok_or_else(|| AppError::Input("roster XLSX: no sheet".into()))?;
    let range = wb
        .worksheet_range(&name)
        .map_err(|e| AppError::Input(format!("roster XLSX: {e}")))?;
    let rows = range
        .rows()
        .map(|r| {
            r.iter()
                .map(|c| match c {
                    Data::Int(i) => (i.to_string(), true),
                    Data::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => {
                        (format!("{}", *f as i64), true)
                    }
                    Data::Float(f) => (f.to_string(), true),
                    Data::String(s) => (s.clone(), false),
                    Data::Bool(b) => (b.to_string(), false),
                    Data::Empty => (String::new(), false),
                    other => (other.to_string(), false),
                })
                .collect()
        })
        .collect();
    roster_from_rows(rows)
}

/// Parses `"1-5,9"` into 1-based question numbers.
pub fn parse_question_list(s: &str, questions: u16) -> Result<Vec<u16>, String> {
    let mut out = BTreeSet::new();
    for part in s.split([',', ';', ' ']).filter(|p| !p.trim().is_empty()) {
        let p = part.trim().trim_start_matches(['Q', 'q']);
        let (a, b) = match p.split_once(['-', '–']) {
            Some((a, b)) => (a.trim(), b.trim().trim_start_matches(['Q', 'q'])),
            None => (p, p),
        };
        let a: u16 = a.parse().map_err(|_| format!("bad question `{part}`"))?;
        let b: u16 = b.parse().map_err(|_| format!("bad question `{part}`"))?;
        if a == 0 || b < a || b > questions {
            return Err(format!("question range `{part}` outside 1–{questions}"));
        }
        out.extend(a..=b);
    }
    Ok(out.into_iter().collect())
}

/// Reads the CLO file (CSV): header `clo,domain,title[,title_ar][,plo],questions[,weight][,note]`,
/// one or more rows per CLO; `questions` is a list like `1-5,9`.
pub fn read_clo_csv(bytes: &[u8], questions: u16) -> Result<OutcomeSetup, AppError> {
    let mut r = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(bytes);
    let headers: Vec<String> = r
        .headers()
        .map_err(|e| AppError::Input(format!("CLO CSV: {e}")))?
        .iter()
        .map(norm_header)
        .collect();
    let col = |names: &[&str]| headers.iter().position(|h| names.contains(&h.as_str()));
    let c_clo = col(&["clo", "code", "clo code"])
        .ok_or_else(|| AppError::Input("CLO CSV: no `clo` column".into()))?;
    let c_dom = col(&["domain", "nqf domain"]);
    let c_title = col(&["title", "text", "clo text", "description"]);
    let c_title_ar = col(&["title ar", "text ar", "arabic"]);
    let c_plo = col(&["plo", "plos"]);
    let c_q = col(&["questions", "items", "question"])
        .ok_or_else(|| AppError::Input("CLO CSV: no `questions` column".into()))?;
    let c_w = col(&["weight", "w"]);
    let c_note = col(&["note", "notes"]);
    let mut setup = OutcomeSetup::default();
    for rec in r.records() {
        let rec = rec.map_err(|e| AppError::Input(format!("CLO CSV: {e}")))?;
        let get = |c: Option<usize>| {
            c.and_then(|i| rec.get(i))
                .map(|v| v.trim().to_string())
                .unwrap_or_default()
        };
        let code = get(Some(c_clo));
        if code.is_empty() {
            continue;
        }
        if !setup.clos.iter().any(|c| c.code == code) {
            let dom = get(c_dom);
            let domain = if dom.is_empty() {
                DomainLetter::K
            } else {
                DomainLetter::parse(&dom).ok_or_else(|| {
                    AppError::Input(format!("CLO CSV: domain `{dom}` is not K, S or V"))
                })?
            };
            setup.clos.push(CloDefinition {
                code: code.clone(),
                title: get(c_title),
                title_ar: get(c_title_ar),
                domain,
                plo: get(c_plo),
            });
        }
        let qs = parse_question_list(&get(Some(c_q)), questions)
            .map_err(|m| AppError::Input(format!("CLO CSV {code}: {m}")))?;
        let w = get(c_w);
        let weight = if w.is_empty() { "1".to_string() } else { w };
        omr_scoring::parse_points(&weight)
            .map_err(|e| AppError::Input(format!("CLO CSV {code}: weight `{weight}`: {e}")))?;
        for q in qs {
            setup.mapping.push(CloMapping {
                question: q,
                clo: code.clone(),
                weight: weight.clone(),
                note: get(c_note),
            });
        }
    }
    if setup.clos.is_empty() {
        return Err(AppError::Input("CLO CSV: no CLO rows".into()));
    }
    Ok(setup)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_entries_parse() {
        assert_eq!(parse_key_entry("B", 5), Ok(ParsedKey::Options(vec![1])));
        assert_eq!(
            parse_key_entry("c|A", 5),
            Ok(ParsedKey::Options(vec![0, 2]))
        );
        assert_eq!(parse_key_entry(" * ", 5), Ok(ParsedKey::Void));
        assert_eq!(parse_key_entry("", 5), Ok(ParsedKey::Missing));
        assert_eq!(parse_key_entry("ب", 4), Ok(ParsedKey::Options(vec![1])));
        assert_eq!(parse_key_entry("3", 4), Ok(ParsedKey::Options(vec![2])));
        assert!(parse_key_entry("F", 5).is_err());
    }

    #[test]
    fn key_csv_long_and_wide() {
        let forms = vec!["A".to_string(), "B".to_string()];
        let long = b"question,A,B,points\n1,B,C,2\n2,A|D,*,1\n3,,E,\n";
        let (k, p) = read_key_csv(long, &forms, 3).unwrap();
        assert_eq!(k[0].answers, vec!["B", "A|D", ""]);
        assert_eq!(k[1].answers, vec!["C", "*", "E"]);
        assert_eq!(p.unwrap(), vec!["2", "1", "1"]);
        let wide = b"form,Q1,Q2,Q3\nB,C,*,E\nA,B,A|D,C\n";
        let (k, p) = read_key_csv(wide, &forms, 3).unwrap();
        assert_eq!(k[0].form, "A");
        assert_eq!(k[1].answers, vec!["C", "*", "E"]);
        assert!(p.is_none());
        let single = b"question,key\n1,A\n2,B\n3,C\n";
        let (k, _) = read_key_csv(single, &forms, 3).unwrap();
        assert_eq!(k.len(), 2);
        assert_eq!(k[1].answers, vec!["A", "B", "C"]);
    }

    #[test]
    fn roster_and_clo_csv() {
        let r = read_roster_csv(
            "\u{feff}Student ID,Name,Section\n900000001,Test One,101\n,skip,\n900000002,Test Two,102\n"
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[1].id, "900000002");
        assert_eq!(r[0].section, "101");
        let c = read_clo_csv(
            b"clo,domain,title,plo,questions,weight,note\nCLO1,K,Explain,K1,1-3,1,\nCLO2,S,Apply,S1,\"4,5\",0.5,half\n",
            5,
        )
        .unwrap();
        assert_eq!(c.clos.len(), 2);
        assert_eq!(c.mapping.len(), 5);
        assert_eq!(c.mapping[4].weight, "0.5");
        assert_eq!(parse_question_list("1-3, 7", 10).unwrap(), vec![1, 2, 3, 7]);
        assert!(parse_question_list("9-12", 10).is_err());
    }

    #[test]
    fn definition_requires_cancel_mode_in_json() {
        let d = ExamDefinition::new(
            "Quiz",
            TemplateChoice::Legacy("iau-ccsit-v1".into()),
            29,
            5,
            vec!["A".into(), "B".into()],
            CancelMode::EraseOnly,
        );
        d.validate().unwrap();
        let mut v = serde_json::to_value(&d).unwrap();
        v.as_object_mut().unwrap().remove("cancel_mode");
        assert!(serde_json::from_value::<ExamDefinition>(v).is_err());
        assert_eq!(d.exam_id, d.derived_exam_id());
    }
}
