//! `DemoBackend`: a [`Backend`] over rich, entirely synthetic data, so the shell runs, renders
//! and is tested before `omr-app` exposes its view-models.
//!
//! No real student data: names are invented, student IDs are 10 digits starting with 9 (so they
//! cannot collide with real IAU numbers), national IDs are masked, and every scan crop is drawn
//! procedurally by [`crate::crops`]. Statistics come from a seeded synthetic cohort, so they are
//! internally consistent (histogram, grade counts and mean all derive from the same scores).

use crate::backend::*;
use crate::crops::{self, IdGridSpec, MarkStyle, RowSpec};
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// A rendered item: registered crop, original crop and bubble geometry.
type Rendered = (CropImage, Option<CropImage>, Vec<BubbleView>);
/// (question, form, serial, fills, suggested option, sub-reason).
type MarkRow = (u32, &'static str, &'static str, [f32; 5], u8, &'static str);
/// (question, form, serial, fills, erased option, kept option).
type ErasureRow = (u32, &'static str, &'static str, [f32; 5], usize, usize);
/// (letter, Arabic letter, lower bound, upper bound, GPA on 5, GPA on 4).
type Band = (&'static str, &'static str, f64, Option<f64>, f64, f64);

/// Exam ids of the two fully modelled exams.
pub const CS211: &str = "cs211-mt1";
pub const MATH101: &str = "math101-mt1";

// ------------------------------------------------------------------------------------ rng

/// xorshift64* with a fixed seed: reproducible synthetic data.
#[derive(Debug, Clone)]
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn normal(&mut self, mean: f64, sd: f64) -> f64 {
        let u1 = self.unit().max(1e-12);
        let u2 = self.unit();
        mean + sd * (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }
}

// ------------------------------------------------------------------------------------ items

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Open,
    /// Deferred: still open, sorted after every other open item of its group.
    Deferred(u64),
    Resolved,
    Escalated,
    Rescan,
}

impl Status {
    fn is_open(self) -> bool {
        matches!(self, Status::Open | Status::Deferred(_))
    }
}

#[derive(Debug, Clone)]
struct DemoItem {
    id: ItemId,
    exam: ExamId,
    lane: Lane,
    code: String,
    code_title: String,
    sub_reason: String,
    severity: Severity,
    kind: ReviewKind,
    question: u32,
    form: String,
    serial: String,
    /// Sort key inside the group (top-bubble darkness, descending).
    order: f32,
    fills: Vec<f32>,
    states: Vec<MarkState>,
    draws: Vec<MarkStyle>,
    suggestion: Option<Suggestion>,
    group_key: String,
    status: Status,
    decision: Option<Decision>,
    identity: Option<IdentityEvidence>,
    id_grid: Option<IdGridSpec>,
    notice: Option<NoticeEvidence>,
    label: String,
    detail: String,
}

#[derive(Debug, Clone)]
struct HistoryEntry {
    record: DecisionRecord,
    /// Items touched and their status/decision before the action.
    prior: Vec<(ItemId, Status, Option<Decision>)>,
}

struct DemoExam {
    summary: ExamSummary,
    /// Static counts for exams without modelled review items.
    static_exceptions: Option<SeverityCounts>,
    setup: Option<ExamSetup>,
    results: Option<Results>,
    outcomes: Option<Outcomes>,
    resolved_base: u32,
    options: Vec<String>,
}

/// Synthetic backend; see the module docs.
pub struct DemoBackend {
    exams: Vec<DemoExam>,
    items: Vec<DemoItem>,
    history: Vec<HistoryEntry>,
    seq: u64,
    /// Pages recognised in the live MATH 101 batch (advanced by `pump`).
    live_pages: f64,
    live_running: bool,
    last_pump: Option<Instant>,
    crop_cache: RefCell<HashMap<ItemId, Rendered>>,
    /// Four procedurally drawn sheets the batch thumbnails cycle through.
    thumb_images: Vec<CropImage>,
}

impl Default for DemoBackend {
    fn default() -> Self {
        Self::new()
    }
}

fn letters(n: usize) -> Vec<String> {
    (0..n)
        .map(|i| ((b'A' + i as u8) as char).to_string())
        .collect()
}

fn term_now() -> TermLabel {
    TermLabel {
        label: "1448 H, 1st term".into(),
        label_ar: "الفصل الأول ١٤٤٨ هـ".into(),
        gregorian: "Fall 2026".into(),
    }
}

/// Wilson 95 % interval for x successes of n (§11.3).
pub fn wilson(x: u32, n: u32) -> (f64, f64) {
    if n == 0 {
        return (0.0, 1.0);
    }
    let (n, z) = (n as f64, 1.959964_f64);
    let p = x as f64 / n;
    let denom = 1.0 + z * z / n;
    let centre = (p + z * z / (2.0 * n)) / denom;
    let half = z * (p * (1.0 - p) / n + z * z / (4.0 * n * n)).sqrt() / denom;
    ((centre - half).max(0.0), (centre + half).min(1.0))
}

/// Fisher-z 95 % interval of a correlation r over n examinees (§12.1).
pub fn r_interval(r: f64, n: u32) -> (f64, f64) {
    let z = r.clamp(-0.999, 0.999).atanh();
    let half = 1.96 / ((n as f64 - 3.0).max(1.0)).sqrt();
    ((z - half).tanh(), (z + half).tanh())
}

impl DemoBackend {
    pub fn new() -> Self {
        let mut b = Self {
            exams: Vec::new(),
            items: Vec::new(),
            history: Vec::new(),
            seq: 0,
            live_pages: 1284.0,
            live_running: true,
            last_pump: None,
            crop_cache: RefCell::new(HashMap::new()),
            thumb_images: (0..4)
                .map(|k| crops::page_thumbnail(11 + k, 84, 108))
                .collect(),
        };
        b.build_exams();
        b.build_cs211_items();
        b.build_math101_items();
        b
    }

    /// Number of distinct cached crops (for tests of lazy rendering).
    pub fn cached_crops(&self) -> usize {
        self.crop_cache.borrow().len()
    }

    /// Starts or pauses the simulated live batch.
    pub fn set_live(&mut self, running: bool) {
        self.live_running = running;
        self.last_pump = None;
    }

    fn exam(&self, id: &ExamId) -> Option<&DemoExam> {
        self.exams.iter().find(|e| &e.summary.id == id)
    }

    fn exam_mut(&mut self, id: &ExamId) -> Option<&mut DemoExam> {
        self.exams.iter_mut().find(|e| &e.summary.id == id)
    }

    // -------------------------------------------------------------------------- exams

    #[allow(clippy::too_many_arguments)]
    fn summary(
        id: &str,
        code: &str,
        title: &str,
        title_ar: &str,
        assessment: &str,
        date: &str,
        hijri: &str,
        versions: usize,
        status: ExamStatus,
        scanned: u32,
        expected: u32,
    ) -> ExamSummary {
        ExamSummary {
            id: ExamId(id.into()),
            course_code: code.into(),
            course_title: title.into(),
            course_title_ar: title_ar.into(),
            assessment: assessment.into(),
            term: term_now(),
            date: date.into(),
            date_hijri: format!("{hijri} AH"),
            versions: letters(versions),
            status,
            sheets_scanned: scanned,
            sheets_expected: expected,
            open_exceptions: SeverityCounts::default(),
            key_status: "Key v1 · published".into(),
            mapped_items: 0,
            mapping_approval: true,
            total_items: 0,
            roster_count: expected,
            sections: vec!["171".into()],
            last_scan: "—".into(),
            batch_id: "—".into(),
        }
    }

    fn build_exams(&mut self) {
        let mut cs = Self::summary(
            CS211,
            "CS 211",
            "Data Structures",
            "هياكل البيانات",
            "Midterm 1",
            "2026-10-12",
            "1448/05/01",
            2,
            ExamStatus::InReview,
            186,
            190,
        );
        cs.key_status = "Key v2 · published".into();
        cs.roster_count = 190;
        cs.sections = vec!["171".into(), "172".into(), "175".into()];
        cs.last_scan = "Today, 10:42".into();
        cs.batch_id = "scn_8f291a".into();
        let cs_setup = cs211_setup();
        cs.total_items = cs_setup.mapping.len() as u32;
        cs.mapped_items = mapped_items(&cs_setup);
        let (cs_results, cs_outcomes) = cs211_results(&cs_setup);

        let mut math = Self::summary(
            MATH101,
            "MATH 101",
            "Calculus I",
            "التفاضل والتكامل ١",
            "Midterm 1",
            "2026-10-14",
            "1448/05/03",
            4,
            ExamStatus::Scanning,
            1284,
            1672,
        );
        math.roster_count = 1672;
        math.sections = vec![
            "101".into(),
            "102".into(),
            "103".into(),
            "104".into(),
            "105".into(),
        ];
        math.last_scan = "Live · batch 3 of 5".into();
        math.batch_id = "scn_a40c17".into();
        let math_setup = math101_setup();
        math.total_items = math_setup.mapping.len() as u32;
        math.mapped_items = mapped_items(&math_setup);
        math.key_status = "Key v1 · published".into();

        let mut others = vec![
            (
                Self::summary(
                    "phys110-final",
                    "PHYS 110",
                    "General Physics I",
                    "الفيزياء العامة ١",
                    "Final",
                    "2026-10-08",
                    "1448/04/26",
                    4,
                    ExamStatus::Ready,
                    840,
                    840,
                ),
                SeverityCounts {
                    info: 3,
                    warn: 12,
                    ..Default::default()
                },
            ),
            (
                Self::summary(
                    "chem101-q2",
                    "CHEM 101",
                    "General Chemistry I",
                    "الكيمياء العامة ١",
                    "Quiz 2",
                    "2026-10-11",
                    "1448/04/29",
                    3,
                    ExamStatus::InReview,
                    412,
                    418,
                ),
                SeverityCounts {
                    error: 11,
                    warn: 17,
                    ..Default::default()
                },
            ),
            (
                Self::summary(
                    "stat201-q3",
                    "STAT 201",
                    "Introduction to Statistics",
                    "مقدمة في الإحصاء",
                    "Quiz 3",
                    "2026-10-15",
                    "1448/05/04",
                    1,
                    ExamStatus::Scanning,
                    140,
                    350,
                ),
                SeverityCounts {
                    error: 2,
                    warn: 1,
                    ..Default::default()
                },
            ),
            (
                Self::summary(
                    "engl102-mt1",
                    "ENGL 102",
                    "Academic Writing",
                    "الكتابة الأكاديمية",
                    "Midterm 1",
                    "2026-10-18",
                    "1448/05/07",
                    2,
                    ExamStatus::Setup,
                    0,
                    210,
                ),
                SeverityCounts::default(),
            ),
            (
                Self::summary(
                    "biol101-q1",
                    "BIOL 101",
                    "General Biology",
                    "الأحياء العامة",
                    "Quiz 1",
                    "2026-10-05",
                    "1448/04/23",
                    1,
                    ExamStatus::Released,
                    300,
                    300,
                ),
                SeverityCounts::default(),
            ),
        ];
        let mut islm = Self::summary(
            "islm101-final",
            "ISLM 101",
            "Islamic Culture",
            "الثقافة الإسلامية",
            "Final",
            "2026-05-20",
            "1447/12/04",
            2,
            ExamStatus::Released,
            650,
            650,
        );
        islm.term = TermLabel {
            label: "1447 H, 2nd term".into(),
            label_ar: "الفصل الثاني ١٤٤٧ هـ".into(),
            gregorian: "Spring 2026".into(),
        };
        others.push((islm, SeverityCounts::default()));

        self.exams.push(DemoExam {
            summary: cs,
            static_exceptions: None,
            setup: Some(cs_setup),
            results: Some(cs_results),
            outcomes: Some(cs_outcomes),
            resolved_base: 128,
            options: letters(5),
        });
        self.exams.push(DemoExam {
            summary: math,
            static_exceptions: None,
            setup: Some(math_setup),
            results: None,
            outcomes: None,
            resolved_base: 41,
            options: letters(4),
        });
        for (mut s, counts) in others {
            s.total_items = 30;
            s.mapped_items = if s.status == ExamStatus::Setup { 0 } else { 30 };
            if s.status != ExamStatus::Setup {
                s.last_scan = "Earlier this week".into();
                s.batch_id = format!("scn_{}", &s.id.0[..4]);
            }
            if s.status == ExamStatus::Setup {
                s.key_status = "Key v1 · draft".into();
            }
            self.exams.push(DemoExam {
                summary: s,
                static_exceptions: Some(counts),
                setup: None,
                results: None,
                outcomes: None,
                resolved_base: 0,
                options: letters(5),
            });
        }
    }

    // -------------------------------------------------------------------------- items

    fn next_id(&self) -> ItemId {
        ItemId(self.items.len() as u64 + 1)
    }

    #[allow(clippy::too_many_arguments)]
    fn push_mark(
        &mut self,
        exam: &str,
        code: &str,
        title: &str,
        severity: Severity,
        sub: &str,
        q: u32,
        form: &str,
        serial: &str,
        fills: &[f32],
        states: Option<&[MarkState]>,
        draws: Option<&[MarkStyle]>,
        suggestion: Option<(Decision, SuggestionSource)>,
        kind: ReviewKind,
    ) {
        let hi = 0.55; // upper edge of the hysteresis band
        let states: Vec<MarkState> = match states {
            Some(s) => s.to_vec(),
            None => fills
                .iter()
                .map(|&f| {
                    if f >= hi {
                        MarkState::Filled
                    } else if f >= 0.25 {
                        MarkState::Ambiguous
                    } else {
                        MarkState::Blank
                    }
                })
                .collect(),
        };
        let draws: Vec<MarkStyle> = match draws {
            Some(d) => d.to_vec(),
            None => fills
                .iter()
                .map(|&f| {
                    if f >= hi {
                        MarkStyle::Filled(f.min(0.95) + 0.04)
                    } else if f >= 0.2 {
                        MarkStyle::Light(f + 0.12)
                    } else {
                        MarkStyle::Empty
                    }
                })
                .collect(),
        };
        let suggestion = suggestion.map(|(decision, source)| {
            let mut sorted: Vec<f32> = fills.to_vec();
            sorted.sort_by(|a, b| b.total_cmp(a));
            let fill = match decision {
                Decision::Option(i) => fills[i as usize],
                _ => sorted[0],
            };
            Suggestion {
                decision,
                source,
                fill,
                margin: (sorted[0] - sorted.get(1).copied().unwrap_or(0.0)).max(0.0),
            }
        });
        let top = fills.iter().copied().fold(0.0_f32, f32::max);
        let id = self.next_id();
        let group_key = match kind {
            ReviewKind::BlindVerify => format!("verify-{}", id.0),
            _ => format!(
                "{code}|{q}|{form}|{:?}",
                suggestion.as_ref().map(|s| s.decision)
            ),
        };
        self.items.push(DemoItem {
            id,
            exam: ExamId(exam.into()),
            lane: Lane::Marks,
            code: code.into(),
            code_title: title.into(),
            sub_reason: sub.into(),
            severity,
            kind,
            question: q,
            form: form.into(),
            serial: serial.into(),
            order: -top,
            fills: fills.to_vec(),
            states,
            draws,
            suggestion,
            group_key,
            status: Status::Open,
            decision: None,
            identity: None,
            id_grid: None,
            notice: None,
            label: format!("Q{q:02}"),
            detail: format!("Form {form}"),
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn push_identity(
        &mut self,
        exam: &str,
        code: &str,
        title: &str,
        severity: Severity,
        sub: &str,
        serial: &str,
        evidence: IdentityEvidence,
        grid: IdGridSpec,
        suggestion: Decision,
    ) {
        let id = self.next_id();
        let posterior = evidence.candidates.first().map_or(0.0, |c| c.posterior);
        self.items.push(DemoItem {
            id,
            exam: ExamId(exam.into()),
            lane: Lane::Identity,
            code: code.into(),
            code_title: title.into(),
            sub_reason: sub.into(),
            severity,
            kind: ReviewKind::Identity,
            question: 0,
            form: String::new(),
            serial: serial.into(),
            order: -posterior,
            fills: Vec::new(),
            states: Vec::new(),
            draws: Vec::new(),
            suggestion: Some(Suggestion {
                decision: suggestion,
                source: SuggestionSource::Engine,
                fill: posterior,
                margin: 0.0,
            }),
            group_key: format!("id-{}", id.0),
            status: Status::Open,
            decision: None,
            identity: Some(evidence),
            id_grid: Some(grid),
            notice: None,
            label: format!("Booklet {serial}"),
            detail: sub.replace('_', " "),
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn push_notice(
        &mut self,
        exam: &str,
        lane: Lane,
        code: &str,
        title: &str,
        severity: Severity,
        sub: &str,
        label: &str,
        notice: NoticeEvidence,
    ) {
        let id = self.next_id();
        self.items.push(DemoItem {
            id,
            exam: ExamId(exam.into()),
            lane,
            code: code.into(),
            code_title: title.into(),
            sub_reason: sub.into(),
            severity,
            kind: ReviewKind::Notice,
            question: 0,
            form: String::new(),
            serial: String::new(),
            order: 0.0,
            fills: Vec::new(),
            states: Vec::new(),
            draws: Vec::new(),
            suggestion: Some(Suggestion {
                decision: Decision::Acknowledge,
                source: SuggestionSource::Rule(code.into()),
                fill: 0.0,
                margin: 0.0,
            }),
            group_key: format!("notice-{}", id.0),
            status: Status::Open,
            decision: None,
            identity: None,
            id_grid: None,
            notice: Some(notice),
            label: label.into(),
            detail: sub.replace('_', " "),
        });
    }

    fn build_cs211_items(&mut self) {
        use Decision::*;
        use MarkState as M;
        use MarkStyle as S;
        let e = SuggestionSource::Engine;
        let (low, multi, erase) = (
            ("MRK-001", "Low-confidence mark"),
            ("MRK-002", "Multiple marks"),
            ("MRK-003", "Eraser residue"),
        );
        let mk = ReviewKind::Mark;
        let err = Severity::Error;
        // MRK-001: sorted by top-bubble darkness inside the group (§13.2)
        let rows: &[MarkRow] = &[
            (
                17,
                "B",
                "0412",
                [0.06, 0.82, 0.05, 0.31, 0.04],
                1,
                "light second mark",
            ),
            (
                17,
                "B",
                "0377",
                [0.05, 0.80, 0.06, 0.29, 0.05],
                1,
                "light second mark",
            ),
            (
                17,
                "B",
                "0450",
                [0.05, 0.79, 0.04, 0.30, 0.06],
                1,
                "light second mark",
            ),
            (
                26,
                "B",
                "0603",
                [0.52, 0.05, 0.06, 0.05, 0.04],
                0,
                "half fill",
            ),
            (
                9,
                "A",
                "0233",
                [0.04, 0.05, 0.47, 0.06, 0.05],
                2,
                "half fill",
            ),
            (6, "B", "0518", [0.05, 0.44, 0.06, 0.07, 0.05], 1, "light"),
            (
                13,
                "A",
                "0129",
                [0.07, 0.06, 0.05, 0.41, 0.36],
                3,
                "two light candidates",
            ),
            (22, "A", "0105", [0.38, 0.05, 0.04, 0.05, 0.04], 0, "light"),
        ];
        for &(q, form, serial, fills, opt, sub) in rows {
            self.push_mark(
                CS211,
                low.0,
                low.1,
                err,
                sub,
                q,
                form,
                serial,
                &fills,
                None,
                None,
                Some((Option(opt), e.clone())),
                mk,
            );
        }
        for &(q, form, serial, fills) in &[
            (4, "B", "0288", [0.86, 0.05, 0.81, 0.04, 0.05]),
            (19, "A", "0174", [0.05, 0.83, 0.06, 0.07, 0.79]),
            (19, "A", "0219", [0.06, 0.80, 0.05, 0.06, 0.77]),
        ] {
            self.push_mark(
                CS211,
                multi.0,
                multi.1,
                err,
                "two full marks",
                q,
                form,
                serial,
                &fills,
                None,
                None,
                Some((Multi, e.clone())),
                mk,
            );
        }
        let erasures: &[ErasureRow] = &[
            (11, "A", "0156", [0.29, 0.04, 0.06, 0.88, 0.05], 0, 3),
            (24, "B", "0337", [0.05, 0.33, 0.84, 0.05, 0.04], 1, 2),
            (8, "A", "0402", [0.81, 0.05, 0.04, 0.06, 0.27], 4, 0),
        ];
        for &(q, form, serial, fills, erased, kept) in erasures {
            let mut states = [M::Blank; 5];
            let mut draws = [S::Empty; 5];
            states[erased] = M::Ambiguous;
            states[kept] = M::Filled;
            draws[erased] = S::Erased(0.62);
            draws[kept] = S::Filled(0.92);
            self.push_mark(
                CS211,
                erase.0,
                erase.1,
                err,
                "resolved erasure",
                q,
                form,
                serial,
                &fills,
                Some(&states),
                Some(&draws),
                Some((Option(kept as u8), e.clone())),
                mk,
            );
        }
        self.push_mark(
            CS211,
            "MRK-005",
            "Tick instead of a fill",
            err,
            "check_or_x",
            14,
            "A",
            "0302",
            &[0.05, 0.31, 0.05, 0.04, 0.06],
            Some(&[M::Blank, M::Ambiguous, M::Blank, M::Blank, M::Blank]),
            Some(&[S::Empty, S::Tick, S::Empty, S::Empty, S::Empty]),
            Some((Option(1), SuggestionSource::Rule("fill-only mode".into()))),
            mk,
        );
        self.push_mark(
            CS211,
            "MRK-010",
            "Stain, smudge or crease",
            err,
            "flips under perturbation",
            25,
            "B",
            "0467",
            &[0.06, 0.07, 0.58, 0.12, 0.05],
            Some(&[M::Blank, M::Blank, M::Unreadable, M::Blank, M::Blank]),
            Some(&[S::Empty, S::Empty, S::Smudge, S::Empty, S::Empty]),
            Some((Unreadable, e.clone())),
            mk,
        );
        self.push_mark(
            CS211,
            "REG-003",
            "High residual (fold, curl)",
            err,
            "fold across row",
            28,
            "B",
            "0611",
            &[0.05, 0.06, 0.05, 0.71, 0.05],
            None,
            None,
            Some((Option(3), e.clone())),
            mk,
        );
        for &(q, form, serial, fills, opt) in &[
            (20, "A", "0145", [0.04, 0.05, 0.06, 0.76, 0.05], 3u8),
            (16, "B", "0359", [0.78, 0.05, 0.06, 0.05, 0.04], 0u8),
        ] {
            let mut draws: Vec<MarkStyle> = fills
                .iter()
                .map(|&f| if f > 0.5 { S::Filled(0.85) } else { S::Empty })
                .collect();
            draws[1] = S::Streak;
            self.push_mark(
                CS211,
                "CAP-006",
                "Scanner streaks",
                Severity::Warn,
                "masked column",
                q,
                form,
                serial,
                &fills,
                None,
                Some(&draws),
                Some((Option(opt), e.clone())),
                mk,
            );
        }
        // blind verify sample: auto-accepted questions, engine reading withheld (§13.4)
        for &(q, form, serial, fills) in &[
            (5, "A", "0244", [0.05, 0.84, 0.04, 0.06, 0.05]),
            (21, "B", "0498", [0.04, 0.05, 0.06, 0.05, 0.79]),
        ] {
            self.push_mark(
                CS211,
                "VERIFY",
                "Blind verify sample",
                Severity::Info,
                "verify",
                q,
                form,
                serial,
                &fills,
                None,
                None,
                None,
                ReviewKind::BlindVerify,
            );
        }

        // ---- identity lane
        let ch = |c: &str, agrees: bool, d: &str| ChannelEvidence {
            channel: c.into(),
            agrees,
            detail: d.into(),
        };
        #[allow(clippy::too_many_arguments)]
        let cand = |name: &str,
                    ar: &str,
                    id: &str,
                    nid: &str,
                    section: &str,
                    seat: &str,
                    track: &str,
                    p: f32,
                    channels: Vec<ChannelEvidence>| IdentityCandidate {
            name: name.into(),
            name_ar: ar.into(),
            student_id: id.into(),
            national_id_masked: format!("••••••{nid}"),
            section: section.into(),
            campus: "Main campus".into(),
            track: track.into(),
            seat: seat.into(),
            posterior: p,
            channels,
        };
        let ev = IdentityEvidence {
            read_id: "9261034471".into(),
            boxes_read: "9261034417".into(),
            hypothesis: "Writer error: columns 9 and 10 swapped on the grid; the write-in boxes and the name agree with candidate 1.".into(),
            candidates: vec![
                cand("Sara Al-Qahtani", "سارة القحطاني", "9261034417", "3456", "171", "B-14", "Female", 0.91, vec![
                    ch("ID grid", false, "9261034471 · 2 digits swapped"),
                    ch("ID boxes", true, "9261034417"),
                    ch("Name", true, "match 0.94"),
                    ch("Seat map", true, "B-14 expected"),
                ]),
                cand("Lama Al-Shehri", "لمى الشهري", "9261034477", "7720", "171", "B-22", "Female", 0.05, vec![
                    ch("ID grid", false, "1 digit differs"),
                    ch("ID boxes", false, "2 digits differ"),
                    ch("Name", false, "match 0.21"),
                    ch("Seat map", false, "B-22, not adjacent"),
                ]),
                cand("Haya Al-Mutairi", "هيا المطيري", "9261084417", "1184", "172", "C-03", "Female", 0.02, vec![
                    ch("ID grid", false, "2 digits differ"),
                    ch("ID boxes", false, "1 digit differs"),
                    ch("Name", false, "match 0.12"),
                    ch("Seat map", false, "other room"),
                ]),
            ],
            neighbours: vec!["B-13 · Nouf Al-Harbi".into(), "B-15 · Ghada Al-Zahrani".into()],
        };
        let marks = id_marks("9261034471", &[]);
        self.push_identity(
            CS211,
            "IDN-004",
            "Identity unresolved",
            err,
            "writer_error",
            "0277",
            ev,
            IdGridSpec {
                boxes: "9261034417".into(),
                marks,
                seed: 277,
            },
            Decision::Candidate(0),
        );
        let ev = IdentityEvidence {
            read_id: "92610?4425".into(),
            boxes_read: "9261034425".into(),
            hypothesis: "Column 6 carries two marks (3 and 8); the write-in box reads 3.".into(),
            candidates: vec![
                cand(
                    "Nouf Al-Harbi",
                    "نوف الحربي",
                    "9261034425",
                    "9031",
                    "172",
                    "C-11",
                    "Female",
                    0.52,
                    vec![
                        ch("ID grid", false, "column 6 ambiguous (3/8)"),
                        ch("ID boxes", true, "9261034425"),
                        ch("Name", true, "match 0.88"),
                        ch("Seat map", true, "C-11 expected"),
                    ],
                ),
                cand(
                    "Ghada Al-Zahrani",
                    "غادة الزهراني",
                    "9261084425",
                    "5512",
                    "172",
                    "C-19",
                    "Female",
                    0.44,
                    vec![
                        ch("ID grid", false, "column 6 ambiguous (3/8)"),
                        ch("ID boxes", false, "1 digit differs"),
                        ch("Name", false, "match 0.30"),
                        ch("Seat map", false, "C-19, not adjacent"),
                    ],
                ),
            ],
            neighbours: vec![
                "C-10 · Maha Al-Ghamdi".into(),
                "C-12 · Dana Al-Subaie".into(),
            ],
        };
        let marks = id_marks("9261034425", &[(5, 8, MarkStyle::Light(0.62))]);
        self.push_identity(
            CS211,
            "IDN-004",
            "Identity unresolved",
            err,
            "id_ambiguous",
            "0533",
            ev,
            IdGridSpec {
                boxes: "9261034425".into(),
                marks,
                seed: 533,
            },
            Decision::Candidate(0),
        );
        let ev = IdentityEvidence {
            read_id: "9261157702".into(),
            boxes_read: "9261157702".into(),
            hypothesis: "No roster student within two digits; grid and boxes agree. Possibly a late registration.".into(),
            candidates: vec![cand("Faisal Al-Otaibi", "فيصل العتيبي", "9261157720", "6603", "175", "A-07", "Male", 0.18, vec![
                ch("ID grid", false, "2 digits differ"),
                ch("ID boxes", false, "2 digits differ"),
                ch("Name", false, "name box blank"),
                ch("Seat map", false, "A-07, other room"),
            ])],
            neighbours: vec!["A-06 · Omar Al-Dosari".into(), "A-08 · Turki Al-Shammari".into()],
        };
        let marks = id_marks("9261157702", &[]);
        self.push_identity(
            CS211,
            "IDN-004",
            "Identity unresolved",
            err,
            "not_on_roster",
            "0590",
            ev,
            IdGridSpec {
                boxes: "9261157702".into(),
                marks,
                seed: 590,
            },
            Decision::NotOnRoster,
        );
        let ev = IdentityEvidence {
            read_id: "9261022290".into(),
            boxes_read: "9261022290".into(),
            hypothesis: "Two booklets resolve to one student: booklet 0388 was bound to the same ID at 10:31.".into(),
            candidates: vec![cand("Abeer Al-Anazi", "عبير العنزي", "9261022290", "4471", "171", "B-02", "Female", 0.97, vec![
                ch("ID grid", true, "exact"),
                ch("ID boxes", true, "exact"),
                ch("Name", true, "match 0.91"),
                ch("Seat map", false, "B-02 already bound to 0388"),
            ])],
            neighbours: vec!["B-01 · Reem Al-Dossari".into(), "B-03 · Shahad Al-Qarni".into()],
        };
        let marks = id_marks("9261022290", &[]);
        self.push_identity(
            CS211,
            "IDN-005",
            "Two booklets resolve to one student",
            err,
            "duplicate_binding",
            "0614",
            ev,
            IdGridSpec {
                boxes: "9261022290".into(),
                marks,
                seed: 614,
            },
            Decision::Duplicate,
        );
        self.push_notice(
            CS211, Lane::Identity, "IDN-007", "Provisional student pending registrar match",
            Severity::Warn, "provisional", "Student P-0007",
            NoticeEvidence {
                headline: "Provisional record P-0007 (Dana Al-Subaie, دانة السبيعي) is not yet matched to the SIS.".into(),
                lines: vec![
                    ("Booklet".into(), "0640".into()),
                    ("Created by".into(), "Exam control, 2026-10-12 09:58".into()),
                    ("Outbox".into(), "LMS/SIS pushes withheld until matched".into()),
                ],
                consequence: "Results stay provisional; printing and export remain allowed.".into(),
            },
        );
        // ---- capture lane
        self.push_notice(
            CS211, Lane::Capture, "CAP-002", "Effective DPI too low", err, "below_100_dpi",
            "Page 0129 · p1",
            NoticeEvidence {
                headline: "Page rejected: effective resolution 92 dpi (tag-centre distances), below the 100 dpi floor.".into(),
                lines: vec![
                    ("Booklet".into(), "0129, page 1 of 1".into()),
                    ("Device".into(), "fi-8170 · declared 300 dpi".into()),
                    ("Measured".into(), "92 dpi (bubble Ø 17 px)".into()),
                    ("Stack position".into(), "batch 2, sheet 41".into()),
                ],
                consequence: "The page goes to the rescan pull list (imprint 0129).".into(),
            },
        );
        self.push_notice(
            CS211,
            Lane::Capture,
            "COL-005",
            "Batch count mismatch",
            err,
            "header_count",
            "Batch 2 count",
            NoticeEvidence {
                headline: "Header sheet expects 190 booklets; 186 were scanned.".into(),
                lines: vec![
                    ("Missing serials".into(), "0397, 0457, 0512, 0588".into()),
                    (
                        "Absent list".into(),
                        "4 students marked absent by exam control".into(),
                    ),
                    ("Duplicates ignored".into(), "1 (CAP-011)".into()),
                ],
                consequence:
                    "Acknowledging records a reasoned count correction against the absent list."
                        .into(),
            },
        );
        // ---- scoring lane
        self.push_notice(
            CS211, Lane::Scoring, "SCO-001", "Probable mis-key", err, "rank_1", "Q12 · key C",
            NoticeEvidence {
                headline: "Probable mis-key on Q12: the keyed option C has r_ir −0.14 while D has r_d 0.21.".into(),
                lines: vec![
                    ("Difficulty p".into(), "0.21 (key C)".into()),
                    ("Option D".into(), "chosen by 58 %, r 0.21".into()),
                    ("Stability".into(), "fires in 97 % of 1,000 resamples".into()),
                ],
                consequence: "Acknowledge keeps the key with a note; change the key from Results (what-if preview).".into(),
            },
        );
        self.push_notice(
            CS211,
            Lane::Scoring,
            "SCO-002",
            "Possibly flawed item",
            Severity::Warn,
            "top_group",
            "Q07 · top group",
            NoticeEvidence {
                headline: "On Q07 the top 27 % prefer distractor C over the key.".into(),
                lines: vec![
                    ("p_U(C)".into(), "0.46 vs key 0.38".into()),
                    ("r_ir".into(), "0.08".into()),
                ],
                consequence: "Acknowledge keeps the item; void or re-key it from Results.".into(),
            },
        );
    }

    fn build_math101_items(&mut self) {
        let e = SuggestionSource::Engine;
        let mut rng = Rng::new(101);
        let forms = ["A", "B", "C", "D"];
        let mut serial = 1100u32;
        let mut next_serial = |rng: &mut Rng| {
            serial += 1 + rng.below(9) as u32;
            format!("{serial:04}")
        };
        let low = |rng: &mut Rng| 0.05 + rng.unit() as f32 * 0.04;
        // (code, title, severity, count): the counts the live batch view shows
        let plan: [(&str, &str, Severity, usize); 4] = [
            ("MRK-001", "Low-confidence mark", Severity::Error, 23),
            ("MRK-002", "Multiple marks", Severity::Error, 9),
            ("MRK-003", "Eraser residue", Severity::Error, 4),
            ("REG-003", "High residual (fold, curl)", Severity::Warn, 2),
        ];
        for (code, title, sev, count) in plan {
            for _ in 0..count {
                let q = 1 + rng.below(40) as u32;
                let form = forms[rng.below(4) as usize];
                let serial = next_serial(&mut rng);
                let mut fills: Vec<f32> = (0..4).map(|_| low(&mut rng)).collect();
                let a = rng.below(4) as usize;
                let b = (a + 1 + rng.below(3) as usize) % 4;
                let mut draws: Option<Vec<MarkStyle>> = None;
                let mut states: Option<Vec<MarkState>> = None;
                let (sugg, sub) = match code {
                    "MRK-001" if rng.unit() < 0.5 => {
                        fills[a] = 0.36 + rng.unit() as f32 * 0.18;
                        (Decision::Option(a as u8), "light")
                    }
                    "MRK-001" => {
                        fills[a] = 0.76 + rng.unit() as f32 * 0.1;
                        fills[b] = 0.27 + rng.unit() as f32 * 0.1;
                        (Decision::Option(a as u8), "light second mark")
                    }
                    "MRK-002" => {
                        fills[a] = 0.78 + rng.unit() as f32 * 0.1;
                        fills[b] = 0.76 + rng.unit() as f32 * 0.1;
                        (Decision::Multi, "two full marks")
                    }
                    "MRK-003" => {
                        fills[a] = 0.82 + rng.unit() as f32 * 0.08;
                        fills[b] = 0.26 + rng.unit() as f32 * 0.08;
                        let mut d = vec![MarkStyle::Empty; 4];
                        let mut st = vec![MarkState::Blank; 4];
                        d[a] = MarkStyle::Filled(0.9);
                        d[b] = MarkStyle::Erased(0.6);
                        st[a] = MarkState::Filled;
                        st[b] = MarkState::Ambiguous;
                        draws = Some(d);
                        states = Some(st);
                        (Decision::Option(a as u8), "resolved erasure")
                    }
                    _ => {
                        fills[a] = 0.7 + rng.unit() as f32 * 0.15;
                        (Decision::Option(a as u8), "curl near the row")
                    }
                };
                self.push_mark(
                    MATH101,
                    code,
                    title,
                    sev,
                    sub,
                    q,
                    form,
                    &serial,
                    &fills,
                    states.as_deref(),
                    draws.as_deref(),
                    Some((sugg, e.clone())),
                    ReviewKind::Mark,
                );
            }
        }
        // identity: six unresolved bubbled IDs with synthetic roster candidates
        let ch = |c: &str, agrees: bool, d: &str| ChannelEvidence {
            channel: c.into(),
            agrees,
            detail: d.into(),
        };
        for k in 0..6u64 {
            let serial = next_serial(&mut rng);
            let digits: String = "9262"
                .chars()
                .chain((0..6).map(|_| char::from(b'0' + rng.below(10) as u8)))
                .collect();
            let mut swapped: Vec<char> = digits.chars().collect();
            swapped.swap(7, 8);
            let read: String = swapped.iter().collect();
            let (name, name_ar) = synthetic_name(&mut rng);
            let (name2, name2_ar) = synthetic_name(&mut rng);
            let mut other: Vec<char> = digits.chars().collect();
            other[5] = char::from(b'0' + (other[5] as u8 - b'0' + 3) % 10);
            let track = if k % 2 == 0 { "Female" } else { "Male" };
            let ev = IdentityEvidence {
                read_id: read.clone(),
                boxes_read: digits.clone(),
                hypothesis: "Writer error: two adjacent grid columns swapped; the write-in boxes agree with candidate 1.".into(),
                candidates: vec![
                    IdentityCandidate {
                        name,
                        name_ar,
                        student_id: digits.clone(),
                        national_id_masked: format!("••••••{:04}", rng.below(10_000)),
                        section: format!("{}", 101 + k % 5),
                        campus: "Main campus".into(),
                        track: track.into(),
                        seat: format!("D-{:02}", 3 + k * 4),
                        posterior: 0.88,
                        channels: vec![
                            ch("ID grid", false, "2 digits swapped"),
                            ch("ID boxes", true, "exact"),
                            ch("Name", true, "match 0.90"),
                            ch("Seat map", true, "expected"),
                        ],
                    },
                    IdentityCandidate {
                        name: name2,
                        name_ar: name2_ar,
                        student_id: other.iter().collect(),
                        national_id_masked: format!("••••••{:04}", rng.below(10_000)),
                        section: format!("{}", 101 + (k + 2) % 5),
                        campus: "Main campus".into(),
                        track: track.into(),
                        seat: format!("E-{:02}", 5 + k * 3),
                        posterior: 0.07,
                        channels: vec![
                            ch("ID grid", false, "3 digits differ"),
                            ch("ID boxes", false, "1 digit differs"),
                            ch("Name", false, "match 0.18"),
                            ch("Seat map", false, "other room"),
                        ],
                    },
                ],
                neighbours: vec![format!("D-{:02} · neighbour seat", 2 + k * 4)],
            };
            let marks = id_marks(&read, &[]);
            self.push_identity(
                MATH101,
                "IDN-004",
                "Identity unresolved",
                Severity::Error,
                "writer_error",
                &serial,
                ev,
                IdGridSpec {
                    boxes: digits,
                    marks,
                    seed: 900 + k,
                },
                Decision::Candidate(0),
            );
        }
        for k in 0..3u32 {
            let serial = next_serial(&mut rng);
            self.push_notice(
                MATH101,
                Lane::Capture,
                "CAP-002",
                "Effective DPI too low",
                Severity::Error,
                "below_100_dpi",
                &format!("Page {serial} · p1"),
                NoticeEvidence {
                    headline: format!(
                        "Page rejected: effective resolution {} dpi, below the 100 dpi floor.",
                        88 + k * 3
                    ),
                    lines: vec![
                        ("Booklet".into(), format!("{serial}, page 1 of 2")),
                        ("Device".into(), "fi-8170 · declared 300 dpi".into()),
                    ],
                    consequence: format!(
                        "The page goes to the rescan pull list (imprint {serial})."
                    ),
                },
            );
        }
        self.push_notice(
            MATH101,
            Lane::Capture,
            "CAP-011",
            "Byte-identical duplicate file",
            Severity::Info,
            "duplicate_file",
            "File scan_0412.jpg",
            NoticeEvidence {
                headline:
                    "The same file arrived twice through the hot folder; the copy was ignored."
                        .into(),
                lines: vec![("Object".into(), "blake3 8e1f…a2c0".into())],
                consequence: "Nothing changes: the duplicate stays linked to the existing page."
                    .into(),
            },
        );
    }

    /// Open exceptions of an exam by code (blind verify excluded), worst first.
    fn code_counts(&self, exam: &ExamId) -> Vec<CodeCount> {
        let mut out: Vec<CodeCount> = Vec::new();
        for i in self
            .items
            .iter()
            .filter(|i| &i.exam == exam && i.status.is_open() && i.kind != ReviewKind::BlindVerify)
        {
            match out.iter_mut().find(|c| c.code == i.code) {
                Some(c) => c.count += 1,
                None => out.push(CodeCount {
                    code: i.code.clone(),
                    title: i.code_title.clone(),
                    severity: i.severity,
                    count: 1,
                }),
            }
        }
        out.sort_by(|a, b| {
            b.severity
                .cmp(&a.severity)
                .then(b.count.cmp(&a.count))
                .then(a.code.cmp(&b.code))
        });
        out
    }

    // -------------------------------------------------------------------------- queue logic

    fn lane_items<'a>(&'a self, exam: &'a ExamId, lane: Lane) -> Vec<&'a DemoItem> {
        let mut v: Vec<&DemoItem> = self
            .items
            .iter()
            .filter(|i| &i.exam == exam && i.lane == lane && i.status.is_open())
            .collect();
        v.sort_by(|a, b| {
            b.severity
                .cmp(&a.severity)
                .then_with(|| (a.code == "VERIFY").cmp(&(b.code == "VERIFY")))
                .then_with(|| a.code.cmp(&b.code))
                .then_with(|| defer_rank(a.status).cmp(&defer_rank(b.status)))
                .then_with(|| a.order.total_cmp(&b.order))
                .then_with(|| a.id.cmp(&b.id))
        });
        v
    }

    fn open_counts(&self, exam: &ExamId) -> SeverityCounts {
        let mut c = SeverityCounts::default();
        for i in self
            .items
            .iter()
            .filter(|i| &i.exam == exam && i.status.is_open())
        {
            if i.kind == ReviewKind::BlindVerify {
                continue; // verify items never count as exceptions (§13.4)
            }
            match i.severity {
                Severity::Info => c.info += 1,
                Severity::Warn => c.warn += 1,
                Severity::Error => c.error += 1,
                Severity::Blocker => c.blocker += 1,
            }
        }
        c
    }

    fn group_confirm(&self, item: &DemoItem) -> GroupConfirm {
        let members: Vec<ItemId> = self
            .lane_items(&item.exam, item.lane)
            .iter()
            .filter(|i| i.group_key == item.group_key)
            .map(|i| i.id)
            .collect();
        let size = members.len() as u32;
        let unreadable = item.states.contains(&MarkState::Unreadable)
            || matches!(
                item.suggestion.as_ref().map(|s| s.decision),
                Some(Decision::Unreadable)
            );
        let (allowed, reason) = if item.kind != ReviewKind::Mark {
            (false, "Only mark items can be batch-confirmed".to_string())
        } else if size < 2 {
            (false, "No other open item shares this group".to_string())
        } else if unreadable {
            (
                false,
                "UNREADABLE bubbles are never batch-confirmed".to_string(),
            )
        } else if item.severity == Severity::Blocker {
            (false, "BLOCKER items are never batch-confirmed".to_string())
        } else {
            (true, String::new())
        };
        GroupConfirm {
            size,
            members,
            allowed,
            reason,
        }
    }

    fn render_item(&self, item: &DemoItem) -> Rendered {
        if let Some(hit) = self.crop_cache.borrow().get(&item.id) {
            return hit.clone();
        }
        let options = self.exam(&item.exam).map_or(5, |e| e.options.len());
        let made = match item.kind {
            ReviewKind::Mark | ReviewKind::Form | ReviewKind::BlindVerify => {
                let labels: Vec<char> = (0..options).map(|i| (b'A' + i as u8) as char).collect();
                let (crop, geo) = crops::answer_row(&RowSpec {
                    question: item.question,
                    labels: labels.clone(),
                    marks: item.draws.clone(),
                    seed: item.id.0 * 1_000 + item.question as u64,
                });
                let original = crops::original_of(&crop, item.id.0);
                let bubbles = geo
                    .centres
                    .iter()
                    .enumerate()
                    .map(|(i, &(cx, cy))| BubbleView {
                        label: labels[i].to_string(),
                        cx,
                        cy,
                        r: geo.radius,
                        fill: item.fills.get(i).copied().unwrap_or(0.0),
                        state: item.states.get(i).copied().unwrap_or(MarkState::Blank),
                    })
                    .collect();
                (crop, Some(original), bubbles)
            }
            ReviewKind::Identity => {
                let spec = item.id_grid.clone().expect("identity grid");
                let (crop, geo) = crops::id_grid(&spec);
                let original = crops::original_of(&crop, item.id.0);
                let mut bubbles = Vec::new();
                for (c, col) in spec.marks.iter().enumerate() {
                    for (d, m) in col.iter().enumerate() {
                        let state = match m {
                            MarkStyle::Filled(_) => MarkState::Filled,
                            MarkStyle::Light(_) | MarkStyle::Erased(_) => MarkState::Ambiguous,
                            MarkStyle::Smudge | MarkStyle::Streak => MarkState::Unreadable,
                            _ => continue,
                        };
                        let (cx, cy) = geo.centres[c][d];
                        bubbles.push(BubbleView {
                            label: format!("column {} digit {d}", c + 1),
                            cx,
                            cy,
                            r: geo.radius,
                            fill: if state == MarkState::Filled {
                                0.86
                            } else {
                                0.48
                            },
                            state,
                        });
                    }
                }
                (crop, Some(original), bubbles)
            }
            ReviewKind::Notice => {
                let low = item.code == "CAP-002";
                (crops::page_scan(item.id.0, low), None, Vec::new())
            }
        };
        self.crop_cache.borrow_mut().insert(item.id, made.clone());
        made
    }

    fn other_fills(item: &DemoItem) -> Vec<f32> {
        let mut rng = Rng::new(item.id.0 * 31 + 7);
        (0..24)
            .map(|_| (0.80 + rng.normal(0.0, 0.035)) as f32)
            .collect()
    }

    fn summary_text(&self, item: &DemoItem, bubbles: &[BubbleView]) -> String {
        match item.kind {
            ReviewKind::Mark | ReviewKind::Form => {
                let parts: Vec<String> = bubbles
                    .iter()
                    .filter(|b| b.state != MarkState::Blank || b.fill >= 0.2)
                    .map(|b| {
                        let word = match b.state {
                            MarkState::Filled => "filled",
                            MarkState::Ambiguous => "light",
                            MarkState::Unreadable => "unreadable",
                            MarkState::Blank => "blank",
                        };
                        format!("{} {word} {:.2}", b.label, b.fill)
                    })
                    .collect();
                let sugg = item
                    .suggestion
                    .as_ref()
                    .map(|s| {
                        let what = decision_word(s.decision, bubbles);
                        let src = match &s.source {
                            SuggestionSource::Engine => "engine".to_string(),
                            SuggestionSource::Rule(r) => format!("rule, {r}"),
                            SuggestionSource::Ai { model } => format!("AI, {model}"),
                        };
                        format!(" Suggestion {what}, {src}.")
                    })
                    .unwrap_or_default();
                format!(
                    "Question {}, form {}. {}.{sugg}",
                    item.question,
                    item.form,
                    if parts.is_empty() {
                        "No marks".to_string()
                    } else {
                        parts.join(", ")
                    }
                )
            }
            ReviewKind::BlindVerify => format!(
                "Question {}, form {}. Blind verify: enter what is marked; the engine's reading is withheld.",
                item.question, item.form
            ),
            ReviewKind::Identity => {
                let ev = item.identity.as_ref().expect("identity evidence");
                let top = ev
                    .candidates
                    .first()
                    .map(|c| format!(" Top candidate {}, {:.2}.", c.name, c.posterior))
                    .unwrap_or_default();
                format!(
                    "Booklet {}. ID grid reads {}, boxes read {}.{top}",
                    item.serial, ev.read_id, ev.boxes_read
                )
            }
            ReviewKind::Notice => item
                .notice
                .as_ref()
                .map(|n| n.headline.clone())
                .unwrap_or_default(),
        }
    }

    fn valid_decision(&self, item: &DemoItem, d: Decision) -> bool {
        let options = self.exam(&item.exam).map_or(5, |e| e.options.len());
        match item.kind {
            ReviewKind::Mark | ReviewKind::BlindVerify => match d {
                Decision::Option(i) => (i as usize) < options,
                Decision::Blank | Decision::Multi | Decision::Unreadable => true,
                _ => false,
            },
            ReviewKind::Form => matches!(d, Decision::Option(i) if (i as usize) < options),
            ReviewKind::Identity => match d {
                Decision::Candidate(i) => {
                    (i as usize)
                        < item
                            .identity
                            .as_ref()
                            .map_or(0, |e| e.candidates.len().min(3))
                }
                Decision::NotOnRoster | Decision::Duplicate => true,
                _ => false,
            },
            ReviewKind::Notice => d == Decision::Acknowledge,
        }
    }

    fn record(
        &mut self,
        item: ItemId,
        action: ReviewAction,
        prior: Vec<(ItemId, Status, Option<Decision>)>,
    ) {
        self.seq += 1;
        let (question, serial) = self
            .items
            .iter()
            .find(|i| i.id == item)
            .map(|i| (i.question, i.serial.clone()))
            .unwrap_or_default();
        self.history.push(HistoryEntry {
            record: DecisionRecord {
                seq: self.seq,
                item,
                question,
                serial,
                action,
                reason: None,
                undone: false,
            },
            prior,
        });
    }

    // -------------------------------------------------------------------------- scan

    fn thumb_state(serial: u32) -> ThumbState {
        let h = serial.wrapping_mul(2_654_435_761) >> 24;
        match h % 37 {
            3 => ThumbState::Review {
                code: "MRK-001".into(),
                severity: Severity::Error,
            },
            11 => ThumbState::Review {
                code: "IDN-004".into(),
                severity: Severity::Error,
            },
            19 => ThumbState::Review {
                code: "REG-003".into(),
                severity: Severity::Warn,
            },
            27 => ThumbState::Rejected {
                code: "CAP-002".into(),
                severity: Severity::Error,
            },
            31 => ThumbState::Review {
                code: "MRK-002".into(),
                severity: Severity::Error,
            },
            _ => ThumbState::Auto,
        }
    }

    /// Gives every thumbnail one of the four drawn sheets, cycling in display order.
    fn with_images(&self, mut thumbs: Vec<Thumbnail>) -> Vec<Thumbnail> {
        for (i, t) in thumbs.iter_mut().enumerate() {
            t.image = self.thumb_images.get(i % 4).cloned();
        }
        thumbs
    }

    fn math_batch(&self) -> BatchProgress {
        let total = 1672u32;
        let done = (self.live_pages.round() as u32).min(total);
        let running = self.live_running && done < total;
        let in_flight = if running { 18.min(total - done) } else { 0 };
        let queued = total - done - in_flight;
        let pps = if running { 62.0 } else { 0.0 };
        let remaining = Duration::from_secs_f64(((total - done) as f64 / 62.0).max(0.0));
        // sheet serials of the window: 24 recognised, 3 reading, 3 queued
        let last = 1100 + done / 4;
        let first = last.saturating_sub(23);
        let mut thumbnails = Vec::new();
        for s in first..=last {
            thumbnails.push(Thumbnail {
                serial: format!("{s:04}"),
                state: Self::thumb_state(s),
                image: None,
            });
        }
        for k in 1..=3 {
            thumbnails.push(Thumbnail {
                serial: format!("{:04}", last + k),
                state: if running {
                    ThumbState::Reading
                } else {
                    ThumbState::Auto
                },
                image: None,
            });
        }
        for k in 4..=6 {
            thumbnails.push(Thumbnail {
                serial: format!("{:04}", last + k),
                state: if running {
                    ThumbState::Queued
                } else {
                    ThumbState::Auto
                },
                image: None,
            });
        }
        let ts = |s: u32| {
            let total = 10 * 3600 + 42 * 60 + 18 - (last - s) / 2;
            format!(
                "{:02}:{:02}:{:02}",
                total / 3600,
                (total / 60) % 60,
                total % 60
            )
        };
        let mut activity = Vec::new();
        for s in (first..=last).rev().take(7) {
            let what = match Self::thumb_state(s) {
                ThumbState::Auto => "auto".to_string(),
                ThumbState::Review { code, .. } => code,
                ThumbState::Rejected { code, .. } => format!("{code} · 92 dpi · rejected"),
                _ => String::new(),
            };
            activity.push(format!(
                "{}  page {} · {s:04} · {what}",
                ts(s),
                done - (last - s)
            ));
        }
        BatchProgress {
            exam: ExamId(MATH101.into()),
            batch_label: "Batch 3 of 5".into(),
            source: "hot folder".into(),
            device: "fi-8170 · 300 dpi · gray".into(),
            running,
            pages_done: done,
            pages_total: total,
            pages_per_sec: pps,
            floor_pages_per_sec: 40.0,
            max_pages_per_sec: 120.0,
            auto_accepted: 0.992,
            abstention_rate: 0.008,
            first_result: Duration::from_millis(2100),
            remaining,
            in_flight,
            queued,
            stages: vec![
                StageCount {
                    stage: "Ingest".into(),
                    count: (done + 17).min(total),
                },
                StageCount {
                    stage: "Decode".into(),
                    count: (done + 12).min(total),
                },
                StageCount {
                    stage: "Register".into(),
                    count: (done + 6).min(total),
                },
                StageCount {
                    stage: "Recognise".into(),
                    count: done,
                },
                StageCount {
                    stage: "Identify".into(),
                    count: done.saturating_sub(4),
                },
                StageCount {
                    stage: "Quarantined".into(),
                    count: 3,
                },
            ],
            exceptions: self.code_counts(&ExamId(MATH101.into())),
            thumbnails: self.with_images(thumbnails),
            reconciliation: vec![
                ReconLine {
                    label: "Header sheet expected".into(),
                    value: "420".into(),
                    tone: Tone::Neutral,
                },
                ReconLine {
                    label: "Scanned (this batch)".into(),
                    value: "418".into(),
                    tone: Tone::Neutral,
                },
                ReconLine {
                    label: "Missing serials".into(),
                    value: "0397, 0457".into(),
                    tone: Tone::Error,
                },
                ReconLine {
                    label: "Duplicates ignored (CAP-011)".into(),
                    value: "1".into(),
                    tone: Tone::Neutral,
                },
                ReconLine {
                    label: "Blank backs dropped (CAP-012)".into(),
                    value: "418".into(),
                    tone: Tone::Neutral,
                },
            ],
            reconciliation_code: Some(("COL-005".into(), Severity::Error)),
            activity,
            engine: "engine manifest 7f3a…b21c".into(),
            workers: 4,
            sheets: SheetCounts::default(),
            pausable: true,
            append: demo_append(),
        }
        .with_sheet_counts()
    }

    fn cs_batch(&self) -> BatchProgress {
        let mut thumbnails = Vec::new();
        for s in 590..620u32 {
            let state = match s {
                590 => ThumbState::Review {
                    code: "IDN-004".into(),
                    severity: Severity::Error,
                },
                603 => ThumbState::Review {
                    code: "MRK-001".into(),
                    severity: Severity::Error,
                },
                611 => ThumbState::Review {
                    code: "REG-003".into(),
                    severity: Severity::Error,
                },
                614 => ThumbState::Review {
                    code: "IDN-005".into(),
                    severity: Severity::Error,
                },
                _ => ThumbState::Auto,
            };
            thumbnails.push(Thumbnail {
                serial: format!("{s:04}"),
                state,
                image: None,
            });
        }
        let counts = self.open_counts(&ExamId(CS211.into()));
        BatchProgress {
            exam: ExamId(CS211.into()),
            batch_label: "Batch 2 of 2 · complete".into(),
            source: "file import".into(),
            device: "fi-7160 · 200 dpi · gray".into(),
            running: false,
            pages_done: 186,
            pages_total: 186,
            pages_per_sec: 48.0,
            floor_pages_per_sec: 40.0,
            max_pages_per_sec: 120.0,
            auto_accepted: 0.996,
            abstention_rate: 0.004,
            first_result: Duration::from_millis(1800),
            remaining: Duration::ZERO,
            in_flight: 0,
            queued: 0,
            stages: vec![
                StageCount {
                    stage: "Ingest".into(),
                    count: 187,
                },
                StageCount {
                    stage: "Decode".into(),
                    count: 187,
                },
                StageCount {
                    stage: "Register".into(),
                    count: 186,
                },
                StageCount {
                    stage: "Recognise".into(),
                    count: 186,
                },
                StageCount {
                    stage: "Identify".into(),
                    count: 186,
                },
                StageCount {
                    stage: "Quarantined".into(),
                    count: 1,
                },
            ],
            exceptions: self.code_counts(&ExamId(CS211.into())),
            thumbnails: self.with_images(thumbnails),
            reconciliation: vec![
                ReconLine {
                    label: "Header sheet expected".into(),
                    value: "190".into(),
                    tone: Tone::Neutral,
                },
                ReconLine {
                    label: "Scanned".into(),
                    value: "186".into(),
                    tone: Tone::Neutral,
                },
                ReconLine {
                    label: "Missing serials".into(),
                    value: "0397, 0457, 0512, 0588".into(),
                    tone: Tone::Error,
                },
                ReconLine {
                    label: "Open exceptions".into(),
                    value: counts.total().to_string(),
                    tone: Tone::Warning,
                },
            ],
            reconciliation_code: Some(("COL-005".into(), Severity::Error)),
            activity: vec![
                "10:42:07  batch 2 closed · 186 pages".into(),
                "10:42:06  page 186 · 0619 · auto".into(),
                "10:42:06  page 185 · 0614 · IDN-005".into(),
                "10:42:05  page 184 · 0611 · REG-003".into(),
                "10:42:05  page 183 · 0607 · auto".into(),
            ],
            engine: "engine manifest 7f3a…b21c".into(),
            workers: 4,
            sheets: SheetCounts::default(),
            pausable: true,
            append: demo_append(),
        }
        .with_sheet_counts()
    }
}

/// The demo writes nothing, so it cannot add scans.
fn demo_append() -> AppendInfo {
    AppendInfo {
        allowed: false,
        note: "Demo data: nothing is read or written. Open a graded exam (--project or \
               --workspace) to add scans"
            .into(),
    }
}

impl BatchProgress {
    /// The demo's sheet pills, from its modelled exception counts: rejected = CAP-002 pages,
    /// review = one sheet per WARN-or-worse exception, auto = the rest of the pages done.
    fn with_sheet_counts(mut self) -> Self {
        let rejected: u32 = self
            .exceptions
            .iter()
            .filter(|c| c.code == "CAP-002")
            .map(|c| c.count)
            .sum();
        let review: u32 = self
            .exceptions
            .iter()
            .filter(|c| c.code != "CAP-002" && c.severity >= Severity::Warn)
            .map(|c| c.count)
            .sum();
        self.sheets = SheetCounts {
            auto: self.pages_done.saturating_sub(review + rejected),
            review,
            rejected,
        };
        self
    }
}

fn defer_rank(s: Status) -> u64 {
    match s {
        Status::Deferred(n) => n,
        _ => 0,
    }
}

fn decision_word(d: Decision, bubbles: &[BubbleView]) -> String {
    match d {
        Decision::Option(i) => bubbles
            .get(i as usize)
            .map_or_else(|| ((b'A' + i) as char).to_string(), |b| b.label.clone()),
        Decision::Blank => "blank".into(),
        Decision::Multi => "multiple marks".into(),
        Decision::Unreadable => "unreadable".into(),
        Decision::Candidate(i) => format!("candidate {}", i + 1),
        Decision::NotOnRoster => "not on roster".into(),
        Decision::Duplicate => "duplicate".into(),
        Decision::Acknowledge => "acknowledge".into(),
    }
}

/// An invented name and its Arabic form (no real student data).
fn synthetic_name(rng: &mut Rng) -> (String, String) {
    const FIRST: [(&str, &str); 16] = [
        ("Sara", "سارة"),
        ("Lama", "لمى"),
        ("Haya", "هيا"),
        ("Nouf", "نوف"),
        ("Reem", "ريم"),
        ("Shahad", "شهد"),
        ("Joud", "جود"),
        ("Raghad", "رغد"),
        ("Faisal", "فيصل"),
        ("Omar", "عمر"),
        ("Turki", "تركي"),
        ("Khalid", "خالد"),
        ("Saud", "سعود"),
        ("Nawaf", "نواف"),
        ("Majed", "ماجد"),
        ("Fahad", "فهد"),
    ];
    const FAMILY: [(&str, &str); 12] = [
        ("Al-Qahtani", "القحطاني"),
        ("Al-Shehri", "الشهري"),
        ("Al-Mutairi", "المطيري"),
        ("Al-Harbi", "الحربي"),
        ("Al-Zahrani", "الزهراني"),
        ("Al-Ghamdi", "الغامدي"),
        ("Al-Anazi", "العنزي"),
        ("Al-Subaie", "السبيعي"),
        ("Al-Otaibi", "العتيبي"),
        ("Al-Dosari", "الدوسري"),
        ("Al-Shammari", "الشمري"),
        ("Al-Malki", "المالكي"),
    ];
    let (f, fa) = FIRST[rng.below(FIRST.len() as u64) as usize];
    let (l, la) = FAMILY[rng.below(FAMILY.len() as u64) as usize];
    (format!("{f} {l}"), format!("{fa} {la}"))
}

fn id_marks(digits: &str, extra: &[(usize, usize, MarkStyle)]) -> Vec<[MarkStyle; 10]> {
    let mut marks = vec![[MarkStyle::Empty; 10]; digits.chars().count()];
    for (c, d) in digits.chars().enumerate() {
        if let Some(d) = d.to_digit(10) {
            marks[c][d as usize] = MarkStyle::Filled(0.86);
        }
    }
    for &(c, d, m) in extra {
        marks[c][d] = m;
    }
    marks
}

fn mapped_items(setup: &ExamSetup) -> u32 {
    setup
        .mapping
        .iter()
        .filter(|r| {
            r.status == MappingStatus::Approved && r.cells.iter().any(|c| c.weight.is_some())
        })
        .count() as u32
}

// ------------------------------------------------------------------------------------ setup data

fn cs211_clos() -> Vec<CloDef> {
    let c = |code: &str, title: &str, ar: &str, domain, plo: &str| CloDef {
        code: code.into(),
        title: title.into(),
        title_ar: ar.into(),
        domain,
        plo: plo.into(),
    };
    vec![
        c(
            "CLO1",
            "Describe the behaviour of core data structures",
            "وصف سلوك هياكل البيانات الأساسية",
            NqfDomain::Knowledge,
            "K1",
        ),
        c(
            "CLO2",
            "Explain the complexity of standard operations",
            "شرح تعقيد العمليات القياسية",
            NqfDomain::Knowledge,
            "K2",
        ),
        c(
            "CLO3",
            "Apply lists, stacks and queues to problems",
            "تطبيق القوائم والمكدسات والطوابير لحل المسائل",
            NqfDomain::Skills,
            "S1",
        ),
        c(
            "CLO4",
            "Analyse tree and hash-table algorithms",
            "تحليل خوارزميات الأشجار وجداول التجزئة",
            NqfDomain::Skills,
            "S2",
        ),
        c(
            "CLO5",
            "Justify a data-structure choice responsibly",
            "تبرير اختيار هيكل البيانات بمسؤولية",
            NqfDomain::Values,
            "V1",
        ),
    ]
}

fn key_letters(seed: u64, n: usize, options: u8) -> Vec<String> {
    let mut rng = Rng::new(seed);
    (0..n)
        .map(|_| ((b'A' + rng.below(options as u64) as u8) as char).to_string())
        .collect()
}

fn cs211_setup() -> ExamSetup {
    let n = 29usize;
    let mut key_a = key_letters(211, n, 5);
    key_a[11] = "C".into(); // Q12: the flagged probable mis-key (SCO-001)
    key_a[16] = "B".into(); // Q17
    // version B prints the same items in another order with permuted options
    let key_b: Vec<String> = (0..n)
        .map(|i| {
            let src = &key_a[(i * 7 + 3) % n];
            let c = src.as_bytes()[0] - b'A';
            (((c + 2) % 5 + b'A') as char).to_string()
        })
        .collect();
    let clos = cs211_clos();
    let mut mapping = Vec::new();
    for q in 1..=n as u32 {
        let mut cells = vec![MappingCell::default(); clos.len()];
        let single = match q {
            1..=6 => Some(0),
            7..=12 => Some(1),
            13..=19 => Some(2),
            20..=26 => Some(3),
            27..=28 => Some(4),
            _ => None,
        };
        if let Some(c) = single {
            cells[c].weight = Some(1.0);
        }
        let mut status = MappingStatus::Approved;
        match q {
            5 => {
                cells[0].weight = Some(0.5);
                cells[1].weight = Some(0.5);
                cells[1].note = "Stem asks for the Big-O of insertion".into();
            }
            18 => {
                cells[2].weight = Some(0.6);
                cells[3].weight = Some(0.4);
                cells[3].note = "قراءة الشجرة بعد الإدراج".into();
            }
            24 => {
                cells[3].weight = Some(0.8);
                cells[3].note = "Partial: hashing collisions only".into();
                status = MappingStatus::Draft;
            }
            27 | 28 => {
                status = MappingStatus::Proposed;
                cells[4].note = "Scenario: choose a structure for a clinic queue".into();
            }
            29 => status = MappingStatus::Draft,
            _ => {}
        }
        mapping.push(MappingRow {
            question: q,
            points: 1.0,
            cells,
            status,
        });
    }
    ExamSetup {
        exam: ExamId(CS211.into()),
        roster_students: 120,
        template: TemplateInfo {
            id: "iau-ccsit-v1".into(),
            kind: TemplateKind::Legacy,
            name: "IAU CCSIT answer sheet (legacy, landscape A4)".into(),
            questions: n as u32,
            capacity: 100,
            option_labels: letters(5),
            label_set: LabelSet::Latin,
            versions: letters(2),
            cancel_mode: CancelMode::EraseOnly,
            dpi: 200,
            page_identity: "None on the form: bubble-lattice registration (§6.10)".into(),
            digest: "sha384 41c2…9e0d".into(),
        },
        keys: vec![
            VersionKey {
                version: "A".into(),
                answers: key_a,
            },
            VersionKey {
                version: "B".into(),
                answers: key_b,
            },
        ],
        key_status: "Key v2 · published".into(),
        key_published: true,
        clos,
        mapping_mode: MappingMode::Split,
        mapping,
        lints: Vec::new(), // computed in `exam_setup`
    }
}

fn math101_setup() -> ExamSetup {
    let n = 40usize;
    let clos = vec![
        CloDef {
            code: "CLO1".into(),
            title: "Evaluate limits and continuity".into(),
            title_ar: "حساب النهايات ودراسة الاتصال".into(),
            domain: NqfDomain::Knowledge,
            plo: "K1".into(),
        },
        CloDef {
            code: "CLO2".into(),
            title: "Apply differentiation rules".into(),
            title_ar: "تطبيق قواعد الاشتقاق".into(),
            domain: NqfDomain::Skills,
            plo: "S1".into(),
        },
        CloDef {
            code: "CLO3".into(),
            title: "Solve optimisation problems".into(),
            title_ar: "حل مسائل القيم القصوى".into(),
            domain: NqfDomain::Skills,
            plo: "S2".into(),
        },
        CloDef {
            code: "CLO4".into(),
            title: "Interpret definite integrals".into(),
            title_ar: "تفسير التكامل المحدد".into(),
            domain: NqfDomain::Knowledge,
            plo: "K2".into(),
        },
    ];
    let mapping = (1..=n as u32)
        .map(|q| {
            let mut cells = vec![MappingCell::default(); 4];
            cells[((q - 1) / 10) as usize].weight = Some(1.0);
            MappingRow {
                question: q,
                points: 1.0,
                cells,
                status: MappingStatus::Approved,
            }
        })
        .collect();
    ExamSetup {
        exam: ExamId(MATH101.into()),
        roster_students: 120,
        template: TemplateInfo {
            id: "gen:a4-mcq-40x4 rev 3".into(),
            kind: TemplateKind::Generated,
            name: "Generated A4 sheet, 40 questions × 4 options, 4 forms".into(),
            questions: n as u32,
            capacity: 50,
            option_labels: letters(4),
            label_set: LabelSet::Latin,
            versions: letters(4),
            cancel_mode: CancelMode::EraseOnly,
            dpi: 300,
            page_identity: "QR v3-M page code (96-bit tag) + AprilTag 36h11".into(),
            digest: "sha384 b07e…11a4".into(),
        },
        keys: (0..4)
            .map(|v| VersionKey {
                version: ((b'A' + v as u8) as char).to_string(),
                answers: key_letters(101 + v, n, 4),
            })
            .collect(),
        key_status: "Key v1 · published".into(),
        key_published: true,
        clos,
        mapping_mode: MappingMode::Full,
        mapping,
        lints: Vec::new(),
    }
}

fn setup_lints(setup: &ExamSetup) -> Vec<SetupLint> {
    let mut lints = Vec::new();
    for (ci, clo) in setup.clos.iter().enumerate() {
        let items = setup
            .mapping
            .iter()
            .filter(|r| r.cells[ci].weight.is_some())
            .count();
        if items < 3 {
            lints.push(SetupLint {
                code: "SCO-014".into(),
                severity: Severity::Warn,
                message: format!(
                    "Thin evidence: {} has {items} item{} (minimum 3)",
                    clo.code,
                    if items == 1 { "" } else { "s" }
                ),
            });
        }
    }
    let unmapped: Vec<String> = setup
        .mapping
        .iter()
        .filter(|r| r.cells.iter().all(|c| c.weight.is_none()))
        .map(|r| format!("Q{:02}", r.question))
        .collect();
    // Appendix A: SCO-013 "Scored item with no CLO mapping" (WARN)
    if !unmapped.is_empty() {
        lints.push(SetupLint {
            code: "SCO-013".into(),
            severity: Severity::Warn,
            message: format!(
                "Unmapped: {} carr{} no CLO evidence",
                unmapped.join(", "),
                if unmapped.len() == 1 { "ies" } else { "y" }
            ),
        });
    }
    if setup.mapping_mode == MappingMode::Split {
        let bad: Vec<String> = setup
            .mapping
            .iter()
            .filter(|r| {
                let s: f64 = r.cells.iter().filter_map(|c| c.weight).sum();
                s > 0.0 && (s - 1.0).abs() > 1e-9
            })
            .map(|r| format!("Q{:02}", r.question))
            .collect();
        // Appendix A: SCO-011 "Item→CLO mapping invalid" (split weights sum ≠ 1; ERROR)
        if !bad.is_empty() {
            lints.push(SetupLint {
                code: "SCO-011".into(),
                severity: Severity::Error,
                message: format!("Split weights must sum to 1: {}", bad.join(", ")),
            });
        }
    }
    let pending = setup
        .mapping
        .iter()
        .filter(|r| r.status != MappingStatus::Approved)
        .count();
    if pending > 0 {
        lints.push(SetupLint {
            code: "SCO-032".into(),
            severity: Severity::Warn,
            message: format!(
                "{pending} mapping rows not approved: reports carry a draft watermark"
            ),
        });
    }
    lints
}

// ------------------------------------------------------------------------------------ results

fn cs211_results(setup: &ExamSetup) -> (Results, Outcomes) {
    let n_students = 186u32;
    let mut rng = Rng::new(1448);
    let mut scores: Vec<f64> = (0..n_students)
        .map(|_| rng.normal(71.4, 12.8).clamp(17.0, 100.0))
        .map(|x| (x * 29.0 / 100.0).round() * 100.0 / 29.0) // whole marks out of 29
        .collect();
    scores.sort_by(f64::total_cmp);
    let mean = scores.iter().sum::<f64>() / n_students as f64;
    let median = (scores[92] + scores[93]) / 2.0;
    let sd = (scores.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n_students as f64).sqrt();
    let pass = scores.iter().filter(|&&x| x >= 60.0).count() as f64 / n_students as f64;
    let mut counts = vec![0u32; 20];
    for &x in &scores {
        counts[((x / 5.0).floor() as usize).min(19)] += 1;
    }
    let bands: [Band; 9] = [
        ("A+", "أ+", 95.0, None, 5.0, 4.0),
        ("A", "أ", 90.0, Some(95.0), 4.75, 3.75),
        ("B+", "ب+", 85.0, Some(90.0), 4.5, 3.5),
        ("B", "ب", 80.0, Some(85.0), 4.0, 3.0),
        ("C+", "ج+", 75.0, Some(80.0), 3.5, 2.5),
        ("C", "ج", 70.0, Some(75.0), 3.0, 2.0),
        ("D+", "د+", 65.0, Some(70.0), 2.5, 1.5),
        ("D", "د", 60.0, Some(65.0), 2.0, 1.0),
        ("F", "هـ", f64::NEG_INFINITY, Some(60.0), 1.0, 0.0),
    ];
    let grades = bands
        .iter()
        .map(|&(l, ar, lo, hi, g5, g4)| GradeBand {
            letter: l.into(),
            letter_ar: ar.into(),
            lo: if lo.is_finite() { lo } else { 0.0 },
            hi,
            gpa5: Some(g5),
            gpa4: Some(g4),
            count: scores
                .iter()
                .filter(|&&x| x + 1e-9 >= lo && hi.is_none_or(|h| x + 1e-9 < h))
                .count() as u32,
            fail: l == "F",
        })
        .collect();
    let alpha = 0.84;
    let sem_points = sd / 100.0 * 29.0 * (1.0_f64 - alpha).sqrt();

    // item analysis
    let mut items = Vec::new();
    let mut irng = Rng::new(29);
    for q in 1..=29u32 {
        let key = (setup.keys[0].answers[q as usize - 1].as_bytes()[0] - b'A') as usize;
        let mut p = (0.72 + irng.normal(0.0, 0.12)).clamp(0.3, 0.93);
        let mut r = (0.34 + irng.normal(0.0, 0.08)).clamp(0.12, 0.55);
        let mut flags = Vec::new();
        let mut shares = vec![0.0; 5];
        match q {
            12 => {
                p = 0.21;
                r = -0.14;
                flags.push(ItemFlag {
                    message: "Probable mis-key: D".into(),
                    code: Some("SCO-001".into()),
                    tone: Tone::Error,
                    stable: true,
                });
            }
            7 => {
                p = 0.38;
                r = 0.08;
                flags.push(ItemFlag {
                    message: "Top group prefers C".into(),
                    code: Some("SCO-002".into()),
                    tone: Tone::Warning,
                    stable: true,
                });
            }
            3 => {
                p = 0.96;
                r = 0.05;
                flags.push(ItemFlag {
                    message: "Too easy · little information".into(),
                    code: None,
                    tone: Tone::Info,
                    stable: true,
                });
            }
            15 => {
                p = 0.23;
                r = 0.06;
                flags.push(ItemFlag {
                    message: "Too hard or guessed".into(),
                    code: None,
                    tone: Tone::Info,
                    stable: false,
                });
            }
            21 => {
                r = 0.09;
                flags.push(ItemFlag {
                    message: "Low discrimination".into(),
                    code: None,
                    tone: Tone::Info,
                    stable: true,
                });
            }
            _ => {}
        }
        shares[key] = p;
        let mut rest = 1.0 - p - 0.01;
        let others: Vec<usize> = (0..5).filter(|&o| o != key).collect();
        for (k, &o) in others.iter().enumerate() {
            let s = if k == others.len() - 1 {
                rest
            } else {
                rest * (0.25 + irng.unit() * 0.3)
            };
            shares[o] = s;
            rest -= s;
        }
        if q == 12 {
            shares = vec![0.07, 0.09, 0.21, 0.58, 0.04];
        }
        if q == 7 {
            shares = vec![0.12, 0.38, 0.35, 0.09, 0.05];
        }
        items.push(ItemStats {
            question: q,
            label: String::new(),
            p: Some(p),
            r_ir: Some(r),
            r_ci: Some(r_interval(r, n_students)),
            band: Some(DiscriminationBand::from_r(r)),
            option_shares: shares,
            keyed: Some(key as u8),
            flags,
        });
    }
    let results = Results {
        exam: ExamId(CS211.into()),
        key_label: "Key v2 · published".into(),
        stats: ResultStats {
            students: n_students,
            roster: 190,
            absent: 4,
            mean_pct: mean,
            median_pct: median,
            sd_pct: sd,
            pass_rate: pass,
            alpha: Some(alpha),
            alpha_ci: Some((0.80, 0.87)),
            sem_points: Some(sem_points),
            open_errors: 0, // filled from the live queue
        },
        histogram: ScoreHistogram {
            bin_width: 5.0,
            counts,
            pass_mark: 60.0,
            mean,
            sem: sem_points * 100.0 / 29.0,
        },
        scale_label: "Saudi scale (CUA 1444/9/1) · GPA basis 5.00".into(),
        grades,
        statuses: vec![
            StatusCount {
                code: "DN".into(),
                label: "Denied entry".into(),
                label_ar: "محروم".into(),
                count: 2,
            },
            StatusCount {
                code: "W".into(),
                label: "Withdrawn".into(),
                label_ar: "منسحب".into(),
                count: 3,
            },
            StatusCount {
                code: "IC".into(),
                label: "Incomplete".into(),
                label_ar: "غير مكتمل".into(),
                count: 1,
            },
            StatusCount {
                code: "IP".into(),
                label: "In progress".into(),
                label_ar: "مستمر".into(),
                count: 0,
            },
        ],
        items,
        option_labels: letters(5),
        readiness: Vec::new(), // filled from the live queue
        release_blocked: None,
        blackboard_blocked: Some(
            "Demo data: nothing is written. Open a graded exam with a supplied key and a roster \
             to write the Blackboard upload"
                .into(),
        ),
    };

    // outcomes: attainment per CLO from the same cohort
    let target = 0.70;
    let tau = 0.60;
    let specs: [(f64, u32, &str); 5] = [
        (0.74, 151, "Q01–06 · 6 items · Σw 5.5"),
        (0.78, 160, "Q05, Q07–12 · 7 items · Σw 6.5"),
        (0.69, 134, "Q13–19 · 7 items · Σw 6.6"),
        (0.62, 108, "Q18, Q20–26 · 8 items · Σw 7.2"),
        (0.75, 147, "Q27–28 · 2 items · thin evidence (SCO-014)"),
    ];
    let clos: Vec<CloAttainment> = setup
        .clos
        .iter()
        .zip(specs.iter())
        .map(|(c, &(mu, x, note))| {
            let pi = x as f64 / n_students as f64;
            let ci = wilson(x, n_students);
            let half = 1.973 * 0.16 / (n_students as f64).sqrt();
            CloAttainment {
                code: c.code.clone(),
                title: c.title.clone(),
                title_ar: c.title_ar.clone(),
                domain: c.domain,
                target,
                average: mu,
                average_ci: (mu - half, mu + half),
                proportion: pi,
                proportion_ci: ci,
                n: n_students,
                verdict: if pi >= target {
                    Verdict::Attained
                } else {
                    Verdict::NotAttained
                },
                ci_contains_target: ci.0 <= target && target <= ci.1,
                mapping_note: note.into(),
            }
        })
        .collect();
    let methods = [
        "Midterm 1 Q1–6",
        "Midterm 1 Q5, Q7–12",
        "Midterm 1 Q13–19",
        "Midterm 1 Q18, Q20–26",
        "Midterm 1 Q27–28",
    ];
    let mut per_domain: HashMap<NqfDomain, u32> = HashMap::new();
    let tp154_b1 = clos
        .iter()
        .zip(setup.clos.iter())
        .zip(methods.iter())
        .map(|((a, c), m)| {
            let k = per_domain.entry(c.domain).or_insert(0);
            *k += 1;
            let d = match c.domain {
                NqfDomain::Knowledge => 1,
                NqfDomain::Skills => 2,
                NqfDomain::Values => 3,
            };
            Tp154Row {
                domain: c.domain,
                number: format!("{d}.{k}"),
                clo: c.title.clone(),
                plo: c.plo.clone(),
                methods: (*m).into(),
                targeted: format!("{:.0} % of students ≥ {:.0} %", target * 100.0, tau * 100.0),
                actual: format!(
                    "π {:.0} % · μ {:.0} %",
                    a.proportion * 100.0,
                    a.average * 100.0
                ),
                comment: match a.verdict {
                    Verdict::Attained if a.ci_contains_target => {
                        "Attained; within the 95 % CI of the target".into()
                    }
                    Verdict::Attained if c.code == "CLO5" => {
                        "Attained; thin evidence (2 items)".into()
                    }
                    Verdict::Attained => "Attained".into(),
                    Verdict::NotAttained => "Not attained: B.2 recommendation required".into(),
                    Verdict::InsufficientEvidence => "Insufficient evidence".into(),
                },
            }
        })
        .collect();
    let outcomes = Outcomes {
        exam: ExamId(CS211.into()),
        policy: PolicyView {
            class_method: ClassMethod::ThresholdProportion,
            tau,
            target,
            verdict_basis: "point".into(),
            mapping_basis: "approved mappings only".into(),
            version: "Policy v3 · sha384 9a1c…07e2".into(),
        },
        clos,
        tp154_b1,
        classification: "Restricted · مقيد".into(),
    };
    (results, outcomes)
}

// ------------------------------------------------------------------------------------ trait impl

impl Backend for DemoBackend {
    fn is_demo(&self) -> bool {
        true
    }

    fn release(&mut self, exam: &ExamId) -> Result<String, BackendError> {
        let _ = exam;
        Err(BackendError::Denied(
            "Demo data: nothing is released. Open your exams with omr-desktop --workspace FOLDER"
                .into(),
        ))
    }

    fn dashboard(&self) -> Dashboard {
        let exams: Vec<ExamSummary> = self
            .exams
            .iter()
            .map(|e| {
                let mut s = e.summary.clone();
                s.open_exceptions = e
                    .static_exceptions
                    .unwrap_or_else(|| self.open_counts(&s.id));
                if s.id.0 == MATH101 {
                    s.sheets_scanned = (self.live_pages.round() as u32).min(s.sheets_expected);
                }
                if let Some(setup) = &e.setup {
                    s.mapped_items = mapped_items(setup);
                    s.key_status = setup.key_status.clone();
                }
                s
            })
            .collect();
        let open: u32 = exams.iter().map(|e| e.open_exceptions.total()).sum();
        let errors: u32 = exams.iter().map(|e| e.open_exceptions.blocking()).sum();
        Dashboard {
            user: UserInfo {
                name: "Dr. Huda Al-Qahtani".into(),
                role: "Instructor".into(),
                initials: "HA".into(),
            },
            term: term_now(),
            stats: DashboardStats {
                sheets_this_week: 3_812,
                sheets_today: 412,
                open_exceptions: open,
                open_errors: errors,
                median_review_secs: Some(1.4),
                throughput_pages_per_sec: Some(62.0),
            },
            exams,
            engine: "omr 0.1.0 · manifest 7f3a…b21c".into(),
            workspace: "~/OMR/1448-1/workspace.omr (demo data, synthetic)".into(),
            data_note: "Demo data is synthetic: invented names, IDs starting with 9, and \
                        procedurally drawn scans. No real student data is used."
                .into(),
        }
    }

    fn default_exam(&self) -> Option<ExamId> {
        Some(ExamId(CS211.into()))
    }

    fn exam_setup(&self, exam: &ExamId) -> Option<ExamSetup> {
        let mut s = self.exam(exam)?.setup.clone()?;
        s.lints = setup_lints(&s);
        Some(s)
    }

    fn set_key(
        &mut self,
        exam: &ExamId,
        version: usize,
        question: u32,
        answer: &str,
    ) -> Result<(), BackendError> {
        let e = self
            .exam_mut(exam)
            .ok_or_else(|| BackendError::NotFound(format!("No exam {exam}")))?;
        let options = e.options.clone();
        let setup = e
            .setup
            .as_mut()
            .ok_or_else(|| BackendError::NotFound("This exam has no setup yet".into()))?;
        let key = setup
            .keys
            .get_mut(version)
            .ok_or_else(|| BackendError::NotFound(format!("No version {}", version + 1)))?;
        let slot = key
            .answers
            .get_mut(question as usize - 1)
            .ok_or_else(|| BackendError::NotFound(format!("No question {question}")))?;
        let answer = answer.trim().to_uppercase();
        if !answer.is_empty()
            && !answer
                .split(',')
                .all(|a| options.iter().any(|o| o == a.trim()))
        {
            return Err(BackendError::Invalid(format!(
                "Key must be one of {}",
                options.join(", ")
            )));
        }
        if *slot != answer {
            *slot = answer;
            // a change to a published key starts a new draft version (§10.8)
            if setup.key_published {
                setup.key_published = false;
                setup.key_status = "Key v3 · draft".into();
            }
        }
        Ok(())
    }

    fn set_mapping_weight(
        &mut self,
        exam: &ExamId,
        question: u32,
        clo: usize,
        weight: Option<f64>,
    ) -> Result<(), BackendError> {
        if let Some(w) = weight
            && !(w > 0.0 && w <= 1.0)
        {
            return Err(BackendError::Invalid(
                "Weight must be in (0, 1]; leave the cell empty for no mapping".into(),
            ));
        }
        let setup = self
            .exam_mut(exam)
            .and_then(|e| e.setup.as_mut())
            .ok_or_else(|| BackendError::NotFound(format!("No setup for {exam}")))?;
        let row = setup
            .mapping
            .iter_mut()
            .find(|r| r.question == question)
            .ok_or_else(|| BackendError::NotFound(format!("No question {question}")))?;
        let cell = row
            .cells
            .get_mut(clo)
            .ok_or_else(|| BackendError::NotFound(format!("No CLO {}", clo + 1)))?;
        if cell.weight != weight {
            cell.weight = weight;
            row.status = MappingStatus::Draft;
        }
        Ok(())
    }

    fn set_mapping_note(
        &mut self,
        exam: &ExamId,
        question: u32,
        clo: usize,
        note: &str,
    ) -> Result<(), BackendError> {
        let setup = self
            .exam_mut(exam)
            .and_then(|e| e.setup.as_mut())
            .ok_or_else(|| BackendError::NotFound(format!("No setup for {exam}")))?;
        let cell = setup
            .mapping
            .iter_mut()
            .find(|r| r.question == question)
            .and_then(|r| r.cells.get_mut(clo))
            .ok_or_else(|| {
                BackendError::NotFound(format!("No cell Q{question} × CLO{}", clo + 1))
            })?;
        cell.note = note.to_string();
        Ok(())
    }

    fn batch_progress(&self, exam: &ExamId) -> Option<BatchProgress> {
        match exam.0.as_str() {
            MATH101 => Some(self.math_batch()),
            CS211 => Some(self.cs_batch()),
            _ => None,
        }
    }

    fn review_queue(&self, exam: &ExamId, lane: Lane) -> ReviewQueue {
        let lanes = Lane::ALL
            .iter()
            .map(|&l| LaneCount {
                lane: l,
                open: self.lane_items(exam, l).len() as u32,
            })
            .collect();
        let mut groups: Vec<ExceptionGroup> = Vec::new();
        for item in self.lane_items(exam, lane) {
            let q = QueueItem {
                id: item.id,
                label: item.label.clone(),
                detail: item.detail.clone(),
                question: item.question,
                form: item.form.clone(),
                serial: item.serial.clone(),
                severity: item.severity,
                fills: if item.kind == ReviewKind::Mark {
                    item.fills.clone()
                } else {
                    Vec::new()
                },
                kind: item.kind,
                deferred: matches!(item.status, Status::Deferred(_)),
            };
            match groups.last_mut() {
                Some(g) if g.code == item.code => g.items.push(q),
                _ => groups.push(ExceptionGroup {
                    code: item.code.clone(),
                    title: item.code_title.clone(),
                    severity: item.severity,
                    items: vec![q],
                }),
            }
        }
        let all: Vec<&DemoItem> = self.items.iter().filter(|i| &i.exam == exam).collect();
        let base = self.exam(exam).map_or(0, |e| e.resolved_base);
        let closed = all.iter().filter(|i| !i.status.is_open()).count() as u32;
        ReviewQueue {
            lane,
            lanes,
            groups,
            resolved: base + closed,
            total: base + all.len() as u32,
            median_secs_per_item: Some(1.4),
            audit_head: format!("{:04x}…b21c", 0x7f3a + self.seq as u32),
            two_person_rule: true,
            unavailable: None,
        }
    }

    fn review_item(&self, exam: &ExamId, id: ItemId) -> Option<ReviewItem> {
        let item = self.items.iter().find(|i| i.id == id && &i.exam == exam)?;
        let (crop, original, mut bubbles) = self.render_item(item);
        let blind = item.kind == ReviewKind::BlindVerify;
        if blind {
            // the engine's reading never leaves the core for a blind item (§13.4, §15.3):
            // geometry only, no fill and no state
            for b in &mut bubbles {
                b.fill = 0.0;
                b.state = MarkState::Blank;
            }
        }
        let other_fills = if item.kind == ReviewKind::Mark {
            Self::other_fills(item)
        } else {
            Vec::new()
        };
        let mut sorted = other_fills.clone();
        sorted.sort_by(f32::total_cmp);
        let typical = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0);
        let summary = self.summary_text(item, &bubbles);
        Some(ReviewItem {
            id: item.id,
            code: item.code.clone(),
            code_title: item.code_title.clone(),
            sub_reason: item.sub_reason.clone(),
            severity: item.severity,
            kind: item.kind,
            question: item.question,
            form: item.form.clone(),
            serial: item.serial.clone(),
            page: 1,
            dpi: if item.exam.0 == MATH101 { 300 } else { 200 },
            crop,
            original,
            bubbles,
            threshold: 0.45,
            band: (0.35, 0.55),
            suggestion: if blind { None } else { item.suggestion.clone() },
            other_fills,
            typical_full_fill: typical,
            answered: 27,
            summary,
            group: self.group_confirm(item),
            current: item.decision,
            identity: item.identity.clone(),
            notice: item.notice.clone(),
            reasons: Vec::new(),
            reasons_needing_note: Vec::new(),
        })
    }

    fn apply(
        &mut self,
        exam: &ExamId,
        id: ItemId,
        action: ReviewAction,
    ) -> Result<ApplyOutcome, BackendError> {
        let idx = self
            .items
            .iter()
            .position(|i| i.id == id && &i.exam == exam)
            .ok_or_else(|| BackendError::NotFound(format!("No {id} in {exam}")))?;
        if !self.items[idx].status.is_open() {
            return Err(BackendError::Denied(format!("{id} is already closed")));
        }
        let lane = self.items[idx].lane;
        let order_before: Vec<ItemId> = self.lane_items(exam, lane).iter().map(|i| i.id).collect();
        let mut affected = vec![id];
        let mut prior = vec![(id, self.items[idx].status, self.items[idx].decision)];
        match &action {
            ReviewAction::Resolve { decision, .. } => {
                if !self.valid_decision(&self.items[idx], *decision) {
                    return Err(BackendError::Invalid(format!(
                        "{decision:?} does not apply to this item"
                    )));
                }
                self.items[idx].status = Status::Resolved;
                self.items[idx].decision = Some(*decision);
            }
            ReviewAction::ApplyToGroup { decision } => {
                let confirm = self.group_confirm(&self.items[idx]);
                if !confirm.allowed {
                    return Err(BackendError::Denied(confirm.reason));
                }
                if !self.valid_decision(&self.items[idx], *decision) {
                    return Err(BackendError::Invalid(format!(
                        "{decision:?} does not apply to this item"
                    )));
                }
                let key = self.items[idx].group_key.clone();
                for it in self.items.iter_mut().filter(|i| {
                    &i.exam == exam && i.group_key == key && i.status.is_open() && i.id != id
                }) {
                    prior.push((it.id, it.status, it.decision));
                    affected.push(it.id);
                }
                for it in self.items.iter_mut().filter(|i| affected.contains(&i.id)) {
                    it.status = Status::Resolved;
                    it.decision = Some(*decision);
                }
            }
            ReviewAction::SetStudent { .. } => {
                return Err(BackendError::Denied(
                    "The demo has no registrar: choose a listed candidate (1–3) or N (not on \
                     roster)"
                        .into(),
                ));
            }
            // `record` below assigns seq + 1 to this event: deferred items sort by it
            ReviewAction::Defer => self.items[idx].status = Status::Deferred(self.seq + 1),
            ReviewAction::Escalate => self.items[idx].status = Status::Escalated,
            ReviewAction::AddToRescan => self.items[idx].status = Status::Rescan,
        }
        self.record(id, action, prior);
        let order_after: Vec<ItemId> = self.lane_items(exam, lane).iter().map(|i| i.id).collect();
        let pos = order_before.iter().position(|&x| x == id).unwrap_or(0);
        let next = order_before[pos + 1..]
            .iter()
            .copied()
            .find(|x| order_after.contains(x) && *x != id)
            .or_else(|| order_after.iter().copied().find(|&x| x != id))
            .or_else(|| order_after.first().copied());
        Ok(ApplyOutcome { affected, next })
    }

    fn undo(&mut self, exam: &ExamId) -> Result<Option<ItemId>, BackendError> {
        let Some(pos) = self.history.iter().rposition(|h| {
            !h.record.undone
                && self
                    .items
                    .iter()
                    .any(|i| i.id == h.record.item && &i.exam == exam)
        }) else {
            return Ok(None);
        };
        let entry = self.history[pos].clone();
        for (id, status, decision) in &entry.prior {
            if let Some(it) = self.items.iter_mut().find(|i| i.id == *id) {
                it.status = *status;
                it.decision = *decision;
            }
        }
        self.history[pos].record.undone = true;
        self.seq += 1;
        Ok(Some(entry.record.item))
    }

    fn review_history(&self, exam: &ExamId) -> Vec<DecisionRecord> {
        self.history
            .iter()
            .filter(|h| {
                self.items
                    .iter()
                    .any(|i| i.id == h.record.item && &i.exam == exam)
            })
            .map(|h| h.record.clone())
            .collect()
    }

    fn results(&self, exam: &ExamId) -> Option<Results> {
        let mut r = self.exam(exam)?.results.clone()?;
        let counts = self.open_counts(exam);
        r.stats.open_errors = counts.blocking();
        let lane_err = |lane: Lane| {
            self.lane_items(exam, lane)
                .iter()
                .filter(|i| i.severity >= Severity::Error && i.kind != ReviewKind::BlindVerify)
                .count()
        };
        let (m, i, c, s) = (
            lane_err(Lane::Marks),
            lane_err(Lane::Identity),
            lane_err(Lane::Capture),
            lane_err(Lane::Scoring),
        );
        let blocking = counts.blocking();
        r.readiness = vec![
            ReadinessCheck {
                tone: if blocking > 0 {
                    Tone::Error
                } else {
                    Tone::Success
                },
                text: if blocking > 0 {
                    format!(
                        "{blocking} ERROR exceptions open (Marks {m}, Identity {i}, Capture {c}, Scoring {s})"
                    )
                } else {
                    "No ERROR or BLOCKER exceptions open".into()
                },
                action: if blocking > 0 {
                    "Open".into()
                } else {
                    String::new()
                },
                target: (blocking > 0).then_some(CheckTarget::Review(None)),
            },
            // SCO-001 is ERROR in Appendix A: the check carries the error glyph
            ReadinessCheck {
                tone: Tone::Error,
                text: "1 probable mis-key flagged (Q12, SCO-001)".into(),
                action: "Review".into(),
                target: Some(CheckTarget::Review(Some(Lane::Scoring))),
            },
            ReadinessCheck {
                tone: Tone::Success,
                text: "Blind verify 2 % · 0 disagreements".into(),
                action: String::new(),
                target: None,
            },
            ReadinessCheck {
                tone: Tone::Error,
                text: "Batch count: 4 serials missing (COL-005)".into(),
                action: "Reconcile".into(),
                target: Some(CheckTarget::Scan),
            },
            ReadinessCheck {
                tone: Tone::Info,
                text: "Two-person rule on · co-signer: Dr. Reem Al-Harbi".into(),
                action: String::new(),
                target: None,
            },
        ];
        r.release_blocked = (blocking > 0).then(|| format!("{blocking} ERROR open"));
        Some(r)
    }

    fn outcomes(&self, exam: &ExamId) -> Option<Outcomes> {
        self.exam(exam)?.outcomes.clone()
    }

    fn pause_batch(&mut self, exam: &ExamId, paused: bool) -> Result<(), BackendError> {
        if exam.0 != MATH101 {
            return Err(BackendError::Denied(format!("{exam} has no running batch")));
        }
        self.set_live(!paused);
        Ok(())
    }

    fn pump(&mut self) -> bool {
        if !self.live_running || self.live_pages >= 1672.0 {
            self.last_pump = None;
            return false;
        }
        let now = Instant::now();
        let changed = if let Some(last) = self.last_pump {
            let dt = now.duration_since(last).as_secs_f64().min(1.0);
            // slowed down 8× so the live view is watchable
            self.live_pages = (self.live_pages + dt * 62.0 / 8.0).min(1672.0);
            true
        } else {
            false
        };
        self.last_pump = Some(now);
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cs() -> ExamId {
        ExamId(CS211.into())
    }

    #[test]
    fn no_real_ids_only_synthetic_nines() {
        let b = DemoBackend::new();
        for item in &b.items {
            if let Some(ev) = &item.identity {
                for c in &ev.candidates {
                    assert_eq!(c.student_id.len(), 10);
                    assert!(c.student_id.starts_with('9'), "{}", c.student_id);
                    assert!(c.national_id_masked.starts_with("••••••"));
                }
                assert!(ev.read_id.starts_with('9'));
            }
        }
    }

    #[test]
    fn queue_is_ordered_by_severity_then_code() {
        let b = DemoBackend::new();
        let q = b.review_queue(&cs(), Lane::Marks);
        let keys: Vec<(Severity, String)> = q
            .groups
            .iter()
            .map(|g| (g.severity, g.code.clone()))
            .collect();
        for w in keys.windows(2) {
            assert!(w[0].0 >= w[1].0, "{keys:?}");
            if w[0].0 == w[1].0 && w[0].1 != "VERIFY" && w[1].1 != "VERIFY" {
                assert!(w[0].1 < w[1].1, "{keys:?}");
            }
        }
        assert_eq!(q.groups[0].code, "MRK-001");
        assert_eq!(q.groups[0].items[0].label, "Q17");
        assert_eq!(q.lanes.iter().map(|l| l.open).sum::<u32>(), 30);
    }

    #[test]
    fn resolve_advances_and_undo_reopens() {
        let mut b = DemoBackend::new();
        let q = b.review_queue(&cs(), Lane::Marks);
        let first = q.groups[0].items[0].id;
        let second = q.groups[0].items[1].id;
        let out = b
            .apply(
                &cs(),
                first,
                ReviewAction::Resolve {
                    decision: Decision::Option(1),
                    accepted_suggestion: true,
                    note: String::new(),
                },
            )
            .unwrap();
        assert_eq!(out.next, Some(second));
        assert_eq!(b.review_history(&cs()).len(), 1);
        let q2 = b.review_queue(&cs(), Lane::Marks);
        assert_eq!(q2.resolved, q.resolved + 1);
        assert_eq!(b.undo(&cs()).unwrap(), Some(first));
        assert!(b.review_history(&cs())[0].undone);
        let q3 = b.review_queue(&cs(), Lane::Marks);
        assert_eq!((q3.groups, q3.resolved), (q.groups, q.resolved));
    }

    #[test]
    fn group_confirm_resolves_every_member() {
        let mut b = DemoBackend::new();
        let first = b.review_queue(&cs(), Lane::Marks).groups[0].items[0].id;
        let item = b.review_item(&cs(), first).unwrap();
        assert!(
            item.group.allowed && item.group.size == 3,
            "{:?}",
            item.group
        );
        let out = b
            .apply(
                &cs(),
                first,
                ReviewAction::ApplyToGroup {
                    decision: Decision::Option(1),
                },
            )
            .unwrap();
        assert_eq!(out.affected.len(), 3);
        let smudge = b.items.iter().find(|i| i.code == "MRK-010").unwrap().id;
        let err = b.apply(
            &cs(),
            smudge,
            ReviewAction::ApplyToGroup {
                decision: Decision::Unreadable,
            },
        );
        assert!(matches!(err, Err(BackendError::Denied(_))));
    }

    #[test]
    fn invalid_decisions_are_rejected() {
        let mut b = DemoBackend::new();
        let first = b.review_queue(&cs(), Lane::Marks).groups[0].items[0].id;
        let r = b.apply(
            &cs(),
            first,
            ReviewAction::Resolve {
                decision: Decision::Option(7),
                accepted_suggestion: false,
                note: String::new(),
            },
        );
        assert!(matches!(r, Err(BackendError::Invalid(_))));
        let r = b.apply(
            &cs(),
            first,
            ReviewAction::Resolve {
                decision: Decision::Candidate(0),
                accepted_suggestion: false,
                note: String::new(),
            },
        );
        assert!(matches!(r, Err(BackendError::Invalid(_))));
    }

    #[test]
    fn blind_items_hide_the_engine_reading() {
        let b = DemoBackend::new();
        let q = b.review_queue(&cs(), Lane::Marks);
        let verify = q.groups.iter().find(|g| g.code == "VERIFY").unwrap();
        assert!(verify.items[0].fills.is_empty());
        let item = b.review_item(&cs(), verify.items[0].id).unwrap();
        assert!(item.suggestion.is_none());
        assert!(item.other_fills.is_empty());
        // no per-bubble reading either: geometry only
        assert!(!item.bubbles.is_empty());
        assert!(
            item.bubbles
                .iter()
                .all(|b| b.fill == 0.0 && b.state == MarkState::Blank)
        );
        assert!(!item.summary.contains("filled"));
    }

    #[test]
    fn setup_lints_use_appendix_a_codes() {
        let b = DemoBackend::new();
        let s = b.exam_setup(&cs()).unwrap();
        for l in &s.lints {
            assert!(
                l.code.len() == 7 && l.code.starts_with("SCO-"),
                "lint code {:?} is not an Appendix A code",
                l.code
            );
        }
        assert!(
            s.lints
                .iter()
                .any(|l| l.code == "SCO-013" && l.severity == Severity::Warn)
        );
        assert!(
            s.lints
                .iter()
                .any(|l| l.code == "SCO-011" && l.severity == Severity::Error)
        );
    }

    #[test]
    fn results_are_internally_consistent() {
        let b = DemoBackend::new();
        let r = b.results(&cs()).unwrap();
        let hist: u32 = r.histogram.counts.iter().sum();
        let grades: u32 = r.grades.iter().map(|g| g.count).sum();
        assert_eq!(hist, r.stats.students);
        assert_eq!(grades, r.stats.students);
        assert_eq!(r.items.len(), 29);
        for it in &r.items {
            let s: f64 = it.option_shares.iter().sum();
            assert!((s - 0.99).abs() < 0.02, "Q{} shares {s}", it.question);
            let (r, ci) = (it.r_ir.unwrap(), it.r_ci.unwrap());
            assert!(ci.0 <= r && r <= ci.1);
        }
        assert!(r.release_blocked.is_some());
        let o = b.outcomes(&cs()).unwrap();
        assert_eq!(o.clos.len(), 5);
        assert_eq!(o.tp154_b1.len(), 5);
        let clo4 = &o.clos[3];
        assert_eq!(clo4.verdict, Verdict::NotAttained);
        let clo3 = &o.clos[2];
        assert!(clo3.ci_contains_target);
    }

    #[test]
    fn wilson_matches_the_spec_example() {
        // §11.3 worked example: 18 of 25 -> [52.4 %, 85.7 %]
        let (lo, hi) = wilson(18, 25);
        assert!(
            (lo - 0.524).abs() < 0.001 && (hi - 0.857).abs() < 0.001,
            "{lo} {hi}"
        );
    }

    #[test]
    fn setup_edits_validate_and_lint() {
        let mut b = DemoBackend::new();
        assert!(b.set_mapping_weight(&cs(), 3, 1, Some(1.5)).is_err());
        b.set_mapping_weight(&cs(), 3, 1, Some(0.5)).unwrap();
        let s = b.exam_setup(&cs()).unwrap();
        assert_eq!(s.mapping[2].cells[1].weight, Some(0.5));
        assert_eq!(s.mapping[2].status, MappingStatus::Draft);
        assert!(
            s.lints
                .iter()
                .any(|l| l.severity == Severity::Error && l.message.contains("Q03"))
        );
        b.set_mapping_note(&cs(), 3, 1, "ملاحظة").unwrap();
        assert_eq!(
            b.exam_setup(&cs()).unwrap().mapping[2].cells[1].note,
            "ملاحظة"
        );
        assert!(b.set_key(&cs(), 0, 1, "Z").is_err());
        b.set_key(&cs(), 0, 1, "e").unwrap();
        let s = b.exam_setup(&cs()).unwrap();
        assert_eq!(s.keys[0].answers[0], "E");
        assert!(!s.key_published);
    }

    #[test]
    fn live_batch_advances_only_when_pumped() {
        let mut b = DemoBackend::new();
        let math = ExamId(MATH101.into());
        let p0 = b.batch_progress(&math).unwrap();
        assert_eq!(p0.pages_done, 1284);
        assert_eq!(p0.thumbnails.len(), 30);
        assert_eq!(p0.pages_done + p0.in_flight + p0.queued, p0.pages_total);
        b.live_pages += 100.0;
        let p1 = b.batch_progress(&math).unwrap();
        assert_eq!(p1.pages_done, 1384);
    }
}
