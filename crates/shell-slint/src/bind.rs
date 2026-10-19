//! Binding: copies [`Backend`] view-models into the Slint globals and turns UI callbacks into
//! backend calls. Formatting for display lives here; decisions never do (spec §4.7).

use crate::backend::{
    self as b, Backend, Decision, ExamId, ItemId, Lane, ReviewAction, ReviewKind,
};
use crate::keymap::{self, GlobalKey, KeyContext, ReviewKey};
use crate::ui::{self as s, AppWindow};
use slint::{
    ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel,
};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

// ------------------------------------------------------------------------------------ helpers

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

/// The model to set on a list property for `rows`. Slint destroys and rebuilds every element of
/// a repeater whose model is replaced, and focus, hover and a press in progress go with them; so
/// a property that already holds a `VecModel` keeps it, with only the changed rows set and the
/// tail pushed or removed. Anything else (the empty default) gets a new `VecModel` once.
fn sync<T: Clone + PartialEq + 'static>(current: ModelRc<T>, rows: Vec<T>) -> ModelRc<T> {
    sync_with(current, rows, |_, _| {})
}

/// [`sync`] for rows that hold lists of their own: `adopt(old, new)` gives the new row the old
/// row's inner models (see [`adopt`]), so an unchanged row compares equal (models compare by
/// identity) and the elements inside it stay as well.
fn sync_with<T: Clone + PartialEq + 'static>(
    current: ModelRc<T>,
    rows: Vec<T>,
    adopt: impl Fn(&T, &mut T),
) -> ModelRc<T> {
    let Some(vm) = current.as_any().downcast_ref::<VecModel<T>>() else {
        return model(rows);
    };
    let n = rows.len();
    for (i, mut row) in rows.into_iter().enumerate() {
        match vm.row_data(i) {
            Some(old) => {
                adopt(&old, &mut row);
                if old != row {
                    vm.set_row_data(i, row);
                }
            }
            None => vm.push(row),
        }
    }
    while vm.row_count() > n {
        vm.remove(vm.row_count() - 1);
    }
    current
}

/// Syncs a new row's inner list into the old row's model and puts that model in the new row.
fn adopt<T: Clone + PartialEq + 'static>(old: &ModelRc<T>, new: &mut ModelRc<T>) {
    *new = sync(old.clone(), new.iter().collect());
}

fn ss(s: impl AsRef<str>) -> SharedString {
    SharedString::from(s.as_ref())
}

/// "1,284".
pub fn thousands(n: u32) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Signed decimal with a true minus sign (U+2212), as in typeset tables.
fn signed(x: f64, places: usize) -> String {
    let t = format!("{:.*}", places, x.abs());
    if x < 0.0 && t.chars().any(|c| c.is_ascii_digit() && c != '0') {
        format!("\u{2212}{t}")
    } else {
        t
    }
}

fn pct(x: f64) -> String {
    format!("{:.0} %", x * 100.0)
}

/// Joins the non-empty parts with `sep` (view-models leave unknown parts empty).
fn join(parts: &[&str], sep: &str) -> String {
    parts
        .iter()
        .filter(|p| !p.trim().is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(sep)
}

/// The parallel run's request from the Compare field: Remark's workbooks, and any `.csv`
/// among them as handwritten student numbers (`file,page,student_id`).
fn compare_request(typed: &str) -> b::RemarkRequest {
    let (handwritten, workbooks): (Vec<_>, Vec<_>) = scan_paths(typed).into_iter().partition(|p| {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("csv"))
    });
    b::RemarkRequest {
        workbooks,
        points: b::RemarkPoints::FromWorkbooks,
        handwritten,
    }
}

/// The scan files or folders typed into the Add scans field: separated by `;` or new lines,
/// quotes stripped, `~/` (or `~\` on Windows) for the home folder.
pub fn scan_paths(text: &str) -> Vec<std::path::PathBuf> {
    let home = std::env::home_dir();
    text.split([';', '\n'])
        .map(|p| p.trim().trim_matches(|c| c == '"' || c == '\''))
        .filter(|p| !p.is_empty())
        .map(|p| {
            let rest = p
                .strip_prefix('~')
                .and_then(|r| r.strip_prefix(std::path::is_separator));
            (p, rest)
        })
        .map(|(p, rest)| match (rest, &home) {
            (Some(rest), Some(h)) => h.join(rest),
            _ => std::path::PathBuf::from(p),
        })
        .collect()
}

/// A blank sheet for a thumbnail whose page image is not available (never an invented scan).
/// True when 1-based question `q` is in `ranges` (`1-8,11,13-14`).
fn in_ranges(ranges: &str, q: usize) -> bool {
    ranges.split(',').any(|r| {
        let mut ends = r.split('-').map(|x| x.trim().parse::<usize>().ok());
        match (ends.next().flatten(), ends.next()) {
            (Some(a), None) => a == q,
            (Some(a), Some(Some(z))) => (a..=z).contains(&q),
            _ => false,
        }
    })
}

fn blank_sheet() -> Image {
    let mut px = SharedPixelBuffer::<Rgba8Pixel>::new(1, 1);
    px.make_mut_slice()[0] = Rgba8Pixel {
        r: 0xF4,
        g: 0xF2,
        b: 0xEC,
        a: 0xFF,
    };
    Image::from_rgba8(px)
}

fn tone(t: b::Tone) -> s::Tone {
    match t {
        b::Tone::Neutral => s::Tone::Neutral,
        b::Tone::Primary => s::Tone::Primary,
        b::Tone::Accent => s::Tone::Accent,
        b::Tone::Success => s::Tone::Success,
        b::Tone::Warning => s::Tone::Warning,
        b::Tone::Error => s::Tone::Error,
        b::Tone::Info => s::Tone::Info,
    }
}

fn severity(v: b::Severity) -> s::Severity {
    match v {
        b::Severity::Info => s::Severity::Info,
        b::Severity::Warn => s::Severity::Warn,
        b::Severity::Error => s::Severity::Error,
        b::Severity::Blocker => s::Severity::Blocker,
    }
}

fn mark(m: b::MarkState) -> s::MarkState {
    match m {
        b::MarkState::Filled => s::MarkState::Selected,
        b::MarkState::Ambiguous => s::MarkState::Ambiguous,
        b::MarkState::Unreadable => s::MarkState::Unreadable,
        b::MarkState::Blank => s::MarkState::Blank,
    }
}

fn status_tone(st: b::ExamStatus) -> s::Tone {
    match st {
        b::ExamStatus::Setup => s::Tone::Neutral,
        b::ExamStatus::Scanning => s::Tone::Info,
        b::ExamStatus::InReview => s::Tone::Warning,
        b::ExamStatus::Ready | b::ExamStatus::Released => s::Tone::Success,
    }
}

/// Converts a backend crop to a Slint image (one copy; the shell never keeps scans on disk).
/// A malformed crop (byte count not `width × height × 4`) shows as a neutral grey tile instead
/// of panicking inside Slint.
pub fn image(c: &b::CropImage) -> Image {
    let expected = (c.width as usize)
        .checked_mul(c.height as usize)
        .and_then(|n| n.checked_mul(4));
    if c.width == 0 || c.height == 0 || expected != Some(c.rgba.len()) {
        let mut grey = SharedPixelBuffer::<Rgba8Pixel>::new(1, 1);
        grey.make_mut_slice()[0] = Rgba8Pixel {
            r: 0xC9,
            g: 0xC9,
            b: 0xC9,
            a: 0xFF,
        };
        return Image::from_rgba8(grey);
    }
    Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
        &c.rgba, c.width, c.height,
    ))
}

/// The matrix selection and the selected cell's editor (title, CLO, weight, note).
fn show_selected_cell(su: &s::SetupVM, setup: &b::ExamSetup, row: usize, col: usize) {
    su.set_sel_row(row as i32);
    su.set_sel_col(col as i32);
    if let (Some(r), Some(c)) = (setup.mapping.get(row), setup.clos.get(col)) {
        let cell = r.cells.get(col);
        su.set_sel_title(ss(format!("Q{:02} × {}", r.question, c.code)));
        su.set_sel_clo(ss(&c.title));
        su.set_sel_weight(ss(fmt_weight(cell.and_then(|c| c.weight))));
        su.set_sel_note(ss(cell.map_or("", |c| c.note.as_str())));
    }
}

/// Parses a weight typed by the user: "" clears; "0.5", "0,5", "٠٫٥" and "½" are accepted.
pub fn parse_weight(text: &str) -> Result<Option<f64>, String> {
    let t: String = text
        .trim()
        .chars()
        .map(|c| match c {
            '\u{0660}'..='\u{0669}' => char::from_u32(c as u32 - 0x0660 + '0' as u32).unwrap_or(c),
            '\u{06F0}'..='\u{06F9}' => char::from_u32(c as u32 - 0x06F0 + '0' as u32).unwrap_or(c),
            '٫' | ',' => '.',
            _ => c,
        })
        .collect();
    match t.as_str() {
        "" | "-" | "–" => Ok(None),
        "½" => Ok(Some(0.5)),
        _ => match t.parse::<f64>() {
            // "NaN", "inf" and friends parse as f64 but are not weights
            Ok(w) if w.is_finite() => Ok(Some(w)),
            _ => Err(format!(
                "\"{text}\" is not a number; type a weight such as 0.5"
            )),
        },
    }
}

fn fmt_weight(w: Option<f64>) -> String {
    match w {
        None => String::new(),
        Some(w) if (w - w.round()).abs() < 1e-9 => format!("{w:.0}"),
        Some(w) => {
            let t = format!("{w:.2}");
            t.trim_end_matches('0').to_string()
        }
    }
}

fn decision_word(d: Decision, labels: &[String]) -> String {
    match d {
        Decision::Option(i) => labels
            .get(i as usize)
            .cloned()
            .unwrap_or_else(|| ((b'A' + i) as char).to_string()),
        Decision::Blank => "Blank".into(),
        Decision::Multi => "Multi".into(),
        Decision::Unreadable => "Unreadable".into(),
        Decision::Candidate(i) => format!("candidate {}", i + 1),
        Decision::NotOnRoster => "not on roster".into(),
        Decision::Duplicate => "duplicate".into(),
        Decision::Acknowledge => "acknowledge".into(),
    }
}

/// Reason codes shown before "more reasons" (the family's first ones; OTHER always follows).
const FIRST_REASONS: usize = 4;
/// The last reason chip: shows every code on offer, or the first ones again.
const MORE_REASONS: &str = "\u{0}more";

/// "AMBIGUOUS_MARK" → "Ambiguous mark" (reason codes as words on the chips).
fn reason_words(code: &str) -> String {
    let lower = code.replace('_', " ").to_lowercase();
    let mut chars = lower.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn help_for(code: &str) -> &'static str {
    match code {
        "MRK-001" => {
            "MRK-001 Low-confidence mark: the bubble's darkness falls in the hysteresis band or the engine's stages disagree. Decide what is marked from the paper, not from the key (no key is shown)."
        }
        "MRK-002" => {
            "MRK-002 Multiple marks: two or more bubbles read as filled. Choose Multi unless one mark is clearly an erasure or a stray."
        }
        "MRK-003" => {
            "MRK-003 Eraser residue: an erased mark next to a firm one (cancel mode erase_only). The firm mark usually stands; residue alone is not an answer."
        }
        "MRK-005" => {
            "MRK-005 Tick, X or circled letter instead of a fill: in fill-only mode a tick is reviewed; record the option it clearly designates, or Unreadable."
        }
        "MRK-010" => {
            "MRK-010 Stain, smudge or crease: the decision flips under perturbation. If the mark cannot be read with confidence, choose Unreadable (rescan); never Blank."
        }
        "REG-003" => {
            "REG-003 High residual (fold, curl): the row may be displaced. Check the overlays sit on the printed bubbles before deciding; F2 shows the original."
        }
        "CAP-006" => {
            "CAP-006 Scanner streaks: dirty glass masks a column. If more than 30 % of a bubble is masked it is Unreadable; otherwise decide as usual."
        }
        "VERIFY" => {
            "Blind verify sample (§13.4): an auto-accepted question drawn at random. Enter what is marked; the engine's reading is withheld so your answer is an independent label."
        }
        "IDN-004" => {
            "IDN-004 Identity unresolved: compare the grid, the write-in boxes, the name and the seat map. Keys 1–3 choose a candidate, N = not on roster, D = duplicate."
        }
        "IDN-005" => {
            "IDN-005 Two booklets resolve to one student: open both and decide which binding is wrong. Neutral wording: never assume misconduct."
        }
        "IDN-007" => {
            "IDN-007 Provisional student: results stay provisional until the registrar matches the record; acknowledging keeps them so."
        }
        "CAP-002" => {
            "CAP-002 Effective DPI too low: below 100 dpi the page is not graded. Acknowledge to put it on the rescan pull list."
        }
        "COL-005" => {
            "COL-005 Batch count mismatch: the header sheet's expected count differs from the sheets scanned. Reconcile against the absent list or rescan."
        }
        "SCO-001" => {
            "SCO-001 Probable mis-key: the key has negative discrimination while a distractor is positive. Use Results to preview a key change (what-if) before deciding."
        }
        "SCO-002" => {
            "SCO-002 Possibly flawed item: the top group prefers a distractor. Review the item text; void or re-key from Results."
        }
        _ => "Help for this code is in Appendix A of the specification.",
    }
}

// ------------------------------------------------------------------------------------ state

struct State {
    exam: Option<ExamId>,
    exam_rows: Vec<ExamId>,
    exam_filter: usize,
    drawer_open: bool,
    lane: Lane,
    /// Lanes as the backend lists them (the role may see only some), indexed like the chips.
    lanes: Vec<Lane>,
    /// Item handles of the queue rows and the contact sheet: the UI passes row indices, never
    /// raw `ItemId`s (a u64 does not survive Slint's i32).
    queue_ids: Vec<ItemId>,
    contact_ids: Vec<ItemId>,
    item: Option<ItemId>,
    current: Option<b::ReviewItem>,
    /// Ordered open items of the lane (for next / previous).
    order: Vec<ItemId>,
    pending: Option<Decision>,
    /// The override reason chosen for the pending decision, and the codes on offer (§13.4).
    reason: Option<String>,
    reason_codes: Vec<String>,
    /// Every reason code on offer, not only the family's first ones.
    reasons_all: bool,
    /// Identity items: a student number the reviewer typed, pending like a chosen answer.
    typed_student: Option<String>,
    collapsed: BTreeSet<String>,
    overlay: bool,
    show_all: bool,
    registered: bool,
    zoom: i32,
    help: bool,
    single_key: bool,
    sel_row: usize,
    sel_col: usize,
    search_request: i32,
    /// Key from key sheets: the version shown (index into the summary's versions) and the
    /// instructor's decisions on its rows, taken here and sent with the confirmation
    /// (form, 1-based question) → `B`, `A|C` or `*`.
    ks_version: usize,
    ks_picks: BTreeMap<(usize, u32), String>,
    /// The exam whose key summary was last shown on its own (a held exam opens Setup on its
    /// key summary once; the instructor may then go back to the matrix).
    ks_seen: Option<ExamId>,
    /// The Remark import read on the New-exam form (§3.5) and what was asked for it.
    remark_new: Option<b::RemarkImportView>,
    remark_new_request: b::RemarkRequest,
    /// The workbooks the New-exam form uses (after *Use this import*), as the import names them.
    remark_new_using: Option<String>,
    /// The Remark import read in Setup for an exam, and what was asked for it.
    remark_setup: Option<(ExamId, b::RemarkImportView)>,
    remark_setup_request: b::RemarkRequest,
    /// The import's version shown.
    remark_version: usize,
    /// The parallel run of an exam with Remark (§23.7) and what was asked for it.
    parallel: Option<(ExamId, b::ParallelRunView)>,
    parallel_request: b::RemarkRequest,
}

/// The shell controller: owns the backend handle and keeps the UI globals in sync.
pub struct Shell {
    ui: slint::Weak<AppWindow>,
    backend: Rc<RefCell<dyn Backend>>,
    st: RefCell<State>,
    timer: RefCell<Option<slint::Timer>>,
}

impl Shell {
    /// Binds `backend` to `ui`, wires every callback and fills every screen.
    pub fn bind(ui: &AppWindow, backend: Rc<RefCell<dyn Backend>>) -> Rc<Self> {
        let exam = backend.borrow().default_exam();
        let shell = Rc::new(Self {
            ui: ui.as_weak(),
            backend,
            st: RefCell::new(State {
                exam,
                exam_rows: Vec::new(),
                exam_filter: 0,
                drawer_open: true,
                lane: Lane::Marks,
                lanes: Lane::ALL.to_vec(),
                queue_ids: Vec::new(),
                contact_ids: Vec::new(),
                item: None,
                current: None,
                order: Vec::new(),
                pending: None,
                reason: None,
                reason_codes: Vec::new(),
                reasons_all: false,
                typed_student: None,
                collapsed: BTreeSet::new(),
                overlay: true,
                show_all: false,
                registered: true,
                zoom: 3,
                help: false,
                single_key: true,
                sel_row: 17,
                sel_col: 3,
                search_request: 0,
                ks_version: 0,
                ks_picks: BTreeMap::new(),
                ks_seen: None,
                remark_new: None,
                remark_new_request: b::RemarkRequest::default(),
                remark_new_using: None,
                remark_setup: None,
                remark_setup_request: b::RemarkRequest::default(),
                remark_version: 0,
                parallel: None,
                parallel_request: b::RemarkRequest::default(),
            }),
            timer: RefCell::new(None),
        });
        shell.wire(ui);
        shell.refresh_all();
        shell
    }

    fn ui(&self) -> AppWindow {
        self.ui.upgrade().expect("window alive")
    }

    fn exam(&self) -> Option<ExamId> {
        self.st.borrow().exam.clone()
    }

    fn say(&self, text: impl AsRef<str>, t: s::Tone) {
        let ui = self.ui();
        let app = ui.global::<s::App>();
        app.set_message(ss(text));
        app.set_message_tone(t);
    }

    /// Starts the ~2 Hz timer that lets the backend fold in background work (live batch view).
    /// Windowed mode only; headless tests and screenshots stay deterministic without it.
    pub fn start_live_updates(self: &Rc<Self>) {
        let timer = slint::Timer::default();
        let weak = Rc::downgrade(self);
        timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(500),
            move || {
                if let Some(shell) = weak.upgrade() {
                    if shell.ui.upgrade().is_none() {
                        return; // the window is gone; nothing to refresh
                    }
                    shell.pump();
                }
            },
        );
        *self.timer.borrow_mut() = Some(timer);
    }

    /// Lets the backend fold in background work (a grading run, an added batch) and refreshes
    /// what it changed. A reloaded project has new review item IDs, so the review selection of
    /// that exam starts again from the first item. Returns true when anything changed.
    pub fn pump(&self) -> bool {
        let changed = self.backend.borrow_mut().pump();
        let reloaded = self.backend.borrow_mut().take_reloaded();
        if reloaded.is_empty() {
            if changed {
                // twice a second during a run: the screen on show only (the others are
                // refreshed when they are shown)
                self.refresh_dashboard();
                self.refresh_if_shown(s::Page::Scan);
                // flag stability (§12.2) arrives from a background job
                self.refresh_if_shown(s::Page::Results);
            }
            return changed;
        }
        let current = self.exam().is_some_and(|e| reloaded.contains(&e));
        if current {
            let mut st = self.st.borrow_mut();
            st.item = None;
            st.current = None;
            st.pending = None;
            st.reason = None;
        }
        self.refresh_all();
        if current {
            self.restart_review();
        }
        true
    }

    // -------------------------------------------------------------------------- navigation

    /// Shows `page` (and refreshes what it shows).
    pub fn navigate(&self, page: s::Page) {
        let ui = self.ui();
        ui.global::<s::App>().set_page(page);
        self.refresh_remark();
        match page {
            s::Page::Review => {
                if self.st.borrow().item.is_none() {
                    self.refresh_review();
                    let first = self.st.borrow().order.first().copied();
                    self.select_item(first);
                } else {
                    self.refresh_review();
                }
            }
            // the exams table and drawer: a change made on another screen may not have
            // refreshed them
            s::Page::Exams => self.refresh_dashboard(),
            page => self.refresh_page(page),
        }
    }

    /// The screen on show.
    fn shown(&self) -> s::Page {
        self.ui().global::<s::App>().get_page()
    }

    /// Fills the view-model of `page`'s screen from the backend.
    fn refresh_page(&self, page: s::Page) {
        match page {
            s::Page::Setup => self.refresh_setup(),
            s::Page::Scan => self.refresh_scan(),
            s::Page::Review => self.refresh_review(),
            s::Page::Results => self.refresh_results(),
            s::Page::Outcomes => self.refresh_outcomes(),
            s::Page::Exams | s::Page::Settings => {}
        }
    }

    /// Refreshes `page` when it is on show. A screen off show needs nothing: `navigate`
    /// refreshes the screen it shows (Exams: the dashboard), so what changed meanwhile is read
    /// then. The dashboard and the chrome are on every screen: changes refresh them directly.
    fn refresh_if_shown(&self, page: s::Page) {
        if self.shown() == page {
            self.refresh_page(page);
        }
    }

    /// Opens the first item of the current exam's queue: now when Review is on show, else when
    /// it is next shown (`navigate` opens the first item into an empty frame), so its crops are
    /// decoded only for a frame on show.
    fn restart_review(&self) {
        if self.shown() == s::Page::Review {
            let first = self.st.borrow().order.first().copied();
            self.select_item(first);
        } else {
            let mut st = self.st.borrow_mut();
            st.item = None;
            st.current = None;
            st.pending = None;
            st.reason = None;
        }
    }

    /// Makes `exam` the current exam (breadcrumbs, setup, scan, review, results, outcomes).
    pub fn select_exam(&self, exam: &ExamId) {
        {
            let mut st = self.st.borrow_mut();
            if st.exam.as_ref() != Some(exam) {
                st.item = None;
                st.current = None;
                st.pending = None;
                st.lane = Lane::Marks;
                st.sel_row = 0;
                st.sel_col = 0;
                st.ks_version = 0;
                st.ks_picks.clear();
                st.remark_version = 0;
                self.ui().global::<s::SetupVM>().set_ks_open(false);
                let ui = self.ui();
                let rm = ui.global::<s::RemarkVM>();
                rm.set_setup_open(false);
                rm.set_cmp_open(false);
                rm.set_cmp_bar(false);
            }
            st.exam = Some(exam.clone());
        }
        self.refresh_all();
        self.restart_review();
    }

    /// Refreshes what every screen shares (the dashboard and the chrome, the Remark context) and
    /// the screen on show. A first look at an exam is costly on some screens (Scan decodes the
    /// page thumbnails, Outcomes computes the attainment), and nobody waits for a screen that is
    /// not shown: `navigate` refreshes each one when it is shown.
    pub fn refresh_all(&self) {
        self.refresh_dashboard();
        self.refresh_page(self.shown());
        self.refresh_remark();
    }

    /// Refreshes every screen, shown or not: for headless renders and tests that read the
    /// view-model of a screen off show.
    pub fn refresh_all_pages(&self) {
        self.refresh_dashboard();
        self.refresh_setup();
        self.refresh_scan();
        self.refresh_review();
        self.refresh_results();
        self.refresh_outcomes();
        self.refresh_remark();
    }

    // -------------------------------------------------------------------------- wiring

    fn wire(self: &Rc<Self>, ui: &AppWindow) {
        let app = ui.global::<s::App>();
        let me = self.clone();
        app.on_navigate(move |p| me.navigate(p));
        let me = self.clone();
        app.on_key(move |t, ctrl, shift, alt| me.global_key(&t, ctrl, shift, alt));
        let me = self.clone();
        app.on_request(move |what| me.request(&what));
        let me = self.clone();
        app.on_set_single_key(move |v| {
            me.st.borrow_mut().single_key = v;
            me.ui().global::<s::App>().set_single_key(v);
        });
        let weak = ui.as_weak();
        app.on_set_dark(move |v| {
            if let Some(ui) = weak.upgrade() {
                ui.global::<s::Theme>().set_dark(v);
            }
        });
        let weak = ui.as_weak();
        app.on_set_reduce_motion(move |v| {
            if let Some(ui) = weak.upgrade() {
                ui.global::<s::Theme>().set_reduce_motion(v);
            }
        });

        // exams
        let ex = ui.global::<s::ExamsVM>();
        let me = self.clone();
        ex.on_select(move |id| {
            // by ID: the exam pressed, wherever a refresh has moved it since
            let id = me
                .st
                .borrow()
                .exam_rows
                .iter()
                .find(|e| e.0 == id.as_str())
                .cloned();
            if let Some(id) = id {
                me.st.borrow_mut().drawer_open = true;
                me.select_exam(&id);
            }
        });
        let me = self.clone();
        ex.on_move(move |d| me.move_exam(d));
        let me = self.clone();
        ex.on_set_filter(move |i| {
            me.st.borrow_mut().exam_filter = i.max(0) as usize;
            me.refresh_dashboard();
        });
        let me = self.clone();
        ex.on_open(move |p| me.navigate(p));
        let me = self.clone();
        ex.on_close_drawer(move || {
            me.st.borrow_mut().drawer_open = false;
            me.refresh_dashboard();
        });
        let me = self.clone();
        ex.on_create_exam(move || me.create_exam());

        // setup
        let su = ui.global::<s::SetupVM>();
        let me = self.clone();
        su.on_cycle_key(move |row, v| me.cycle_key(row as usize, v as usize));
        let me = self.clone();
        su.on_replace_key(move |path| me.replace_key(&path));
        let me = self.clone();
        su.on_ks_read(move |path| me.read_key_sheets(&path));
        let me = self.clone();
        su.on_ks_select_version(move |i| {
            me.st.borrow_mut().ks_version = i.max(0) as usize;
            me.refresh_keysheets();
        });
        let me = self.clone();
        su.on_ks_choose(move |row, opt| me.key_sheet_choose(row, opt));
        let me = self.clone();
        su.on_ks_confirm(move |all| me.confirm_key_sheets(all));
        let me = self.clone();
        su.on_ks_discard(move || me.discard_key_sheets());
        let me = self.clone();
        su.on_set_key(move |row, v, t| me.set_key(row as usize, v as usize, &t));
        let me = self.clone();
        su.on_set_weight(move |row, c, t| me.set_weight(row as usize, c as usize, &t));
        let me = self.clone();
        su.on_set_note(move |row, c, t| me.set_note(row as usize, c as usize, &t));
        let me = self.clone();
        su.on_approve_mappings(move || me.approve_mappings());
        let me = self.clone();
        su.on_replace_clos(move |path| me.replace_clos(&path));
        let me = self.clone();
        su.on_select_cell(move |row, c| me.select_cell(row.max(0) as usize, c.max(0) as usize));

        // scan
        let sc = ui.global::<s::ScanVM>();
        let me = self.clone();
        sc.on_open_review(move || me.navigate(s::Page::Review));
        let me = self.clone();
        sc.on_open_code(move |code| me.open_code(&code));
        let me = self.clone();
        sc.on_open_sheet(move |serial, code| {
            if code.starts_with("KEY") {
                // the instructor's key sheet: its key summary in Setup
                me.ui().global::<s::SetupVM>().set_ks_open(true);
                me.navigate(s::Page::Setup);
            } else if code.is_empty() {
                me.say(
                    format!(
                        "Sheet {serial}: the page viewer is not built yet; its review items open from the codes on the right"
                    ),
                    s::Tone::Info,
                );
            } else {
                me.open_code(&code);
            }
        });
        let me = self.clone();
        sc.on_toggle_pause(move || me.toggle_pause());
        let me = self.clone();
        sc.on_add_scans(move |text| me.add_scans(&text));

        // review
        let rv = ui.global::<s::ReviewVM>();
        let me = self.clone();
        rv.on_select_lane(move |i| {
            let lane = usize::try_from(i)
                .ok()
                .and_then(|i| me.st.borrow().lanes.get(i).copied());
            if let Some(lane) = lane {
                me.select_lane(lane);
            }
        });
        let me = self.clone();
        rv.on_select_item(move |row| {
            let id = usize::try_from(row)
                .ok()
                .and_then(|r| me.st.borrow().queue_ids.get(r).copied());
            if id.is_some() {
                me.select_item(id);
            }
        });
        let me = self.clone();
        rv.on_select_contact(move |n| {
            let id = usize::try_from(n)
                .ok()
                .and_then(|n| me.st.borrow().contact_ids.get(n).copied());
            if id.is_some() {
                me.select_item(id);
            }
        });
        let me = self.clone();
        rv.on_toggle_group(move |code| {
            {
                let mut st = me.st.borrow_mut();
                let code = code.to_string();
                if !st.collapsed.remove(&code) {
                    st.collapsed.insert(code);
                }
            }
            me.refresh_review();
        });
        let me = self.clone();
        rv.on_choose_option(move |i| me.choose(Decision::Option(i as u8)));
        let me = self.clone();
        rv.on_choose_special(move |i| {
            let kind = me.st.borrow().current.as_ref().map(|c| c.kind);
            let d = match (kind, i) {
                (Some(ReviewKind::Identity), 0) => Decision::NotOnRoster,
                (Some(ReviewKind::Identity), _) => Decision::Duplicate,
                (_, 0) => Decision::Blank,
                (_, 1) => Decision::Multi,
                _ => Decision::Unreadable,
            };
            me.choose(d);
        });
        let me = self.clone();
        rv.on_choose_candidate(move |i| me.choose(Decision::Candidate(i as u8)));
        let me = self.clone();
        rv.on_choose_reason(move |i| me.choose_reason(i));
        let me = self.clone();
        rv.on_accept(move || me.accept());
        let me = self.clone();
        rv.on_apply_group(move || me.apply_group());
        let me = self.clone();
        rv.on_defer(move || me.act(ReviewAction::Defer));
        let me = self.clone();
        rv.on_escalate(move || me.act(ReviewAction::Escalate));
        let me = self.clone();
        rv.on_rescan(move || me.act(ReviewAction::AddToRescan));
        let me = self.clone();
        rv.on_undo(move || me.undo());
        let me = self.clone();
        rv.on_toggle_overlay(move || me.toggle(|st| st.overlay = !st.overlay));
        let me = self.clone();
        rv.on_toggle_show_all(move || me.toggle(|st| st.show_all = !st.show_all));
        let me = self.clone();
        rv.on_toggle_registered(move || me.toggle(|st| st.registered = !st.registered));
        let me = self.clone();
        rv.on_zoom_by(move |d| me.toggle(|st| st.zoom = (st.zoom + d).clamp(2, 4)));
        let me = self.clone();
        rv.on_toggle_help(move || me.toggle(|st| st.help = !st.help));
        let me = self.clone();
        rv.on_use_student(move |t| me.use_student(&t));
        let me = self.clone();
        rv.on_search(move |t| {
            // a typed number decides an identity item; a name search is not built yet
            let is_number = !t.trim().is_empty()
                && t.chars().all(|c| c.is_ascii_digit() || c == ' ' || c == '-');
            let identity = me
                .st
                .borrow()
                .current
                .as_ref()
                .is_some_and(|c| c.kind == ReviewKind::Identity);
            if is_number && identity {
                me.use_student(&t);
            } else {
                me.say(
                    format!(
                        "Roster search by name (\"{t}\") is not built yet: type the student number, choose a candidate (1–3) or N (not on roster)"
                    ),
                    s::Tone::Info,
                )
            }
        });
        let me = self.clone();
        rv.on_key(move |t, ctrl, shift, alt| me.review_key(&t, ctrl, shift, alt));

        // results
        let rs = ui.global::<s::ResultsVM>();
        let weak = ui.as_weak();
        rs.on_select_item(move |i| {
            if let Some(ui) = weak.upgrade() {
                ui.global::<s::ResultsVM>().set_selected_item(i);
            }
        });
        let me = self.clone();
        rs.on_release(move || me.release());
        rs.set_release_note(ss(
            "Release is the instructor's deliberate act (§13.4): the score run and key version \
             are frozen in the audit log and the exports are written, the Blackboard upload with \
             them. A later change (a review decision, a key change, added scans) supersedes the \
             release; release again then.",
        ));
        let me = self.clone();
        rs.on_what_if(move || me.what_if());
        let me = self.clone();
        rs.on_check_action(move |i| me.check_action(i));
        let me = self.clone();
        rs.on_export_all(move || me.export());
        let me = self.clone();
        rs.on_export_blackboard(move |column| me.export_blackboard(&column));

        // outcomes
        let me = self.clone();
        ui.global::<s::OutcomesVM>()
            .on_export_tp154(move || me.export());

        // Remark migration (§3.5) and the parallel run (§23.7)
        let rm = ui.global::<s::RemarkVM>();
        let me = self.clone();
        rm.on_read(move |books, points| me.read_remark(&books, &points));
        let me = self.clone();
        rm.on_use_suggested(move || me.reread_remark(b::RemarkPoints::Suggested));
        let me = self.clone();
        rm.on_select_version(move |i| {
            me.st.borrow_mut().remark_version = i.max(0) as usize;
            me.refresh_remark();
        });
        let me = self.clone();
        rm.on_apply(move || me.apply_remark());
        let me = self.clone();
        rm.on_discard(move || me.discard_remark());
        let me = self.clone();
        rm.on_clear_new(move || {
            me.st.borrow_mut().remark_new_using = None;
            me.say(
                "The New-exam form no longer uses the Remark import: give a key file, key sheets \
                 or no key",
                s::Tone::Info,
            );
            me.refresh_remark();
        });
        let me = self.clone();
        rm.on_compare(move |books| {
            me.compare_remark(compare_request(&books));
        });
        let me = self.clone();
        rm.on_cmp_use_suggested(move || {
            let mut req = me.st.borrow().parallel_request.clone();
            req.points = b::RemarkPoints::Suggested;
            me.compare_remark(req);
        });
        let me = self.clone();
        rm.on_cmp_use_exam_points(move || {
            let mut req = me.st.borrow().parallel_request.clone();
            req.points = b::RemarkPoints::Exam;
            me.compare_remark(req);
        });
        let me = self.clone();
        rm.on_save(move |dir| me.save_parallel_run(&dir));
        let me = self.clone();
        rm.on_queue(move || me.queue_parallel_differences());
    }

    fn toggle(&self, f: impl FnOnce(&mut State)) {
        f(&mut self.st.borrow_mut());
        self.push_item();
    }

    fn request(&self, what: &str) {
        if what == "New exam" && self.backend.borrow().can_create_exam() {
            self.ui().global::<s::ExamsVM>().set_new_open(true);
            return;
        }
        let how = match what {
            "New exam" if self.backend.borrow().is_demo() => {
                "this window shows invented demo data. Start `omr-desktop` without --demo (it \
                 opens ~/OMR Exams) or with --workspace <your exams folder>, then New exam grades \
                 your scans"
            }
            "New exam" => {
                "grade the scans with `omr-desktop --grade <scans…> --template iau-ccsit-v1 \
                 --questions N --key key.csv --workspace <this folder>` (or `omr grade`), and the \
                 exam opens here"
            }
            "Stop the batch" => "a running batch grades to the end",
            "Rescan pull list" => "`omr review list --lane capture` lists the pages to rescan",
            "Queue filter" => "`omr review list --lane marks` lists one lane",
            w if w.starts_with("Search for") => {
                "the table lists every exam of this folder; narrow it with the filter chips above \
                 it"
            }
            _ => "the `omr` command line has every command (`omr --help`)",
        };
        self.say(
            format!("{what}: not in the desktop shell yet; {how}"),
            s::Tone::Info,
        );
    }

    /// Validates the New-exam form and starts grading it; the new exam opens on the Scan
    /// screen, which follows the run.
    fn create_exam(&self) {
        let ui = self.ui();
        let ex = ui.global::<s::ExamsVM>();
        let form = b::NewExam {
            title: ex.get_new_title().to_string(),
            course: ex.get_new_course().to_string(),
            scans: ex.get_new_scans().to_string(),
            sheet: ex.get_new_sheet().to_string(),
            questions: ex.get_new_questions().to_string(),
            options: ex.get_new_options().to_string(),
            forms: ex.get_new_forms().to_string(),
            key: ex.get_new_key().to_string(),
            roster: ex.get_new_roster().to_string(),
            clo: ex.get_new_clo().to_string(),
            key_sheets: ex.get_new_key_sheets().to_string(),
            remark: self
                .st
                .borrow()
                .remark_new_using
                .clone()
                .unwrap_or_default(),
        };
        let r = self.backend.borrow_mut().create_exam(&form);
        match r {
            Ok((exam, what)) => {
                ex.set_new_open(false);
                // a fresh form next time (the sheet is kept: the same sheet is usual)
                ex.set_new_title(SharedString::new());
                ex.set_new_course(SharedString::new());
                ex.set_new_scans(SharedString::new());
                ex.set_new_questions(SharedString::new());
                ex.set_new_options(SharedString::new());
                ex.set_new_forms(SharedString::new());
                ex.set_new_key(SharedString::new());
                ex.set_new_roster(SharedString::new());
                ex.set_new_clo(SharedString::new());
                ex.set_new_key_sheets(SharedString::new());
                {
                    // the import is used up: the new exam carries its key and objectives
                    let mut st = self.st.borrow_mut();
                    st.remark_new = None;
                    st.remark_new_using = None;
                }
                self.select_exam(&exam);
                self.navigate(s::Page::Scan);
                self.say(what, s::Tone::Info);
            }
            Err(e) => self.say(e.to_string(), s::Tone::Warning),
        }
    }

    // ------------------------------------------------------------------ Remark (§3.5, §23.7)

    /// The import the screen shown works on: the New-exam form's (Exams) or the current
    /// exam's (Setup); `None` on other screens.
    fn remark_target(&self) -> Option<Option<ExamId>> {
        match self.ui().global::<s::App>().get_page() {
            s::Page::Exams => Some(None),
            s::Page::Setup => self.exam().map(Some),
            _ => None,
        }
    }

    /// Reads the typed Remark workbooks (and points file) for the New-exam form or Setup and
    /// shows the import for review; nothing changes until it is applied.
    fn read_remark(&self, books: &str, points: &str) {
        let points = points.trim();
        let req = b::RemarkRequest {
            workbooks: scan_paths(books),
            points: match scan_paths(points).into_iter().next() {
                Some(p) if !points.is_empty() => b::RemarkPoints::File(p),
                _ => b::RemarkPoints::FromWorkbooks,
            },
            handwritten: Vec::new(),
        };
        self.run_remark_import(req);
    }

    /// Reads the last workbooks again with other points (the suggested ones: the instructor's
    /// explicit choice).
    fn reread_remark(&self, points: b::RemarkPoints) {
        let Some(target) = self.remark_target() else {
            return;
        };
        let mut req = {
            let st = self.st.borrow();
            match target {
                None => st.remark_new_request.clone(),
                Some(_) => st.remark_setup_request.clone(),
            }
        };
        req.points = points;
        self.run_remark_import(req);
    }

    fn run_remark_import(&self, req: b::RemarkRequest) {
        let Some(target) = self.remark_target() else {
            return;
        };
        let r = self
            .backend
            .borrow_mut()
            .read_remark_import(target.as_ref(), &req);
        let view = match r {
            Ok(v) => v,
            Err(e) => {
                self.say(e.to_string(), s::Tone::Warning);
                return;
            }
        };
        let msg = match &view.refusal {
            Some(r) => (
                format!("Remark import refused: {}", r.message),
                s::Tone::Warning,
            ),
            None => (
                format!(
                    "Read {} Remark workbook(s): {} version(s), {} questions; nothing changes \
                     until you {}",
                    view.files.len(),
                    view.versions.len(),
                    view.questions,
                    if target.is_none() {
                        "use the import"
                    } else {
                        "apply it"
                    }
                ),
                s::Tone::Info,
            ),
        };
        {
            let mut st = self.st.borrow_mut();
            st.remark_version = 0;
            match &target {
                None => {
                    st.remark_new = Some(view);
                    st.remark_new_request = req;
                    // a new reading replaces the one the form used
                    st.remark_new_using = None;
                }
                Some(exam) => {
                    st.remark_setup = Some((exam.clone(), view));
                    st.remark_setup_request = req;
                }
            }
        }
        let ui = self.ui();
        let rm = ui.global::<s::RemarkVM>();
        rm.set_show_key(false);
        if target.is_some() {
            rm.set_setup_bar(false);
            rm.set_setup_open(true);
        }
        self.say(msg.0, msg.1);
        self.refresh_remark();
    }

    /// Applies the import: on the New-exam form it fills the form (graded with the import's key,
    /// points and learning objectives); in Setup the exam's key and points are replaced.
    fn apply_remark(&self) {
        let Some(target) = self.remark_target() else {
            return;
        };
        let ui = self.ui();
        let rm = ui.global::<s::RemarkVM>();
        match target {
            None => {
                let Some(view) = self
                    .st
                    .borrow()
                    .remark_new
                    .clone()
                    .filter(|v| v.refusal.is_none())
                else {
                    return;
                };
                let ex = ui.global::<s::ExamsVM>();
                ex.set_new_questions(ss(view.questions.to_string()));
                ex.set_new_forms(ss(view
                    .versions
                    .iter()
                    .map(|v| v.form.as_str())
                    .collect::<Vec<_>>()
                    .join(",")));
                // the import is the key source: a key file or key sheets would be a second one
                ex.set_new_key(SharedString::new());
                ex.set_new_key_sheets(SharedString::new());
                let objectives = view.versions.iter().any(|v| !v.objectives.is_empty());
                if objectives {
                    ex.set_new_clo(SharedString::new());
                }
                self.st.borrow_mut().remark_new_using = Some(view.files.join("; "));
                rm.set_new_panel(false);
                self.say(
                    format!(
                        "The form uses the Remark import: {} questions, versions {}; type the \
                         scans, then Grade exam",
                        view.questions,
                        view.versions
                            .iter()
                            .map(|v| v.form.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    s::Tone::Info,
                );
            }
            Some(exam) => {
                let r = self.backend.borrow_mut().apply_remark_import(&exam);
                match r {
                    Ok(what) => {
                        self.st.borrow_mut().remark_setup = None;
                        rm.set_setup_open(false);
                        self.say(what, s::Tone::Success);
                        self.refresh_setup();
                        self.refresh_if_shown(s::Page::Results);
                        self.refresh_dashboard();
                    }
                    Err(e) => self.say(e.to_string(), s::Tone::Warning),
                }
            }
        }
        self.refresh_remark();
    }

    /// Puts the import aside (nothing was changed).
    fn discard_remark(&self) {
        let Some(target) = self.remark_target() else {
            return;
        };
        self.backend
            .borrow_mut()
            .discard_remark_import(target.as_ref());
        {
            let mut st = self.st.borrow_mut();
            match target {
                None => {
                    st.remark_new = None;
                    st.remark_new_using = None;
                }
                Some(_) => st.remark_setup = None,
            }
        }
        self.ui().global::<s::RemarkVM>().set_setup_open(false);
        self.say(
            "Remark import put aside: nothing was changed",
            s::Tone::Info,
        );
        self.refresh_remark();
    }

    /// Runs the parallel run with Remark's workbooks and shows it in place of the results.
    fn compare_remark(&self, req: b::RemarkRequest) {
        let Some(exam) = self.exam() else { return };
        let r = self.backend.borrow_mut().compare_remark(&exam, &req);
        match r {
            Ok(view) => {
                let msg = match &view.refusal {
                    Some(r) => (
                        format!("Parallel run refused: {}", r.message),
                        s::Tone::Warning,
                    ),
                    None => {
                        let c = &view.counts;
                        (
                            format!(
                                "Parallel run with Remark (Restricted): {} differences and {} \
                                 pending questions to adjudicate; nothing was written{}",
                                c.differ + c.differ_pending,
                                c.agree_pending,
                                if c.not_queued > 0 {
                                    "; the comparison holds nothing until you queue its \
                                     differences for review"
                                } else {
                                    ""
                                }
                            ),
                            s::Tone::Info,
                        )
                    }
                };
                let dir = self.backend.borrow().parallel_run_dir(&exam);
                {
                    let mut st = self.st.borrow_mut();
                    st.parallel = Some((exam, view));
                    st.parallel_request = req;
                }
                let ui = self.ui();
                let rm = ui.global::<s::RemarkVM>();
                rm.set_cmp_bar(false);
                rm.set_cmp_open(true);
                if rm.get_save_path().is_empty()
                    && let Some(d) = dir
                {
                    rm.set_save_path(ss(d.display().to_string()));
                }
                self.say(msg.0, msg.1);
                self.refresh_remark();
            }
            Err(e) => self.say(e.to_string(), s::Tone::Warning),
        }
    }

    /// Queues the parallel run's settled differences for review (they then hold release),
    /// then shows the comparison again as the exam now stands.
    fn queue_parallel_differences(&self) {
        let Some(exam) = self.exam() else { return };
        let r = self.backend.borrow_mut().queue_parallel_differences(&exam);
        match r {
            Ok(what) => {
                let req = self.st.borrow().parallel_request.clone();
                self.compare_remark(req);
                self.refresh_dashboard();
                self.refresh_if_shown(s::Page::Results);
                self.say(what, s::Tone::Success);
            }
            Err(e) => self.say(e.to_string(), s::Tone::Warning),
        }
    }

    /// Writes the Restricted parallel-run report into the typed folder.
    fn save_parallel_run(&self, dir: &str) {
        let Some(exam) = self.exam() else { return };
        let path = scan_paths(dir).into_iter().next().unwrap_or_default();
        let r = self.backend.borrow_mut().save_parallel_run(&exam, &path);
        match r {
            Ok(what) => self.say(what, s::Tone::Success),
            Err(e) => self.say(e.to_string(), s::Tone::Warning),
        }
    }

    /// Releases the results through the backend (confirmed in the release bar).
    fn release(&self) {
        let Some(exam) = self.exam() else { return };
        let r = self.backend.borrow_mut().release(&exam);
        match r {
            Ok(what) => {
                self.say(what, s::Tone::Success);
                self.refresh_dashboard();
                self.refresh_if_shown(s::Page::Results);
            }
            Err(e) => self.say(e.to_string(), s::Tone::Warning),
        }
    }

    /// Replaces the answer key from the typed key file and rescores.
    fn replace_key(&self, path: &str) {
        let Some(exam) = self.exam() else { return };
        let path = std::path::PathBuf::from(path.trim());
        let r = self.backend.borrow_mut().replace_key(&exam, &path);
        match r {
            Ok(what) => {
                let ui = self.ui();
                let su = ui.global::<s::SetupVM>();
                su.set_key_open(false);
                su.set_key_path(SharedString::new());
                self.say(what, s::Tone::Success);
                self.refresh_setup();
                self.refresh_if_shown(s::Page::Results);
                self.refresh_dashboard();
            }
            Err(e) => self.say(e.to_string(), s::Tone::Warning),
        }
    }

    /// Reads the typed key-sheet files (the instructor's filled answer sheets) and shows the
    /// key they give for confirmation; nothing in the exam changes yet.
    fn read_key_sheets(&self, text: &str) {
        let Some(exam) = self.exam() else { return };
        let paths = scan_paths(text);
        let r = self.backend.borrow_mut().read_key_sheets(&exam, &paths);
        match r {
            Ok(what) => {
                {
                    let mut st = self.st.borrow_mut();
                    st.ks_version = 0;
                    st.ks_picks.clear();
                }
                let ui = self.ui();
                let su = ui.global::<s::SetupVM>();
                su.set_ks_bar_open(false);
                su.set_ks_path(SharedString::new());
                su.set_ks_open(true);
                self.say(what, s::Tone::Info);
                self.refresh_setup();
            }
            Err(e) => self.say(e.to_string(), s::Tone::Warning),
        }
    }

    /// The instructor's decision on a key-sheet row that waits for one: option `opt`
    /// (0-based), `-1` void, `-2` every option marked on the sheet. Choosing it again takes it
    /// back. Sent with the confirmation, never on its own.
    fn key_sheet_choose(&self, row: i32, opt: i32) {
        let Some(exam) = self.exam() else { return };
        let Some(sum) = self.backend.borrow().key_sheets(&exam) else {
            return;
        };
        let vi = self
            .st
            .borrow()
            .ks_version
            .min(sum.versions.len().saturating_sub(1));
        let Some(v) = sum.versions.get(vi) else {
            return;
        };
        let Some(r) = usize::try_from(row).ok().and_then(|i| v.rows.get(i)) else {
            return;
        };
        let letter = |o: u8| char::from(b'A' + o).to_string();
        let answer = match opt {
            -1 => "*".to_string(),
            -2 => r
                .marked
                .iter()
                .map(|&o| letter(o))
                .collect::<Vec<_>>()
                .join("|"),
            o if o >= 0 => letter(o as u8),
            _ => return,
        };
        if answer.is_empty() {
            return;
        }
        {
            let mut st = self.st.borrow_mut();
            let k = (v.form, r.question);
            if st.ks_picks.get(&k) == Some(&answer) {
                st.ks_picks.remove(&k);
            } else {
                st.ks_picks.insert(k, answer.clone());
            }
        }
        let word = match answer.as_str() {
            "*" => "void (nobody is marked on it)".to_string(),
            a if a.contains('|') => format!("{} all accepted", a.replace('|', " or ")),
            a => a.to_string(),
        };
        self.say(
            format!(
                "Version {} question {}: {word}; recorded when you confirm",
                v.version, r.question
            ),
            s::Tone::Info,
        );
        self.refresh_keysheets();
    }

    /// Confirms the key read from key sheets: the version shown, or every version waiting.
    fn confirm_key_sheets(&self, all: bool) {
        let Some(exam) = self.exam() else { return };
        let Some(sum) = self.backend.borrow().key_sheets(&exam) else {
            return;
        };
        let vi = self
            .st
            .borrow()
            .ks_version
            .min(sum.versions.len().saturating_sub(1));
        let Some(v) = sum.versions.get(vi) else {
            return;
        };
        let version = (!all).then_some(v.form);
        let decisions: Vec<b::KeyRowDecision> = self
            .st
            .borrow()
            .ks_picks
            .iter()
            .filter(|((f, _), _)| version.is_none_or(|x| x == *f))
            .map(|((f, q), a)| b::KeyRowDecision {
                version: *f,
                question: *q,
                answer: a.clone(),
            })
            .collect();
        let r = self
            .backend
            .borrow_mut()
            .confirm_key_sheets(&exam, version, &decisions);
        match r {
            Ok(what) => {
                {
                    let mut st = self.st.borrow_mut();
                    st.ks_picks
                        .retain(|(f, _), _| version.is_some_and(|x| x != *f));
                }
                self.say(what, s::Tone::Success);
                // the key may now be in force: every screen shows the scored exam
                self.refresh_all();
                self.restart_review();
            }
            Err(e) => self.say(e.to_string(), s::Tone::Warning),
        }
    }

    /// Puts aside key-sheet files read and not confirmed.
    fn discard_key_sheets(&self) {
        let Some(exam) = self.exam() else { return };
        let r = self.backend.borrow_mut().discard_key_sheets(&exam);
        match r {
            Ok(what) => {
                self.st.borrow_mut().ks_picks.clear();
                let still = self.backend.borrow().key_sheets(&exam).is_some();
                self.ui().global::<s::SetupVM>().set_ks_open(still);
                self.say(what, s::Tone::Info);
                self.refresh_setup();
            }
            Err(e) => self.say(e.to_string(), s::Tone::Warning),
        }
    }

    /// Writes the exam's exports (TP-154 tables included) through the backend.
    fn export(&self) {
        let Some(exam) = self.exam() else { return };
        let r = self.backend.borrow_mut().export(&exam);
        match r {
            Ok(where_) => self.say(where_, s::Tone::Success),
            Err(e) => self.say(e.to_string(), s::Tone::Warning),
        }
    }

    /// Writes the Blackboard grade-center upload, into an existing column when one is given.
    fn export_blackboard(&self, column: &str) {
        let Some(exam) = self.exam() else { return };
        let column = Some(column.trim()).filter(|c| !c.is_empty());
        let r = self.backend.borrow_mut().export_blackboard(&exam, column);
        match r {
            Ok(where_) => self.say(where_, s::Tone::Success),
            Err(e) => self.say(e.to_string(), s::Tone::Warning),
        }
    }

    /// Starts adding the typed scan files or folders to the exam (graded again in the
    /// background; the Scan screen follows it through `pump`).
    fn add_scans(&self, text: &str) {
        let Some(exam) = self.exam() else { return };
        let paths = scan_paths(text);
        let r = self.backend.borrow_mut().add_scans(&exam, &paths);
        match r {
            Ok(what) => {
                let ui = self.ui();
                let sc = ui.global::<s::ScanVM>();
                sc.set_add_open(false);
                sc.set_add_path(SharedString::new());
                self.say(what, s::Tone::Info);
                self.refresh_scan();
                self.refresh_dashboard();
            }
            Err(e) => self.say(e.to_string(), s::Tone::Warning),
        }
    }

    fn toggle_pause(&self) {
        let Some(exam) = self.exam() else { return };
        let running = self.ui().global::<s::ScanVM>().get_running();
        let r = self.backend.borrow_mut().pause_batch(&exam, running);
        match r {
            Ok(()) => {
                self.say(
                    if running {
                        "Batch paused"
                    } else {
                        "Batch resumed"
                    },
                    s::Tone::Info,
                );
                self.refresh_scan();
            }
            Err(e) => self.say(e.to_string(), s::Tone::Warning),
        }
    }

    fn what_if(&self) {
        self.say(
            "What-if preview is not built yet (§12.2): change the key in Setup to rescore, then undo it there",
            s::Tone::Info,
        );
    }

    /// Follows readiness check `i`'s action to where the backend says it leads.
    fn check_action(&self, i: i32) {
        let target = self.exam().and_then(|e| {
            let r = self.backend.borrow().results(&e)?;
            r.readiness.get(usize::try_from(i).ok()?)?.target
        });
        match target {
            Some(b::CheckTarget::Review(lane)) => {
                if let Some(lane) = lane {
                    self.select_lane(lane);
                }
                self.navigate(s::Page::Review);
            }
            Some(b::CheckTarget::Scan) => self.navigate(s::Page::Scan),
            Some(b::CheckTarget::Setup) => self.navigate(s::Page::Setup),
            Some(b::CheckTarget::Outcomes) => self.navigate(s::Page::Outcomes),
            None => {}
        }
    }

    /// Moves the exams-table selection by `delta` rows (↑ / ↓), keeping the drawer as it is.
    fn move_exam(&self, delta: i32) {
        let next = {
            let st = self.st.borrow();
            if st.exam_rows.is_empty() {
                return;
            }
            let last = st.exam_rows.len() as i32 - 1;
            let pos = st
                .exam
                .as_ref()
                .and_then(|e| st.exam_rows.iter().position(|r| r == e))
                .map_or(if delta >= 0 { -1 } else { last + 1 }, |p| p as i32);
            st.exam_rows
                .get((pos + delta).clamp(0, last) as usize)
                .cloned()
        };
        if let Some(id) = next {
            self.select_exam(&id);
        }
    }

    // -------------------------------------------------------------------------- keys

    fn global_key(&self, text: &str, ctrl: bool, _shift: bool, alt: bool) -> bool {
        const SCREENS: [s::Page; 6] = [
            s::Page::Exams,
            s::Page::Setup,
            s::Page::Scan,
            s::Page::Review,
            s::Page::Results,
            s::Page::Outcomes,
        ];
        match keymap::global_key(text, ctrl, alt) {
            Some(GlobalKey::Screen(n)) => match SCREENS.get(n as usize) {
                Some(&page) => {
                    self.navigate(page);
                    true
                }
                None => false,
            },
            Some(GlobalKey::Settings) => {
                self.navigate(s::Page::Settings);
                true
            }
            Some(GlobalKey::Dismiss) => {
                let page = self.ui().global::<s::App>().get_page();
                if page == s::Page::Exams && self.st.borrow().drawer_open {
                    self.st.borrow_mut().drawer_open = false;
                    self.refresh_dashboard();
                    true
                } else if page == s::Page::Review && self.st.borrow().help {
                    self.toggle(|st| st.help = false);
                    true
                } else {
                    false
                }
            }
            None => self.page_chord(text, ctrl, alt),
        }
    }

    /// The Ctrl chords each screen advertises on its buttons (DESIGN §9).
    fn page_chord(&self, text: &str, ctrl: bool, alt: bool) -> bool {
        use keymap::Chord;
        let Some(chord) = keymap::page_chord(text, ctrl, alt) else {
            return false;
        };
        let page = self.ui().global::<s::App>().get_page();
        match (page, chord) {
            (s::Page::Exams, Chord::Letter('n')) => self.request("New exam"),
            (s::Page::Exams, Chord::Letter('k')) => {
                let ui = self.ui();
                let ex = ui.global::<s::ExamsVM>();
                ex.set_search_request(ex.get_search_request().wrapping_add(1));
            }
            (s::Page::Scan, Chord::Space) => self.toggle_pause(),
            (s::Page::Scan, Chord::Letter('r')) => self.request("Rescan pull list"),
            (s::Page::Results, Chord::Letter('p')) => self.what_if(),
            _ => return false,
        }
        true
    }

    /// Handles a key pressed while the review canvas (or a control inside it) has focus.
    pub fn review_key(&self, text: &str, ctrl: bool, shift: bool, alt: bool) -> bool {
        let (context, single_key) = {
            let st = self.st.borrow();
            let ctx = match st.current.as_ref().map(|c| (c.kind, c)) {
                Some((ReviewKind::Identity, c)) => KeyContext::Identity {
                    candidates: c.identity.as_ref().map_or(0, |e| e.candidates.len().min(3)),
                },
                Some((ReviewKind::Notice, _)) => KeyContext::Notice,
                // Form items take the option keys too (the version bubbles); Blank, Multi and
                // Unreadable are refused by `choose`.
                Some((_, c)) => KeyContext::Mark {
                    options: c.bubbles.len(),
                },
                None => KeyContext::Notice,
            };
            (ctx, st.single_key)
        };
        let Some(cmd) = keymap::review_key(text, ctrl, shift, alt, context, single_key) else {
            return false;
        };
        let mark_ctx = matches!(context, KeyContext::Mark { .. });
        match cmd {
            ReviewKey::Option(i) => self.choose(Decision::Option(i)),
            ReviewKey::Candidate(i) => self.choose(Decision::Candidate(i)),
            ReviewKey::NotOnRoster => self.choose(Decision::NotOnRoster),
            ReviewKey::Duplicate => self.choose(Decision::Duplicate),
            ReviewKey::Blank if mark_ctx => self.choose(Decision::Blank),
            ReviewKey::Multi if mark_ctx => self.choose(Decision::Multi),
            ReviewKey::Unreadable if mark_ctx => self.choose(Decision::Unreadable),
            ReviewKey::Blank | ReviewKey::Multi | ReviewKey::Unreadable => return false,
            ReviewKey::Accept => self.accept(),
            ReviewKey::ApplyToGroup => self.apply_group(),
            ReviewKey::Next => self.step(1),
            ReviewKey::Previous => self.step(-1),
            ReviewKey::Defer => self.act(ReviewAction::Defer),
            ReviewKey::Escalate => self.act(ReviewAction::Escalate),
            ReviewKey::Rescan => self.act(ReviewAction::AddToRescan),
            ReviewKey::Undo => self.undo(),
            ReviewKey::ToggleOverlay => self.toggle(|st| st.overlay = !st.overlay),
            ReviewKey::ToggleOriginal => self.toggle(|st| st.registered = !st.registered),
            ReviewKey::Help => self.toggle(|st| st.help = !st.help),
            ReviewKey::ZoomIn => self.toggle(|st| st.zoom = (st.zoom + 1).min(4)),
            ReviewKey::ZoomOut => self.toggle(|st| st.zoom = (st.zoom - 1).max(2)),
            ReviewKey::Search => {
                let n = {
                    let mut st = self.st.borrow_mut();
                    st.search_request += 1;
                    st.search_request
                };
                self.ui().global::<s::ReviewVM>().set_search_request(n);
            }
        }
        true
    }

    // -------------------------------------------------------------------------- review actions

    fn select_lane(&self, lane: Lane) {
        {
            let mut st = self.st.borrow_mut();
            st.lane = lane;
            st.item = None;
            st.current = None;
            st.pending = None;
        }
        self.refresh_review();
        let first = self.st.borrow().order.first().copied();
        self.select_item(first);
    }

    fn open_code(&self, code: &str) {
        let Some(exam) = self.exam() else { return };
        // the key-sheet hold is lifted by confirming the key in Setup, never in the queue
        if code == "SCO-003" && self.backend.borrow().hold(&exam).is_some() {
            self.ui().global::<s::SetupVM>().set_ks_open(true);
            self.navigate(s::Page::Setup);
            return;
        }
        for lane in Lane::ALL {
            let q = self.backend.borrow().review_queue(&exam, lane);
            if let Some(id) = q
                .groups
                .iter()
                .find(|g| g.code == code)
                .and_then(|g| g.items.first())
                .map(|i| i.id)
            {
                {
                    let mut st = self.st.borrow_mut();
                    st.lane = lane;
                    st.collapsed.remove(code);
                }
                self.ui().global::<s::App>().set_page(s::Page::Review);
                self.select_item(Some(id));
                return;
            }
        }
        self.navigate(s::Page::Review);
        self.say(
            format!("No open {code} items in this exam's queue"),
            s::Tone::Info,
        );
    }

    /// Shows item `id` (or nothing) in the review frame.
    pub fn select_item(&self, id: Option<ItemId>) {
        if self.st.borrow().item != id {
            // a number typed for another booklet never carries over
            self.ui()
                .global::<s::ReviewVM>()
                .set_student_entry(SharedString::new());
        }
        let exam = self.exam();
        let item = match (exam, id) {
            (Some(e), Some(id)) => self.backend.borrow().review_item(&e, id),
            _ => None,
        };
        {
            let mut st = self.st.borrow_mut();
            st.item = item.as_ref().map(|i| i.id);
            st.pending = item.as_ref().and_then(|i| i.current);
            st.reason = None;
            st.reasons_all = false;
            st.typed_student = None;
            if let Some(code) = item.as_ref().map(|i| i.code.clone()) {
                st.collapsed.remove(&code);
            }
            st.current = item;
            st.show_all = self.ui().global::<s::App>().get_show_all_default();
        }
        self.refresh_review();
    }

    fn step(&self, delta: i32) {
        let next = {
            let st = self.st.borrow();
            let pos = st
                .item
                .and_then(|id| st.order.iter().position(|&x| x == id))
                .map_or(0, |p| p as i32 + delta);
            if st.order.is_empty() {
                None
            } else {
                Some(st.order[pos.clamp(0, st.order.len() as i32 - 1) as usize])
            }
        };
        if next.is_some() {
            self.select_item(next);
        }
    }

    /// Sets the pending answer (shown as the selected chip; committed by Accept / Enter).
    pub fn choose(&self, d: Decision) {
        let valid = {
            let st = self.st.borrow();
            match (st.current.as_ref(), d) {
                (None, _) => false,
                (Some(c), Decision::Option(i)) => {
                    matches!(
                        c.kind,
                        ReviewKind::Mark | ReviewKind::Form | ReviewKind::BlindVerify
                    ) && (i as usize) < c.bubbles.len()
                }
                (Some(c), Decision::Candidate(i)) => {
                    c.kind == ReviewKind::Identity
                        && (i as usize)
                            < c.identity.as_ref().map_or(0, |e| e.candidates.len().min(3))
                }
                (Some(c), Decision::NotOnRoster | Decision::Duplicate) => {
                    c.kind == ReviewKind::Identity
                }
                (Some(c), Decision::Acknowledge) => c.kind == ReviewKind::Notice,
                (Some(c), _) => matches!(c.kind, ReviewKind::Mark | ReviewKind::BlindVerify),
            }
        };
        if !valid {
            return;
        }
        {
            let mut st = self.st.borrow_mut();
            // choosing the same thing twice clears it (toggle), as chips are checkable
            st.pending = if st.pending == Some(d) { None } else { Some(d) };
            st.reason = None;
            st.typed_student = None;
        }
        self.push_item();
    }

    /// Picks the override reason `i` of those on offer (again: clears it); the last chip may
    /// instead show every code or the first ones again.
    pub fn choose_reason(&self, i: i32) {
        {
            let mut st = self.st.borrow_mut();
            let code = usize::try_from(i)
                .ok()
                .and_then(|i| st.reason_codes.get(i).cloned());
            match code.as_deref() {
                None => return,
                Some(MORE_REASONS) => st.reasons_all = !st.reasons_all,
                Some(_) => st.reason = if st.reason == code { None } else { code },
            }
        }
        self.push_item();
    }

    /// Identity items: takes the typed student number (digits only; spaces and dashes as
    /// written are dropped) as the pending decision, in place of a listed candidate.
    pub fn use_student(&self, text: &str) {
        let id: String = text.chars().filter(char::is_ascii_digit).collect();
        let ok = {
            let st = self.st.borrow();
            st.current
                .as_ref()
                .is_some_and(|c| c.kind == ReviewKind::Identity)
        };
        if !ok {
            self.say(
                "A student number decides identity items only: open the Identity lane",
                s::Tone::Info,
            );
            return;
        }
        if id.is_empty() {
            self.say(
                "Type the student number as digits (the number written on the sheet)",
                s::Tone::Warning,
            );
            return;
        }
        {
            let mut st = self.st.borrow_mut();
            st.pending = None;
            st.reason = None;
            st.typed_student = Some(id);
        }
        self.push_item();
    }

    /// True when the typed student number overrides the engine and no reason is chosen yet.
    fn typed_reason_missing(&self, id: &str) -> bool {
        let st = self.st.borrow();
        let (Some(exam), Some(item)) = (st.exam.as_ref(), st.item) else {
            return false;
        };
        st.reason.is_none()
            && self
                .backend
                .borrow()
                .needs_reason_for_student(exam, item, id)
    }

    /// The chosen reason needs a note the reviewer has not written (§14.4).
    fn note_missing(&self) -> Option<String> {
        let st = self.st.borrow();
        let reason = st.reason.as_ref()?;
        let c = st.current.as_ref()?;
        let note = self.ui().global::<s::ReviewVM>().get_note();
        (c.reasons_needing_note.contains(reason) && note.trim().is_empty())
            .then(|| reason_words(reason))
    }

    /// True when committing `d` on the current item needs a reason the reviewer has not given.
    fn reason_missing(&self, d: Decision) -> bool {
        let st = self.st.borrow();
        let (Some(exam), Some(item)) = (st.exam.as_ref(), st.item) else {
            return false;
        };
        st.reason.is_none() && self.backend.borrow().needs_reason(exam, item, d)
    }

    /// Commits the pending answer, or the suggestion when nothing was chosen.
    pub fn accept(&self) {
        let typed = self.st.borrow().typed_student.clone();
        if let Some(student_id) = typed {
            if self.typed_reason_missing(&student_id) {
                self.say(
                    "This overrides the engine's suggestion: choose a reason first (§13.4)",
                    s::Tone::Warning,
                );
                return;
            }
            if let Some(reason) = self.note_missing() {
                self.say(
                    format!(
                        "The reason \"{reason}\" needs a note: write why in the note field (§14.4)"
                    ),
                    s::Tone::Warning,
                );
                return;
            }
            let note = self.ui().global::<s::ReviewVM>().get_note().to_string();
            self.act(ReviewAction::SetStudent { student_id, note });
            return;
        }
        let (decision, accepted_suggestion) = {
            let st = self.st.borrow();
            let Some(c) = st.current.as_ref() else { return };
            // a blind item is never committed with the engine's reading
            let sugg = c
                .suggestion
                .as_ref()
                .filter(|_| c.kind != ReviewKind::BlindVerify)
                .map(|s| s.decision);
            match (st.pending, sugg, c.kind) {
                (_, _, ReviewKind::Notice) => (Decision::Acknowledge, true),
                (Some(p), s, _) => (p, Some(p) == s),
                (None, Some(s), _) => (s, true),
                (None, None, kind) => {
                    drop(st);
                    self.say(
                        if kind == ReviewKind::Identity {
                            "Choose a candidate (1–3), N (not on roster), or type the student \
                             number"
                        } else {
                            "Choose what is marked first (blind verify shows no suggestion)"
                        },
                        s::Tone::Warning,
                    );
                    return;
                }
            }
        };
        if self.reason_missing(decision) {
            self.say(
                "This overrides the engine's suggestion: choose a reason first (§13.4)",
                s::Tone::Warning,
            );
            return;
        }
        if let Some(reason) = self.note_missing() {
            self.say(
                format!(
                    "The reason \"{reason}\" needs a note: write why in the note field (§14.4)"
                ),
                s::Tone::Warning,
            );
            return;
        }
        let note = self.ui().global::<s::ReviewVM>().get_note().to_string();
        self.act(ReviewAction::Resolve {
            decision,
            accepted_suggestion,
            note,
        });
    }

    fn apply_group(&self) {
        let decision = {
            let st = self.st.borrow();
            let Some(c) = st.current.as_ref() else { return };
            if c.kind != ReviewKind::Mark || !c.group.allowed {
                let reason = c.group.reason.clone();
                drop(st);
                self.say(reason, s::Tone::Warning);
                return;
            }
            st.pending.or(c.suggestion.as_ref().map(|s| s.decision))
        };
        if let Some(decision) = decision {
            if self.reason_missing(decision) {
                self.say(
                    "This overrides the engine's suggestion: choose a reason first (§13.4)",
                    s::Tone::Warning,
                );
                return;
            }
            if let Some(reason) = self.note_missing() {
                self.say(
                    format!(
                        "The reason \"{reason}\" needs a note: write why in the note field (§14.4)"
                    ),
                    s::Tone::Warning,
                );
                return;
            }
            self.act(ReviewAction::ApplyToGroup { decision });
        }
    }

    fn act(&self, action: ReviewAction) {
        let (exam, item, label, labels) = {
            let st = self.st.borrow();
            let (Some(exam), Some(c)) = (st.exam.clone(), st.current.as_ref()) else {
                return;
            };
            let label = if c.question > 0 {
                format!("Q{:02} ({})", c.question, c.serial)
            } else {
                format!("{} {}", c.code, c.serial)
            };
            let labels: Vec<String> = c.bubbles.iter().map(|b| b.label.clone()).collect();
            (exam, c.id, label, labels)
        };
        let what = match &action {
            ReviewAction::Resolve { decision, .. } => {
                format!("Recorded {label}: {}", decision_word(*decision, &labels))
            }
            ReviewAction::ApplyToGroup { decision } => format!(
                "Applied {} to the group of {label}",
                decision_word(*decision, &labels)
            ),
            ReviewAction::SetStudent { student_id, .. } => {
                format!("Recorded {label}: student {student_id}")
            }
            ReviewAction::Defer => format!("Deferred {label}"),
            ReviewAction::Escalate => format!("Escalated {label} to the instructor"),
            ReviewAction::AddToRescan => format!("Added {label} to the rescan list"),
        };
        let reason = self.st.borrow().reason.clone();
        let result = self
            .backend
            .borrow_mut()
            .apply_with_reason(&exam, item, action, reason);
        match result {
            Ok(out) => {
                let n = out.affected.len();
                self.say(
                    if n > 1 {
                        format!("{what} ({n} items)")
                    } else {
                        what
                    },
                    s::Tone::Success,
                );
                self.ui()
                    .global::<s::ReviewVM>()
                    .set_note(SharedString::new());
                self.refresh_review();
                self.select_item(out.next);
                self.refresh_dashboard();
                self.refresh_if_shown(s::Page::Results);
            }
            Err(e) => self.say(e.to_string(), s::Tone::Error),
        }
    }

    fn undo(&self) {
        let Some(exam) = self.exam() else { return };
        let result = self.backend.borrow_mut().undo(&exam);
        match result {
            Ok(Some(id)) => {
                self.say(
                    "Undone: the item is open again (compensating event)",
                    s::Tone::Info,
                );
                // the re-opened item may sit in another lane
                for lane in Lane::ALL {
                    let q = self.backend.borrow().review_queue(&exam, lane);
                    if q.groups.iter().any(|g| g.items.iter().any(|i| i.id == id)) {
                        self.st.borrow_mut().lane = lane;
                        break;
                    }
                }
                self.select_item(Some(id));
                self.refresh_dashboard();
                self.refresh_if_shown(s::Page::Results);
            }
            Ok(None) => self.say("Nothing to undo", s::Tone::Neutral),
            Err(e) => self.say(e.to_string(), s::Tone::Error),
        }
    }

    // -------------------------------------------------------------------------- setup edits

    fn setup_row(&self, row: usize) -> Option<(ExamId, b::ExamSetup, u32)> {
        let exam = self.exam()?;
        let setup = self.backend.borrow().exam_setup(&exam)?;
        let q = setup.mapping.get(row)?.question;
        Some((exam, setup, q))
    }

    fn cycle_key(&self, row: usize, version: usize) {
        let Some((exam, setup, q)) = self.setup_row(row) else {
            return;
        };
        let options = &setup.template.option_labels;
        let cur = setup
            .keys
            .get(version)
            .and_then(|k| k.answers.get(row))
            .cloned()
            .unwrap_or_default();
        let next = match options.iter().position(|o| *o == cur) {
            Some(i) => options[(i + 1) % options.len()].clone(),
            None => options.first().cloned().unwrap_or_default(),
        };
        let r = self.backend.borrow_mut().set_key(&exam, version, q, &next);
        let v = setup
            .keys
            .get(version)
            .map(|k| k.version.clone())
            .unwrap_or_default();
        self.after_setup_edit(r, format!("Key Q{q:02} version {v}: {next}"));
    }

    fn set_key(&self, row: usize, version: usize, text: &str) {
        let Some((exam, setup, q)) = self.setup_row(row) else {
            return;
        };
        // typed key letters follow the label set; Arabic letters map by position (§20.2)
        let answer = if text.is_empty() {
            String::new()
        } else {
            match keymap::option_for(text, setup.template.option_labels.len()) {
                Some(i) => setup.template.option_labels[i as usize].clone(),
                None => text.to_uppercase(),
            }
        };
        let r = self
            .backend
            .borrow_mut()
            .set_key(&exam, version, q, &answer);
        let v = setup
            .keys
            .get(version)
            .map(|k| k.version.clone())
            .unwrap_or_default();
        self.after_setup_edit(
            r,
            format!(
                "Key Q{q:02} version {v}: {}",
                if answer.is_empty() {
                    "cleared"
                } else {
                    &answer
                }
            ),
        );
    }

    /// Saves the weight of a cell. The selection is left as it is: it follows the focus
    /// ([`Self::select_cell`]), which may already be on the next cell (Tab, or a click
    /// elsewhere, commits the cell it leaves).
    fn set_weight(&self, row: usize, clo: usize, text: &str) {
        let Some((exam, setup, q)) = self.setup_row(row) else {
            return;
        };
        let r = match parse_weight(text) {
            Ok(w) => self
                .backend
                .borrow_mut()
                .set_mapping_weight(&exam, q, clo, w),
            Err(e) => Err(b::BackendError::Invalid(e)),
        };
        let code = setup
            .clos
            .get(clo)
            .map(|c| c.code.clone())
            .unwrap_or_default();
        self.after_setup_edit(
            r,
            format!(
                "Weight Q{q:02} × {code}: {}",
                if text.trim().is_empty() {
                    "cleared"
                } else {
                    text.trim()
                }
            ),
        );
    }

    /// Approves the imported CLO mapping rows (an audited approval by the signed-in principal).
    fn approve_mappings(&self) {
        let Some(exam) = self.exam() else { return };
        let r = self.backend.borrow_mut().approve_mappings(&exam);
        match r {
            Ok(what) => {
                self.say(what, s::Tone::Success);
                self.refresh_setup();
                self.refresh_if_shown(s::Page::Outcomes);
                self.refresh_dashboard();
            }
            Err(e) => self.say(e.to_string(), s::Tone::Warning),
        }
    }

    /// Replaces the CLOs and mapping from the typed CLO file.
    fn replace_clos(&self, path: &str) {
        let Some(exam) = self.exam() else { return };
        let path = scan_paths(path).into_iter().next().unwrap_or_default();
        let r = self.backend.borrow_mut().replace_clos(&exam, &path);
        match r {
            Ok(what) => {
                self.ui().global::<s::SetupVM>().set_clo_path(ss(""));
                self.say(what, s::Tone::Success);
                self.refresh_setup();
                self.refresh_if_shown(s::Page::Outcomes);
                self.refresh_dashboard();
            }
            Err(e) => self.say(e.to_string(), s::Tone::Warning),
        }
    }

    fn set_note(&self, row: usize, clo: usize, text: &str) {
        let Some((exam, setup, q)) = self.setup_row(row) else {
            return;
        };
        let r = self
            .backend
            .borrow_mut()
            .set_mapping_note(&exam, q, clo, text);
        let code = setup
            .clos
            .get(clo)
            .map(|c| c.code.clone())
            .unwrap_or_default();
        self.after_setup_edit(r, format!("Note saved for Q{q:02} × {code}"));
    }

    fn after_setup_edit(&self, r: Result<(), b::BackendError>, ok: String) {
        let ui = self.ui();
        let su = ui.global::<s::SetupVM>();
        match r {
            Ok(()) => {
                su.set_error(SharedString::new());
                self.say(ok, s::Tone::Success);
            }
            Err(e) => {
                su.set_error(ss(e.to_string()));
                self.say(e.to_string(), s::Tone::Error);
            }
        }
        self.refresh_setup();
        self.refresh_dashboard();
    }

    // -------------------------------------------------------------------------- refresh: chrome + exams

    fn refresh_dashboard(&self) {
        let ui = self.ui();
        let d = self.backend.borrow().dashboard();
        let app = ui.global::<s::App>();
        app.set_user_name(ss(&d.user.name));
        app.set_user_role(ss(&d.user.role));
        app.set_user_initials(ss(&d.user.initials));
        app.set_engine(ss(&d.engine));
        app.set_workspace(ss(&d.workspace));
        app.set_data_note(ss(&d.data_note));
        app.set_demo(self.backend.borrow().is_demo());

        let ex = ui.global::<s::ExamsVM>();
        ex.set_can_create(self.backend.borrow().can_create_exam());
        ex.set_new_note(ss(format!(
            "Graded into a new folder of {}. Without a key file the key is inferred: the \
             results stay provisional until you replace it in Setup.",
            if d.workspace.is_empty() {
                "the exams folder"
            } else {
                d.workspace.as_str()
            }
        )));
        ex.set_term_label(ss(join(&[&d.term.label, &d.term.gregorian], " · ")));
        ex.set_sheets_week(ss(thousands(d.stats.sheets_this_week)));
        ex.set_sheets_today(ss(format!("+{} today", thousands(d.stats.sheets_today))));
        ex.set_open_exceptions(ss(thousands(d.stats.open_exceptions)));
        ex.set_open_errors(ss(format!("{} ERROR or BLOCKER", d.stats.open_errors)));
        match d.stats.median_review_secs {
            Some(m) => {
                ex.set_median_review(ss(format!("{m:.1}")));
                ex.set_median_caption(ss("Marks lane, this week"));
            }
            None => {
                ex.set_median_review(ss("–"));
                ex.set_median_caption(ss("review times are not recorded yet"));
            }
        }
        ex.set_throughput(ss(d
            .stats
            .throughput_pages_per_sec
            .map_or("–".to_string(), |t| format!("{t:.0}"))));

        let statuses = [
            ("All", None),
            ("Setup", Some(b::ExamStatus::Setup)),
            ("Scanning", Some(b::ExamStatus::Scanning)),
            ("In review", Some(b::ExamStatus::InReview)),
            ("Ready to release", Some(b::ExamStatus::Ready)),
            ("Released", Some(b::ExamStatus::Released)),
        ];
        ex.set_filters(sync(
            ex.get_filters(),
            statuses
                .iter()
                .map(|(label, st)| s::FilterChip {
                    label: ss(label),
                    count: d
                        .exams
                        .iter()
                        .filter(|e| st.is_none_or(|x| e.status == x))
                        .count() as i32,
                })
                .collect(),
        ));
        let (filter, current, drawer_open) = {
            let st = self.st.borrow();
            (st.exam_filter, st.exam.clone(), st.drawer_open)
        };
        ex.set_filter(filter as i32);
        let wanted = statuses.get(filter).and_then(|x| x.1);
        let shown: Vec<&b::ExamSummary> = d
            .exams
            .iter()
            .filter(|e| wanted.is_none_or(|w| e.status == w))
            .collect();
        let rows: Vec<s::ExamRow> = shown
            .iter()
            .map(|e| {
                let progress = if e.sheets_expected == 0 {
                    0.0
                } else {
                    e.sheets_scanned as f32 / e.sheets_expected as f32
                };
                let c = e.open_exceptions;
                s::ExamRow {
                    id: ss(&e.id.0),
                    code: ss(&e.course_code),
                    title: ss(&e.course_title),
                    title_ar: ss(&e.course_title_ar),
                    assessment: ss(&e.assessment),
                    date: ss(&e.date),
                    date_hijri: ss(&e.date_hijri),
                    term: ss(&e.term.label),
                    forms: model(e.versions.iter().map(ss).collect()),
                    scanned: ss(thousands(e.sheets_scanned)),
                    expected: ss(thousands(e.sheets_expected)),
                    progress,
                    progress_tone: if progress >= 1.0 {
                        s::Tone::Success
                    } else if e.status == b::ExamStatus::Setup {
                        s::Tone::Neutral
                    } else {
                        s::Tone::Warning
                    },
                    info: c.info as i32,
                    warn: c.warn as i32,
                    error: c.error as i32,
                    blocker: c.blocker as i32,
                    status: ss(e.status.label()),
                    status_tone: status_tone(e.status),
                    label: ss(format!(
                        "{} {}, {}, {}, {} of {} sheets, {} open exceptions",
                        e.course_code,
                        e.course_title,
                        e.assessment,
                        e.status.label(),
                        e.sheets_scanned,
                        e.sheets_expected,
                        c.total()
                    )),
                }
            })
            .collect();
        let selected = current
            .as_ref()
            .and_then(|id| shown.iter().position(|e| &e.id == id))
            .map_or(-1, |i| i as i32);
        self.st.borrow_mut().exam_rows = shown.iter().map(|e| e.id.clone()).collect();
        ex.set_rows(sync_with(ex.get_rows(), rows, |o, n| {
            adopt(&o.forms, &mut n.forms)
        }));
        ex.set_selected(selected);

        let cur = current
            .as_ref()
            .and_then(|id| d.exams.iter().find(|e| &e.id == id));
        ex.set_drawer_open(drawer_open && cur.is_some());
        if let Some(e) = cur {
            app.set_course(ss(join(&[&e.course_code, &e.course_title], " ")));
            app.set_assessment(ss(&e.assessment));
            app.set_term(ss(&e.term.label));
            app.set_term_gregorian(ss(&e.term.gregorian));
            let open = e.open_exceptions;
            let review_open: u32 = self.lane_total(&e.id);
            app.set_review_open(review_open as i32);
            let (title, text, t) = match e.status {
                b::ExamStatus::InReview if open.blocking() > 0 => (
                    "Review required".to_string(),
                    format!(
                        "{} ERROR exception{} block{} release, and {} warning{} need{} a look.",
                        open.blocking(),
                        if open.blocking() == 1 { "" } else { "s" },
                        if open.blocking() == 1 { "s" } else { "" },
                        open.warn,
                        if open.warn == 1 { "" } else { "s" },
                        if open.warn == 1 { "s" } else { "" },
                    ),
                    b::Tone::Warning,
                ),
                b::ExamStatus::InReview => (
                    "Review in progress".into(),
                    "Only warnings remain open.".into(),
                    b::Tone::Warning,
                ),
                b::ExamStatus::Scanning => (
                    "Scanning in progress".into(),
                    format!(
                        "{} of {} sheets recognised; exceptions reach the review queue as they appear.",
                        thousands(e.sheets_scanned),
                        thousands(e.sheets_expected)
                    ),
                    b::Tone::Info,
                ),
                b::ExamStatus::Ready => (
                    "Ready to release".into(),
                    "No ERROR exceptions are open. Release is a deliberate act of the instructor."
                        .into(),
                    b::Tone::Success,
                ),
                b::ExamStatus::Released => (
                    "Released".into(),
                    "Results were released; later changes need four-eyes approval (§10.8).".into(),
                    b::Tone::Success,
                ),
                b::ExamStatus::Setup => (
                    "Setup in progress".into(),
                    "Publish the key and approve the CLO mapping before scanning.".into(),
                    b::Tone::Neutral,
                ),
            };
            ex.set_drawer(s::ExamDrawer {
                code: ss(&e.course_code),
                title: ss(&e.course_title),
                title_ar: ss(&e.course_title_ar),
                assessment: ss(&e.assessment),
                status: ss(e.status.label()),
                status_tone: status_tone(e.status),
                callout_title: ss(title),
                callout_text: ss(text),
                callout_tone: tone(t),
                review_label: ss(format!("Continue review ({review_open})")),
                can_review: review_open > 0,
                key_status: ss(&e.key_status),
                mapped: e.mapped_items as i32,
                mapped_word: ss(if e.mapping_approval {
                    "approved"
                } else {
                    "mapped"
                }),
                total: e.total_items as i32,
                roster: ss(if e.roster_count == 0 {
                    "No roster".to_string()
                } else {
                    format!("{} students", thousands(e.roster_count))
                }),
                sections: ss(e.sections.join(", ")),
                last_scan: ss(&e.last_scan),
                batch_id: ss(&e.batch_id),
                term: ss(join(&[&e.term.label, &e.term.gregorian], " · ")),
                term_ar: ss(&e.term.label_ar),
            });
        }
    }

    fn lane_total(&self, exam: &ExamId) -> u32 {
        self.backend
            .borrow()
            .review_queue(exam, Lane::Marks)
            .lanes
            .iter()
            .map(|l| l.open)
            .sum()
    }

    // -------------------------------------------------------------------------- refresh: setup

    fn refresh_setup(&self) {
        let ui = self.ui();
        let su = ui.global::<s::SetupVM>();
        let setup = self
            .exam()
            .and_then(|e| self.backend.borrow().exam_setup(&e));
        su.set_available(setup.is_some());
        let Some(setup) = setup else {
            su.set_has_keysheets(false);
            su.set_ks_open(false);
            return;
        };
        su.set_mappings_proposed(setup.mappings_proposed as i32);
        let t = &setup.template;
        su.set_template_id(ss(&t.id));
        su.set_template_kind(ss(match t.kind {
            b::TemplateKind::Legacy => format!("Legacy form · {}", t.name),
            b::TemplateKind::Generated => format!("Generated · {}", t.name),
        }));
        let labels = match t.label_set {
            b::LabelSet::Latin => "Latin labels",
            b::LabelSet::Arabic => "Arabic labels",
            b::LabelSet::Numeric => "numeric labels",
        };
        let kv = |l: &str, v: String| s::KV {
            label: ss(l),
            value: ss(v),
            tone: s::Tone::Neutral,
        };
        su.set_template(sync(
            su.get_template(),
            vec![
                kv(
                    "Questions",
                    format!("{} of {} printed", t.questions, t.capacity),
                ),
                kv(
                    "Options",
                    format!(
                        "{}–{} · {labels}",
                        t.option_labels.first().cloned().unwrap_or_default(),
                        t.option_labels.last().cloned().unwrap_or_default()
                    ),
                ),
                kv("Versions", t.versions.join(", ")),
                kv("Cancel mode", t.cancel_mode.label().into()),
                kv("Resolution", format!("{} dpi", t.dpi)),
                kv("Page identity", t.page_identity.clone()),
                kv("Digest", t.digest.clone()),
            ],
        ));
        su.set_versions(sync(
            su.get_versions(),
            setup.keys.iter().map(|k| ss(&k.version)).collect(),
        ));
        su.set_options(sync(
            su.get_options(),
            t.option_labels.iter().map(ss).collect(),
        ));
        su.set_has_roster(setup.roster_students > 0);
        su.set_key_status(ss(&setup.key_status));
        su.set_key_published(setup.key_published);
        su.set_can_replace_key(self.backend.borrow().can_replace_key(&setup.exam));
        su.set_mapping_mode(ss(match setup.mapping_mode {
            b::MappingMode::Split => "Split mode: each row's weights sum to 1",
            b::MappingMode::Full => "Full mode: each mapped CLO gets the full item",
        }));
        let total_points: f64 = setup
            .mapping
            .iter()
            .map(|r| r.points)
            .sum::<f64>()
            .max(1e-9);
        su.set_clos(sync(
            su.get_clos(),
            setup
                .clos
                .iter()
                .enumerate()
                .map(|(ci, c)| {
                    let weight = |r: &b::MappingRow| r.cells.get(ci).and_then(|c| c.weight);
                    let evidence: f64 = setup
                        .mapping
                        .iter()
                        .map(|r| weight(r).unwrap_or(0.0) * r.points)
                        .sum();
                    let items = setup.mapping.iter().filter(|r| weight(r).is_some()).count();
                    let share = (evidence / total_points) as f32;
                    s::CloHead {
                        code: ss(&c.code),
                        domain: ss(c.domain.letter()),
                        title: ss(&c.title),
                        title_ar: ss(&c.title_ar),
                        plo: ss(&c.plo),
                        share,
                        share_text: ss(format!("{:.0}%", share * 100.0)),
                        items: items as i32,
                    }
                })
                .collect(),
        ));
        let (sel_row, sel_col) = {
            let mut st = self.st.borrow_mut();
            st.sel_row = st.sel_row.min(setup.mapping.len().saturating_sub(1));
            st.sel_col = st.sel_col.min(setup.clos.len().saturating_sub(1));
            (st.sel_row, st.sel_col)
        };
        let rows: Vec<s::MapRow> = setup
            .mapping
            .iter()
            .enumerate()
            .map(|(ri, r)| {
                let sum: f64 = r.cells.iter().filter_map(|c| c.weight).sum();
                let any = r.cells.iter().any(|c| c.weight.is_some());
                let sum_ok = match setup.mapping_mode {
                    b::MappingMode::Split => !any || (sum - 1.0).abs() < 1e-9,
                    b::MappingMode::Full => true,
                };
                let label = format!("Q{:02}", r.question);
                s::MapRow {
                    q: r.question as i32,
                    label: ss(&label),
                    keys: model(
                        setup
                            .keys
                            .iter()
                            .map(|k| ss(k.answers.get(ri).cloned().unwrap_or_default()))
                            .collect(),
                    ),
                    points: ss(fmt_weight(Some(r.points))),
                    cells: model(
                        r.cells
                            .iter()
                            .enumerate()
                            .map(|(ci, c)| s::MapCell {
                                text: ss(fmt_weight(c.weight)),
                                weight: c.weight.unwrap_or(0.0) as f32,
                                mapped: c.weight.is_some(),
                                has_note: !c.note.is_empty(),
                                selected: ri == sel_row && ci == sel_col,
                                label: ss(format!(
                                    "{label} {} weight",
                                    setup.clos.get(ci).map_or("", |c| c.code.as_str())
                                )),
                            })
                            .collect(),
                    ),
                    sum: ss(if any {
                        fmt_weight(Some(sum))
                    } else {
                        "–".into()
                    }),
                    sum_ok,
                    status: ss(match r.status {
                        b::MappingStatus::Draft => "draft",
                        b::MappingStatus::Proposed => "proposed",
                        b::MappingStatus::Approved => "approved",
                        b::MappingStatus::InUse => "in use",
                    }),
                    status_tone: match r.status {
                        b::MappingStatus::Draft => s::Tone::Neutral,
                        b::MappingStatus::Proposed => s::Tone::Accent,
                        b::MappingStatus::Approved => s::Tone::Success,
                        b::MappingStatus::InUse => s::Tone::Primary,
                    },
                }
            })
            .collect();
        su.set_rows(sync_with(su.get_rows(), rows, |o, n| {
            adopt(&o.keys, &mut n.keys);
            adopt(&o.cells, &mut n.cells);
        }));
        su.set_lints(sync(
            su.get_lints(),
            setup
                .lints
                .iter()
                .map(|l| s::LintRow {
                    code: ss(&l.code),
                    severity: severity(l.severity),
                    message: ss(&l.message),
                })
                .collect(),
        ));
        show_selected_cell(&su, &setup, sel_row, sel_col);
        su.set_ks_option_labels(sync(
            su.get_ks_option_labels(),
            t.option_labels.iter().map(ss).collect(),
        ));
        self.refresh_keysheets();
    }

    /// Moves the matrix selection to the cell (`row`, `col`). A cell that takes focus calls this,
    /// so only the two cells' marks and the cell editor change: rebuilding the rows would destroy
    /// the text input that has just taken the focus.
    pub fn select_cell(&self, row: usize, col: usize) {
        let before = {
            let mut st = self.st.borrow_mut();
            let before = (st.sel_row, st.sel_col);
            st.sel_row = row;
            st.sel_col = col;
            before
        };
        let ui = self.ui();
        let su = ui.global::<s::SetupVM>();
        let rows = su.get_rows();
        let mark = |(r, c): (usize, usize), on: bool| {
            let Some(cells) = rows.row_data(r).map(|x| x.cells) else {
                return;
            };
            if let Some(mut cell) = cells.row_data(c)
                && cell.selected != on
            {
                cell.selected = on;
                cells.set_row_data(c, cell);
            }
        };
        mark(before, false);
        mark((row, col), true);
        let setup = self
            .exam()
            .and_then(|e| self.backend.borrow().exam_setup(&e));
        if let Some(setup) = setup {
            show_selected_cell(&su, &setup, row, col);
        }
    }

    /// The key read from key sheets (§10.1), as the instructor checks and confirms it: one
    /// version at a time, each row's scan beside what it gives the key, the rows that wait for
    /// a decision highlighted with what to do.
    fn refresh_keysheets(&self) {
        let ui = self.ui();
        let su = ui.global::<s::SetupVM>();
        let exam = self.exam();
        su.set_can_read_keysheets(
            exam.as_ref()
                .is_some_and(|e| self.backend.borrow().can_replace_key(e)),
        );
        let sum = exam
            .as_ref()
            .and_then(|e| self.backend.borrow().key_sheets(e));
        su.set_has_keysheets(sum.is_some());
        let Some(sum) = sum else {
            su.set_ks_open(false);
            return;
        };
        // the key sheets of a held exam (or files just read) are what Setup is about until
        // confirmed: shown on their own the first time the exam is
        if (sum.pending || sum.preview) && self.st.borrow().ks_seen != exam {
            self.st.borrow_mut().ks_seen = exam.clone();
            su.set_ks_open(true);
        }
        let vi = {
            let mut st = self.st.borrow_mut();
            st.ks_version = st.ks_version.min(sum.versions.len().saturating_sub(1));
            st.ks_version
        };
        let picks = self.st.borrow().ks_picks.clone();
        su.set_ks_source(ss(&sum.source));
        su.set_ks_preview(sum.preview);
        su.set_ks_pending(sum.pending);
        su.set_ks_consequence(ss(&sum.consequence));
        let undecided = |v: &b::KeySheetVersion| {
            v.rows
                .iter()
                .filter(|r| r.status.open() && !picks.contains_key(&(v.form, r.question)))
                .count()
        };
        su.set_ks_versions(sync(
            su.get_ks_versions(),
            sum.versions
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let state = if v.confirmed.is_some() {
                        "confirmed".to_string()
                    } else {
                        match undecided(v) {
                            0 => "ready to confirm".to_string(),
                            1 => "1 to decide".to_string(),
                            n => format!("{n} to decide"),
                        }
                    };
                    s::KsVersion {
                        label: ss(format!("Version {} · {state}", v.version)),
                        open: undecided(v) as i32,
                        confirmed: v.confirmed.is_some(),
                        selected: i == vi,
                        a11y: ss(format!("Key sheets version {}, {state}", v.version)),
                    }
                })
                .collect(),
        ));
        su.set_ks_version(vi as i32);
        let mut notes: Vec<SharedString> = sum
            .other_versions
            .iter()
            .map(|(v, from)| {
                ss(format!(
                    "Version {v}: no key sheet; its key comes from {from}"
                ))
            })
            .collect();
        notes.extend(sum.notes.iter().map(ss));
        su.set_ks_notes(sync(su.get_ks_notes(), notes));
        let scan_row = |k: &b::KeySheetScan| s::KsScan {
            label: ss(&k.label),
            origin: ss(&k.origin),
            id_read: ss(&k.id_read),
            version: ss(&k.version),
            registration: ss(&k.registration),
            refusal: ss(k.refusal.as_deref().unwrap_or("")),
        };
        let Some(v) = sum.versions.get(vi) else {
            // no usable key sheet: only the refusals and what to do
            su.set_ks_version_title(ss("No key read"));
            su.set_ks_version_status(ss(
                "No key sheet gives a key: see why beside each sheet, rescan it, or replace the \
                 key from a key file",
            ));
            su.set_ks_rows(sync(su.get_ks_rows(), Vec::new()));
            su.set_ks_crops(sync(su.get_ks_crops(), Vec::new()));
            su.set_ks_scans(sync(
                su.get_ks_scans(),
                sum.unusable.iter().map(scan_row).collect(),
            ));
            su.set_ks_confirm_label(ss("Confirm"));
            su.set_ks_can_confirm(false);
            su.set_ks_can_confirm_all(false);
            su.set_ks_confirmed(false);
            return;
        };
        su.set_ks_version_letter(ss(&v.version));
        su.set_ks_scans(sync(
            su.get_ks_scans(),
            v.sheets
                .iter()
                .chain(sum.unusable.iter())
                .map(scan_row)
                .collect(),
        ));
        let left = undecided(v);
        let decided_here = v
            .rows
            .iter()
            .filter(|r| picks.contains_key(&(v.form, r.question)))
            .count();
        su.set_ks_version_title(ss(format!(
            "Version {} · {} questions",
            v.version,
            v.rows.len()
        )));
        su.set_ks_version_status(ss(match (&v.confirmed, left) {
            (Some(by), _) => format!("Confirmed: {by}"),
            (None, 0) if decided_here > 0 => format!(
                "Every row read or decided ({decided_here} by you): check them against the \
                 sheet, then confirm"
            ),
            (None, 0) => {
                "Every row read clearly: check them against the sheet, then confirm".to_string()
            }
            (None, 1) => "1 highlighted row needs your decision before you confirm".to_string(),
            (None, n) => format!("{n} highlighted rows need your decision before you confirm"),
        }));
        su.set_ks_confirmed(v.confirmed.is_some());
        su.set_ks_confirm_label(ss(format!("Confirm version {}", v.version)));
        su.set_ks_can_confirm(v.confirmed.is_none() && left == 0);
        let waiting: Vec<&b::KeySheetVersion> = sum
            .versions
            .iter()
            .filter(|x| x.confirmed.is_none())
            .collect();
        su.set_ks_can_confirm_all(
            sum.versions.len() > 1
                && !waiting.is_empty()
                && waiting.iter().all(|x| undecided(x) == 0),
        );
        let labels: Vec<String> = su
            .get_ks_option_labels()
            .iter()
            .map(|l| l.to_string())
            .collect();
        let letter = |o: u8| {
            labels
                .get(usize::from(o))
                .cloned()
                .unwrap_or_else(|| char::from(b'A' + o).to_string())
        };
        su.set_ks_rows(sync(
            su.get_ks_rows(),
            v.rows
                .iter()
                .map(|r| {
                    let pick = picks
                        .get(&(v.form, r.question))
                        .cloned()
                        .unwrap_or_default();
                    let marked = if r.status == b::KeyRowStatus::MultiMark && r.marked.len() > 1 {
                        r.marked
                            .iter()
                            .map(|&o| letter(o))
                            .collect::<Vec<_>>()
                            .join("|")
                    } else {
                        String::new()
                    };
                    let tone = match r.status {
                        b::KeyRowStatus::Read => s::Tone::Success,
                        b::KeyRowStatus::Decided => s::Tone::Accent,
                        b::KeyRowStatus::Blank | b::KeyRowStatus::Disagree => s::Tone::Error,
                        _ => s::Tone::Warning,
                    };
                    let gives = if r.status.open() {
                        if pick.is_empty() {
                            "needs your decision".to_string()
                        } else {
                            format!("your answer {pick}, recorded when you confirm")
                        }
                    } else if r.answer.is_empty() {
                        "gives no key entry".to_string()
                    } else {
                        format!("key {}", r.answer)
                    };
                    s::KsRow {
                        q: ss(format!("Q{:02}", r.question)),
                        read: ss(&r.read),
                        answer: ss(if r.answer.is_empty() {
                            "—"
                        } else {
                            &r.answer
                        }),
                        status: ss(r.status.label()),
                        status_tone: tone,
                        open: r.status.open(),
                        highlight: r.status != b::KeyRowStatus::Read,
                        pick: ss(&pick),
                        marked: ss(&marked),
                        detail: ss(&r.detail),
                        key_now: ss(r.key_file.as_ref().map_or(String::new(), |k| {
                            format!("exam's key now: {}", if k.is_empty() { "–" } else { k })
                        })),
                        has_crop: r.crop.is_some(),
                        label: ss(format!(
                            "Question {} version {}: the sheet reads {}, {}, {gives}",
                            r.question,
                            v.version,
                            r.read,
                            r.status.label()
                        )),
                    }
                })
                .collect(),
        ));
        let blank = blank_sheet();
        su.set_ks_crops(sync(
            su.get_ks_crops(),
            v.rows
                .iter()
                .map(|r| r.crop.as_ref().map_or_else(|| blank.clone(), image))
                .collect(),
        ));
    }

    // -------------------------------------------------------------------------- refresh: scan

    fn refresh_scan(&self) {
        let ui = self.ui();
        let sc = ui.global::<s::ScanVM>();
        let exam = self.exam();
        let p = exam
            .as_ref()
            .and_then(|e| self.backend.borrow().batch_progress(e));
        sc.set_available(p.is_some());
        if let Some(e) = &exam {
            sc.set_review_open(self.lane_total(e) as i32);
        }
        let Some(p) = p else { return };
        let total = p.pages_total.max(1);
        sc.set_batch_label(ss(&p.batch_label));
        sc.set_source(ss(&p.source));
        sc.set_device(ss(&p.device));
        sc.set_running(p.running);
        sc.set_pps(p.pages_per_sec as f32);
        sc.set_pps_max(p.max_pages_per_sec as f32);
        sc.set_pps_floor(p.floor_pages_per_sec as f32);
        sc.set_pps_text(ss(format!("{:.0}", p.pages_per_sec)));
        sc.set_pages_value(ss(thousands(p.pages_done)));
        sc.set_pages_caption(ss(format!(
            "of {} · {:.0} %",
            thousands(p.pages_total),
            100.0 * p.pages_done as f64 / total as f64
        )));
        sc.set_auto_value(ss(format!("{:.1}", p.auto_accepted * 100.0)));
        sc.set_auto_caption(ss(format!(
            "questions · AR {:.1} %",
            p.abstention_rate * 100.0
        )));
        sc.set_first_value(ss(format!("{:.1}", p.first_result.as_secs_f64())));
        sc.set_first_caption(ss("target < 3 s (A3)"));
        let secs = p.remaining.as_secs();
        sc.set_remaining_value(ss(format!("{}:{:02}", secs / 60, secs % 60)));
        sc.set_remaining_caption(ss(if p.queued + p.in_flight > 0 {
            format!("m:ss · {} pages queued", thousands(p.queued + p.in_flight))
        } else {
            "batch complete".to_string()
        }));
        sc.set_done_frac(p.pages_done as f32 / total as f32);
        sc.set_flight_frac(p.in_flight as f32 / total as f32);
        sc.set_stages(sync(
            sc.get_stages(),
            p.stages
                .iter()
                .map(|st| s::KV {
                    label: ss(&st.stage),
                    value: ss(thousands(st.count)),
                    tone: if st.stage == "Quarantined" && st.count > 0 {
                        s::Tone::Warning
                    } else {
                        s::Tone::Neutral
                    },
                })
                .collect(),
        ));
        let max = p
            .exceptions
            .iter()
            .map(|c| c.count)
            .max()
            .unwrap_or(1)
            .max(1);
        // Worst codes first (the backend sorts by severity, then count). Seven rows fit the column
        // at 1280 × 800; a longer list shows six and counts the rest (they are in the queue).
        const CODE_ROWS: usize = 7;
        let shown = if p.exceptions.len() > CODE_ROWS {
            CODE_ROWS - 1
        } else {
            p.exceptions.len()
        };
        sc.set_codes_more((p.exceptions.len() - shown) as i32);
        sc.set_codes(sync(
            sc.get_codes(),
            p.exceptions
                .iter()
                .take(shown)
                .map(|c| s::CodeBar {
                    code: ss(&c.code),
                    title: ss(&c.title),
                    severity: severity(c.severity),
                    count: c.count as i32,
                    frac: c.count as f32 / max as f32,
                })
                .collect(),
        ));
        let thumbs: Vec<s::Thumb> = p
            .thumbnails
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let (state, code, sev, word) = match &t.state {
                    b::ThumbState::Auto => (
                        0,
                        String::new(),
                        b::Severity::Info,
                        "auto-accepted".to_string(),
                    ),
                    b::ThumbState::Review { code, severity } => {
                        (1, code.clone(), *severity, format!("needs review, {code}"))
                    }
                    b::ThumbState::Rejected { code, severity } => {
                        (2, code.clone(), *severity, format!("rejected, {code}"))
                    }
                    b::ThumbState::Reading => {
                        (3, String::new(), b::Severity::Info, "reading".into())
                    }
                    b::ThumbState::Queued => (4, String::new(), b::Severity::Info, "queued".into()),
                    // the instructor's key sheet: never a student (§10.1)
                    b::ThumbState::KeySheet {
                        version,
                        confirmed,
                        refused,
                    } => (
                        if *refused { 6 } else { 5 },
                        if version.is_empty() {
                            "KEY".to_string()
                        } else {
                            format!("KEY {version}")
                        },
                        if *refused {
                            b::Severity::Error
                        } else {
                            b::Severity::Info
                        },
                        format!(
                            "{}not a student, {}",
                            if version.is_empty() {
                                String::new()
                            } else {
                                format!("version {version}, ")
                            },
                            if *refused {
                                "gives no key: open Setup for why"
                            } else if *confirmed {
                                "its key is confirmed"
                            } else {
                                "its key waits for your confirmation in Setup"
                            }
                        ),
                    ),
                };
                let is_key = matches!(t.state, b::ThumbState::KeySheet { .. });
                s::Thumb {
                    serial: ss(&t.serial),
                    state,
                    code: ss(code),
                    severity: severity(sev),
                    variant: i as i32,
                    label: ss(if is_key {
                        format!("Key sheet {}, {word}", t.serial)
                    } else {
                        format!("Sheet {}, {word}", t.serial)
                    }),
                }
            })
            .collect();
        sc.set_thumbs(sync(sc.get_thumbs(), thumbs));
        let blank = blank_sheet();
        sc.set_thumb_images(sync(
            sc.get_thumb_images(),
            p.thumbnails
                .iter()
                .map(|t| t.image.as_ref().map_or_else(|| blank.clone(), image))
                .collect(),
        ));
        // sheets, not exception items: a sheet with three open items is one sheet to review
        sc.set_rejected_count(ss(thousands(p.sheets.rejected)));
        sc.set_review_count(ss(thousands(p.sheets.review)));
        sc.set_auto_count(ss(thousands(p.sheets.auto)));
        sc.set_key_sheet_count(p.sheets.key_sheets as i32);
        sc.set_pausable(p.pausable);
        sc.set_can_add(p.append.allowed && !p.running);
        sc.set_add_note(ss(&p.append.note));
        sc.set_recon(sync(
            sc.get_recon(),
            p.reconciliation
                .iter()
                .map(|r| s::KV {
                    label: ss(&r.label),
                    value: ss(&r.value),
                    tone: tone(r.tone),
                })
                .collect(),
        ));
        match &p.reconciliation_code {
            Some((code, sev)) => {
                sc.set_recon_code(ss(code));
                sc.set_recon_severity(severity(*sev));
            }
            None => sc.set_recon_code(SharedString::new()),
        }
        sc.set_activity(sync(sc.get_activity(), p.activity.iter().map(ss).collect()));
        sc.set_engine(ss(&p.engine));
        sc.set_workers(p.workers as i32);
    }

    // -------------------------------------------------------------------------- refresh: review

    fn refresh_review(&self) {
        let ui = self.ui();
        let rv = ui.global::<s::ReviewVM>();
        let Some(exam) = self.exam() else {
            rv.set_has_item(false);
            return;
        };
        let lane = self.st.borrow().lane;
        let q = self.backend.borrow().review_queue(&exam, lane);
        self.st.borrow_mut().lanes = q.lanes.iter().map(|l| l.lane).collect();
        rv.set_lanes(sync(
            rv.get_lanes(),
            q.lanes
                .iter()
                .map(|l| s::LaneChip {
                    label: ss(l.lane.label()),
                    count: l.open as i32,
                    selected: l.lane == lane,
                })
                .collect(),
        ));
        rv.set_resolved(q.resolved as i32);
        rv.set_total(q.total as i32);
        rv.set_median(ss(q
            .median_secs_per_item
            .map_or("median time not recorded".to_string(), |m| {
                format!("median {m:.1} s / item")
            })));
        rv.set_audit_head(ss(&q.audit_head));
        rv.set_two_person(q.two_person_rule);
        ui.global::<s::App>()
            .set_review_open(q.lanes.iter().map(|l| l.open).sum::<u32>() as i32);

        let (current, collapsed) = {
            let st = self.st.borrow();
            (st.item, st.collapsed.clone())
        };
        let mut entries = Vec::new();
        let mut order = Vec::new();
        let mut queue_ids = Vec::new();
        let mut y = 0.0_f32;
        let mut selected_y = 0.0_f32;
        for g in &q.groups {
            let expanded = !collapsed.contains(&g.code);
            entries.push(s::QueueEntry {
                header: true,
                code: ss(&g.code),
                title: ss(&g.title),
                severity: severity(g.severity),
                count: g.items.len() as i32,
                expanded,
                ..Default::default()
            });
            y += 32.0;
            for it in &g.items {
                order.push(it.id);
                if !expanded {
                    continue;
                }
                let selected = Some(it.id) == current;
                if selected {
                    selected_y = y;
                }
                let blind = it.kind == ReviewKind::BlindVerify;
                queue_ids.push(it.id);
                entries.push(s::QueueEntry {
                    header: false,
                    code: ss(&g.code),
                    severity: severity(it.severity),
                    id: queue_ids.len() as i32 - 1,
                    label: ss(&it.label),
                    detail: ss(&it.detail),
                    serial: ss(match it.kind {
                        ReviewKind::Mark | ReviewKind::BlindVerify => {
                            format!("Booklet {}", it.serial)
                        }
                        _ => g.code.clone(),
                    }),
                    // the sparkline of a blind item stays empty, whatever the backend sent
                    fills: model(if blind { Vec::new() } else { it.fills.clone() }),
                    selected,
                    blind,
                    deferred: it.deferred,
                    a11y: ss(format!(
                        "{}{}, {}{}{}",
                        it.label,
                        if it.question > 0 {
                            format!(" {}, booklet {}", it.detail, it.serial)
                        } else {
                            format!(", {}", it.detail)
                        },
                        g.code,
                        if blind { ", blind verify" } else { "" },
                        if it.deferred { ", deferred" } else { "" }
                    )),
                    ..Default::default()
                });
                y += 62.0;
            }
        }
        rv.set_queue(sync_with(rv.get_queue(), entries, |o, n| {
            adopt(&o.fills, &mut n.fills)
        }));
        rv.set_selected_y(selected_y);
        rv.set_empty_text(ss(q
            .unavailable
            .clone()
            .unwrap_or_else(|| format!("No open items in the {} lane", lane.label()))));
        {
            let mut st = self.st.borrow_mut();
            st.order = order;
            st.queue_ids = queue_ids;
        }
        self.push_item();
    }

    /// Pushes the current item and the reviewer's pending choice into the decision frame.
    fn push_item(&self) {
        let ui = self.ui();
        let rv = ui.global::<s::ReviewVM>();
        let st = self.st.borrow();
        rv.set_overlay(st.overlay);
        rv.set_show_all(st.show_all);
        rv.set_registered(st.registered);
        rv.set_zoom(st.zoom);
        rv.set_help_open(st.help);
        let Some(c) = st.current.as_ref() else {
            rv.set_has_item(false);
            rv.set_code(SharedString::new());
            rv.set_reason_needed(false);
            rv.set_note_required(false);
            return;
        };
        rv.set_has_item(true);
        rv.set_help_text(ss(help_for(&c.code)));
        let kind = match c.kind {
            // a form item is decided in the mark frame: its options are the version bubbles
            ReviewKind::Mark | ReviewKind::Form => 0,
            ReviewKind::BlindVerify => 1,
            ReviewKind::Identity => 2,
            ReviewKind::Notice => 3,
        };
        rv.set_kind(kind);
        rv.set_code(ss(&c.code));
        rv.set_code_title(ss(&c.code_title));
        rv.set_sub_reason(ss(&c.sub_reason));
        rv.set_severity(severity(c.severity));
        // without a separate original the crop is the scan itself, overlays projected onto it
        let view = match (c.original.is_some(), st.registered) {
            (false, _) => "scan",
            (true, true) => "registered",
            (true, false) => "original scan",
        };
        rv.set_caption(ss(match c.kind {
            ReviewKind::Mark | ReviewKind::BlindVerify => format!(
                "Question {} · Form {} · booklet {} · page {} · {view}, {} dpi",
                c.question, c.form, c.serial, c.page, c.dpi
            ),
            ReviewKind::Form => format!(
                "Version bubbles · booklet {} · page {} · {view}, {} dpi",
                c.serial, c.page, c.dpi
            ),
            ReviewKind::Identity => format!(
                "Booklet {} · student number grid · {view}, {} dpi",
                c.serial, c.dpi
            ),
            ReviewKind::Notice => format!("{} · {}", c.code, c.code_title),
        }));
        rv.set_question_label(ss(if c.kind == ReviewKind::Form {
            "Version".to_string()
        } else {
            format!("Question {}", c.question)
        }));
        rv.set_crop(image(&c.crop));
        rv.set_crop_w(c.crop.width as f32);
        rv.set_crop_h(c.crop.height as f32);
        match &c.original {
            Some(o) => {
                rv.set_original(image(o));
                rv.set_has_original(true);
            }
            None => rv.set_has_original(false),
        }
        let labels: Vec<String> = c.bubbles.iter().map(|b| b.label.clone()).collect();
        // Blind verify (§13.4): the engine's reading is withheld. The shell masks it too, so a
        // backend that sends fills by mistake still cannot leak them into the frame.
        let blind = c.kind == ReviewKind::BlindVerify;
        let bubble = |b: &b::BubbleView| {
            let (fill, state) = if blind {
                (0.0, b::MarkState::Blank)
            } else {
                (b.fill, b.state)
            };
            s::BubbleVM {
                label: ss(&b.label),
                x: b.cx,
                y: b.cy,
                r: b.r,
                fill,
                fill_text: if blind {
                    SharedString::new()
                } else {
                    ss(format!("{fill:.2}"))
                },
                mark: mark(state),
                overlay: !blind && (state != b::MarkState::Blank || st.show_all),
            }
        };
        rv.set_bubbles(sync(
            rv.get_bubbles(),
            c.bubbles.iter().map(bubble).collect(),
        ));
        rv.set_fill_bars(sync(
            rv.get_fill_bars(),
            if matches!(c.kind, ReviewKind::Mark | ReviewKind::Form) {
                c.bubbles.iter().map(bubble).collect()
            } else {
                Vec::new()
            },
        ));
        rv.set_threshold(c.threshold);
        rv.set_threshold_text(ss(format!(
            "Threshold {:.2} · hysteresis band {:.2}–{:.2}",
            c.threshold, c.band.0, c.band.1
        )));
        rv.set_band_lo(c.band.0);
        rv.set_band_hi(c.band.1);
        // blind: no suggestion reaches the frame, whatever the backend sent (§13.4)
        let shown = c.suggestion.as_ref().filter(|_| !blind);
        let sugg = shown.map(|s| s.decision);
        rv.set_has_suggestion(shown.is_some());
        if let Some(sg) = shown {
            rv.set_suggestion_letter(ss(decision_word(sg.decision, &labels)));
            rv.set_suggestion_word(ss(decision_word(sg.decision, &labels)));
            rv.set_suggestion_source(ss(match &sg.source {
                b::SuggestionSource::Engine => "Engine suggestion".to_string(),
                b::SuggestionSource::Rule(r) => format!("Rule suggestion · {r}"),
                b::SuggestionSource::Ai { model } => format!("AI suggestion · {model}"),
            }));
            rv.set_suggestion_detail(ss(match (c.kind, sg.decision) {
                (ReviewKind::Identity, Decision::Candidate(i)) => c
                    .identity
                    .as_ref()
                    .and_then(|e| e.candidates.get(i as usize))
                    .map(|cd| format!("Candidate {} · {} · {:.2}", i + 1, cd.name, cd.posterior))
                    .unwrap_or_default(),
                (ReviewKind::Identity, d) => {
                    format!("{} · posterior {:.2}", decision_word(d, &labels), sg.fill)
                }
                // a form is inferred from the answers, not read from a fill
                (ReviewKind::Form, _) => {
                    format!("Likelihood margin {:.1} nats", sg.margin)
                }
                (_, Decision::Option(_)) => {
                    format!("Filled {:.2} · margin {:.2}", sg.fill, sg.margin)
                }
                _ => format!("Top fill {:.2} · margin {:.2}", sg.fill, sg.margin),
            }));
            rv.set_suggestion_tag(ss(join(&[&c.code, &c.sub_reason.replace('_', " ")], " · ")));
        }
        let pending = st.pending;
        rv.set_options(sync(
            rv.get_options(),
            labels
                .iter()
                .enumerate()
                .map(|(i, l)| s::ChipVM {
                    label: ss(l),
                    key: ss(l),
                    suggested: sugg == Some(Decision::Option(i as u8)),
                    selected: pending == Some(Decision::Option(i as u8)),
                })
                .collect(),
        ));
        let specials: Vec<(Decision, &str, &str)> = match c.kind {
            ReviewKind::Identity => vec![
                (Decision::NotOnRoster, "Not on roster", "N"),
                (Decision::Duplicate, "Duplicate", "D"),
            ],
            // a booklet has exactly one form: only a version letter decides it
            ReviewKind::Form => Vec::new(),
            _ => vec![
                (Decision::Blank, "Blank", "Bksp"),
                (Decision::Multi, "Multi", "Ctrl+M"),
                (Decision::Unreadable, "Unreadable", "Ctrl+U"),
            ],
        };
        rv.set_specials(sync(
            rv.get_specials(),
            specials
                .iter()
                .map(|&(d, l, k)| s::ChipVM {
                    label: ss(l),
                    key: ss(k),
                    suggested: sugg == Some(d),
                    selected: pending == Some(d),
                })
                .collect(),
        ));
        if let Some(ev) = &c.identity {
            rv.set_read_id(ss(&ev.read_id));
            rv.set_boxes_read(ss(&ev.boxes_read));
            rv.set_hypothesis(ss(&ev.hypothesis));
            rv.set_neighbours(sync(
                rv.get_neighbours(),
                ev.neighbours.iter().map(ss).collect(),
            ));
            rv.set_candidates(sync_with(
                rv.get_candidates(),
                ev.candidates
                    .iter()
                    .take(3)
                    .enumerate()
                    .map(|(i, cd)| s::Candidate {
                        rank: i as i32 + 1,
                        name: ss(&cd.name),
                        name_ar: ss(&cd.name_ar),
                        student_id: ss(&cd.student_id),
                        national_id: ss(&cd.national_id_masked),
                        meta: ss(join(
                            &[
                                &if cd.section.is_empty() {
                                    String::new()
                                } else {
                                    format!("Sec {}", cd.section)
                                },
                                &cd.campus,
                                &cd.track,
                                &if cd.seat.is_empty() {
                                    String::new()
                                } else {
                                    format!("seat {}", cd.seat)
                                },
                            ],
                            " · ",
                        )),
                        posterior: ss(format!("{:.2}", cd.posterior)),
                        channels: model(
                            cd.channels
                                .iter()
                                .map(|ch| s::KV {
                                    label: ss(&ch.channel),
                                    value: ss(&ch.detail),
                                    tone: if ch.agrees {
                                        s::Tone::Success
                                    } else {
                                        s::Tone::Error
                                    },
                                })
                                .collect(),
                        ),
                        suggested: sugg == Some(Decision::Candidate(i as u8)),
                        selected: pending == Some(Decision::Candidate(i as u8)),
                    })
                    .collect(),
                |o, n| adopt(&o.channels, &mut n.channels),
            ));
        } else {
            rv.set_candidates(sync(rv.get_candidates(), Vec::new()));
        }
        if let Some(n) = &c.notice {
            rv.set_notice_headline(ss(&n.headline));
            rv.set_notice_lines(sync(
                rv.get_notice_lines(),
                n.lines
                    .iter()
                    .map(|(l, v)| s::KV {
                        label: ss(l),
                        value: ss(v),
                        tone: s::Tone::Neutral,
                    })
                    .collect(),
            ));
            rv.set_notice_consequence(ss(&n.consequence));
        }
        rv.set_other_fills(sync(
            rv.get_other_fills(),
            if blind {
                Vec::new()
            } else {
                c.other_fills.clone()
            },
        ));
        rv.set_other_caption(ss(format!(
            "Typical full fill {:.2} · {} answered",
            c.typical_full_fill, c.answered
        )));
        rv.set_summary(ss(&c.summary));
        // an override needs a reason (§13.4, the canonical codes of §14.4): the family's first
        // codes and OTHER, the chosen one, and a last chip that lists every code on offer
        let typed = st.typed_student.as_deref();
        let needs = match typed {
            Some(id) => st
                .exam
                .as_ref()
                .is_some_and(|e| self.backend.borrow().needs_reason_for_student(e, c.id, id)),
            None => pending.is_some_and(|p| {
                st.exam
                    .as_ref()
                    .is_some_and(|e| self.backend.borrow().needs_reason(e, c.id, p))
            }),
        };
        let mut codes: Vec<String> = if st.reasons_all {
            c.reasons.clone()
        } else {
            let mut v: Vec<String> = c.reasons.iter().take(FIRST_REASONS).cloned().collect();
            for extra in ["OTHER"].into_iter().chain(st.reason.as_deref()) {
                if c.reasons.iter().any(|r| r == extra) && !v.iter().any(|r| r == extra) {
                    v.push(extra.to_string());
                }
            }
            v
        };
        let hidden = c.reasons.len().saturating_sub(codes.len());
        if st.reasons_all || hidden > 0 {
            codes.push(MORE_REASONS.into());
        }
        rv.set_reason_needed(needs && !c.reasons.is_empty());
        rv.set_reasons(sync(
            rv.get_reasons(),
            codes
                .iter()
                .map(|code| {
                    if code == MORE_REASONS {
                        s::ChipVM {
                            label: ss(if st.reasons_all {
                                "Fewer reasons".to_string()
                            } else {
                                format!("{hidden} more reasons…")
                            }),
                            key: ss("more"),
                            suggested: false,
                            selected: false,
                        }
                    } else {
                        s::ChipVM {
                            label: ss(reason_words(code)),
                            key: SharedString::new(),
                            suggested: false,
                            selected: st.reason.as_deref() == Some(code.as_str()),
                        }
                    }
                })
                .collect(),
        ));
        rv.set_note_required(
            needs
                && st
                    .reason
                    .as_ref()
                    .is_some_and(|r| c.reasons_needing_note.contains(r)),
        );
        let reason_given = st.reason.is_some();
        rv.set_typed_student(ss(typed.unwrap_or("")));
        let (label, enabled) = match (c.kind, pending) {
            _ if needs && !reason_given => ("Choose a reason".to_string(), false),
            (ReviewKind::Identity, _) if typed.is_some() => (
                format!("Accept student {}", typed.unwrap_or_default()),
                true,
            ),
            (ReviewKind::Notice, _) => ("Acknowledge".to_string(), true),
            (ReviewKind::BlindVerify, None) => ("Choose what is marked".to_string(), false),
            (ReviewKind::Identity, None) => match sugg {
                Some(Decision::Candidate(i)) => (format!("Accept candidate {}", i + 1), true),
                Some(d) => (format!("Accept: {}", decision_word(d, &labels)), true),
                None => ("Choose a candidate".into(), false),
            },
            (_, None) => ("Accept suggestion".to_string(), sugg.is_some()),
            (_, Some(p)) if Some(p) == sugg => ("Accept suggestion".to_string(), true),
            (ReviewKind::Identity, Some(p)) => {
                (format!("Accept {}", decision_word(p, &labels)), true)
            }
            (_, Some(p)) => (format!("Accept {}", decision_word(p, &labels)), true),
        };
        rv.set_accept_label(ss(label));
        rv.set_accept_enabled(enabled);
        rv.set_group_label(ss(format!("Apply to group · {}", c.group.size)));
        rv.set_group_enabled(c.group.allowed && c.kind == ReviewKind::Mark);
        rv.set_group_reason(ss(&c.group.reason));
        // contact sheet: every open member of the group, so batch confirm is an informed act
        let mut tiles = Vec::new();
        let mut contact_ids = Vec::new();
        if c.kind == ReviewKind::Mark
            && c.group.members.len() >= 2
            && let Some(exam) = st.exam.as_ref()
        {
            for &m in c.group.members.iter().take(6) {
                if let Some(it) = self.backend.borrow().review_item(exam, m) {
                    contact_ids.push(m);
                    tiles.push(s::ContactTile {
                        image: image(&it.crop),
                        id: contact_ids.len() as i32 - 1,
                        serial: ss(format!("Booklet {}", it.serial)),
                        current: m == c.id,
                        label: ss(format!(
                            "Open booklet {} in the group{}",
                            it.serial,
                            if m == c.id { ", current" } else { "" }
                        )),
                    });
                }
            }
        }
        rv.set_contact_caption(ss(format!(
            "{} open items share this group (code, question, suggestion, fill bucket)",
            c.group.members.len()
        )));
        rv.set_contact(sync(rv.get_contact(), tiles));
        drop(st);
        let mut st = self.st.borrow_mut();
        st.contact_ids = contact_ids;
        st.reason_codes = codes;
    }

    // -------------------------------------------------------------------------- refresh: results

    fn refresh_results(&self) {
        let ui = self.ui();
        let rs = ui.global::<s::ResultsVM>();
        let exam = self.exam();
        let r = exam.as_ref().and_then(|e| self.backend.borrow().results(e));
        rs.set_available(r.is_some());
        let Some(r) = r else {
            // why there is nothing to show, and where to go (a held key, §10.1)
            match exam.as_ref().and_then(|e| self.backend.borrow().hold(e)) {
                Some(why) => {
                    rs.set_empty_title(ss("Results wait for the key"));
                    rs.set_empty_text(ss(why));
                    rs.set_empty_to_setup(true);
                }
                None => {
                    rs.set_empty_title(ss("No results yet"));
                    rs.set_empty_text(ss(
                        "Results appear once every sheet of the exam is recognised and scored.",
                    ));
                    rs.set_empty_to_setup(false);
                }
            }
            return;
        };
        let st = &r.stats;
        rs.set_key_label(ss(&r.key_label));
        rs.set_students(ss(thousands(st.students)));
        rs.set_students_caption(ss(if st.roster == 0 {
            "no roster: absences are unknown".to_string()
        } else {
            format!("of {} on roster · {} absent", st.roster, st.absent)
        }));
        rs.set_mean(ss(format!("{:.1}", st.mean_pct)));
        rs.set_mean_caption(ss(format!(
            "median {:.0} % · SD {:.1}",
            st.median_pct, st.sd_pct
        )));
        rs.set_pass_rate(ss(format!("{:.1}", st.pass_rate * 100.0)));
        // the lowest passing band of the scale in force (Saudi scale: D, 60 %)
        let lowest_pass = r
            .grades
            .iter()
            .filter(|g| !g.fail)
            .min_by(|a, b| a.lo.total_cmp(&b.lo));
        rs.set_pass_caption(ss(lowest_pass.map_or(String::new(), |g| {
            format!("≥ {:.0} % ({} or better)", g.lo, g.letter)
        })));
        rs.set_alpha(ss(st
            .alpha
            .map_or("–".to_string(), |a| format!("{a:.2}"))));
        rs.set_alpha_caption(ss(match (st.alpha, st.alpha_ci) {
            (Some(_), Some((lo, hi))) => format!("Feldt 95 % CI {lo:.2}–{hi:.2}"),
            (Some(_), None) => "no CI at this N".to_string(),
            (None, _) => "not computable at this N".to_string(),
        }));
        rs.set_sem(ss(st
            .sem_points
            .map_or("–".to_string(), |x| format!("{x:.1}"))));
        rs.set_sem_caption(ss(if st.sem_points.is_some() {
            "±1.96 SEM band at the cut"
        } else {
            "needs α"
        }));
        rs.set_open_errors(ss(st.open_errors.to_string()));
        rs.set_open_caption(ss(if st.open_errors > 0 {
            "ERROR · holds release"
        } else {
            "none open"
        }));
        let max = r.grades.iter().map(|g| g.count).max().unwrap_or(1).max(1);
        rs.set_grades(sync(
            rs.get_grades(),
            r.grades
                .iter()
                .map(|g| s::GradeBar {
                    letter: ss(&g.letter),
                    letter_ar: ss(&g.letter_ar),
                    band: ss(match g.hi {
                        None => format!("≥ {:.0}", g.lo),
                        Some(h) if g.fail => format!("< {h:.0}"),
                        Some(h) => format!("{:.0}–{h:.0}", g.lo),
                    }),
                    count: g.count as i32,
                    frac: g.count as f32 / max as f32,
                    fail: g.fail,
                })
                .collect(),
        ));
        rs.set_statuses(sync(
            rs.get_statuses(),
            r.statuses
                .iter()
                .map(|x| s::StatusBarVM {
                    code: ss(&x.code),
                    label: ss(&x.label),
                    label_ar: ss(&x.label_ar),
                    count: x.count as i32,
                })
                .collect(),
        ));
        rs.set_scale_label(ss(&r.scale_label));
        let hmax = r.histogram.counts.iter().copied().max().unwrap_or(1).max(1);
        rs.set_histogram(sync(
            rs.get_histogram(),
            r.histogram
                .counts
                .iter()
                .map(|&c| c as f32 / hmax as f32)
                .collect(),
        ));
        rs.set_histogram_counts(sync(
            rs.get_histogram_counts(),
            r.histogram.counts.iter().map(|&c| c as i32).collect(),
        ));
        rs.set_pass_mark(r.histogram.pass_mark as f32);
        rs.set_mean_pct(r.histogram.mean as f32);
        rs.set_sem_pct(r.histogram.sem as f32);
        rs.set_histogram_caption(ss(format!(
            "N = {} · {:.0} % bins · mean {:.1} % (▲) · pass mark {:.0} % with ±1.96 SEM",
            st.students, r.histogram.bin_width, r.histogram.mean, r.histogram.pass_mark
        )));
        // Flagged items first: mis-key and flawed-item flags (ranks 1–4) with high confidence,
        // then those of low confidence "(proposed)", then the informational ones; within each
        // by rank, then stability (§12.2 "sort by rank, then stability"; the backend lists each
        // item's flags in rank order), then by question.
        let share = |f: &b::ItemFlag| match &f.stability {
            b::FlagStability::High { share, .. } | b::FlagStability::Low { share, .. } => {
                share.unwrap_or(1.0)
            }
            _ => 1.0,
        };
        let flag_rank = |it: &b::ItemStats| match it.flags.first() {
            None => (2u8, 0u8, u8::MAX, 0i64),
            Some(f) => (
                u8::from(f.rank > 4),
                u8::from(!f.stable),
                f.rank,
                -((share(f) * 1000.0).round() as i64),
            ),
        };
        let mut items: Vec<&b::ItemStats> = r.items.iter().collect();
        items.sort_by_key(|it| (flag_rank(it), it.question));
        rs.set_items(sync_with(
            rs.get_items(),
            items
                .into_iter()
                .map(|it| {
                    let flag = it.flags.first();
                    let flag_text = flag
                        .map(|f| {
                            let mut t = f.message.clone();
                            if let Some(c) = &f.code {
                                t.push_str(&format!(" · {c}"));
                            }
                            if !f.stable {
                                t.push_str(" (proposed)");
                            }
                            t
                        })
                        .unwrap_or_default();
                    let pct = |s: &Option<f64>| s.map(|x| format!("{:.0} %", x * 100.0));
                    // (cell text, tone, kind, words for the row's label)
                    let (stab, stab_tone, stab_kind, stab_words) = match flag.map(|f| &f.stability)
                    {
                        Some(b::FlagStability::High { share, note }) => (
                            pct(share).unwrap_or_else(|| "high".into()),
                            s::Tone::Success,
                            1,
                            format!("high confidence: {note}"),
                        ),
                        Some(b::FlagStability::Low { share, note }) => (
                            match pct(share) {
                                Some(p) => format!("{p} · proposed"),
                                None => "proposed".into(),
                            },
                            s::Tone::Accent,
                            2,
                            format!("low confidence, proposed: {note}"),
                        ),
                        Some(b::FlagStability::Computing) => (
                            "computing…".into(),
                            s::Tone::Neutral,
                            3,
                            "stability computing".into(),
                        ),
                        _ => (String::new(), s::Tone::Neutral, 0, String::new()),
                    };
                    let q = if it.label.is_empty() {
                        format!("Q{:02}", it.question)
                    } else {
                        it.label.clone()
                    };
                    // what was not computed reads "–", never 0
                    let p = it.p.map_or("–".to_string(), |p| format!("{p:.2}"));
                    let r = it.r_ir.map_or("–".to_string(), |r| signed(r, 2));
                    let band = it.band.map_or("not computed", |b| b.word());
                    s::ItemRow {
                        q: ss(&q),
                        p: ss(&p),
                        r: ss(&r),
                        r_ci: ss(it.r_ci.map_or("–".to_string(), |(lo, hi)| {
                            format!("[{}, {}]", signed(lo, 2), signed(hi, 2))
                        })),
                        r_negative: it.r_ir.is_some_and(|r| r < 0.0),
                        steps: it.band.map_or(0, |b| b.steps() as i32),
                        band: ss(band),
                        shares: model(it.option_shares.iter().map(|&x| x as f32).collect()),
                        keyed: it.keyed.map_or(-1, |k| k as i32),
                        flag: ss(&flag_text),
                        // a low-confidence flag is a proposal (gold, DESIGN §3.2), never the
                        // red of an established mis-key
                        flag_tone: flag.map_or(s::Tone::Neutral, |f| {
                            if f.stable {
                                tone(f.tone)
                            } else if f.rank <= 4 {
                                s::Tone::Accent
                            } else {
                                tone(f.tone)
                            }
                        }),
                        has_flag: flag.is_some(),
                        stability: ss(&stab),
                        stability_tone: stab_tone,
                        stability_kind: stab_kind,
                        selected: false,
                        label: ss(format!(
                            "{q}, p {p}, r {r}, {band}{}{}",
                            if flag_text.is_empty() {
                                String::new()
                            } else {
                                format!(", {flag_text}")
                            },
                            if stab_words.is_empty() {
                                String::new()
                            } else {
                                format!(", {stab_words}")
                            }
                        )),
                    }
                })
                .collect(),
            |o, n| adopt(&o.shares, &mut n.shares),
        ));
        rs.set_option_labels(sync(
            rs.get_option_labels(),
            r.option_labels.iter().map(ss).collect(),
        ));
        rs.set_readiness(sync(
            rs.get_readiness(),
            r.readiness
                .iter()
                .map(|c| s::CheckRow {
                    tone: tone(c.tone),
                    text: ss(&c.text),
                    action: ss(&c.action),
                })
                .collect(),
        ));
        rs.set_release_blocked(r.release_blocked.is_some());
        rs.set_release_reason(ss(r.release_blocked.unwrap_or_default()));
        rs.set_can_export(true);
        rs.set_blackboard_ready(r.blackboard_blocked.is_none());
        rs.set_blackboard_note(ss(r.blackboard_blocked.unwrap_or_else(|| {
            "Only final results go into the upload (Username = roster student ID, the raw score); \
             students with an open ERROR item are left out, so Blackboard keeps their current \
             value"
                .into()
        })));
    }

    // -------------------------------------------------------------------------- refresh: outcomes

    /// Fills `RemarkVM`: the import of the screen shown (the New-exam form's on Exams, the
    /// current exam's in Setup) and the current exam's parallel run.
    fn refresh_remark(&self) {
        let ui = self.ui();
        let rm = ui.global::<s::RemarkVM>();
        let exam = self.exam();
        let page = ui.global::<s::App>().get_page();
        {
            let be = self.backend.borrow();
            rm.set_can_new(be.can_import_remark(None));
            rm.set_can_setup(exam.as_ref().is_some_and(|e| be.can_import_remark(Some(e))));
            rm.set_can_compare(exam.is_some() && !be.is_demo());
        }
        let st = self.st.borrow();
        // the New-exam form's use of the import
        match (&st.remark_new_using, &st.remark_new) {
            (Some(files), Some(v)) => {
                let objectives: BTreeSet<&str> = v
                    .versions
                    .iter()
                    .flat_map(|x| x.objectives.iter().map(|o| o.code.as_str()))
                    .collect();
                rm.set_new_using(true);
                rm.set_new_using_text(ss(format!(
                    "From the Remark import ({files}): the key with exact points of versions {}{} \
                     · {} key-sheet row(s) excluded",
                    v.versions
                        .iter()
                        .map(|x| x.form.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    if objectives.is_empty() {
                        String::new()
                    } else {
                        format!(
                            " · {} learning objective(s) as CLO mappings, imported, needs review",
                            objectives.len()
                        )
                    },
                    v.key_sheets.len()
                )));
            }
            _ => {
                rm.set_new_using(false);
                rm.set_new_using_text(SharedString::new());
            }
        }
        let (context, view) = match page {
            s::Page::Exams => (1, st.remark_new.clone()),
            s::Page::Setup => (
                2,
                st.remark_setup
                    .as_ref()
                    .filter(|(e, _)| Some(e) == exam.as_ref())
                    .map(|(_, v)| v.clone()),
            ),
            _ => (0, None),
        };
        rm.set_context(context);
        if context == 2 && view.is_none() {
            rm.set_setup_open(false);
        }
        self.fill_import(&rm, view.as_ref(), st.remark_version, context);
        let run = st
            .parallel
            .as_ref()
            .filter(|(e, _)| Some(e) == exam.as_ref())
            .map(|(_, v)| v.clone());
        drop(st);
        self.fill_parallel(&rm, run.as_ref());
    }

    fn fill_import(
        &self,
        rm: &s::RemarkVM,
        view: Option<&b::RemarkImportView>,
        version: usize,
        context: i32,
    ) {
        let Some(v) = view else {
            rm.set_has(false);
            rm.set_refused(false);
            rm.set_versions(sync(rm.get_versions(), Vec::new()));
            rm.set_rows(sync(rm.get_rows(), Vec::new()));
            rm.set_facts(sync(rm.get_facts(), Vec::new()));
            rm.set_objectives(sync(rm.get_objectives(), Vec::new()));
            rm.set_key_sheets(sync(rm.get_key_sheets(), Vec::new()));
            rm.set_workbooks(sync(rm.get_workbooks(), Vec::new()));
            rm.set_notes(sync(rm.get_notes(), Vec::new()));
            return;
        };
        rm.set_apply_label(ss(if context == 1 {
            "Use this import"
        } else {
            "Apply key and points"
        }));
        if let Some(r) = &v.refusal {
            rm.set_has(false);
            rm.set_refused(true);
            rm.set_refusal(ss(&r.message));
            rm.set_remedy(ss(&r.remedy));
            rm.set_suggestion(ss(&r.suggestion));
            rm.set_title(ss("Remark import refused"));
            rm.set_files(ss(v.files.join(" · ")));
            rm.set_can_apply(false);
            rm.set_versions(sync(rm.get_versions(), Vec::new()));
            rm.set_rows(sync(rm.get_rows(), Vec::new()));
            rm.set_facts(sync(rm.get_facts(), Vec::new()));
            rm.set_objectives(sync(rm.get_objectives(), Vec::new()));
            rm.set_key_sheets(sync(rm.get_key_sheets(), Vec::new()));
            rm.set_workbooks(sync(rm.get_workbooks(), Vec::new()));
            rm.set_notes(sync(rm.get_notes(), Vec::new()));
            return;
        }
        rm.set_has(true);
        rm.set_refused(false);
        let vi = version.min(v.versions.len().saturating_sub(1));
        rm.set_version(vi as i32);
        rm.set_title(ss(format!(
            "Remark import · {} workbook(s) · version{} {} · {} questions",
            v.workbooks.len(),
            if v.versions.len() == 1 { "" } else { "s" },
            v.versions
                .iter()
                .map(|x| x.form.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            v.questions
        )));
        rm.set_files(ss(v.files.join(" · ")));
        rm.set_versions(sync(
            rm.get_versions(),
            v.versions
                .iter()
                .enumerate()
                .map(|(i, x)| s::RmVersion {
                    label: ss(format!("Version {} · {} pts", x.form, x.total)),
                    selected: i == vi,
                    a11y: ss(format!(
                        "Remark import version {}, {} questions, {} points",
                        x.form, v.questions, x.total
                    )),
                })
                .collect(),
        ));
        rm.set_applies(ss(&v.applies));
        rm.set_can_apply(v.can_apply);
        let kv = |label: &str, value: String, t: s::Tone| s::KV {
            label: ss(label),
            value: ss(value),
            tone: t,
        };
        if let Some(x) = v.versions.get(vi) {
            let mut facts = vec![
                kv("Points", x.points_source.clone(), s::Tone::Neutral),
                kv(
                    "Total · values",
                    format!("{} points · {}", x.total, x.distinct),
                    s::Tone::Neutral,
                ),
                kv(
                    "Remark totals",
                    format!(
                        "{} of {} rows reproduced exactly{}",
                        x.reproduced,
                        x.rows_checked,
                        if x.group_points_checked > 0 {
                            format!(
                                " · objective points {} of {}",
                                x.group_points_checked, x.group_points_checked
                            )
                        } else {
                            String::new()
                        }
                    ),
                    if x.reproduced == x.rows_checked {
                        s::Tone::Success
                    } else {
                        s::Tone::Error
                    },
                ),
                kv(
                    "Rows",
                    format!(
                        "{} students · {} key-sheet row(s) excluded",
                        x.students, x.key_sheet_rows
                    ),
                    s::Tone::Neutral,
                ),
            ];
            if !v.points_note.is_empty() {
                facts.push(kv("Points given", v.points_note.clone(), s::Tone::Accent));
            }
            rm.set_facts(sync(rm.get_facts(), facts));
            rm.set_key_counts(ss(&x.key_counts));
            let objective_of = |q: usize| -> String {
                x.objectives
                    .iter()
                    .find(|o| in_ranges(&o.questions, q))
                    .map(|o| o.code.clone())
                    .unwrap_or_default()
            };
            rm.set_rows(sync(
                rm.get_rows(),
                (0..usize::from(v.questions))
                    .map(|q| s::RmRow {
                        q: ss(format!("{:02}", q + 1)),
                        key: ss(x.key.get(q).cloned().unwrap_or_default()),
                        points: ss(x.points.get(q).cloned().unwrap_or_default()),
                        objective: ss(objective_of(q + 1)),
                    })
                    .collect(),
            ));
        }
        // the learning objectives of every version (one CLO each), with their questions
        let mut codes: Vec<String> = Vec::new();
        for x in &v.versions {
            for o in &x.objectives {
                if !codes.contains(&o.code) {
                    codes.push(o.code.clone());
                }
            }
        }
        rm.set_objectives(sync(
            rm.get_objectives(),
            codes
                .iter()
                .map(|code| {
                    let per: Vec<(&str, &b::RemarkObjective)> = v
                        .versions
                        .iter()
                        .filter_map(|x| {
                            x.objectives
                                .iter()
                                .find(|o| &o.code == code)
                                .map(|o| (x.form.as_str(), o))
                        })
                        .collect();
                    let same = per.windows(2).all(|w| w[0].1.questions == w[1].1.questions);
                    let detail = if same {
                        per.first()
                            .map(|(_, o)| {
                                format!("questions {} · {} points", o.questions, o.points)
                            })
                            .unwrap_or_default()
                    } else {
                        per.iter()
                            .map(|(f, o)| {
                                format!(
                                    "version {f}: questions {} · {} points",
                                    o.questions, o.points
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("; ")
                    };
                    s::RmObjective {
                        code: ss(code),
                        label: ss(per.first().map(|(_, o)| o.label.as_str()).unwrap_or("")),
                        detail: ss(format!("{detail} · weight 1")),
                    }
                })
                .collect(),
        ));
        rm.set_key_sheets(sync(
            rm.get_key_sheets(),
            v.key_sheets
                .iter()
                .map(|k| s::RmKeySheet {
                    title: ss(format!("{} row {} · ID {}", k.workbook, k.row, k.masked_id)),
                    detail: ss(&k.why),
                })
                .collect(),
        ));
        rm.set_workbooks(sync(
            rm.get_workbooks(),
            v.workbooks
                .iter()
                .map(|w| s::KV {
                    label: ss(&w.file),
                    value: ss(format!(
                        "{} · {} students · {} key sheet(s)",
                        w.form, w.students, w.key_sheets
                    )),
                    tone: s::Tone::Neutral,
                })
                .collect(),
        ));
        rm.set_notes(sync(rm.get_notes(), v.notes.iter().map(ss).collect()));
    }

    fn fill_parallel(&self, rm: &s::RemarkVM, run: Option<&b::ParallelRunView>) {
        let Some(v) = run else {
            rm.set_cmp_has(false);
            rm.set_cmp_open(false);
            rm.set_cmp_refused(false);
            rm.set_diffs(sync(rm.get_diffs(), Vec::new()));
            rm.set_diff_crops(sync(rm.get_diff_crops(), Vec::new()));
            rm.set_identity(sync(rm.get_identity(), Vec::new()));
            rm.set_totals(sync(rm.get_totals(), Vec::new()));
            rm.set_cmp_key_sheets(sync(rm.get_cmp_key_sheets(), Vec::new()));
            rm.set_cmp_workbooks(sync(rm.get_cmp_workbooks(), Vec::new()));
            rm.set_cmp_tiles(sync(rm.get_cmp_tiles(), Vec::new()));
            rm.set_cmp_hold(ss(""));
            rm.set_cmp_can_queue(false);
            return;
        };
        rm.set_cmp_has(true);
        rm.set_banner(ss(&v.banner));
        rm.set_cmp_title(ss(v.files.join(" · ")));
        rm.set_cmp_points(ss(&v.points));
        if let Some(r) = &v.refusal {
            rm.set_cmp_refused(true);
            rm.set_cmp_refusal(ss(&r.message));
            rm.set_cmp_remedy(ss(&r.remedy));
            rm.set_cmp_suggestion(ss(&r.suggestion));
            rm.set_diffs(sync(rm.get_diffs(), Vec::new()));
            rm.set_diff_crops(sync(rm.get_diff_crops(), Vec::new()));
            rm.set_cmp_tiles(sync(rm.get_cmp_tiles(), Vec::new()));
            rm.set_cmp_hold(ss(""));
            rm.set_cmp_can_queue(false);
            return;
        }
        rm.set_cmp_refused(false);
        let c = &v.counts;
        let paired = c.paired_by_id + c.paired_by_partial_id + c.paired_by_answers;
        let tile = |label: &str, value: String, caption: String, tone: s::Tone| s::CmpTile {
            label: ss(label),
            value: ss(value),
            caption: ss(caption),
            tone,
        };
        let open = c.differ + c.differ_pending;
        rm.set_cmp_hold(ss(&v.hold));
        rm.set_cmp_can_queue(c.not_queued > 0);
        rm.set_cmp_tiles(sync(
            rm.get_cmp_tiles(),
            vec![
                tile(
                    "BOOKLETS PAIRED",
                    format!("{paired}/{}", c.booklets),
                    format!(
                        "ID {} · partial {} · answers {}",
                        c.paired_by_id, c.paired_by_partial_id, c.paired_by_answers
                    ),
                    s::Tone::Neutral,
                ),
                tile(
                    "QUESTIONS EQUAL",
                    c.agree.to_string(),
                    format!("of {} · {} pending, same", c.questions, c.agree_pending),
                    s::Tone::Neutral,
                ),
                tile(
                    "TO ADJUDICATE",
                    open.to_string(),
                    if c.not_queued > 0 {
                        format!(
                            "{} settled · {} pending · {} not in Review",
                            c.differ, c.differ_pending, c.not_queued
                        )
                    } else {
                        format!(
                            "{} settled · {} pending · {} held in Review",
                            c.differ, c.differ_pending, c.held_in_review
                        )
                    },
                    if c.differ > 0 {
                        s::Tone::Error
                    } else if open > 0 {
                        s::Tone::Warning
                    } else {
                        s::Tone::Success
                    },
                ),
                tile(
                    "TOTALS EQUAL",
                    format!("{}/{}", c.totals_settled_equal, c.totals_settled),
                    if c.maximum_differences > 0 {
                        "the exam's maximum is not Remark's: other points".to_string()
                    } else {
                        format!(
                            "this exam's grades · provisional {}/{} if resolved",
                            c.totals_provisional_equal_if_remark, c.totals_provisional
                        )
                    },
                    if c.totals_settled_equal == c.totals_settled && c.maximum_differences == 0 {
                        s::Tone::Success
                    } else {
                        s::Tone::Error
                    },
                ),
                tile(
                    "IDENTITY",
                    c.identity.to_string(),
                    "differences to check".into(),
                    if c.identity > 0 {
                        s::Tone::Warning
                    } else {
                        s::Tone::Success
                    },
                ),
                tile(
                    "KEY SHEETS",
                    c.key_sheet_rows.to_string(),
                    "graded as students".into(),
                    if c.key_sheet_rows > 0 {
                        s::Tone::Warning
                    } else {
                        s::Tone::Neutral
                    },
                ),
            ],
        ));
        let mut crops = Vec::new();
        let diffs: Vec<s::CmpDiff> = v
            .differences
            .iter()
            .map(|d| {
                let (status, tone) = match d.agreement {
                    b::ParallelAgreement::Differ => {
                        (format!("differs · ours {}", d.status), s::Tone::Error)
                    }
                    b::ParallelAgreement::DifferPending => (
                        "differs · ours pending review".to_string(),
                        s::Tone::Warning,
                    ),
                    b::ParallelAgreement::AgreePending => (
                        "same reading · ours pending review".to_string(),
                        s::Tone::Info,
                    ),
                };
                crops.push(d.crop.as_ref().map_or_else(blank_sheet, image));
                s::CmpDiff {
                    q: ss(format!("Q{:02}", d.question)),
                    booklet: ss(&d.booklet),
                    student: ss(&d.student),
                    remark: ss(&d.remark),
                    ours: ss(&d.ours),
                    label: ss(format!(
                        "Q{} {}: Remark {}, ours {}, {status}",
                        d.question, d.booklet, d.remark, d.ours
                    )),
                    status: ss(status),
                    tone,
                    findings: ss(&d.findings),
                    has_crop: d.crop.is_some(),
                }
            })
            .collect();
        rm.set_diffs(sync(rm.get_diffs(), diffs));
        rm.set_diff_crops(sync(rm.get_diff_crops(), crops));
        let line = |title: String, detail: String, tone: s::Tone| s::CmpLine {
            title: ss(title),
            detail: ss(detail),
            tone,
        };
        rm.set_identity(sync(
            rm.get_identity(),
            v.identity
                .iter()
                .map(|i| {
                    line(
                        i.kind.clone(),
                        format!(
                            "{} · ours {} · Remark {} · {}",
                            if i.booklet.is_empty() {
                                "no booklet"
                            } else {
                                i.booklet.as_str()
                            },
                            i.ours,
                            i.remark,
                            i.detail
                        ),
                        s::Tone::Warning,
                    )
                })
                .collect(),
        ));
        // the points first: totals cannot agree where the exam carries other points
        let mut totals: Vec<s::CmpLine> = Vec::new();
        if !v.maximum_differences.is_empty() {
            totals.push(line(
                format!("Maximum differs: {}", v.maximum_differences.join("; ")),
                "this exam is graded with other points than Remark's, so no total can equal \
                 Remark's; check the key's points (the totals are this exam's own grades)"
                    .into(),
                s::Tone::Error,
            ));
        }
        if !v.points_differences.is_empty() {
            totals.push(line(
                format!(
                    "Points differ on {} question(s)",
                    v.points_differences.len()
                ),
                v.points_differences
                    .iter()
                    .take(6)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("; ")
                    + if v.points_differences.len() > 6 {
                        " …"
                    } else {
                        ""
                    },
                s::Tone::Warning,
            ));
        }
        totals.extend(v.totals.iter().map(|t| {
            let pending = !t.pending.is_empty();
            line(
                format!("{}: ours {}, Remark {}", t.booklet, t.ours, t.remark),
                if pending {
                    format!(
                        "{} · pending Q{}: {} if resolved as Remark read · {}",
                        t.student, t.pending, t.if_remark, t.grades
                    )
                } else {
                    format!("{} · {}", t.student, t.grades)
                },
                if pending {
                    s::Tone::Info
                } else {
                    s::Tone::Error
                },
            )
        }));
        rm.set_totals(sync(rm.get_totals(), totals));
        rm.set_cmp_key_sheets(sync(
            rm.get_cmp_key_sheets(),
            v.key_sheets
                .iter()
                .map(|k| {
                    line(
                        format!("{}: {} key-sheet row(s)", k.workbook, k.rows),
                        format!(
                            "Remark reported {} · without them {}",
                            k.reported, k.without
                        ),
                        s::Tone::Warning,
                    )
                })
                .collect(),
        ));
        let mut books: Vec<s::CmpLine> = v
            .workbooks
            .iter()
            .map(|w| {
                if w.in_scope {
                    line(
                        format!(
                            "{} · {} · {}/{} paired · {} key sheet(s)",
                            w.file, w.form, w.paired, w.rows, w.key_sheets
                        ),
                        String::new(),
                        s::Tone::Success,
                    )
                } else {
                    line(
                        format!("{} · {} · another section, not compared", w.file, w.form),
                        String::new(),
                        s::Tone::Neutral,
                    )
                }
            })
            .collect();
        for k in &v.key_differences {
            books.push(line(
                format!("Remark's key differs: {k}"),
                "the exam's key and Remark's Answer Key row disagree; check the key".into(),
                s::Tone::Error,
            ));
        }

        rm.set_cmp_workbooks(sync(rm.get_cmp_workbooks(), books));
    }

    fn refresh_outcomes(&self) {
        let ui = self.ui();
        let ov = ui.global::<s::OutcomesVM>();
        let o = self.exam().and_then(|e| self.backend.borrow().outcomes(&e));
        ov.set_available(o.is_some());
        let Some(o) = o else { return };
        let p = &o.policy;
        let method = match p.class_method {
            b::ClassMethod::Average => "average μ",
            b::ClassMethod::ThresholdProportion => "π",
            b::ClassMethod::PerQuestion => "ρ (per question)",
            b::ClassMethod::Levels => "attainment levels",
            b::ClassMethod::Eamu => "EAMU bands",
        };
        ov.set_policy(ss(format!(
            "Direct evidence · {} · T = {}, τ = {} · verdict on {method}, {} basis",
            p.mapping_basis,
            pct(p.target),
            pct(p.tau),
            p.verdict_basis
        )));
        ov.set_method_label(ss(format!(
            "{} · τ {} · T {}",
            match p.class_method {
                b::ClassMethod::Average => "Average",
                b::ClassMethod::ThresholdProportion => "Threshold-proportion",
                b::ClassMethod::PerQuestion => "Per-question",
                b::ClassMethod::Levels => "Levels",
                b::ClassMethod::Eamu => "EAMU",
            },
            pct(p.tau),
            pct(p.target)
        )));
        ov.set_policy_version(ss(&p.version));
        ov.set_target(p.target as f32);
        ov.set_classification(ss(&o.classification));
        let deciding = match p.class_method {
            b::ClassMethod::Average => 0,
            b::ClassMethod::ThresholdProportion => 1,
            b::ClassMethod::PerQuestion | b::ClassMethod::Levels | b::ClassMethod::Eamu => -1,
        };
        let ci = |(lo, hi): (f64, f64)| format!("{:.0}–{:.0} %", lo * 100.0, hi * 100.0);
        let rows: Vec<s::CloRow> = o
            .clos
            .iter()
            .map(|c| {
                let (verdict, vt) = match c.verdict {
                    b::Verdict::Attained => ("Attained", s::Tone::Success),
                    b::Verdict::NotAttained => ("Not attained", s::Tone::Error),
                    b::Verdict::InsufficientEvidence => ("Insufficient evidence", s::Tone::Warning),
                };
                s::CloRow {
                    code: ss(&c.code),
                    domain: ss(c.domain.letter()),
                    title: ss(&c.title),
                    title_ar: ss(&c.title_ar),
                    target: c.target as f32,
                    average: c.average as f32,
                    average_text: ss(format!("μ {:.0}%", c.average * 100.0)),
                    average_lo: c.average_ci.0 as f32,
                    average_hi: c.average_ci.1 as f32,
                    proportion: c.proportion as f32,
                    proportion_text: ss(format!("π {:.0}%", c.proportion * 100.0)),
                    proportion_lo: c.proportion_ci.0 as f32,
                    proportion_hi: c.proportion_ci.1 as f32,
                    average_ci_text: ss(format!("μ CI {}", ci(c.average_ci))),
                    proportion_ci_text: ss(format!("π CI {}", ci(c.proportion_ci))),
                    deciding,
                    n: ss(c.n.to_string()),
                    verdict: ss(verdict),
                    verdict_tone: vt,
                    badge: c.ci_contains_target,
                    note: ss(&c.mapping_note),
                    label: ss(format!(
                        "{} {}, domain {}, average {:.0} % (95 % CI {}), share at or above threshold {:.0} % (95 % CI {}), verdict on {method}: {verdict}{}, n {}",
                        c.code,
                        c.title,
                        c.domain.letter(),
                        c.average * 100.0,
                        ci(c.average_ci),
                        c.proportion * 100.0,
                        ci(c.proportion_ci),
                        if c.ci_contains_target { ", within the 95 % CI of the target" } else { "" },
                        c.n
                    )),
                }
            })
            .collect();
        let attained = o
            .clos
            .iter()
            .filter(|c| c.verdict == b::Verdict::Attained)
            .count();
        let near = o.clos.iter().filter(|c| c.ci_contains_target).count();
        let missing: Vec<&str> = o
            .clos
            .iter()
            .filter(|c| c.verdict == b::Verdict::NotAttained)
            .map(|c| c.code.as_str())
            .collect();
        ov.set_attained_summary(ss(format!(
            "{attained} of {} CLOs attained · {near} within the 95 % CI of the target{}",
            o.clos.len(),
            if missing.is_empty() {
                String::new()
            } else {
                format!(
                    " · {} need{} a B.2 recommendation",
                    missing.join(", "),
                    if missing.len() == 1 { "s" } else { "" }
                )
            }
        )));
        ov.set_clos(sync(ov.get_clos(), rows));
        // §11.4: the TP-154 Actual level prints μ and π; the footnote names the deciding method,
        // τ and T (and the policy version the result is bound to)
        ov.set_footnote(ss(format!(
            "Actual level: π is the share of students with P ≥ τ = {}; μ is the class average. \
             The verdict uses {method} against T = {} ({} basis) · {}.",
            pct(p.tau),
            pct(p.target),
            p.verdict_basis,
            p.version
        )));
        let mut b1 = Vec::new();
        let mut last = None;
        for r in &o.tp154_b1 {
            if last != Some(r.domain) {
                let n = match r.domain {
                    b::NqfDomain::Knowledge => "1.0",
                    b::NqfDomain::Skills => "2.0",
                    b::NqfDomain::Values => "3.0",
                };
                b1.push(s::B1Row {
                    header: true,
                    number: ss(n),
                    domain_title: ss(r.domain.title()),
                    domain_title_ar: ss(r.domain.title_ar()),
                    ..Default::default()
                });
                last = Some(r.domain);
            }
            // characters per line of each wrapping column at its width (text-xs)
            let lines = [
                (&r.clo, 48),
                (&r.methods, 24),
                (&r.actual, 23),
                (&r.comment, 36),
            ]
            .iter()
            .map(|(t, per)| t.chars().count().div_ceil(*per))
            .max()
            .unwrap_or(1)
            .clamp(1, 4);
            b1.push(s::B1Row {
                header: false,
                lines: lines as i32,
                number: ss(&r.number),
                clo: ss(&r.clo),
                plo: ss(&r.plo),
                methods: ss(&r.methods),
                targeted: ss(&r.targeted),
                actual: ss(&r.actual),
                comment: ss(&r.comment),
                comment_tone: if r.comment.starts_with("Not attained") {
                    s::Tone::Error
                } else {
                    s::Tone::Neutral
                },
                ..Default::default()
            });
        }
        ov.set_b1(sync(ov.get_b1(), b1));
    }
}

/// Current pending choice and item, for tests and the accessible review list.
impl Shell {
    /// Switches the review lane without selecting an item.
    /// Selects a cell of the mapping matrix (row = item index, col = CLO index).
    /// Opens or closes the Add scans bar (Scan) and the Exports bar (Results).
    /// Opens or closes the floating bars (screenshots): Add scans, Exports and the New-exam
    /// form (opened only where the backend can create an exam).
    pub fn set_bars(&self, add_scans: bool, exports: bool, new_exam: bool) {
        let ui = self.ui();
        ui.global::<s::ScanVM>().set_add_open(add_scans);
        ui.global::<s::ResultsVM>().set_export_open(exports);
        ui.global::<s::ExamsVM>()
            .set_new_open(new_exam && self.backend.borrow().can_create_exam());
    }

    /// Closes the Remark panels, bars and views and clears the status line (screenshots stage
    /// each screen afresh).
    pub fn close_remark(&self) {
        self.say("", s::Tone::Neutral);
        let ui = self.ui();
        let rm = ui.global::<s::RemarkVM>();
        rm.set_new_panel(false);
        rm.set_setup_bar(false);
        rm.set_setup_open(false);
        rm.set_cmp_bar(false);
        rm.set_cmp_open(false);
    }

    /// Reads Remark's workbooks (`;` between several) for the screen shown, as *Read export*
    /// does: the New-exam form's import on Exams (its panel open), Setup's import in Setup.
    pub fn remark_import(&self, workbooks: &str) {
        let ui = self.ui();
        let rm = ui.global::<s::RemarkVM>();
        rm.set_path(ss(workbooks));
        if ui.global::<s::App>().get_page() == s::Page::Exams {
            rm.set_new_panel(true);
        }
        self.read_remark(workbooks, "");
    }

    /// Runs the parallel run with Remark's workbooks, as *Compare* does.
    pub fn remark_compare(&self, workbooks: &str) {
        let ui = self.ui();
        ui.global::<s::RemarkVM>().set_cmp_path(ss(workbooks));
        self.compare_remark(compare_request(workbooks));
    }

    /// Shows (or hides) Setup's key summary of the key read from key sheets. Asks the backend
    /// whether the current exam has one (Setup's view-model is read only when Setup is shown),
    /// and counts as the exam's first look at it, so Setup does not open the summary on its
    /// own when it is shown next.
    pub fn show_key_sheets(&self, open: bool) {
        let exam = self.exam();
        let has = exam
            .as_ref()
            .is_some_and(|e| self.backend.borrow().key_sheets(e).is_some());
        self.st.borrow_mut().ks_seen = exam;
        self.ui().global::<s::SetupVM>().set_ks_open(open && has);
    }

    pub fn set_lane(&self, lane: Lane) {
        let mut st = self.st.borrow_mut();
        st.lane = lane;
        st.item = None;
        st.current = None;
        st.pending = None;
    }
    pub fn current_item(&self) -> Option<ItemId> {
        self.st.borrow().item
    }
    pub fn pending(&self) -> Option<Decision> {
        self.st.borrow().pending
    }
    pub fn lane(&self) -> Lane {
        self.st.borrow().lane
    }
    /// Model row count helper for tests.
    pub fn queue_len(&self) -> usize {
        self.ui().global::<s::ReviewVM>().get_queue().row_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_parse_in_latin_and_arabic_digits() {
        assert_eq!(parse_weight(""), Ok(None));
        assert_eq!(parse_weight(" 0.5 "), Ok(Some(0.5)));
        assert_eq!(parse_weight("0,25"), Ok(Some(0.25)));
        assert_eq!(parse_weight("٠٫٥"), Ok(Some(0.5)));
        assert_eq!(parse_weight("½"), Ok(Some(0.5)));
        assert!(parse_weight("abc").is_err());
    }

    #[test]
    fn scan_paths_split_and_expand_home() {
        let home = std::env::home_dir();
        let p = scan_paths(" day2/ ; \"b c.pdf\";;~/scans\n");
        assert_eq!(p.len(), 3);
        assert_eq!(p[0], std::path::PathBuf::from("day2/"));
        assert_eq!(p[1], std::path::PathBuf::from("b c.pdf"));
        if let Some(home) = &home {
            assert_eq!(p[2], home.join("scans"));
            let native = format!("~{}scans", std::path::MAIN_SEPARATOR);
            assert_eq!(scan_paths(&native), vec![home.join("scans")]);
        }
        assert!(scan_paths(" ; ").is_empty());
    }

    #[test]
    fn number_formatting() {
        assert_eq!(thousands(1284), "1,284");
        assert_eq!(thousands(12), "12");
        assert_eq!(signed(-0.14, 2), "\u{2212}0.14");
        assert_eq!(signed(-0.001, 2), "0.00");
        assert_eq!(fmt_weight(Some(1.0)), "1");
        assert_eq!(fmt_weight(Some(0.5)), "0.5");
        assert_eq!(fmt_weight(None), "");
    }
}
