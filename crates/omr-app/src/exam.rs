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

/// How key sheets are told apart from the students' sheets (§10.1 key sources). A key sheet is
/// the instructor's own filled answer sheet: the same key-blind recognizer reads it, and the
/// app routes what it read to the key store (`omr-scoring` turns it into key entries); it is
/// never graded as a student, and its key is never used before the instructor confirms it.
///
/// **The rule** (the default suits IAU sheets): a page is a key sheet when
/// 1. its file was named as a key sheet (`omr grade --key-sheet FILE`, `omr key read`), or
/// 2. `detect_in_stack` is on and **every** column of its student-number grid is confidently
///    bubbled with one marker digit (by default 0 or 9: `0000000000`, `9999999999`).
///
/// A key sheet gives the key of the version it declares (a confident version bubble, or a
/// verified page code); on a one-version exam a blank version bubble is that version. A key
/// sheet of a several-version exam whose version is blank or unclear is refused with what to do
/// and holds the run: it is never graded as a student whose number is the marker.
///
/// A marker is used in the stack only when it can never be a student: the exam's ID scheme must
/// reject it (IAU student numbers are `2YYNNNNNNN`, so both default markers are rejected) and no
/// roster student may have it; otherwise detection with that marker is off and the key summary
/// says so. A student number that merely starts or ends with zeros is never a marker. A named
/// key sheet must show a marker too: a key sheet whose grid does not read as one (e.g.
/// `1111111111` from a sheet registered one bubble row off) is refused, never read as a key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeySheetRule {
    /// Find key sheets inside the students' stack (rule 2).
    pub detect_in_stack: bool,
    /// Marker digits: a key sheet bubbles one of them in every student-number column. Empty:
    /// no detection in the stack and no marker check on named key sheets.
    pub markers: Vec<u8>,
    /// SHA-384 (hex) of the files named as key sheets: every sheet in them is a key sheet.
    #[serde(default)]
    pub declared: Vec<String>,
}

impl Default for KeySheetRule {
    fn default() -> Self {
        Self {
            detect_in_stack: true,
            markers: vec![0, 9],
            declared: Vec::new(),
        }
    }
}

impl KeySheetRule {
    /// No key sheets in the stack, no marker check (`--key-sheet-marker none`).
    pub fn off() -> Self {
        Self {
            detect_in_stack: false,
            markers: Vec::new(),
            declared: Vec::new(),
        }
    }

    /// The markers usable in the stack for an ID grid of `digits` columns, and a note for each
    /// marker left out because it could be a real student number (the scheme accepts it, or a
    /// roster student has it).
    pub fn stack_markers(
        &self,
        digits: usize,
        scheme: &omr_identity::StudentIdScheme,
        roster: Option<&[RosterStudent]>,
    ) -> (Vec<u8>, Vec<String>) {
        let mut ok = Vec::new();
        let mut notes = Vec::new();
        if !self.detect_in_stack || digits == 0 {
            return (ok, notes);
        }
        for &d in &self.markers {
            let id: String = std::iter::repeat_n(char::from(b'0' + d), digits).collect();
            if scheme.accepts(&id) {
                notes.push(format!(
                    "key-sheet marker {id} is not used in the stack: under the {} student-ID \
                     scheme it could be a real student number (name key sheets with --key-sheet)",
                    scheme.name()
                ));
            } else if roster.is_some_and(|r| {
                r.iter()
                    .any(|s| omr_identity::normalize_id(&s.id) == omr_identity::normalize_id(&id))
            }) {
                notes.push(format!(
                    "key-sheet marker {id} is not used in the stack: a roster student has this \
                     number (name key sheets with --key-sheet)"
                ));
            } else {
                ok.push(d);
            }
        }
        (ok, notes)
    }
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
    /// Digits of a student number under the scheme (IAU 10, PNU and KSU 9).
    pub fn digit_count(&self) -> usize {
        match self {
            IdSchemeChoice::Iau => 10,
            IdSchemeChoice::Pnu | IdSchemeChoice::Ksu => 9,
            IdSchemeChoice::Digits(n) => *n,
        }
    }

    /// The scheme for a printed ID grid of `digits` columns: IAU's for 10, else `digits:N`.
    pub fn for_grid(digits: usize) -> Self {
        if digits == 10 {
            IdSchemeChoice::Iau
        } else {
            IdSchemeChoice::Digits(digits)
        }
    }

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
    /// 1-based question number (printed position on every form, or on `form` alone).
    pub question: u16,
    /// CLO code.
    pub clo: String,
    /// Weight as an exact decimal or fraction string ("1", "0.5", "1/3").
    pub weight: String,
    /// Free bilingual note.
    #[serde(default)]
    pub note: String,
    /// The version (form letter) the row applies to; empty: the printed question on every
    /// version. Versions whose questions evidence different CLOs (e.g. Remark learning
    /// objectives imported per version, §3.5) map each version's printed questions.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub form: String,
    /// Where the row came from when it is not the coordinator's own entry (§11.2 `source`):
    /// e.g. `Remark import`. Such a row is `proposed` ("imported, needs review"): it counts in
    /// the attainment shown, and the official TP-154 tables carry the draft watermark until
    /// the coordinator approves the mapping (SCO-032).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub imported: String,
    /// Who approved an imported row (an audited `MappingApproved` act, §11.2): the row is then
    /// `approved` with its source kept. Empty: not approved (an imported row stays `proposed`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub approved_by: String,
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
    /// Points per question of a version whose printed questions carry other points than
    /// `points` (form letter → one value per question), e.g. versions that shuffle questions
    /// of different weights (§3.5 Remark import). Versions not listed take `points`. A key
    /// whose versions carry different points never shares master items across them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub form_points: BTreeMap<String, Vec<String>>,
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
    /// How key sheets are recognised (§10.1; see [`KeySheetRule`]).
    #[serde(default)]
    pub key_sheets: KeySheetRule,
    /// Page-code secret K_exam of a generated sheet (hex, 32 bytes). A secret (§6.4): never
    /// printed in reports or event payloads, and never serialised: the session snapshot leaves
    /// it out and the project keeps it in its private `secrets/` file (read back on open).
    #[serde(default, skip_serializing)]
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
        // Own sheets: the scheme of the printed ID grid (CR-12).
        let id_scheme = match &template {
            TemplateChoice::Generated(spec) if spec.id_digits > 0 => {
                IdSchemeChoice::for_grid(usize::from(spec.id_digits))
            }
            _ => IdSchemeChoice::default(),
        };
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
            form_points: BTreeMap::new(),
            key: KeySource::None,
            roster: None,
            id_scheme,
            outcomes: None,
            key_sheets: KeySheetRule::default(),
            page_code_key_hex: None,
        };
        d.exam_id = d.derived_exam_id();
        d
    }

    /// The exam ID: for a generated sheet, the exam its sheets were printed for (the page
    /// code carries it, so the project and the sheets name one exam, SEC-04); for a legacy
    /// sheet, a deterministic ID from the title, course and template (so repeated runs of the
    /// same definition produce identical outputs).
    pub fn derived_exam_id(&self) -> ExamId {
        if let TemplateChoice::Generated(spec) = &self.template {
            return spec.exam;
        }
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
                if self.exam_id != spec.exam {
                    return bad(format!(
                        "the exam is {} but its sheets were printed for exam {} (the spec's \
                         `exam`); grade with the spec the sheets were rendered from",
                        self.exam_id, spec.exam
                    ));
                }
                if spec.id_digits > 0 && usize::from(spec.id_digits) != self.id_scheme.digit_count()
                {
                    return bad(format!(
                        "the sheet's student-ID grid has {} digits but the ID scheme {:?} has \
                         {}: pass --id-scheme {}",
                        spec.id_digits,
                        self.id_scheme,
                        self.id_scheme.digit_count(),
                        if spec.id_digits == 10 {
                            "iau".to_string()
                        } else {
                            format!("digits:{}", spec.id_digits)
                        }
                    ));
                }
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
        for (f, pts) in &self.form_points {
            if !self.forms.contains(f) {
                return bad(format!(
                    "points for version `{f}`, which the exam does not declare (forms {})",
                    self.forms.join(", ")
                ));
            }
            if pts.len() != usize::from(self.questions) {
                return bad(format!(
                    "{} point values of version {f} for {} questions",
                    pts.len(),
                    self.questions
                ));
            }
            for p in pts {
                omr_scoring::parse_points(p).map_err(|e| {
                    AppError::Definition(format!("points `{p}` of version {f}: {e}"))
                })?;
            }
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
                        AppError::Definition(format!(
                            "key of form {} question {}: {m}",
                            k.form,
                            q + 1
                        ))
                    })?;
                }
            }
        }
        if let Some(r) = &self.roster {
            check_roster(r, &self.id_scheme)?;
        }
        let ks = &self.key_sheets;
        if ks.markers.iter().any(|d| *d > 9) {
            return bad(format!(
                "key-sheet markers {:?}: each marker is one digit 0–9",
                ks.markers
            ));
        }
        if ks.markers.iter().collect::<BTreeSet<_>>().len() != ks.markers.len() {
            return bad(format!("key-sheet markers repeat: {:?}", ks.markers));
        }
        if ks.detect_in_stack && ks.markers.is_empty() {
            return bad(
                "key-sheet detection in the stack needs at least one marker digit (or turn it \
                 off)"
                    .into(),
            );
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
                if !m.form.is_empty() && !self.forms.contains(&m.form) {
                    return bad(format!(
                        "mapping of question {} for version `{}`, which the exam does not \
                         declare (forms {})",
                        m.question,
                        m.form,
                        self.forms.join(", ")
                    ));
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

    /// Imports roster rows (§9.2): restores leading zeros a spreadsheet dropped when the
    /// student-ID scheme makes it unambiguous ([`restore_leading_zeros`]), then validates the
    /// rows as the snapshot will ([`check_roster`]): duplicate, empty or precision-damaged IDs
    /// and national IDs refuse the roster with what to change. On success the roster is set
    /// and the warnings (repairs, scheme mismatches, converted numeric cells …) are returned
    /// for display.
    pub fn import_roster(&mut self, mut rows: Vec<RosterStudent>) -> Result<Vec<String>, AppError> {
        let scheme = self.id_scheme.scheme();
        let mut notes = Vec::new();
        for (k, r) in rows.iter_mut().enumerate() {
            if let Some(fixed) = restore_leading_zeros(&r.id, &scheme) {
                notes.push(format!(
                    "roster row {}: leading zeros restored ({} → {fixed}) under the {} \
                     student-ID scheme",
                    k + 1,
                    r.id,
                    scheme.name()
                ));
                r.id = fixed;
                r.numeric_cell = true;
            }
        }
        notes.extend(check_roster(&rows, &self.id_scheme)?);
        self.roster = Some(rows);
        Ok(notes)
    }

    /// True when the key is inferred from the sheets (not releasable until replaced).
    pub fn key_inferred(&self) -> bool {
        matches!(self.key, KeySource::Inferred)
    }

    /// The points of every version (in `forms` order): `points` (1 each when empty) unless
    /// `form_points` lists the version.
    pub fn points_by_form(&self) -> Vec<Vec<String>> {
        let n = usize::from(self.questions);
        let base = if self.points.is_empty() {
            vec!["1".to_string(); n]
        } else {
            self.points.clone()
        };
        self.forms
            .iter()
            .map(|f| {
                self.form_points
                    .get(f)
                    .cloned()
                    .unwrap_or_else(|| base.clone())
            })
            .collect()
    }

    /// Sets the points read with a key file ([`KeyFilePoints`]): one column for every version
    /// replaces `points`; columns per version set `form_points` (dropped again when every
    /// version reads the same).
    pub fn set_key_points(&mut self, p: &KeyFilePoints) {
        if let Some(all) = &p.all {
            self.points = all.clone();
        }
        self.form_points = p.per_form.clone();
        if !self.form_points.is_empty() {
            let by_form = self.points_by_form();
            if by_form.iter().all(|x| *x == by_form[0]) {
                self.points = by_form[0].clone();
                self.form_points.clear();
            }
        }
    }

    /// True when some item → CLO mapping row applies to one version only.
    pub fn has_form_mapping(&self) -> bool {
        self.outcomes
            .as_ref()
            .is_some_and(|o| o.mapping.iter().any(|m| !m.form.is_empty()))
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

/// Arabic option labels (abjad order, §13.3).
const ARABIC_OPTIONS: [&str; 10] = ["أ", "ب", "ج", "د", "هـ", "و", "ز", "ح", "ط", "ي"];

/// Parses one option label: Latin letters (either case), Arabic letters or 1-based digits
/// (ASCII or Arabic-Indic).
pub fn parse_option(s: &str, options: u8) -> Option<u8> {
    let t = clean_cell(s);
    let t = t.trim();
    let o = if let Some(i) = ARABIC_OPTIONS
        .iter()
        .position(|a| *a == t || (*a == "هـ" && t == "ه") || (*a == "أ" && t == "ا"))
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

/// Words that void a question (`*` too): dropped from the counted set.
const VOID_WORDS: &[&str] = &[
    "void",
    "voided",
    "drop",
    "dropped",
    "cancelled",
    "canceled",
    "ملغى",
    "ملغي",
    "ملغاة",
];

/// Words that leave an entry missing (SCO-003 until filled in).
const MISSING_WORDS: &[&str] = &["-", "–", "n/a", "na", "none", "?"];

/// Parses a key entry: `"B"`, `"b"`, `"A|C"` (either accepted; also `A/C`, `A;C`, `A,C`,
/// `A C`, `A or C`, `AC`), `"*"` or `"void"`, `""` (missing). Arabic letters and 1-based digits
/// (ASCII or Arabic-Indic) are accepted. Remark's AND syntax (`A+C`, `A&C`, `A and C`: all of
/// these answers required, spec §10.1) is refused with what to write instead: AND keys are not
/// scored yet, and reading them as "either" would credit half-answered items silently.
pub fn parse_key_entry(s: &str, options: u8) -> Result<ParsedKey, String> {
    let t = clean_cell(s);
    let t = t.trim();
    let lower = t.to_lowercase();
    if t.is_empty() || MISSING_WORDS.contains(&lower.as_str()) {
        return Ok(ParsedKey::Missing);
    }
    if t == "*" || VOID_WORDS.contains(&lower.as_str()) {
        return Ok(ParsedKey::Void);
    }
    if t.contains(['+', '&']) || format!(" {lower} ").contains(" and ") {
        return Err(format!(
            "`{}` is an AND key (Remark's partial-credit syntax: all of these answers \
             required, spec §10.1), which this version does not score yet; write `A|C` if \
             either answer earns the question, or `*` to void it",
            t.to_uppercase()
        ));
    }
    let spaced = format!(" {lower} ")
        .replace(" or ", "|")
        .replace(" أو ", "|")
        .replace(" او ", "|");
    let labels: Vec<String> = (0..options)
        .map(|o| char::from(b'A' + o).to_string())
        .collect();
    let mut v = Vec::new();
    for part in spaced
        .split(['|', '/', ';', ',', ' '])
        .filter(|p| !p.is_empty())
    {
        let found: Vec<u8> = match parse_option(part, options) {
            Some(o) => vec![o],
            // Letters run together ("AC") name several accepted options.
            None if part.len() >= 2 && part.bytes().all(|b| b.is_ascii_alphabetic()) => part
                .chars()
                .map(|c| parse_option(&c.to_string(), options))
                .collect::<Option<Vec<u8>>>()
                .ok_or_else(|| bad_option(&part.to_uppercase(), options, &labels))?,
            None => return Err(bad_option(&part.to_uppercase(), options, &labels)),
        };
        for o in found {
            if !v.contains(&o) {
                v.push(o);
            }
        }
    }
    if v.is_empty() {
        return Ok(ParsedKey::Missing);
    }
    v.sort_unstable();
    Ok(ParsedKey::Options(v))
}

fn bad_option(part: &str, options: u8, labels: &[String]) -> String {
    format!(
        "`{part}` is not one of the {options} options ({}); write one letter, several \
         accepted letters like A|C, or * to void the question",
        labels.join(", ")
    )
}

// ---------------------------------------------------------------------------------------------
// Files as instructors produce them (§16.3 import): encodings, delimiters, header aliases.

/// Format and bidi controls stripped from imported cells (§9.2 rule 3, §9.3 step 7).
fn is_format_control(c: char) -> bool {
    matches!(c,
        '\u{00AD}' | '\u{061C}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}'
        | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
}

/// Arabic-Indic (U+0660–0669) and Extended Arabic-Indic (U+06F0–06F9) digits → ASCII.
fn fold_digit(c: char) -> char {
    match c {
        '\u{0660}'..='\u{0669}' => char::from(b'0' + (c as u32 - 0x0660) as u8),
        '\u{06F0}'..='\u{06F9}' => char::from(b'0' + (c as u32 - 0x06F0) as u8),
        '\u{00A0}' => ' ',
        _ => c,
    }
}

/// One imported cell: format and bidi controls stripped, Arabic-Indic digits folded to ASCII,
/// trimmed. Display text (names) keeps its letters unchanged.
pub fn clean_cell(s: &str) -> String {
    s.chars()
        .filter(|c| !is_format_control(*c))
        .map(fold_digit)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Decodes an imported text file: UTF-8 with or without a BOM, UTF-16 with a BOM (Excel
/// "Unicode text"), otherwise Windows-1256, the Arabic ANSI code page of Saudi Windows
/// installations (§9.2, §16.3).
pub fn decode_text(bytes: &[u8]) -> String {
    if let Some((enc, bom)) = encoding_rs::Encoding::for_bom(bytes) {
        return enc
            .decode_without_bom_handling(&bytes[bom..])
            .0
            .into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::WINDOWS_1256
            .decode_without_bom_handling(bytes)
            .0
            .into_owned(),
    }
}

/// The delimiter of a delimited text file from its first non-empty line, outside quotes: tab
/// when present (Excel "Unicode text"), else semicolon when it outnumbers commas (Excel in
/// locales with a decimal comma), else comma.
pub fn sniff_delimiter(text: &str) -> u8 {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let (mut tabs, mut semis, mut commas, mut quoted) = (0, 0, 0, false);
    for c in line.chars() {
        match c {
            '"' => quoted = !quoted,
            '\t' if !quoted => tabs += 1,
            ';' if !quoted => semis += 1,
            ',' if !quoted => commas += 1,
            _ => {}
        }
    }
    if tabs > 0 {
        b'\t'
    } else if semis > commas {
        b';'
    } else {
        b','
    }
}

/// Non-empty records of a delimited file with their 1-based line numbers; cells cleaned
/// ([`clean_cell`]) and trailing empty cells dropped (spreadsheets pad rows).
fn delimited_rows(bytes: &[u8], what: &str) -> Result<Vec<(u64, Vec<String>)>, AppError> {
    let text = decode_text(bytes);
    let mut r = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(sniff_delimiter(&text))
        .from_reader(text.as_bytes());
    let mut out = Vec::new();
    for rec in r.records() {
        let rec = rec.map_err(|e| {
            AppError::Input(format!(
                "{what}: {e}; save the file again as CSV (comma or semicolon separated, UTF-8) \
                 and check that every quote is closed"
            ))
        })?;
        let line = rec.position().map_or(0, |p| p.line());
        let mut cells: Vec<String> = rec.iter().map(clean_cell).collect();
        while cells.last().is_some_and(String::is_empty) {
            cells.pop();
        }
        if !cells.is_empty() {
            out.push((line, cells));
        }
    }
    Ok(out)
}

fn norm_header(h: &str) -> String {
    clean_cell(h)
        .to_lowercase()
        .replace(['_', '-', '.', ':'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A question number as instructors write it: `7`, `Q7`, `q 7`, `#7`, `7.`, `7)`, `س7`,
/// Arabic-Indic digits.
fn parse_question_number(s: &str) -> Option<u32> {
    let t = clean_cell(s);
    let t = t
        .trim_start_matches(['Q', 'q', '#', 'س'])
        .trim()
        .trim_end_matches(['.', ')', ':']);
    t.parse().ok()
}

/// A form (version) label: `A`, `a`, `Form A`, `Version B`, `Model C`, `Key A`, `نموذج أ`.
fn form_label(s: &str) -> String {
    let mut t = clean_cell(s).to_uppercase();
    for p in [
        "VERSION",
        "FORM",
        "MODEL",
        "VARIANT",
        "KEY",
        "BOOKLET",
        "النموذج",
        "نموذج",
    ] {
        if let Some(r) = t.strip_prefix(p) {
            t = r.to_string();
            break;
        }
    }
    let t: String = t
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '_' | ':' | '.' | '(' | ')'))
        .collect();
    match t.as_str() {
        "أ" | "ا" => "A".into(),
        "ب" => "B".into(),
        "ج" => "C".into(),
        "د" => "D".into(),
        "هـ" | "ه" => "E".into(),
        _ => t,
    }
}

const QUESTION_HEADERS: &[&str] = &[
    "question",
    "questions",
    "q",
    "item",
    "items",
    "no",
    "number",
    "#",
    "q no",
    "q#",
    "question no",
    "question number",
    "سؤال",
    "السؤال",
    "رقم السؤال",
];
const FORM_HEADERS: &[&str] = &[
    "form",
    "version",
    "model",
    "variant",
    "booklet",
    "نموذج",
    "النموذج",
];
const POINTS_HEADERS: &[&str] = &[
    "points",
    "point",
    "pts",
    "marks",
    "mark",
    "score",
    "weight",
    "درجة",
    "الدرجة",
    "الدرجات",
];
const SHARED_KEY_HEADERS: &[&str] = &[
    "key",
    "answer",
    "answers",
    "correct",
    "correct answer",
    "answer key",
    "الإجابة",
    "الاجابة",
    "الإجابة الصحيحة",
    "الحل",
    "مفتاح",
];

/// Highest option count an exam supports (key files are checked against the exam's own count
/// by [`ExamDefinition::validate`]).
pub const MAX_OPTIONS: u8 = 10;

fn key_error(line: u64, what: impl std::fmt::Display) -> AppError {
    AppError::Input(format!("key file line {line}: {what}"))
}

/// The accepted key-file layouts, for remediation messages.
pub const KEY_LAYOUTS: &str = "accepted layouts: one row per question (`question,A,B` with \
    a header, or `1,B` / `1,B,C` without one), one row per version (`form,Q1,Q2,…` or \
    `A,B,C,D,…`), or a single column of answers; entries are letters (any case), A|C for \
    several accepted answers, * to void";

/// Reads an answer-key file (CSV or tab/semicolon separated; UTF-8 with or without BOM,
/// UTF-16, Windows-1256; CRLF or LF). Layouts, with or without a header row:
/// - **one row per question**: `question,A,B[,points]` (a `key`/`answer` column is one key
///   for every form), or headerless `1,B` (every form) / `1,B,C` (one column per form, in the
///   exam's form order);
/// - **one row per version**: `form,Q1,Q2,…` / `version,1,2,…`, or headerless `A,B,C,D,…`
///   (the first cell names the form) or unlabeled rows of answers in the exam's form order;
/// - **a single column** of answers, one per question.
///
/// Question numbers may be written `7`, `Q7`, `7.` or in Arabic-Indic digits; questions
/// beyond the exam's count are ignored (the sheet may print more). Entries follow
/// [`parse_key_entry`]. Returns the per-form keys (in `forms` order) and the points column if
/// present. Every malformed line fails with its line number and what to change (R10).
pub fn read_key_csv(
    bytes: &[u8],
    forms: &[String],
    questions: u16,
) -> Result<(Vec<FormKey>, Option<Vec<String>>), AppError> {
    read_key_file(bytes, forms, questions, MAX_OPTIONS)
}

/// The points columns of a key file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyFilePoints {
    /// A `points` column: one value per question for every version.
    pub all: Option<Vec<String>>,
    /// `points_A`, `points_B` … columns (also `points A`, `A points`): one value per question
    /// of that version (form letter → values).
    pub per_form: BTreeMap<String, Vec<String>>,
}

/// A key file as read by [`read_key_file_full`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyFile {
    /// One key per form, in the exam's form order.
    pub forms: Vec<FormKey>,
    /// Its points columns.
    pub points: KeyFilePoints,
}

/// The version a per-version points header names (`points_A`, `points A`, `pts B`,
/// `A points`), from its normalised text ([`norm_header`]).
fn points_form(hn: &str) -> Option<String> {
    let parts: Vec<&str> = hn.split(' ').collect();
    if parts.len() < 2 {
        return None;
    }
    let f = if POINTS_HEADERS.contains(&parts[0]) {
        parts[1..].join(" ")
    } else if POINTS_HEADERS.contains(&parts[parts.len() - 1]) {
        parts[..parts.len() - 1].join(" ")
    } else {
        return None;
    };
    let f = form_label(&f);
    (!f.is_empty() && f.chars().count() <= 2).then_some(f)
}

/// [`read_key_csv`] with the exam's option count (entries are checked against it). A file
/// with points per version (`points_A`, `points_B`) is read here only when every version
/// carries the same points; [`read_key_file_full`] reads them per version.
pub fn read_key_file(
    bytes: &[u8],
    forms: &[String],
    questions: u16,
    options: u8,
) -> Result<(Vec<FormKey>, Option<Vec<String>>), AppError> {
    let k = read_key_file_full(bytes, forms, questions, options)?;
    if k.points.per_form.is_empty() {
        return Ok((k.forms, k.points.all));
    }
    let base = k
        .points
        .all
        .clone()
        .unwrap_or_else(|| vec!["1".to_string(); usize::from(questions)]);
    let by_form: Vec<&Vec<String>> = forms
        .iter()
        .map(|f| k.points.per_form.get(f).unwrap_or(&base))
        .collect();
    let points = match by_form.first() {
        Some(first) if by_form.iter().all(|p| p == first) => Some((*first).clone()),
        _ => {
            return Err(AppError::Input(format!(
                "key file: the versions carry different points ({}); grade with `omr grade \
                 --key` or replace the key with `omr key replace --key`, which read points per \
                 version",
                k.points
                    .per_form
                    .keys()
                    .map(|f| format!("points_{f}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    };
    Ok((k.forms, points))
}

/// Reads an answer-key file as [`read_key_csv`] does, with points per version: besides a
/// `points` column (every version), a `points_A`, `points_B` … column gives the points of the
/// questions of that version (a Remark import writes them when its versions shuffle questions
/// of different weights).
pub fn read_key_file_full(
    bytes: &[u8],
    forms: &[String],
    questions: u16,
    options: u8,
) -> Result<KeyFile, AppError> {
    let rows = delimited_rows(bytes, "key file")?;
    let Some((_, first)) = rows.first() else {
        return Err(AppError::Input(format!("key file: no rows; {KEY_LAYOUTS}")));
    };
    let n = usize::from(questions);
    let h0 = norm_header(&first[0]);
    let entry = |line: u64, col: &str, v: &str| -> Result<String, AppError> {
        parse_key_entry(v, options).map_err(|m| key_error(line, format!("{col}: {m}")))?;
        Ok(v.to_string())
    };
    let points_cell = |line: u64, q: usize, what: &str, v: &str| -> Result<String, AppError> {
        let v = v.replace(',', ".");
        omr_scoring::parse_points(&v).map_err(|e| {
            key_error(
                line,
                format!("{what}`{v}` of question {q}: {e}; write a number like 1 or 0.5"),
            )
        })?;
        Ok(v)
    };
    // One row per question.
    let long = |body: &[(u64, Vec<String>)],
                cols: &[(String, usize)],
                points_col: Option<usize>,
                form_points: &[(String, usize)],
                width: usize|
     -> Result<KeyFile, AppError> {
        let mut keys: Vec<(String, Vec<String>)> = cols
            .iter()
            .map(|(f, _)| (f.clone(), vec![String::new(); n]))
            .collect();
        let mut points = points_col.map(|_| vec!["1".to_string(); n]);
        let mut per_form: BTreeMap<String, Vec<String>> = form_points
            .iter()
            .map(|(f, _)| (f.clone(), vec!["1".to_string(); n]))
            .collect();
        let mut seen: BTreeMap<usize, (u64, Vec<String>)> = BTreeMap::new();
        for (line, row) in body {
            let q = parse_question_number(&row[0]).ok_or_else(|| {
                key_error(
                    *line,
                    format!(
                        "`{}` is not a question number (expected 1–{questions}); {KEY_LAYOUTS}",
                        row[0]
                    ),
                )
            })? as usize;
            if q == 0 {
                return Err(key_error(*line, "questions are numbered from 1"));
            }
            if row.len() > width {
                return Err(key_error(
                    *line,
                    format!(
                        "question {q} has {} cells, the header names {width} columns; remove \
                         the extra cells or name their columns",
                        row.len()
                    ),
                ));
            }
            if q > n {
                continue; // the sheet may print more questions than the exam uses
            }
            let cells: Vec<String> = cols
                .iter()
                .map(|(_, i)| row.get(*i).cloned().unwrap_or_default())
                .collect();
            if let Some((l0, prev)) = seen.get(&q) {
                if *prev != cells {
                    return Err(key_error(
                        *line,
                        format!(
                            "question {q} is listed again (first on line {l0}) with other \
                             answers; keep one row per question"
                        ),
                    ));
                }
                continue;
            }
            for ((f, _), v) in cols.iter().zip(&cells) {
                let col = if f == "*" {
                    format!("question {q}")
                } else {
                    format!("question {q} form {f}")
                };
                let v = entry(*line, &col, v)?;
                if let Some(k) = keys.iter_mut().find(|(kf, _)| kf == f) {
                    k.1[q - 1] = v;
                }
            }
            if let (Some(pc), Some(p)) = (points_col, points.as_mut())
                && let Some(v) = row.get(pc).filter(|v| !v.is_empty())
            {
                p[q - 1] = points_cell(*line, q, "points ", v)?;
            }
            for (f, c) in form_points {
                if let Some(v) = row.get(*c).filter(|v| !v.is_empty())
                    && let Some(p) = per_form.get_mut(f)
                {
                    p[q - 1] = points_cell(*line, q, &format!("points of version {f} "), v)?;
                }
            }
            seen.insert(q, (*line, cells));
        }
        let out = keys
            .into_iter()
            .map(|(form, answers)| FormKey { form, answers })
            .collect();
        if let Some(f) = per_form.keys().find(|f| !forms.contains(f)) {
            return Err(AppError::Input(format!(
                "key file: a points_{f} column for version {f}, which the exam does not declare \
                 (forms {}); rename the column or declare the version (--forms)",
                forms.join(", ")
            )));
        }
        Ok(KeyFile {
            forms: order_keys(out, forms)?,
            points: KeyFilePoints {
                all: points,
                per_form,
            },
        })
    };
    // One row per version: `labelled` rows start with the form letter.
    let wide = |body: &[(u64, Vec<String>)],
                header: Option<&[String]>,
                labelled: bool|
     -> Result<KeyFile, AppError> {
        let skip = usize::from(labelled);
        let qcols: Vec<Option<usize>> = match header {
            Some(h) => h[skip..]
                .iter()
                .map(|c| match parse_question_number(c) {
                    Some(q) if q >= 1 => Ok(Some(q as usize)),
                    _ if c.is_empty() => Ok(None),
                    _ => Err(AppError::Input(format!(
                        "key file header: column `{c}` is not a question number (Q1, Q2, …); \
                         {KEY_LAYOUTS}"
                    ))),
                })
                .collect::<Result<_, _>>()?,
            None => (1..=body.iter().map(|(_, r)| r.len()).max().unwrap_or(0))
                .map(Some)
                .collect(),
        };
        let mut out = Vec::new();
        for (k, (line, row)) in body.iter().enumerate() {
            let form = if labelled {
                form_label(&row[0])
            } else {
                forms.get(k).cloned().ok_or_else(|| {
                    key_error(
                        *line,
                        format!(
                            "{} unlabeled key rows for {} form(s); start each row with its \
                             form letter (A, B, …)",
                            body.len(),
                            forms.len()
                        ),
                    )
                })?
            };
            if out.iter().any(|k: &FormKey| k.form == form) {
                return Err(key_error(
                    *line,
                    format!("form {form} has a second key row; keep one row per form"),
                ));
            }
            let mut answers = vec![String::new(); n];
            for (j, v) in row[skip..].iter().enumerate() {
                if let Some(Some(q)) = qcols.get(j)
                    && *q <= n
                {
                    answers[q - 1] = entry(*line, &format!("form {form} question {q}"), v)?;
                }
            }
            out.push(FormKey { form, answers });
        }
        Ok(KeyFile {
            forms: order_keys(out, forms)?,
            points: KeyFilePoints::default(),
        })
    };
    if QUESTION_HEADERS.contains(&h0.as_str()) {
        let mut cols: Vec<(String, usize)> = Vec::new();
        let mut points_col = None;
        let mut form_points: Vec<(String, usize)> = Vec::new();
        for (i, h) in first.iter().enumerate().skip(1) {
            let hn = norm_header(h);
            if POINTS_HEADERS.contains(&hn.as_str()) {
                points_col = Some(i);
            } else if let Some(f) = points_form(&hn) {
                form_points.push((f, i));
            } else if SHARED_KEY_HEADERS.contains(&hn.as_str()) {
                cols.push(("*".into(), i)); // one key for every form
            } else if !hn.is_empty() {
                cols.push((form_label(h), i));
            }
        }
        if cols.is_empty() {
            return Err(AppError::Input(format!(
                "key file header: no answer column after `{}`; {KEY_LAYOUTS}",
                first[0]
            )));
        }
        return long(&rows[1..], &cols, points_col, &form_points, first.len());
    }
    if FORM_HEADERS.contains(&h0.as_str()) {
        return wide(&rows[1..], Some(&first[..]), true);
    }
    // No header row.
    if parse_question_number(&first[0]).is_some() && first.len() >= 2 {
        let k = rows.iter().map(|(_, r)| r.len()).max().unwrap_or(2) - 1;
        let cols: Vec<(String, usize)> = if k == 1 {
            vec![("*".into(), 1)]
        } else if k == forms.len() {
            forms.iter().cloned().zip(1..).collect()
        } else {
            return Err(key_error(
                rows[0].0,
                format!(
                    "{k} answer columns for {} form(s); add a header row `question,{}`",
                    forms.len(),
                    forms.join(",")
                ),
            ));
        };
        return long(&rows, &cols, None, &[], k + 1);
    }
    if rows.iter().all(|(_, r)| r.len() == 1) {
        // A single column: one answer per question, in order.
        let answers = rows
            .iter()
            .enumerate()
            .take(n)
            .map(|(q, (line, r))| entry(*line, &format!("question {}", q + 1), &r[0]))
            .collect::<Result<Vec<_>, _>>()?;
        let mut answers = answers;
        answers.resize(n, String::new());
        return Ok(KeyFile {
            forms: order_keys(
                vec![FormKey {
                    form: "*".into(),
                    answers,
                }],
                forms,
            )?,
            points: KeyFilePoints::default(),
        });
    }
    let labelled = rows.iter().all(|(_, r)| {
        let f = form_label(&r[0]);
        r.len() != n && forms.contains(&f)
    });
    wide(&rows, None, labelled)
}

/// Points given by the instructor for an import (`omr import remark --points`, §3.5): a CSV
/// with a `question` column and a `points` column (every version) and/or `points_A`,
/// `points_B` … columns (one version each); a key file with points (`question,A,B,points`)
/// works as it is, its answer columns ignored. An empty cell leaves that question's points to
/// be derived. Returns form letter (`""`: every version) → one optional value per question.
pub fn read_points_file(
    bytes: &[u8],
    questions: u16,
) -> Result<BTreeMap<String, Vec<Option<String>>>, AppError> {
    let rows = delimited_rows(bytes, "points file")?;
    let fix = "write a header `question,points` (every version) or `question,points_A,points_B` \
               (per version), one row per question";
    let Some((_, first)) = rows.first() else {
        return Err(AppError::Input(format!("points file: no rows; {fix}")));
    };
    if !QUESTION_HEADERS.contains(&norm_header(&first[0]).as_str()) {
        return Err(AppError::Input(format!(
            "points file: the first column is `{}`, not `question`; {fix}",
            first[0]
        )));
    }
    let mut cols: Vec<(String, usize)> = Vec::new();
    for (i, h) in first.iter().enumerate().skip(1) {
        let hn = norm_header(h);
        if POINTS_HEADERS.contains(&hn.as_str()) {
            cols.push((String::new(), i));
        } else if let Some(f) = points_form(&hn) {
            cols.push((f, i));
        }
    }
    if cols.is_empty() {
        return Err(AppError::Input(format!(
            "points file: no points column; {fix}"
        )));
    }
    let n = usize::from(questions);
    let mut out: BTreeMap<String, Vec<Option<String>>> = cols
        .iter()
        .map(|(f, _)| (f.clone(), vec![None; n]))
        .collect();
    for (line, row) in &rows[1..] {
        let q = parse_question_number(&row[0]).ok_or_else(|| {
            key_error(
                *line,
                format!("`{}` is not a question number; {fix}", row[0]),
            )
        })? as usize;
        if q == 0 || q > n {
            continue;
        }
        for (f, c) in &cols {
            if let Some(v) = row.get(*c).filter(|v| !v.is_empty()) {
                let v = v.replace(',', ".");
                omr_scoring::parse_points(&v).map_err(|e| {
                    key_error(
                        *line,
                        format!("points `{v}` of question {q}: {e}; write a number like 0.75"),
                    )
                })?;
                if let Some(p) = out.get_mut(f) {
                    p[q - 1] = Some(v);
                }
            }
        }
    }
    Ok(out)
}

/// The form letters a key file names (`question,A,B`, `form,…` rows, or labelled rows), for
/// a definition without `--forms`; `None` when the file names none.
pub fn key_file_forms(bytes: &[u8]) -> Option<Vec<String>> {
    let rows = delimited_rows(bytes, "key file").ok()?;
    let (_, first) = rows.first()?;
    let h0 = norm_header(&first[0]);
    let mut out: Vec<String> = Vec::new();
    if QUESTION_HEADERS.contains(&h0.as_str()) {
        for h in first.iter().skip(1) {
            let hn = norm_header(h);
            if !hn.is_empty()
                && !POINTS_HEADERS.contains(&hn.as_str())
                && points_form(&hn).is_none()
                && !SHARED_KEY_HEADERS.contains(&hn.as_str())
            {
                out.push(form_label(h));
            }
        }
    } else if FORM_HEADERS.contains(&h0.as_str()) {
        out = rows[1..].iter().map(|(_, r)| form_label(&r[0])).collect();
    }
    out.retain(|f| !f.is_empty());
    (!out.is_empty()).then_some(out)
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
                    "key file has no key for form {f} (found: {}); add a column or row for \
                     form {f}, or declare the exam's forms to match the file (--forms)",
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

/// Header aliases of the roster columns (English and Arabic SIS exports).
fn roster_column(h: &str) -> Option<&'static str> {
    let h = norm_header(h);
    match h.as_str() {
        "id"
        | "student id"
        | "studentid"
        | "student number"
        | "student no"
        | "student #"
        | "university id"
        | "university number"
        | "id number"
        | "sis id"
        | "الرقم الجامعي"
        | "رقم الطالب"
        | "الرقم الاكاديمي"
        | "الرقم الأكاديمي"
        | "رقم القيد" => Some("id"),
        "name"
        | "student name"
        | "full name"
        | "name en"
        | "english name"
        | "الاسم"
        | "اسم الطالب"
        | "الاسم الكامل" => Some("name"),
        "name ar" | "arabic name" | "الاسم بالعربي" | "الاسم العربي" | "الاسم باللغة العربية" => {
            Some("name_ar")
        }
        "section" | "sec" | "section number" | "crn" | "الشعبة" | "رقم الشعبة" => {
            Some("section")
        }
        _ => None,
    }
}

/// Remediation for an ID damaged by a spreadsheet.
const EXCEL_ID_FIX: &str = "in Excel, format the ID column as Text (or Number with 0 decimal \
    places) before typing or pasting the IDs, or export the roster from the SIS again, then \
    save it as CSV UTF-8 or XLSX";

/// Cleans one roster ID cell (§9.2 rule 1): Arabic-Indic digits folded, a `.0` float artefact
/// dropped and a scientific-notation value expanded **only when no digit was lost**
/// (`2.210345601E+09` → `2210345601`); `2.2E+09` lost digits and is refused. Returns the ID
/// and whether it came from a number (flagged as a converted numeric cell).
pub fn clean_roster_id(raw: &str, numeric_cell: bool) -> Result<(String, bool), String> {
    let t = clean_cell(raw);
    // `…E+n`: Excel's display of a large number in a General cell.
    if let Some((mant, exp)) = t.split_once(['e', 'E'])
        && let Ok(exp) = exp.trim_start_matches('+').parse::<i32>()
        && !mant.is_empty()
        && mant.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && mant.bytes().any(|b| b.is_ascii_digit())
    {
        let (int, frac) = mant.split_once('.').unwrap_or((mant, ""));
        let digits = format!("{int}{frac}");
        let zeros = exp - frac.len() as i32;
        if zeros != 0 || digits.trim_start_matches('0').len() > 15 {
            return Err(format!(
                "`{t}` is a number in scientific notation that lost digits; {EXCEL_ID_FIX}"
            ));
        }
        return Ok((digits.trim_start_matches('0').to_string(), true));
    }
    // `2210345601.0`: a float cell written as text.
    if let Some((int, frac)) = t.split_once('.')
        && !int.is_empty()
        && int.bytes().all(|b| b.is_ascii_digit())
        && !frac.is_empty()
        && frac.bytes().all(|b| b.is_ascii_digit())
    {
        if frac.bytes().all(|b| b == b'0') && int.trim_start_matches('0').len() <= 15 {
            return Ok((int.to_string(), true));
        }
        return Err(format!(
            "`{t}` is a decimal number, not a student ID; {EXCEL_ID_FIX}"
        ));
    }
    Ok((t, numeric_cell))
}

fn roster_from_rows(
    rows: Vec<(u64, Vec<(String, bool)>)>,
    what: &str,
) -> Result<Vec<RosterStudent>, AppError> {
    // The header is the first row (among the first ten: SIS exports put titles above it) that
    // names an ID column.
    let Some(h) = rows
        .iter()
        .take(10)
        .position(|(_, r)| r.iter().any(|(c, _)| roster_column(c) == Some("id")))
    else {
        return Err(AppError::Input(format!(
            "{what}: no student-ID column found; add a header row such as \
             `student_id,name,section` (Arabic headers like الرقم الجامعي and الاسم work too)"
        )));
    };
    let cols: Vec<Option<&'static str>> = rows[h].1.iter().map(|(c, _)| roster_column(c)).collect();
    let find = |name: &str| cols.iter().position(|c| *c == Some(name));
    let id = find("id").expect("the header names an ID column");
    let (name, name_ar, section) = (find("name"), find("name_ar"), find("section"));
    let mut out = Vec::new();
    let mut errors = Vec::new();
    for (line, row) in rows.into_iter().skip(h + 1) {
        let get = |i: Option<usize>| {
            i.and_then(|i| row.get(i))
                .map(|(v, _)| clean_cell(v))
                .unwrap_or_default()
        };
        let (raw, numeric) = row.get(id).cloned().unwrap_or_default();
        if raw.trim().is_empty() {
            if row.iter().any(|(c, _)| !c.trim().is_empty()) {
                errors.push(format!(
                    "line {line}: a row without a student ID; fill it in or delete the row"
                ));
            }
            continue;
        }
        match clean_roster_id(&raw, numeric) {
            Ok((idv, numeric)) => out.push(RosterStudent {
                id: idv,
                name: get(name),
                name_ar: get(name_ar),
                section: get(section),
                numeric_cell: numeric,
            }),
            Err(m) => errors.push(format!("line {line}: student ID {m}")),
        }
    }
    if !errors.is_empty() {
        return Err(AppError::Input(format!(
            "{what}: {} row(s) refused: {}",
            errors.len(),
            errors.join("; ")
        )));
    }
    if out.is_empty() {
        return Err(AppError::Input(format!(
            "{what}: the roster has a header but no students"
        )));
    }
    Ok(out)
}

/// Reads a roster from CSV bytes (comma, semicolon or tab separated; UTF-8 with or without a
/// BOM, UTF-16, Windows-1256): a header row with English or Arabic aliases (see
/// `roster_column`), possibly below title rows. IDs damaged by Excel are repaired when no
/// digit was lost and refused otherwise ([`clean_roster_id`]).
pub fn read_roster_csv(bytes: &[u8]) -> Result<Vec<RosterStudent>, AppError> {
    let rows = delimited_rows(bytes, "roster CSV")?
        .into_iter()
        .map(|(l, r)| (l, r.into_iter().map(|c| (c, false)).collect()))
        .collect();
    roster_from_rows(rows, "roster CSV")
}

/// Reads a roster from the first sheet of an XLSX workbook (calamine). Integer-valued numeric
/// ID cells are written without a decimal part and flagged (§9.2 rule 1).
pub fn read_roster_xlsx(bytes: &[u8]) -> Result<Vec<RosterStudent>, AppError> {
    use calamine::{Data, Reader};
    let fix = "save the roster again as an Excel workbook (.xlsx) or as CSV UTF-8";
    let mut wb = crate::xlsx::open(bytes)
        .map_err(|e| AppError::Input(format!("roster XLSX: {e}; {fix}")))?;
    let name = wb
        .sheet_names()
        .first()
        .cloned()
        .ok_or_else(|| AppError::Input(format!("roster XLSX: no sheet; {fix}")))?;
    let rows = crate::xlsx::sheet_rows(&mut wb, &name)
        .map_err(|e| AppError::Input(format!("roster XLSX: {e}; {fix}")))?
        .into_iter()
        .map(|(rn, r)| {
            let cells = r
                .iter()
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
                .collect();
            (u64::from(rn), cells)
        })
        .collect();
    roster_from_rows(rows, "roster XLSX")
}

/// The leading zeros a spreadsheet dropped from an all-digit ID, restored only when exactly
/// one padded length satisfies the student-ID scheme and the ID as given does not (§9.2 rule
/// 2: the scheme is the institution's fixed-length rule).
pub fn restore_leading_zeros(id: &str, scheme: &omr_identity::StudentIdScheme) -> Option<String> {
    let key = omr_identity::normalize_id(id);
    if key.is_empty() || !key.bytes().all(|b| b.is_ascii_digit()) || scheme.accepts(&key) {
        return None;
    }
    let fits: Vec<String> = (key.len() + 1..=20)
        .map(|n| format!("{key:0>n$}"))
        .filter(|p| scheme.accepts(p))
        .collect();
    (fits.len() == 1).then(|| fits[0].clone())
}

/// Validates roster rows as the snapshot will (§9.2 rule 4): blocking findings (duplicate,
/// empty or precision-damaged IDs, national IDs) refuse the roster with what to change;
/// the warnings (scheme mismatches, converted numeric cells, empty names, mixed lengths) are
/// returned for display.
pub fn check_roster(
    rows: &[RosterStudent],
    scheme: &IdSchemeChoice,
) -> Result<Vec<String>, AppError> {
    use omr_identity::{ImportIssue as I, RosterError, RosterRecord, RosterSnapshot};
    let records = rows
        .iter()
        .map(|r| RosterRecord {
            student_key: r.id.clone(),
            key_from_numeric_cell: r.numeric_cell,
            name: r.name.clone(),
            name_ar: (!r.name_ar.is_empty()).then(|| r.name_ar.clone()),
            section: (!r.section.is_empty()).then(|| r.section.clone()),
            ..RosterRecord::default()
        })
        .collect();
    let s = scheme.scheme();
    let row = |k: &usize| format!("roster row {}", k + 1);
    let text = |i: &I| match i {
        I::DuplicateKey { key, rows } => format!(
            "student ID {key} appears {} times ({}); remove the extra rows or correct the IDs",
            rows.len(),
            rows.iter().map(row).collect::<Vec<_>>().join(", ")
        ),
        I::EmptyKey { row: k } => {
            format!("{} has no student ID; fill it in or delete the row", row(k))
        }
        I::ScientificNotation { row: k }
        | I::FloatArtifact { row: k }
        | I::PrecisionLoss { row: k } => {
            format!(
                "{}: the student ID lost digits in a spreadsheet; {EXCEL_ID_FIX}",
                row(k)
            )
        }
        I::NationalIdAsKey { row: k, masked } => format!(
            "{}: {masked} is a national ID or Iqama, never a student ID; use the university \
             ID column",
            row(k)
        ),
        I::NationalIdColumn {
            national_ids,
            values,
        } => format!(
            "the ID column holds national IDs or Iqamas ({national_ids} of {values}); choose \
             the university student-ID column"
        ),
        I::NumericCellConverted { row: k } => {
            format!(
                "{}: the ID was a number in the spreadsheet; check it",
                row(k)
            )
        }
        I::LeadingZerosRestored { row: k } => format!("{}: leading zeros restored", row(k)),
        I::SchemeMismatch { row: k, reason } => format!(
            "{}: the ID does not fit the {} student-ID scheme ({reason})",
            row(k),
            s.name()
        ),
        I::EmptyName { row: k } => format!("{}: no name", row(k)),
        I::MixedLengths { lengths } => format!(
            "IDs of different lengths: {}",
            lengths
                .iter()
                .map(|(l, n)| format!("{n} × {l} characters"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    match RosterSnapshot::build(records, &s, &omr_identity::ImportOptions::default()) {
        Ok((_, report)) => Ok(report.issues.iter().map(text).collect()),
        Err(RosterError::Blocked(report)) => Err(AppError::Input(format!(
            "roster refused (§9.2): {}",
            report
                .issues
                .iter()
                .filter(|i| i.is_blocking())
                .map(text)
                .collect::<Vec<_>>()
                .join("; ")
        ))),
    }
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

/// Reads the CLO file (CSV): header
/// `clo,domain,title[,title_ar][,plo],questions[,weight][,note][,form][,status][,source]`, one
/// or more rows per CLO; `questions` is a list like `1-5,9`. `form` (or `version`) limits a
/// row to the questions printed on that version; `status` `imported, needs review` (or
/// `proposed`, `draft`) and `source` (e.g. `Remark import`) mark an imported row, which stays
/// proposed until the coordinator approves the mapping (§11.2, SCO-032).
pub fn read_clo_csv(bytes: &[u8], questions: u16) -> Result<OutcomeSetup, AppError> {
    let text = decode_text(bytes);
    let mut r = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .delimiter(sniff_delimiter(&text))
        .from_reader(text.as_bytes());
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
    let c_form = col(&["form", "version", "model"]);
    let c_status = col(&["status", "mapping status"]);
    let c_source = col(&["source", "mapping source"]);
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
        let form = {
            let f = get(c_form);
            if f.is_empty() { f } else { form_label(&f) }
        };
        let status = norm_header(&get(c_status));
        let source = get(c_source);
        let imported = match (status.as_str(), source.as_str()) {
            ("" | "approved", "") => String::new(),
            ("" | "approved", s) if s.eq_ignore_ascii_case("manual") => String::new(),
            ("" | "approved", s) => s.to_string(),
            (st, "")
                if st.contains("import")
                    || st.contains("review")
                    || st == "proposed"
                    || st == "draft" =>
            {
                "import".to_string()
            }
            (st, s)
                if st.contains("import")
                    || st.contains("review")
                    || st == "proposed"
                    || st == "draft" =>
            {
                s.to_string()
            }
            (st, _) => {
                return Err(AppError::Input(format!(
                    "CLO CSV {code}: status `{st}`: write `approved`, or `imported, needs \
                     review` for a row to approve later"
                )));
            }
        };
        for q in qs {
            setup.mapping.push(CloMapping {
                question: q,
                clo: code.clone(),
                weight: weight.clone(),
                note: get(c_note),
                form: form.clone(),
                imported: imported.clone(),
                approved_by: String::new(),
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
        // Regression (F6): Remark's AND syntax was read as "either answer" (A|C), crediting a
        // half-answered item; refused with what to write instead.
        for and in ["A+C", "a & c", "A and C", "A+B+D"] {
            let e = parse_key_entry(and, 5).unwrap_err();
            assert!(e.contains("AND key") && e.contains("A|C"), "{and}: {e}");
        }
        assert_eq!(
            parse_key_entry("A or C", 5),
            Ok(ParsedKey::Options(vec![0, 2]))
        );
        assert_eq!(parse_key_entry("AC", 5), Ok(ParsedKey::Options(vec![0, 2])));
        // Not a word that merely contains the letters.
        assert!(parse_key_entry("band", 5).is_err());
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
            "\u{feff}Student ID,Name,Section\n900000001,Test One,101\n,,\n900000002,Test Two,102\n"
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
