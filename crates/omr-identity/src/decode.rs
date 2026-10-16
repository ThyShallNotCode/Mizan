//! Per-booklet identity decoding (§9.5, §9.7, §9.9): roster-constrained fusion of the bubbled
//! ID grid (A), an optional typed/OCR-read ID string (B) and an optional page-code binding (E)
//! over a roster snapshot plus the "not on roster" hypothesis ∅, or rosterless decoding
//! (`identity_mode = candidate_number`).
//!
//! The resolver never snaps a read to the nearest roster entry: a read that is not a roster ID
//! (or within one writer error of one) is IDN-004 `not_on_roster`, and a one-edit near miss
//! can only proceed with a second, independent channel (§9.5 gap test).

use crate::channels::{
    AParts, GridModel, TypedModel, WriterErrorKind, logsumexp, lookup_neighbours, neighbour_count,
    writer_neighbours, writer_relation,
};
use crate::national::{IdentifierClass, classify_identifier, mask_digits};
use crate::normalize::{fold_digit, normalize_id};
use crate::roster::{RosterSnapshot, SisStatus};
use crate::scheme::StudentIdScheme;
use omr_model::{ColumnState, Exception, IdGridRead, Scope, Severity, StudentKey};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Quantises nats to 1/1024 nat (§9.6, §9.7), so decisions compare integers.
pub fn q(x: f64) -> i64 {
    if x.is_nan() {
        0
    } else {
        (x.clamp(-1.0e9, 1.0e9) * 1024.0).round() as i64
    }
}

/// Nats from quanta.
pub fn nats(qv: i64) -> f64 {
    qv as f64 / 1024.0
}

/// Where channel B's string came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TypedSource {
    /// Typed by an operator or a fillable-PDF return (low character error).
    Typed,
    /// Read by OCR/HTR from the handwritten ID line.
    Ocr,
}

/// Channel B input: an ID string. `?` (or `_`, `*`) marks an unreadable character; spaces and
/// hyphens are ignored; Arabic-Indic and full-width digits are folded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypedId {
    /// The text.
    pub text: String,
    /// Source.
    pub source: TypedSource,
}

/// Everything known about one booklet's identity.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct BookletEvidence {
    /// The bubbled ID grid as read by recognition (channel A).
    pub grid: Option<IdGridRead>,
    /// A typed or OCR-read ID string (channel B).
    pub typed: Option<TypedId>,
    /// The student bound to the booklet's serial on personalised sheets (channel E).
    pub bound_student: Option<StudentKey>,
}

/// Exam-instance inputs fixed before decoding that shape the prior π(s) (§9.5).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PriorContext {
    /// Sections sitting this session (`None`: every roster section).
    pub session_sections: Option<BTreeSet<String>>,
    /// Students marked absent in an imported attendance list.
    pub absent: BTreeSet<StudentKey>,
}

impl PriorContext {
    /// The absent keys in canonical form (§9.2 rule 3), so raw attendance keys still match.
    pub(crate) fn absent_keys(&self) -> BTreeSet<String> {
        self.absent.iter().map(|k| normalize_id(&k.0)).collect()
    }

    /// True when the student is expected in this session: every roster section when
    /// `session_sections` is unset, otherwise a student of a sitting section (or one without a
    /// section).
    pub(crate) fn expects(&self, section: Option<&String>) -> bool {
        match (&self.session_sections, section) {
            (Some(sections), Some(sec)) => sections.contains(sec),
            _ => true,
        }
    }
}

/// Decision parameters (§9.5–§9.7 initial values; all unverified until calibrated).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IdentityPolicy {
    /// β: probability a column is left blank.
    pub beta_blank: f64,
    /// μ: probability a column is double-marked.
    pub mu_double: f64,
    /// Log-odds assumed for marked/unmarked bubbles when recognition gives states only.
    pub default_logit: f64,
    /// ε_w: writer error (written ID ≠ the student's ID).
    pub eps_writer: f64,
    /// ε_x: execution error (bubbled digit ≠ written digit in one column).
    pub eps_exec: f64,
    /// Per-character error of typed strings.
    pub typed_char_error: f64,
    /// Per-character error of OCR-read strings.
    pub ocr_char_error: f64,
    /// ε_qr: sheet-swap rate for the page-code binding.
    pub eps_qr: f64,
    /// π(∅).
    pub pi_null: f64,
    /// L_id: auto-accept logit threshold (nats).
    pub l_id: f64,
    /// τ_m: margin threshold (nats).
    pub tau_m: f64,
    /// θ_f: channel support/contradiction threshold (nats).
    pub theta_f: f64,
    /// h: hysteresis band (nats).
    pub hysteresis: f64,
    /// G_∅: gap above which a read is not on the roster (nats; must exceed G_w + 2).
    pub g_null: f64,
    /// λ_new: extra cost of the private ∅ column (nats).
    pub lambda_new: f64,
    /// λ_dup: extra cost of a second copy of a student (nats).
    pub lambda_dup: f64,
    /// Allow AB alone to auto-accept (needs a check digit in the scheme; off by default).
    pub id_only_auto_accept: bool,
    /// Prior weight of students outside the session's sections.
    pub out_of_session_weight: f64,
    /// Prior weight of students marked absent, DN or W.
    pub absent_weight: f64,
    /// Candidates scored exactly per booklet (the rest are bounded by the tail term).
    pub block_k: usize,
}

impl Default for IdentityPolicy {
    fn default() -> Self {
        Self {
            beta_blank: 0.02,
            mu_double: 0.01,
            default_logit: 6.9,
            eps_writer: 0.01,
            eps_exec: 0.01,
            typed_char_error: 0.002,
            ocr_char_error: 0.02,
            eps_qr: 1e-3,
            pi_null: 0.01,
            l_id: 6.9,
            tau_m: 4.6,
            theta_f: 4.6,
            hysteresis: 0.5,
            g_null: 12.0,
            lambda_new: 2.0,
            lambda_dup: 5.0,
            id_only_auto_accept: false,
            out_of_session_weight: 0.1,
            absent_weight: 0.1,
            block_k: 20,
        }
    }
}

/// IDN-004 sub-reasons (Appendix A).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SubReason {
    /// A blank ID column.
    IdBlank,
    /// A double-marked or unreadable ID column.
    IdAmbiguous,
    /// The read is not a roster student.
    NotOnRoster,
    /// Posterior below L_id.
    LowPosterior,
    /// Margin below τ_m.
    LowMargin,
    /// Channels disagree (bubbles vs written ID).
    ChannelConflict,
    /// A one-edit writer error links the read to the roster ID.
    WriterError,
    /// A national ID/Iqama or phone number was written instead of the student ID.
    WrongIdentifier,
    /// Letters in the ID field (or digits in the name field).
    WrongField,
    /// Namesake group (channel C; not produced by the MVP channels).
    Namesake,
    /// Batch assignment differs from the booklet's own best candidate.
    AssignmentChanged,
    /// Assigned to a student marked absent, DN or W.
    AssignedToAbsent,
    /// The roster changed after the decision.
    RosterDiff,
    /// The student belongs to another section, campus or track.
    CrossSection,
}

impl SubReason {
    /// The Appendix A spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            SubReason::IdBlank => "id_blank",
            SubReason::IdAmbiguous => "id_ambiguous",
            SubReason::NotOnRoster => "not_on_roster",
            SubReason::LowPosterior => "low_posterior",
            SubReason::LowMargin => "low_margin",
            SubReason::ChannelConflict => "channel_conflict",
            SubReason::WriterError => "writer_error",
            SubReason::WrongIdentifier => "wrong_identifier",
            SubReason::WrongField => "wrong_field",
            SubReason::Namesake => "namesake",
            SubReason::AssignmentChanged => "assignment_changed",
            SubReason::AssignedToAbsent => "assigned_to_absent",
            SubReason::RosterDiff => "roster_diff",
            SubReason::CrossSection => "cross_section",
        }
    }

    /// Logged as INFO when the booklet is nevertheless auto-accepted (§9.7 "Severity").
    pub fn info_if_auto_accepted(self) -> bool {
        matches!(
            self,
            SubReason::WriterError
                | SubReason::ChannelConflict
                | SubReason::IdBlank
                | SubReason::IdAmbiguous
                | SubReason::Namesake
                | SubReason::WrongField
                | SubReason::CrossSection
        )
    }
}

/// One finding: a sub-reason with reviewer detail (national-ID digits already masked).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// Sub-reason.
    pub sub_reason: SubReason,
    /// Detail.
    pub detail: String,
}

/// Outcome of the identity decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdentityStatus {
    /// Every §9.7 condition holds, including an independent second channel (or the
    /// `id_only_auto_accept` policy with a check-digit scheme).
    AutoAccepted,
    /// Clean single-channel match: confirm on a contact sheet (shadow mode, §9.7); no exception.
    NeedsConfirmation,
    /// At least one ERROR exception: identity review.
    Review,
}

/// How a candidate's ID relates to what was bubbled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReadRelation {
    /// Every column is a confident single mark equal to the ID.
    Exact,
    /// Equal on all confident columns; the listed columns were blank, double or unreadable.
    WildcardMatch {
        /// Wildcard columns (0-based).
        columns: Vec<u8>,
    },
    /// One writer error away from the read.
    WriterError {
        /// Kind.
        kind: WriterErrorKind,
    },
    /// Further away (or no grid).
    Other,
}

/// A ranked candidate with its evidence (log values quantised to 1/1024 nat).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    /// Roster student.
    pub student: StudentKey,
    /// log P(s | O).
    pub log_posterior_q: i64,
    /// P(s | O).
    pub posterior: f64,
    /// log ℓ_AB(O | s).
    pub ll_ab_q: i64,
    /// Diagnostic log ℓ_A(O_A | s) (channel A alone).
    pub ll_a_q: Option<i64>,
    /// Diagnostic log ℓ_B(O_B | s) (channel B alone).
    pub ll_b_q: Option<i64>,
    /// log ℓ_E(O_E | s).
    pub ll_e_q: Option<i64>,
    /// Relation of the ID to the bubbled read.
    pub relation: ReadRelation,
    /// Section, campus and track for display.
    pub section: Option<String>,
}

/// Channel usage (§9.4 "the evaluated set is stored").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelUse {
    /// Not present on this booklet.
    Absent,
    /// Evaluated.
    Evaluated,
    /// Present but excluded (e.g. a national ID written instead of the student ID).
    Neutralized(String),
}

/// What was read, for display (national-ID-shaped digits masked).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ReadSummary {
    /// The grid: digits, `_` blank, `*` multi, `?` unreadable.
    pub grid: Option<String>,
    /// The typed/OCR string as normalised (`?` unreadable).
    pub typed: Option<String>,
    /// A national-ID-shaped value was masked.
    pub masked: bool,
}

/// Facts recorded with the decision that are not exceptions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IdentityNote {
    /// A read is national-ID shaped but the scheme can produce it (fusion note, §9.9 row 8).
    NationalIdShaped,
    /// Auto-accepted on the ID channel alone (`id_only_auto_accept`).
    IdOnly,
    /// The page-code-bound student is not on the roster snapshot.
    BoundStudentNotOnRoster,
    /// The writer-error hypothesis in words.
    WriterErrorHypothesis(String),
    /// The typed/OCR string agrees with the bubbles.
    ChannelsAgree,
}

/// The identity decision for one booklet.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IdentityDecision {
    /// Status.
    pub status: IdentityStatus,
    /// Best student (suggestion unless auto-accepted); `None` when ∅ is best or nothing is known.
    pub student: Option<StudentKey>,
    /// Ranked candidates (exactly scored set), best first.
    pub candidates: Vec<Candidate>,
    /// log P(∅ | O) (quantised); `None` in rosterless mode.
    pub null_log_posterior_q: Option<i64>,
    /// logit P(best | O) (quantised).
    pub logit_q: Option<i64>,
    /// Single-booklet margin Δ (quantised).
    pub margin_q: Option<i64>,
    /// Gap G_i of the ∅ test (quantised).
    pub gap_q: Option<i64>,
    /// What was read.
    pub read: ReadSummary,
    /// Channel A, B, E usage.
    pub channels: [ChannelUse; 3],
    /// Notes.
    pub notes: Vec<IdentityNote>,
    /// IDN-004 findings.
    pub findings: Vec<Finding>,
    /// IDN-006 hard conflict (page-code binding vs handwriting) detail.
    pub binding_conflict: Option<String>,
    /// Exceptions rendered from the findings under the final status.
    pub exceptions: Vec<Exception>,
    /// The roster snapshot hash the decision used.
    pub snapshot_sha384: Option<String>,
}

impl IdentityDecision {
    fn new(snapshot: Option<String>) -> Self {
        Self {
            status: IdentityStatus::Review,
            student: None,
            candidates: Vec::new(),
            null_log_posterior_q: None,
            logit_q: None,
            margin_q: None,
            gap_q: None,
            read: ReadSummary::default(),
            channels: [ChannelUse::Absent, ChannelUse::Absent, ChannelUse::Absent],
            notes: Vec::new(),
            findings: Vec::new(),
            binding_conflict: None,
            exceptions: Vec::new(),
            snapshot_sha384: snapshot,
        }
    }

    /// True when a finding with this sub-reason exists.
    pub fn has(&self, r: SubReason) -> bool {
        self.findings.iter().any(|f| f.sub_reason == r)
    }

    /// The exception severity a sub-reason gets under the current status.
    pub fn severity_of(&self, r: SubReason) -> Severity {
        if self.status == IdentityStatus::AutoAccepted && r.info_if_auto_accepted() {
            Severity::Info
        } else {
            Severity::Error
        }
    }

    /// Re-renders `exceptions` from the findings and the status.
    pub fn render_exceptions(&mut self) {
        let mut out: Vec<Exception> = self
            .findings
            .iter()
            .map(|f| {
                Exception::must("IDN-004", self.severity_of(f.sub_reason), Scope::Booklet)
                    .with_sub_reason(f.sub_reason.as_str())
                    .with_detail(f.detail.clone())
            })
            .collect();
        if let Some(d) = &self.binding_conflict {
            out.push(
                Exception::must("IDN-006", Severity::Error, Scope::Booklet).with_detail(d.clone()),
            );
        }
        self.exceptions = out;
    }

    fn add(&mut self, r: SubReason, detail: impl Into<String>) {
        if !self.has(r) {
            self.findings.push(Finding {
                sub_reason: r,
                detail: detail.into(),
            });
        }
    }
}

/// Parses channel B text into model characters; returns (chars, had letters).
fn parse_typed(text: &str) -> (Vec<Option<u8>>, bool) {
    let mut out = Vec::new();
    let mut letters = false;
    for c in normalize_id(text).chars().map(fold_digit) {
        match c {
            '0'..='9' => out.push(Some(c as u8 - b'0')),
            ' ' | '-' | '.' | '/' => {}
            '?' | '_' | '*' | '#' => out.push(None),
            c if c.is_alphabetic() => {
                letters = true;
                out.push(None);
            }
            _ => out.push(None),
        }
    }
    (out, letters)
}

/// True when either channel's complete read is a national ID/Iqama (or national-ID shaped) or a
/// phone number, whose digits are masked wherever shown (§9.2, §9.9 row 8).
fn reads_need_masking(ev: &BookletEvidence, scheme: &StudentIdScheme) -> bool {
    let masks = |s: &str| {
        let c = classify_identifier(s, scheme);
        c.must_mask() || c == IdentifierClass::Phone
    };
    let grid = ev
        .grid
        .as_ref()
        .and_then(IdGridRead::exact_digits)
        .is_some_and(|e| masks(&e));
    let typed = ev
        .typed
        .as_ref()
        .and_then(|t| {
            parse_typed(&t.text)
                .0
                .into_iter()
                .collect::<Option<Vec<u8>>>()
        })
        .is_some_and(|f| !f.is_empty() && masks(&digits_str(&f)));
    grid || typed
}

/// `x ≥ t` on 1/1024-nat quanta (§9.7: decision values are quantised before comparison).
fn ge_q(x: f64, t: f64) -> bool {
    q(x) >= q(t)
}

fn digits_str(d: &[u8]) -> String {
    d.iter().map(|&x| char::from(b'0' + x)).collect()
}

fn grid_display(g: &IdGridRead) -> String {
    let mut cols = g.columns.clone();
    cols.sort_by_key(|c| c.column);
    cols.iter()
        .map(|c| match (c.state, c.marked.as_slice()) {
            (ColumnState::Single, [d]) => char::from(b'0' + d),
            (ColumnState::Blank, _) => '_',
            (ColumnState::Multi, _) => '*',
            _ => '?',
        })
        .collect()
}

fn typed_display(chars: &[Option<u8>]) -> String {
    chars
        .iter()
        .map(|c| c.map_or('?', |d| char::from(b'0' + d)))
        .collect()
}

struct Fused {
    ab: f64,
    a: Option<f64>,
    b: Option<f64>,
}

struct Channels<'a> {
    grid: Option<&'a GridModel>,
    typed: Option<&'a TypedModel>,
    eps_w: f64,
    eps_x: f64,
}

impl Channels<'_> {
    fn a(&self, u: &[u8]) -> f64 {
        self.grid.map_or(0.0, |g| g.log_l(u))
    }

    fn b(&self, u: &[u8]) -> f64 {
        self.typed.map_or(0.0, |t| t.log_l(u))
    }

    /// Latent-intent fusion `ℓ_AB(O_A, O_B | id = x)` with the writer-error neighbourhood
    /// (§9.5). The execution-error term is applied to the direct intent only.
    fn fuse(&self, x: &[u8]) -> Fused {
        let nb = writer_neighbours(x);
        let k = nb.len().max(1) as f64;
        let (ax, bx) = (self.a(x), self.b(x));
        let jx = match (self.grid, self.typed) {
            (Some(g), Some(t)) => logsumexp([
                (1.0 - self.eps_x).ln() + ax + bx,
                (self.eps_x / 2.0).ln() + ax + t.log_tilde(x),
                (self.eps_x / 2.0).ln() + g.log_tilde(x) + bx,
            ]),
            _ => ax + bx,
        };
        let nb_a: Vec<f64> = nb.iter().map(|u| self.a(u)).collect();
        let nb_b: Vec<f64> = nb.iter().map(|u| self.b(u)).collect();
        let w_direct = (1.0 - self.eps_w).ln();
        let w_nb = (self.eps_w / k).ln();
        let ab = logsumexp([
            w_direct + jx,
            w_nb + logsumexp(nb_a.iter().zip(&nb_b).map(|(a, b)| a + b)),
        ]);
        let a = self
            .grid
            .map(|_| logsumexp([w_direct + ax, w_nb + logsumexp(nb_a.iter().copied())]));
        let b = self
            .typed
            .map(|_| logsumexp([w_direct + bx, w_nb + logsumexp(nb_b.iter().copied())]));
        Fused { ab, a, b }
    }
}

/// Decodes one booklet against a roster snapshot (`identity_mode = roster`).
pub fn decode_booklet(
    ev: &BookletEvidence,
    roster: &RosterSnapshot,
    scheme: &StudentIdScheme,
    prior: &PriorContext,
    policy: &IdentityPolicy,
) -> IdentityDecision {
    let mut d = IdentityDecision::new(Some(roster.sha384().to_owned()));
    let h = policy.hysteresis;
    // A national ID/Iqama or phone number recognised in either channel masks the reads of both
    // (the other channel may show most of the same digits, §9.2, §9.10).
    let mask_reads = reads_need_masking(ev, scheme);

    // ---- Channel A: the bubbled grid.
    let mut grid = ev
        .grid
        .as_ref()
        .filter(|g| !g.columns.is_empty())
        .map(|g| GridModel::new(g, policy.beta_blank, policy.mu_double, policy.default_logit));
    if let (Some(g), Some(raw)) = (&grid, &ev.grid) {
        d.channels[0] = ChannelUse::Evaluated;
        let mut shown = grid_display(raw);
        if mask_reads {
            shown = mask_digits(&shown);
            d.read.masked = true;
        }
        let blanks: Vec<usize> = (0..g.n())
            .filter(|&c| g.states[c] == ColumnState::Blank)
            .collect();
        let ambiguous: Vec<usize> = (0..g.n())
            .filter(|&c| matches!(g.states[c], ColumnState::Multi | ColumnState::Unreadable))
            .collect();
        if g.uninformative() {
            d.channels[0] = ChannelUse::Neutralized("all columns unreadable".into());
        }
        if let Some(exact) = raw.exact_digits() {
            match classify_identifier(&exact, scheme) {
                c @ (IdentifierClass::NationalId(_) | IdentifierClass::Phone) => {
                    let what = if c == IdentifierClass::Phone {
                        "a phone number"
                    } else {
                        "a national ID/Iqama"
                    };
                    shown = mask_digits(&shown);
                    d.read.masked = true;
                    d.add(
                        SubReason::WrongIdentifier,
                        format!("ID grid holds {what} ({shown}), not a student ID"),
                    );
                    d.channels[0] = ChannelUse::Neutralized(format!("{what} bubbled"));
                }
                IdentifierClass::NationalIdShaped(_) => {
                    shown = mask_digits(&shown);
                    d.read.masked = true;
                    d.notes.push(IdentityNote::NationalIdShaped);
                }
                _ => {}
            }
        }
        if !blanks.is_empty() {
            d.add(
                SubReason::IdBlank,
                format!("blank ID column(s) {blanks:?} in {shown}"),
            );
        }
        if !ambiguous.is_empty() {
            d.add(
                SubReason::IdAmbiguous,
                format!("double-marked or unreadable ID column(s) {ambiguous:?} in {shown}"),
            );
        }
        d.read.grid = Some(shown);
        if !matches!(d.channels[0], ChannelUse::Evaluated) {
            grid = None;
        }
    }

    // ---- Channel B: typed or OCR string.
    let mut typed = None;
    if let Some(t) = &ev.typed {
        let (chars, letters) = parse_typed(&t.text);
        let err = match t.source {
            TypedSource::Typed => policy.typed_char_error,
            TypedSource::Ocr => policy.ocr_char_error,
        };
        let model = TypedModel::new(chars, err);
        let mut shown = typed_display(&model.chars);
        if mask_reads {
            shown = mask_digits(&shown);
            d.read.masked = true;
        }
        if letters {
            d.add(
                SubReason::WrongField,
                format!("letters in the written ID ({shown})"),
            );
        }
        if model.uninformative() {
            d.channels[1] = ChannelUse::Neutralized("unreadable".into());
        } else {
            d.channels[1] = ChannelUse::Evaluated;
            if let Some(full) = model.complete() {
                match classify_identifier(&digits_str(&full), scheme) {
                    c @ (IdentifierClass::NationalId(_) | IdentifierClass::Phone) => {
                        shown = mask_digits(&shown);
                        d.read.masked = true;
                        let what = if c == IdentifierClass::Phone {
                            "a phone number"
                        } else {
                            "a national ID/Iqama"
                        };
                        d.add(
                            SubReason::WrongIdentifier,
                            format!("written ID is {what} ({shown}), not a student ID"),
                        );
                        d.channels[1] = ChannelUse::Neutralized(format!("{what} written"));
                    }
                    IdentifierClass::NationalIdShaped(_) => {
                        shown = mask_digits(&shown);
                        d.read.masked = true;
                        if !d.notes.contains(&IdentityNote::NationalIdShaped) {
                            d.notes.push(IdentityNote::NationalIdShaped);
                        }
                    }
                    _ => {}
                }
            }
            if matches!(d.channels[1], ChannelUse::Evaluated) {
                typed = Some(model);
            }
        }
        d.read.typed = Some(shown);
    }

    // ---- Channel E: page-code binding.
    let rows = roster.rows();
    let n_roster = rows.len();
    let bound = ev.bound_student.as_ref().and_then(|k| {
        let p = roster.position(&normalize_id(&k.0));
        if p.is_none() {
            d.notes.push(IdentityNote::BoundStudentNotOnRoster);
        }
        p
    });
    if bound.is_some() {
        d.channels[2] = ChannelUse::Evaluated;
    }
    let other_e = (policy.eps_qr / (n_roster.max(2) - 1) as f64).ln();
    let le = |i: Option<usize>| -> f64 {
        match bound {
            None => 0.0,
            Some(b) if Some(b) == i => (1.0 - policy.eps_qr).ln(),
            Some(_) => other_e,
        }
    };

    if (grid.is_none() && typed.is_none() && bound.is_none()) || n_roster == 0 {
        if d.findings.is_empty() {
            d.add(
                SubReason::IdBlank,
                "no usable identity evidence on the booklet",
            );
        }
        d.status = IdentityStatus::Review;
        d.render_exceptions();
        return d;
    }

    // ---- Priors.
    let absent = prior.absent_keys();
    let weights: Vec<f64> = rows
        .iter()
        .map(|r| {
            let mut w = 1.0;
            if let (Some(sections), Some(sec)) = (&prior.session_sections, &r.record.section)
                && !sections.contains(sec)
            {
                w *= policy.out_of_session_weight;
            }
            if absent.contains(&r.key.0)
                || matches!(
                    r.record.sis_status,
                    Some(SisStatus::DeniedEntry | SisStatus::Withdrawn)
                )
            {
                w *= policy.absent_weight;
            }
            w
        })
        .collect();
    let wsum: f64 = weights.iter().sum();
    let log_pi: Vec<f64> = weights
        .iter()
        .map(|w| ((1.0 - policy.pi_null) * w / wsum).ln())
        .collect();

    let ch = Channels {
        grid: grid.as_ref(),
        typed: typed.as_ref(),
        eps_w: policy.eps_writer,
        eps_x: policy.eps_exec,
    };

    // ---- Channel A, exact for every roster row (implicit neighbourhood sums); channel B direct.
    let digit_rows: Vec<Option<&Vec<u8>>> = rows.iter().map(|r| r.digits.as_ref()).collect();
    let no_digit_channels = grid.is_none() && typed.is_none();
    let a_parts: Vec<Option<AParts>> = digit_rows
        .iter()
        .map(|dg| match (dg, &grid) {
            (Some(x), Some(g)) => Some(g.parts(x, typed.is_some())),
            _ => None,
        })
        .collect();
    let b_direct: Vec<f64> = digit_rows
        .iter()
        .map(|dg| match (dg, &typed) {
            (Some(x), Some(t)) => t.log_l(x),
            _ => 0.0,
        })
        .collect();
    let w_direct = (1.0 - policy.eps_writer).ln();
    let a_latent: Vec<f64> = (0..n_roster)
        .map(|i| match (digit_rows[i], &a_parts[i]) {
            (Some(_), Some(p)) => logsumexp([
                w_direct + p.direct,
                (policy.eps_writer / p.n_nb.max(1) as f64).ln() + p.nb_lse,
            ]),
            (Some(_), None) => 0.0,
            (None, _) if no_digit_channels => 0.0,
            (None, _) => f64::NEG_INFINITY,
        })
        .collect();
    let rank_score: Vec<f64> = (0..n_roster)
        .map(|i| log_pi[i] + a_latent[i] + b_direct[i] + le(Some(i)))
        .collect();

    // ---- ŷ: the unconstrained top read.
    let b_full = typed.as_ref().and_then(TypedModel::complete);
    let y_hat: Option<Vec<u8>> = match (&grid, &typed) {
        (Some(g), Some(t)) if t.chars.len() == g.n() => {
            let comb: Vec<u8> = (0..g.n())
                .map(|c| {
                    (0..10u8)
                        .max_by(|&x, &y| {
                            let sx = g.col(c, x) + t.log_l_pos(x, c);
                            let sy = g.col(c, y) + t.log_l_pos(y, c);
                            sx.partial_cmp(&sy)
                                .unwrap_or(std::cmp::Ordering::Equal)
                                .then(y.cmp(&x))
                        })
                        .unwrap_or(0)
                })
                .collect();
            match b_full.clone() {
                Some(b) if ch.a(&b) + ch.b(&b) > ch.a(&comb) + ch.b(&comb) => Some(b),
                _ => Some(comb),
            }
        }
        (Some(g), _) => Some(g.top()),
        (None, Some(_)) => b_full.clone(),
        _ => None,
    };

    // ---- The exactly scored set S. Without channel B every row is exact (A is exact for all
    // rows). With B: the best rows plus every roster ID within one edit of ŷ or of the written
    // string; the rest is bounded below.
    let mut fused: BTreeMap<usize, Fused> = BTreeMap::new();
    let mut log_tail = f64::NEG_INFINITY;
    if let Some(t) = typed.as_ref() {
        let mut order: Vec<usize> = (0..n_roster)
            .filter(|&i| rank_score[i].is_finite())
            .collect();
        // Ranked on quantised scores (ties → roster order), so the scored set reproduces.
        order.sort_by(|&a, &b| q(rank_score[b]).cmp(&q(rank_score[a])).then(a.cmp(&b)));
        let mut s_set: BTreeSet<usize> = order.iter().take(policy.block_k).copied().collect();
        for probe in [y_hat.as_ref(), b_full.as_ref()].into_iter().flatten() {
            if let Some(i) = roster.position(&digits_str(probe)) {
                s_set.insert(i);
            }
            for u in lookup_neighbours(probe) {
                if let Some(i) = roster.position(&digits_str(&u)) {
                    s_set.insert(i);
                }
            }
        }
        if let Some(b) = bound {
            s_set.insert(b);
        }
        for &i in &s_set {
            if let Some(x) = digit_rows[i] {
                fused.insert(i, ch.fuse(x));
            }
        }
        // Tail bound: exact channel-A sums, channel B bounded by its maximum (and, for strings
        // other than the written one, by the cheapest single edit). Rigorous under the model.
        let fill: Vec<u8> = t.chars.iter().map(|c| c.unwrap_or(0)).collect();
        let b_max = t.log_l(&fill) + 0.01f64.ln_1p();
        let b_nb = if b_full.is_some() {
            (b_max + t.log_rho1() + 2f64.ln()).min(b_max)
        } else {
            b_max
        };
        let terms: Vec<f64> = (0..n_roster)
            .filter(|i| !s_set.contains(i))
            .filter_map(|i| {
                let x = digit_rows[i]?;
                let (ax, at, nb_lse, n_nb) = match &a_parts[i] {
                    Some(p) => (p.direct, p.tilde, p.nb_lse, p.n_nb),
                    None => {
                        let n = neighbour_count(x);
                        (0.0, 0.0, (n.max(1) as f64).ln(), n)
                    }
                };
                let bx = b_direct[i];
                let direct_bound = logsumexp([
                    (1.0 - policy.eps_exec).ln() + ax + bx,
                    (policy.eps_exec / 2.0).ln() + ax + b_max,
                    (policy.eps_exec / 2.0).ln() + at + bx,
                ]);
                let nb_bound = (policy.eps_writer / n_nb.max(1) as f64).ln() + nb_lse + b_nb;
                Some(log_pi[i] + le(Some(i)) + logsumexp([w_direct + direct_bound, nb_bound]))
            })
            .collect();
        log_tail = logsumexp(terms);
    } else {
        for (i, &al) in a_latent.iter().enumerate() {
            if al.is_finite() {
                fused.insert(
                    i,
                    Fused {
                        ab: al,
                        a: grid.as_ref().map(|_| al),
                        b: None,
                    },
                );
            }
        }
    }
    let w_of = |i: usize, f: &Fused| log_pi[i] + f.ab + le(Some(i));
    let null_f = y_hat.as_ref().map(|y| ch.fuse(y));
    let p_fmt = y_hat.as_ref().map_or(0.0, |y| {
        scheme.log_p_fmt(&digits_str(y), roster.year_counts())
    });
    let w_null = policy.pi_null.ln() + p_fmt + null_f.as_ref().map_or(0.0, |f| f.ab) + le(None);
    let best_ab = fused
        .values()
        .map(|f| f.ab)
        .fold(f64::NEG_INFINITY, f64::max);
    let gap = null_f.as_ref().map(|nf| nf.ab - best_ab);
    d.gap_q = gap.map(q);

    // ---- Ranking and posteriors.
    let mut scored: Vec<(Option<usize>, f64)> =
        fused.iter().map(|(&i, f)| (Some(i), w_of(i, f))).collect();
    scored.push((None, w_null));
    // Ranked on 1/1024-nat quanta (§9.6, §9.7) with a fixed tie-break (roster order, ∅ last),
    // so near-ties resolve identically on every platform.
    scored.sort_by(|a, b| {
        q(b.1).cmp(&q(a.1)).then_with(|| match (a.0, b.0) {
            (Some(x), Some(y)) => x.cmp(&y),
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, None) => std::cmp::Ordering::Equal,
        })
    });
    let log_z = logsumexp(scored.iter().map(|s| s.1).chain([log_tail]));
    d.null_log_posterior_q = Some(q(w_null - log_z));

    let grid_read = ev.grid.as_ref();
    let y_read: Option<Vec<u8>> = y_hat.clone();
    for (i, w) in scored
        .iter()
        .filter_map(|(i, w)| i.map(|i| (i, *w)))
        .take(policy.block_k.max(5))
    {
        let f = &fused[&i];
        let row = &rows[i];
        let relation = match (grid_read, &row.digits) {
            (Some(g), Some(x)) if grid.is_some() => relation_to_grid(g, x, y_read.as_deref()),
            _ => ReadRelation::Other,
        };
        let section = [
            row.record.section.as_deref(),
            row.record.campus.as_deref(),
            row.record.track.map(|t| match t {
                crate::roster::Track::Male => "male",
                crate::roster::Track::Female => "female",
                crate::roster::Track::Mixed => "mixed",
            }),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        d.candidates.push(Candidate {
            student: row.key.clone(),
            log_posterior_q: q(w - log_z),
            posterior: (w - log_z).exp(),
            ll_ab_q: q(f.ab),
            ll_a_q: f.a.map(q),
            ll_b_q: f.b.map(q),
            ll_e_q: bound.map(|_| q(le(Some(i)))),
            relation,
            section: (!section.is_empty()).then_some(section),
        });
    }

    let best = scored[0];
    let Some(best_i) = best.0 else {
        let shown = d
            .read
            .grid
            .clone()
            .or(d.read.typed.clone())
            .unwrap_or_default();
        d.add(
            SubReason::NotOnRoster,
            format!("best hypothesis is a student not on the roster (read {shown})"),
        );
        d.status = IdentityStatus::Review;
        d.render_exceptions();
        return d;
    };
    d.student = Some(rows[best_i].key.clone());
    let others = logsumexp(scored[1..].iter().map(|s| s.1).chain([log_tail]));
    let logit = best.1 - others;
    // Students outside the exactly scored set (typed channel only) are bounded by the tail,
    // which also bounds the best of them: the margin uses it so it is never overstated.
    let runner_up = scored[1..].iter().map(|s| s.1).fold(log_tail, f64::max);
    let margin = best.1 - runner_up;
    d.logit_q = Some(q(logit));
    d.margin_q = Some(q(margin));

    // Per-channel LLRs of the best candidate (likelihoods only). For AB the ∅ alternative is
    // the posterior's ℓ(O | ∅) = P_fmt(ŷ)·ℓ_AB(ŷ); for the A-versus-B diagnostic each channel's
    // ∅ alternative is its own best string ("ℓ_A and ℓ_B favour different strings", §9.9 row 5).
    let best_id_digits = rows[best_i].digits.clone().unwrap_or_default();
    let llr = |pick: &dyn Fn(&Fused) -> Option<f64>, null_alt: Option<f64>| -> Option<f64> {
        let own = pick(&fused[&best_i])?;
        let mut alt: Vec<f64> = fused
            .iter()
            .filter(|(i, _)| **i != best_i)
            .filter_map(|(_, f)| pick(f))
            .collect();
        alt.extend(null_alt);
        let m = alt.into_iter().fold(f64::NEG_INFINITY, f64::max);
        Some(own - m)
    };
    let top_a = grid.as_ref().map(GridModel::top);
    let top_b: Option<Vec<u8>> = typed
        .as_ref()
        .map(|t| t.chars.iter().map(|c| c.unwrap_or(0)).collect());
    let alt_a = top_a
        .as_ref()
        .filter(|t| **t != best_id_digits)
        .and_then(|t| ch.fuse(t).a);
    let alt_b = top_b
        .as_ref()
        .filter(|t| typed.as_ref().is_some_and(|m| m.complete().is_some()) && **t != best_id_digits)
        .and_then(|t| ch.fuse(t).b);
    let llr_ab = llr(
        &|f: &Fused| Some(f.ab),
        null_f.as_ref().map(|nf| nf.ab + p_fmt),
    )
    .unwrap_or(f64::INFINITY);
    let llr_a = llr(&|f: &Fused| f.a, alt_a);
    let llr_b = llr(&|f: &Fused| f.b, alt_b);
    let supports = |x: f64| ge_q(x, policy.theta_f + h);
    let contradicts = |x: f64| !ge_q(x, -policy.theta_f + h);
    let e_supports = bound == Some(best_i);

    // ---- ∅ gap test (likelihoods only).
    let best_id = rows[best_i].digits.clone().unwrap_or_default();
    let shown_read = d
        .read
        .grid
        .clone()
        .or(d.read.typed.clone())
        .unwrap_or_default();
    let masked = d.read.masked;
    let show_id = |s: &str| if masked { mask_digits(s) } else { s.to_owned() };
    let mut not_on_roster = false;
    if let (Some(y), Some(gap)) = (&y_hat, gap) {
        let g_w = ((writer_neighbours(y).len().max(1) as f64) / policy.eps_writer).ln();
        let g_null = policy.g_null.max(g_w + 2.0 + h);
        let relation = writer_relation(&best_id, y)
            .map(|k| format!("{k:?}").to_lowercase())
            .unwrap_or_else(|| "near miss".into());
        let hypothesis = format!(
            "read {} → roster {}, {relation}",
            show_id(&digits_str(y)),
            show_id(&digits_str(&best_id))
        );
        if ge_q(gap, g_null + h) {
            not_on_roster = true;
            d.student = None;
            d.add(
                SubReason::NotOnRoster,
                format!("read {shown_read} is not a roster ID (gap {gap:.1} nat ≥ G_∅); never snapped to the nearest entry"),
            );
        } else if ge_q(gap, g_w - 2.0 - h) {
            d.notes
                .push(IdentityNote::WriterErrorHypothesis(hypothesis.clone()));
            // Proceeds only with a supporting second channel, and never within h of G_∅
            // (§9.7 hysteresis: a value in [G_∅ − h, G_∅ + h) is not auto-decided).
            if e_supports && !ge_q(gap, g_null - h) {
                d.add(SubReason::WriterError, hypothesis);
            } else {
                not_on_roster = true;
                d.student = None;
                let why = if e_supports {
                    "its gap is within h of G_∅"
                } else {
                    "no second channel supports it"
                };
                d.add(
                    SubReason::NotOnRoster,
                    format!("read {shown_read} is one writer error from a roster ID and {why} (gap {gap:.1} nat)"),
                );
                d.add(SubReason::WriterError, hypothesis);
            }
        }
    }
    if !not_on_roster {
        if !ge_q(logit, policy.l_id + h) {
            d.add(
                SubReason::LowPosterior,
                format!("logit P = {logit:.2} nat < L_id {:.1} (+h)", policy.l_id),
            );
        }
        if !ge_q(margin, policy.tau_m + h) {
            d.add(
                SubReason::LowMargin,
                format!("margin {margin:.2} nat < τ_m {:.1} (+h)", policy.tau_m),
            );
        }
    }
    // A versus B.
    let mut conflict_settled = false;
    if let (Some(a), Some(b)) = (llr_a, llr_b) {
        if contradicts(a) || contradicts(b) {
            let typed_shown = d.read.typed.clone().unwrap_or_default();
            d.add(
                SubReason::ChannelConflict,
                format!(
                    "bubbled {shown_read} and written {typed_shown} disagree (LLR_A {a:.1}, LLR_B {b:.1} nat)"
                ),
            );
            conflict_settled = e_supports;
        } else if supports(a) && supports(b) {
            d.notes.push(IdentityNote::ChannelsAgree);
        }
    }
    // E versus AB: hard conflict.
    if let Some(b) = bound
        && b != best_i
    {
        d.binding_conflict = Some(format!(
            "page code is bound to {} but the handwriting reads {}",
            rows[b].key, rows[best_i].key
        ));
    }
    // Cross-section.
    if let (Some(sections), Some(sec)) = (&prior.session_sections, &rows[best_i].record.section)
        && !sections.contains(sec)
    {
        d.add(
            SubReason::CrossSection,
            format!(
                "{} belongs to section {sec}, not sitting this session",
                rows[best_i].key
            ),
        );
    }

    // ---- Status (§9.7 conditions 1–4).
    let preventing = d.findings.iter().any(|f| {
        !f.sub_reason.info_if_auto_accepted()
            || (f.sub_reason == SubReason::ChannelConflict && !conflict_settled)
    }) || d.binding_conflict.is_some();
    let ab_supports = supports(llr_ab);
    // ID-only acceptance needs a check-digit scheme *and* an ID that passes it (§9.7); roster
    // rows failing the scheme are only an import warning (§9.2 rule 4).
    let id_only = policy.id_only_auto_accept
        && scheme.has_check_digit()
        && scheme.accepts(&rows[best_i].key.0);
    d.status = if !preventing && ab_supports && (e_supports || id_only) {
        if !e_supports {
            d.notes.push(IdentityNote::IdOnly);
        }
        IdentityStatus::AutoAccepted
    } else if !preventing && ab_supports && d.findings.is_empty() {
        IdentityStatus::NeedsConfirmation
    } else {
        if d.findings.is_empty() && d.binding_conflict.is_none() {
            d.add(
                SubReason::LowMargin,
                format!(
                    "the ID channel does not separate the best candidate (LLR {llr_ab:.1} nat)"
                ),
            );
        }
        IdentityStatus::Review
    };
    d.render_exceptions();
    d
}

/// Re-linkage after a roster change (§9.2 "Roster changes after scanning", §9.9 row 19): when a
/// booklet decoded against a new snapshot resolves differently from its previous decision, the
/// new decision gets IDN-004 `roster_diff` (ERROR) and goes to review; confirmed pairs are
/// never moved silently. Returns true when a difference was flagged.
pub fn mark_roster_diff(previous: &IdentityDecision, current: &mut IdentityDecision) -> bool {
    if previous.snapshot_sha384 == current.snapshot_sha384 || previous.student == current.student {
        return false;
    }
    let show = |s: &Option<StudentKey>| s.as_ref().map_or_else(|| "∅".to_owned(), |k| k.0.clone());
    current.add(
        SubReason::RosterDiff,
        format!(
            "roster snapshot changed: previously {}, now {}",
            show(&previous.student),
            show(&current.student)
        ),
    );
    current.status = IdentityStatus::Review;
    current.render_exceptions();
    true
}

fn relation_to_grid(g: &IdGridRead, id: &[u8], y: Option<&[u8]>) -> ReadRelation {
    let mut cols = g.columns.clone();
    cols.sort_by_key(|c| c.column);
    if cols.len() == id.len() {
        let mut wild = Vec::new();
        let mut ok = true;
        for (i, c) in cols.iter().enumerate() {
            match (c.state, c.marked.as_slice()) {
                (ColumnState::Single, [m]) => ok &= *m == id[i],
                (ColumnState::Multi, ms) => {
                    ok &= ms.contains(&id[i]);
                    wild.push(i as u8);
                }
                _ => wild.push(i as u8),
            }
        }
        if ok {
            return if wild.is_empty() {
                ReadRelation::Exact
            } else {
                ReadRelation::WildcardMatch { columns: wild }
            };
        }
    }
    match y.and_then(|y| writer_relation(id, y)) {
        Some(kind) => ReadRelation::WriterError { kind },
        None => ReadRelation::Other,
    }
}

/// Rosterless decoding (`identity_mode = candidate_number`, §9.1): the key is the bubbled ID
/// itself, accepted only when every column is a confident single mark, the string satisfies
/// the scheme (pattern, check digit, fields) and is not a national ID/Iqama or phone number,
/// and channels B/E (if present) agree. `not_on_roster` is suppressed in this mode.
pub fn decode_rosterless(
    ev: &BookletEvidence,
    scheme: &StudentIdScheme,
    _policy: &IdentityPolicy,
) -> IdentityDecision {
    let mut d = IdentityDecision::new(None);
    let Some(g) = ev.grid.as_ref().filter(|g| !g.columns.is_empty()) else {
        d.add(SubReason::IdBlank, "no ID grid on the booklet");
        d.render_exceptions();
        return d;
    };
    d.channels[0] = ChannelUse::Evaluated;
    let mut shown = grid_display(g);
    let mut cols = g.columns.clone();
    cols.sort_by_key(|c| c.column);
    let blanks: Vec<u8> = cols
        .iter()
        .filter(|c| c.state == ColumnState::Blank)
        .map(|c| c.column)
        .collect();
    let ambiguous: Vec<u8> = cols
        .iter()
        .filter(|c| matches!(c.state, ColumnState::Multi | ColumnState::Unreadable))
        .map(|c| c.column)
        .collect();
    let exact = g.exact_digits();
    let class = exact.as_deref().map(|e| classify_identifier(e, scheme));
    if reads_need_masking(ev, scheme) {
        shown = mask_digits(&shown);
        d.read.masked = true;
    }
    if !blanks.is_empty() {
        d.add(
            SubReason::IdBlank,
            format!("blank ID column(s) {blanks:?} in {shown}"),
        );
    }
    if !ambiguous.is_empty() {
        d.add(
            SubReason::IdAmbiguous,
            format!("double-marked or unreadable ID column(s) {ambiguous:?} in {shown}"),
        );
    }
    d.read.grid = Some(shown.clone());
    let Some(key) = exact else {
        d.render_exceptions();
        return d;
    };
    match class {
        Some(IdentifierClass::NationalId(_)) => {
            d.add(
                SubReason::WrongIdentifier,
                format!("ID grid holds a national ID/Iqama ({shown}); never used as a key"),
            );
        }
        Some(IdentifierClass::Phone) => {
            d.add(
                SubReason::WrongIdentifier,
                format!("ID grid holds a phone number ({shown})"),
            );
        }
        Some(IdentifierClass::NationalIdShaped(_)) => d.notes.push(IdentityNote::NationalIdShaped),
        _ => {}
    }
    if !d.has(SubReason::WrongIdentifier)
        && let Err(e) = scheme.validate(&key)
    {
        d.add(
            SubReason::WriterError,
            format!("read {shown} fails the {} scheme: {e}", scheme.name()),
        );
    }
    if let Some(t) = &ev.typed {
        let (chars, letters) = parse_typed(&t.text);
        let model_shown = typed_display(&chars);
        let tshown = if d.read.masked {
            mask_digits(&model_shown)
        } else {
            model_shown
        };
        d.read.typed = Some(tshown.clone());
        if letters {
            d.add(
                SubReason::WrongField,
                format!("letters in the written ID ({tshown})"),
            );
        }
        let written_class = chars
            .iter()
            .copied()
            .collect::<Option<Vec<u8>>>()
            .map(|f| classify_identifier(&digits_str(&f), scheme))
            .filter(|c| c.is_wrong_identifier());
        if let Some(c) = written_class {
            let what = if c == IdentifierClass::Phone {
                "a phone number"
            } else {
                "a national ID/Iqama"
            };
            d.add(
                SubReason::WrongIdentifier,
                format!("written ID is {what} ({tshown}), not a student ID"),
            );
            d.channels[1] = ChannelUse::Neutralized(format!("{what} written"));
        } else if chars.iter().any(Option::is_some) {
            d.channels[1] = ChannelUse::Evaluated;
            let kd: Vec<u8> = key.bytes().map(|b| b - b'0').collect();
            let agree = chars.len() == kd.len()
                && chars
                    .iter()
                    .zip(&kd)
                    .all(|(c, k)| c.is_none_or(|c| c == *k));
            if agree {
                d.notes.push(IdentityNote::ChannelsAgree);
            } else {
                d.add(
                    SubReason::ChannelConflict,
                    format!("bubbled {shown} and written {tshown} disagree"),
                );
            }
        } else {
            d.channels[1] = ChannelUse::Neutralized("unreadable".into());
        }
    }
    if let Some(b) = &ev.bound_student {
        d.channels[2] = ChannelUse::Evaluated;
        if normalize_id(&b.0) != key {
            d.binding_conflict = Some(format!(
                "page code is bound to {} but the grid reads {shown}",
                b.0
            ));
        }
    }
    let clean = d.findings.is_empty() && d.binding_conflict.is_none();
    if clean {
        d.student = Some(StudentKey(key));
        d.status = IdentityStatus::AutoAccepted;
    }
    d.render_exceptions();
    d
}

impl TypedModel {
    /// log emission of digit `d` at character position `c` (positional combination with A).
    pub(crate) fn log_l_pos(&self, d: u8, c: usize) -> f64 {
        self.emit_ln(d, self.chars.get(c).copied().flatten())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_parsing() {
        let (c, l) = parse_typed("٢٢-٢?A 1");
        assert_eq!(c, vec![Some(2), Some(2), Some(2), None, None, Some(1)]);
        assert!(l);
        assert_eq!(q(1.0), 1024);
        assert_eq!(nats(q(-2.5)), -2.5);
    }
}
