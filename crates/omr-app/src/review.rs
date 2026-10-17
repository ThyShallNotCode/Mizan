//! The review queue (§13.1–§13.2): one item per open ERROR/BLOCKER subject, in lanes, grouped
//! by `group_key = H(code, sub_reason, item/field, suggested value, feature bucket)` and ordered
//! by severity, code, item, group and top-bubble darkness. Every item carries its crop geometry
//! (page, rectangle, bubble overlays with fills) so a UI can render it without recognition.

use crate::engine::{BubbleGeom, GroupGeom};
use crate::model::{PageStatus, ResponseStatus, Session};
use omr_model::{Exception, Response, Severity};
use serde::{Deserialize, Serialize};

/// Review lane (§13.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Lane {
    /// MRK (except MRK-012), CAP-005/006/009, REG-003/004.
    Marks,
    /// IDN-004/005/006/007, COL-004.
    Identity,
    /// CAP-001/002/003/004/…, REG-001/002/005, COL-*, IDN-001/002.
    Capture,
    /// SCO, MRK-012, IDN-003.
    Scoring,
}

impl Lane {
    /// All lanes in display order.
    pub const ALL: [Lane; 4] = [Lane::Marks, Lane::Identity, Lane::Capture, Lane::Scoring];

    /// The lane of an Appendix A code (§13.2 table).
    pub fn of(code: &str) -> Lane {
        match code {
            "MRK-012" | "IDN-003" => Lane::Scoring,
            "COL-004" => Lane::Identity,
            "CAP-005" | "CAP-006" | "CAP-009" | "REG-003" | "REG-004" => Lane::Marks,
            c if c.starts_with("MRK") => Lane::Marks,
            c if c.starts_with("SCO") => Lane::Scoring,
            "IDN-004" | "IDN-005" | "IDN-006" | "IDN-007" => Lane::Identity,
            _ => Lane::Capture,
        }
    }

    /// Label.
    pub fn label(self) -> &'static str {
        match self {
            Lane::Marks => "Marks",
            Lane::Identity => "Identity",
            Lane::Capture => "Capture",
            Lane::Scoring => "Scoring",
        }
    }

    /// Parses a lane name.
    pub fn parse(s: &str) -> Option<Lane> {
        Lane::ALL
            .into_iter()
            .find(|l| l.label().eq_ignore_ascii_case(s.trim()))
    }
}

/// What kind of frame an item needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemKind {
    /// A question's marks.
    Mark,
    /// A booklet's identity.
    Identity,
    /// A booklet's form (version).
    Form,
    /// A page-, batch- or exam-level exception.
    Notice,
}

/// Queue state of an item (§13.1, desktop subset).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemState {
    /// Open.
    Open,
    /// Deferred to the end of the queue.
    Deferred,
    /// Escalated to the instructor.
    Escalated,
    /// On the rescan pull list (still blocking).
    Rescan,
    /// Resolved.
    Resolved,
}

/// A reviewer decision.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Decision {
    /// One option (0-based printed position).
    Option(u8),
    /// Nothing marked.
    Blank,
    /// Several marks (invalid response on a single-answer item).
    Multi(Vec<u8>),
    /// Cannot be read: rescan (never blank).
    Unreadable,
    /// The booklet belongs to this student.
    Student(String),
    /// Not on the roster.
    NotOnRoster,
    /// The booklet's form (index into the definition's forms).
    Form(u8),
    /// Seen; accept the core's handling.
    Acknowledge,
}

impl Decision {
    /// Display text.
    pub fn text(&self, labels: &[String], forms: &[String]) -> String {
        let lab = |o: u8| {
            labels
                .get(usize::from(o))
                .cloned()
                .unwrap_or_else(|| (o + 1).to_string())
        };
        match self {
            Decision::Option(o) => lab(*o),
            Decision::Blank => "blank".into(),
            Decision::Multi(v) => format!(
                "multi {}",
                v.iter().map(|&o| lab(o)).collect::<Vec<_>>().join("+")
            ),
            Decision::Unreadable => "unreadable".into(),
            Decision::Student(s) => format!("student {s}"),
            Decision::NotOnRoster => "not on roster".into(),
            Decision::Form(f) => format!(
                "form {}",
                forms.get(usize::from(*f)).cloned().unwrap_or_default()
            ),
            Decision::Acknowledge => "acknowledged".into(),
        }
    }

    /// Parses CLI text: `A`…, `1`…, `blank`, `multi:A+C`, `unreadable`, `student:ID`,
    /// `not-on-roster`, `form:B`, `ack`.
    pub fn parse(s: &str, options: u8, forms: &[String]) -> Option<Decision> {
        let t = s.trim();
        let lower = t.to_ascii_lowercase();
        match lower.as_str() {
            "blank" | "empty" | "none" => return Some(Decision::Blank),
            "unreadable" | "rescan" => return Some(Decision::Unreadable),
            "not-on-roster" | "notonroster" | "n" => return Some(Decision::NotOnRoster),
            "ack" | "acknowledge" => return Some(Decision::Acknowledge),
            _ => {}
        }
        if let Some(rest) = lower.strip_prefix("multi:") {
            let mut v: Vec<u8> = rest
                .split(['+', ',', '|'])
                .map(|p| crate::exam::parse_option(p, options))
                .collect::<Option<_>>()?;
            v.sort_unstable();
            v.dedup();
            return (v.len() >= 2).then_some(Decision::Multi(v));
        }
        if let Some(rest) = t.strip_prefix("student:") {
            return Some(Decision::Student(rest.trim().to_string()));
        }
        if let Some(rest) = lower.strip_prefix("form:") {
            let up = rest.trim().to_ascii_uppercase();
            return forms
                .iter()
                .position(|f| *f == up)
                .map(|i| Decision::Form(i as u8));
        }
        crate::exam::parse_option(t, options).map(Decision::Option)
    }
}

/// Bubble state drawn over the crop (DESIGN §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverlayState {
    /// Selected.
    Filled,
    /// In the review band or a multi candidate.
    Ambiguous,
    /// Unreadable (never empty).
    Unreadable,
    /// Blank.
    Blank,
}

/// One bubble in crop coordinates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OverlayBubble {
    /// Printed label.
    pub label: String,
    /// Centre x (crop px).
    pub cx: f32,
    /// Centre y (crop px).
    pub cy: f32,
    /// Radius (crop px).
    pub r: f32,
    /// δ fill 0..1.
    pub fill: f32,
    /// State.
    pub state: OverlayState,
}

/// Where the crop comes from and what to draw on it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CropGeom {
    /// Page index (into `Session::pages`).
    pub page: usize,
    /// Rectangle in working-image pixels: x, y, width, height (may extend past the image).
    pub rect: [i32; 4],
    /// Bubbles.
    pub bubbles: Vec<OverlayBubble>,
    /// Selection threshold and hysteresis band on the fill scale.
    pub threshold: f32,
    /// Band `(θ − h, θ + h)`.
    pub band: (f32, f32),
}

/// A review item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReviewItem {
    /// Stable ID (1-based, canonical order).
    pub id: u64,
    /// Lane.
    pub lane: Lane,
    /// Kind.
    pub kind: ItemKind,
    /// Appendix A code.
    pub code: String,
    /// Sub-reason.
    pub sub_reason: Option<String>,
    /// Severity.
    pub severity: Severity,
    /// Catalogue title.
    pub title: String,
    /// Detail (engine wording).
    pub detail: String,
    /// Booklet.
    pub booklet: Option<usize>,
    /// Page.
    pub page: Option<usize>,
    /// Question (0-based).
    pub question: Option<u16>,
    /// The engine's proposal (never a key-derived value).
    pub suggestion: Option<Decision>,
    /// Per-option fills (δ), for the sparkline.
    pub fills: Vec<f32>,
    /// Group key (§13.2).
    pub group_key: u64,
    /// Crop geometry.
    pub crop: Option<CropGeom>,
    /// State.
    pub state: ItemState,
    /// Recorded decision.
    pub resolution: Option<Decision>,
    /// Exception rows in the project store that this item resolves.
    pub store_exceptions: Vec<i64>,
    /// Number of times deferred (orders deferred items last).
    pub deferrals: u32,
}

impl ReviewItem {
    /// True while the item blocks (open, deferred, escalated, rescan).
    pub fn is_open(&self) -> bool {
        self.state != ItemState::Resolved
    }

    /// Top fill (similarity order inside a group).
    pub fn top_fill(&self) -> f32 {
        self.fills.iter().copied().fold(0.0, f32::max)
    }
}

/// A review action (§13.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    /// Record a decision.
    Resolve {
        /// Decision.
        decision: Decision,
        /// The reviewer pressed "accept suggestion".
        accepted_suggestion: bool,
        /// Note.
        note: String,
    },
    /// Apply the decision to every open member of the item's group.
    ApplyToGroup {
        /// Decision.
        decision: Decision,
    },
    /// Move to the end of the queue.
    Defer,
    /// Escalate to the instructor.
    Escalate,
    /// Add the sheet to the rescan pull list.
    AddToRescan,
}

/// What an action changed (for undo).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Change {
    /// Item.
    pub item: u64,
    /// Item state before.
    pub state_before: ItemState,
    /// Resolution before.
    pub resolution_before: Option<Decision>,
    /// Deferrals before.
    pub deferrals_before: u32,
    /// Booklet data before (response / status), when changed.
    pub response_before: Option<(Response, ResponseStatus)>,
    /// Identity before (student, confirmed), when changed.
    pub identity_before: Option<(Option<String>, bool)>,
    /// Form before (form, source), when changed.
    pub form_before: Option<(Option<usize>, crate::model::FormSource)>,
    /// Store exception rows resolved by this change.
    pub resolved_rows: Vec<i64>,
}

/// One recorded review event (newest last).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReviewRecord {
    /// Sequence (1-based).
    pub seq: u64,
    /// Item acted on.
    pub item: u64,
    /// Action.
    pub action: Action,
    /// Changes (one per affected item).
    pub changes: Vec<Change>,
    /// Actor.
    pub actor: String,
    /// True once compensated by an undo.
    pub undone: bool,
}

/// Queue and history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct ReviewState {
    /// Items by ID − 1.
    pub items: Vec<ReviewItem>,
    /// Events.
    pub history: Vec<ReviewRecord>,
}

impl ReviewState {
    /// Item by ID.
    pub fn item(&self, id: u64) -> Option<&ReviewItem> {
        id.checked_sub(1)
            .and_then(|i| self.items.get(i as usize))
            .filter(|it| it.id == id)
    }

    /// Mutable item by ID.
    pub fn item_mut(&mut self, id: u64) -> Option<&mut ReviewItem> {
        id.checked_sub(1)
            .and_then(|i| self.items.get_mut(i as usize))
            .filter(|it| it.id == id)
    }

    /// Open items of a lane (all lanes for `None`) in queue order (§13.2).
    pub fn queue(&self, lane: Option<Lane>) -> Vec<&ReviewItem> {
        let mut v: Vec<&ReviewItem> = self
            .items
            .iter()
            .filter(|i| i.is_open() && lane.is_none_or(|l| i.lane == l))
            .collect();
        v.sort_by(|a, b| queue_order(a, b));
        v
    }

    /// Open members of `item`'s group, in queue order.
    pub fn group_members(&self, item: &ReviewItem) -> Vec<&ReviewItem> {
        self.queue(Some(item.lane))
            .into_iter()
            .filter(|i| i.group_key == item.group_key && i.code == item.code)
            .collect()
    }
}

/// §13.2 order: deferred last; severity (highest first); code; item or field; group;
/// similarity (top-bubble darkness); booklet; ID.
pub fn queue_order(a: &ReviewItem, b: &ReviewItem) -> std::cmp::Ordering {
    (a.state == ItemState::Deferred)
        .cmp(&(b.state == ItemState::Deferred))
        .then(a.deferrals.cmp(&b.deferrals))
        .then(b.severity.cmp(&a.severity))
        .then(a.code.cmp(&b.code))
        .then(a.question.cmp(&b.question))
        .then(a.group_key.cmp(&b.group_key))
        .then(((a.top_fill() * 1024.0) as i32).cmp(&((b.top_fill() * 1024.0) as i32)))
        .then(a.booklet.cmp(&b.booklet))
        .then(a.id.cmp(&b.id))
}

/// FNV-1a 64 over the group-key fields (stable across runs and machines).
pub fn group_key(
    code: &str,
    sub: Option<&str>,
    field: &str,
    suggestion: Option<&Decision>,
    bucket: u8,
) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for &b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h ^= 0xff;
        h = h.wrapping_mul(0x0100_0000_01b3);
    };
    eat(code.as_bytes());
    eat(sub.unwrap_or("").as_bytes());
    eat(field.as_bytes());
    eat(format!("{suggestion:?}").as_bytes());
    eat(&[bucket]);
    h
}

fn suggestion_of(r: &Response) -> Option<Decision> {
    match r {
        Response::Single(o) => Some(Decision::Option(*o)),
        Response::Blank => Some(Decision::Blank),
        Response::Multi(v) | Response::Set(v) => Some(Decision::Multi(v.clone())),
        Response::Unreadable { .. } => None,
    }
}

/// Crop geometry of one bubble group: a padded rectangle around the bubbles; states from the
/// response, the threshold and the band.
pub fn crop_of(
    page: usize,
    g: &GroupGeom,
    labels: &[String],
    response: Option<&Response>,
    theta: f32,
    band: (f32, f32),
    pad_r: f32,
) -> Option<CropGeom> {
    if g.bubbles.is_empty() {
        return None;
    }
    let r = g
        .bubbles
        .iter()
        .map(|b| b.r)
        .fold(0.0f32, f32::max)
        .max(4.0);
    let pad = r * pad_r;
    let x0 = g.bubbles.iter().map(|b| b.cx).fold(f32::MAX, f32::min) - r - pad;
    let y0 = g.bubbles.iter().map(|b| b.cy).fold(f32::MAX, f32::min) - r - pad;
    let x1 = g.bubbles.iter().map(|b| b.cx).fold(f32::MIN, f32::max) + r + pad;
    let y1 = g.bubbles.iter().map(|b| b.cy).fold(f32::MIN, f32::max) + r + pad;
    let rect = [
        x0.floor() as i32,
        y0.floor() as i32,
        (x1 - x0).ceil().max(1.0) as i32,
        (y1 - y0).ceil().max(1.0) as i32,
    ];
    let marked: Vec<u8> = match response {
        Some(Response::Single(o)) => vec![*o],
        Some(Response::Multi(v)) | Some(Response::Set(v)) => v.clone(),
        _ => Vec::new(),
    };
    let bubbles = g
        .bubbles
        .iter()
        .map(|b: &BubbleGeom| OverlayBubble {
            label: labels
                .get(usize::from(b.option))
                .cloned()
                .unwrap_or_else(|| b.option.to_string()),
            cx: b.cx - rect[0] as f32,
            cy: b.cy - rect[1] as f32,
            r: b.r,
            fill: b.fill,
            state: if b.unreadable {
                OverlayState::Unreadable
            } else if marked.contains(&b.option) && b.fill >= band.1 {
                OverlayState::Filled
            } else if b.fill >= band.0 || marked.contains(&b.option) {
                OverlayState::Ambiguous
            } else {
                OverlayState::Blank
            },
        })
        .collect();
    Some(CropGeom {
        page,
        rect,
        bubbles,
        threshold: theta,
        band,
    })
}

/// The most severe blocking exception of a list (ties: lowest code).
fn worst(ex: &[Exception]) -> Option<&Exception> {
    ex.iter()
        .filter(|e| e.severity >= Severity::Error)
        .min_by(|a, b| b.severity.cmp(&a.severity).then(a.code.cmp(&b.code)))
}

fn title(code: &str) -> String {
    omr_model::exceptions::lookup(code)
        .map(|d| d.title.to_string())
        .unwrap_or_default()
}

/// Builds the queue from a session's open subjects (called once after grading).
pub fn build_queue(
    session: &Session,
    option_labels: &[String],
    id_labels: &[String],
) -> Vec<ReviewItem> {
    let mut items: Vec<ReviewItem> = Vec::new();
    let mut push = |mut it: ReviewItem| {
        it.id = items.len() as u64 + 1;
        items.push(it);
    };
    let blank_item = |lane, kind, e: &Exception| ReviewItem {
        id: 0,
        lane,
        kind,
        code: e.code.clone(),
        sub_reason: e.sub_reason.clone(),
        severity: e.severity,
        title: title(&e.code),
        detail: e.detail.clone(),
        booklet: None,
        page: None,
        question: None,
        suggestion: None,
        fills: Vec::new(),
        group_key: 0,
        crop: None,
        state: ItemState::Open,
        resolution: None,
        store_exceptions: Vec::new(),
        deferrals: 0,
    };
    // Marks: one item per pending question.
    for (bi, b) in session.booklets.iter().enumerate() {
        for (q, qs) in b.questions.iter().enumerate() {
            if qs.status != ResponseStatus::Pending {
                continue;
            }
            let e = worst(&qs.recognized.exceptions)
                .cloned()
                .unwrap_or_else(|| {
                    omr_model::Exception::must("MRK-001", Severity::Error, omr_model::Scope::Item)
                        .with_detail("not auto-accepted")
                });
            let page = session.pages.get(qs.page);
            let read = page.and_then(|p| p.read.as_ref());
            let geom = read.and_then(|r| {
                r.questions
                    .iter()
                    .position(|x| usize::from(x.question) == q)
                    .and_then(|k| r.question_geom.get(k))
            });
            let fills: Vec<f32> = geom
                .map(|g| g.bubbles.iter().map(|b| b.fill).collect())
                .unwrap_or_default();
            let (theta, band) = read.map_or((0.2, (0.18, 0.22)), |r| {
                let t = f32::from(r.theta_sel) / 1024.0;
                let h = f32::from(r.hysteresis) / 1024.0;
                (t, (0.08f32.min(t - h), t + h))
            });
            let suggestion = suggestion_of(&qs.recognized.response);
            let top = fills.iter().copied().fold(0.0f32, f32::max);
            let mut it = blank_item(Lane::of(&e.code), ItemKind::Mark, &e);
            it.booklet = Some(bi);
            it.page = Some(qs.page);
            it.question = Some(q as u16);
            it.group_key = group_key(
                &e.code,
                e.sub_reason.as_deref(),
                &format!("q{q}"),
                suggestion.as_ref(),
                (top * 10.0) as u8,
            );
            it.crop = geom.and_then(|g| {
                crop_of(
                    qs.page,
                    g,
                    option_labels,
                    Some(&qs.recognized.response),
                    theta,
                    band,
                    0.6,
                )
            });
            it.suggestion = suggestion;
            it.fills = fills;
            push(it);
        }
    }
    // Identity: one item per booklet with an ERROR identity exception.
    for (bi, b) in session.booklets.iter().enumerate() {
        if let Some(e) = worst(&b.identity.exceptions) {
            let mut it = blank_item(Lane::of(&e.code), ItemKind::Identity, e);
            it.booklet = Some(bi);
            it.page = b.pages.first().copied();
            it.suggestion = b.identity.student.clone().map(Decision::Student);
            it.group_key = group_key(&e.code, e.sub_reason.as_deref(), "id", None, 0);
            if let Some(&p) = b.pages.first()
                && let Some(r) = session.pages[p].read.as_ref()
                && !r.id_geom.is_empty()
            {
                let all = GroupGeom {
                    bubbles: r.id_geom.iter().flat_map(|g| g.bubbles.clone()).collect(),
                };
                let t = f32::from(r.theta_sel) / 1024.0;
                it.crop = crop_of(p, &all, id_labels, None, t, (0.08, t), 0.4);
            }
            push(it);
        }
        // Form: IDN-003 / SCO-005 `inferred`.
        for e in b
            .exceptions
            .iter()
            .filter(|e| e.code == "IDN-003" || e.code == "SCO-005")
        {
            let mut it = blank_item(Lane::Scoring, ItemKind::Form, e);
            it.booklet = Some(bi);
            it.page = b.pages.first().copied();
            it.suggestion = b.inference.as_ref().map(|i| Decision::Form(i.argmax as u8));
            let bucket = b
                .inference
                .as_ref()
                .map_or(0, |i| u8::from(i.margin_milli >= 10_000));
            it.group_key = group_key(
                &e.code,
                e.sub_reason.as_deref(),
                "form",
                it.suggestion.as_ref(),
                bucket,
            );
            if let Some(&p) = b.pages.first()
                && let Some(r) = session.pages[p].read.as_ref()
                && let Some(g) = &r.form_geom
            {
                let t = f32::from(r.theta_sel) / 1024.0;
                it.crop = crop_of(p, g, &session.definition.forms, None, t, (0.08, t), 0.6);
            }
            push(it);
        }
        // Booklet notices (MRK-015 blank booklet …).
        for e in b
            .exceptions
            .iter()
            .filter(|e| e.code != "IDN-003" && e.code != "SCO-005" && e.severity >= Severity::Error)
        {
            let mut it = blank_item(Lane::of(&e.code), ItemKind::Notice, e);
            it.booklet = Some(bi);
            it.page = b.pages.first().copied();
            it.suggestion = Some(Decision::Acknowledge);
            it.group_key = group_key(&e.code, e.sub_reason.as_deref(), "booklet", None, 0);
            push(it);
        }
    }
    // Pages: refusals, unattributed pages, page-level ERRORs on read pages.
    for (pi, p) in session.pages.iter().enumerate() {
        let mut ex: Vec<&Exception> = Vec::new();
        match p.status {
            PageStatus::Refused => ex.extend(p.refusal.iter()),
            PageStatus::Duplicate => {}
            PageStatus::Read | PageStatus::Unattributed => {
                if let Some(r) = &p.read {
                    ex.extend(r.exceptions.iter().filter(|e| {
                        e.severity >= Severity::Error
                            && e.code != "MRK-015"
                            && e.scope != omr_model::Scope::Batch
                    }));
                }
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for e in ex {
            if !seen.insert((e.code.clone(), e.sub_reason.clone())) {
                continue;
            }
            let mut it = blank_item(Lane::of(&e.code), ItemKind::Notice, e);
            it.page = Some(pi);
            it.booklet = p.booklet;
            it.suggestion = Some(Decision::Acknowledge);
            it.group_key = group_key(&e.code, e.sub_reason.as_deref(), "page", None, 0);
            push(it);
        }
    }
    // Exam scope: the inferred key and other exam-level blockers.
    for e in session
        .exam_exceptions
        .iter()
        .filter(|e| e.severity >= Severity::Error)
    {
        let mut it = blank_item(Lane::of(&e.code), ItemKind::Notice, e);
        it.suggestion = Some(Decision::Acknowledge);
        it.group_key = group_key(&e.code, e.sub_reason.as_deref(), "exam", None, 0);
        push(it);
    }
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lanes_follow_the_spec_table() {
        assert_eq!(Lane::of("MRK-001"), Lane::Marks);
        assert_eq!(Lane::of("MRK-012"), Lane::Scoring);
        assert_eq!(Lane::of("IDN-003"), Lane::Scoring);
        assert_eq!(Lane::of("IDN-004"), Lane::Identity);
        assert_eq!(Lane::of("COL-004"), Lane::Identity);
        assert_eq!(Lane::of("CAP-001"), Lane::Capture);
        assert_eq!(Lane::of("CAP-009"), Lane::Marks);
        assert_eq!(Lane::of("REG-001"), Lane::Capture);
        assert_eq!(Lane::of("REG-003"), Lane::Marks);
        assert_eq!(Lane::of("SCO-005"), Lane::Scoring);
    }

    #[test]
    fn decisions_parse() {
        let forms = vec!["A".to_string(), "B".to_string()];
        assert_eq!(Decision::parse("C", 5, &forms), Some(Decision::Option(2)));
        assert_eq!(Decision::parse("blank", 5, &forms), Some(Decision::Blank));
        assert_eq!(
            Decision::parse("multi:A+D", 5, &forms),
            Some(Decision::Multi(vec![0, 3]))
        );
        assert_eq!(
            Decision::parse("form:b", 5, &forms),
            Some(Decision::Form(1))
        );
        assert_eq!(
            Decision::parse("student:2210000001", 5, &forms),
            Some(Decision::Student("2210000001".into()))
        );
        assert_eq!(Decision::parse("Z", 5, &forms), None);
        let k1 = group_key(
            "MRK-001",
            Some("weak_mark"),
            "q3",
            Some(&Decision::Option(1)),
            4,
        );
        let k2 = group_key(
            "MRK-001",
            Some("weak_mark"),
            "q3",
            Some(&Decision::Option(1)),
            4,
        );
        assert_eq!(k1, k2);
        assert_ne!(
            k1,
            group_key("MRK-001", None, "q3", Some(&Decision::Option(1)), 4)
        );
    }
}
