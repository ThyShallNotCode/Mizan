//! Layout: turns a [`TemplateSpec`] into a [`Template`] whose geometry follows the §6.3 table.
//!
//! Every page carries four AprilTag 36h11 corner tags on the safe-box corners, two page-code
//! slots on the diagonal that avoids the binding corner (each with an echo line), and answer
//! columns that flow around the header and the codes, keeping 3 mm quiet zones. Page 1 adds the
//! student-ID grid, the instructions, the cancel instruction, the "how to mark" row and the
//! optional form grid. Questions that do not fit continue on further pages, balanced.
//!
//! The *geometry* ([`LayoutManifest`]) is hashed (§6.1); the *art* ([`PageArt`]: text, rules,
//! examples) is not.

use crate::geometry::*;
use crate::hash::{self, CanonicalError, Sha384Digest};
use crate::lint::{self, LintFinding, LintLevel};
use crate::pagecode::{PageCode, exam_id40};
use crate::spec::{
    CodeDiagonal, DigitLabels, Direction, LabelSet, Paper, SpecError, TemplateSpec,
};
use crate::units::{PtUm, RectUm, Um, um_to_mm};
use omr_model::{BookletSerial, CancelMode, FormCode, PageNo, PointMm};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Generator identifier recorded in (and hashed with) every manifest.
pub const GENERATOR: &str = "omr-forms/layout-v1";

// ---------------------------------------------------------------------------------------------
// Hashed geometry
// ---------------------------------------------------------------------------------------------

/// A page corner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Corner {
    /// Top left.
    TopLeft,
    /// Top right.
    TopRight,
    /// Bottom right.
    BottomRight,
    /// Bottom left.
    BottomLeft,
}

impl Corner {
    /// TL, TR, BR, BL (tag-set ID order, §6.4).
    pub const ALL: [Corner; 4] = [
        Corner::TopLeft,
        Corner::TopRight,
        Corner::BottomRight,
        Corner::BottomLeft,
    ];

    /// Position in [`Corner::ALL`].
    pub fn index(self) -> u16 {
        match self {
            Corner::TopLeft => 0,
            Corner::TopRight => 1,
            Corner::BottomRight => 2,
            Corner::BottomLeft => 3,
        }
    }
}

/// One AprilTag 36h11 corner tag.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FiducialTag {
    /// Tag ID on answer pages (tag set 0: TL 0, TR 1, BR 2, BL 3).
    pub id: u16,
    /// Which corner.
    pub corner: Corner,
    /// The 8 mm square including the one-module white border.
    pub outer: RectUm,
    /// The 6.4 mm black square.
    pub black: RectUm,
}

impl FiducialTag {
    /// The tag ID in tag set `set` (IDs 4·set … 4·set+3, §6.4).
    pub fn id_in_set(&self, set: u16) -> u16 {
        set * TAGS_PER_SET + self.corner.index()
    }

    /// The black square's corners on the page: top-left, top-right, bottom-right, bottom-left.
    pub fn corners(&self) -> [PtUm; 4] {
        let b = self.black;
        [
            PtUm::new(b.x, b.y),
            PtUm::new(b.right(), b.y),
            PtUm::new(b.right(), b.bottom()),
            PtUm::new(b.x, b.bottom()),
        ]
    }

    /// [`FiducialTag::corners`] in millimetres.
    pub fn corners_mm(&self) -> [PointMm; 4] {
        self.corners().map(PtUm::to_mm)
    }

    /// The tag centre in millimetres.
    pub fn centre_mm(&self) -> PointMm {
        self.black.centre().to_mm()
    }
}

/// Horizontal alignment of an echo line.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HAlign {
    /// Flush left.
    Left,
    /// Flush right.
    Right,
}

/// A page-code slot: a QR v3-M symbol and its echo line (§6.4).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CodeSlot {
    /// 0 = the upper copy, 1 = the lower copy.
    pub copy: u8,
    /// Corner region.
    pub corner: Corner,
    /// The symbol (29 × 29 modules, without the quiet zone).
    pub rect: RectUm,
    /// Module size.
    pub module: Um,
    /// Modules per side.
    pub modules: u16,
    /// The echo-line box under the symbol (two lines).
    pub echo: RectUm,
    /// Echo-line alignment (flush with the symbol's outer edge).
    pub echo_align: HAlign,
}

/// Which end of a row a tick is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    /// Left end.
    Left,
    /// Right end.
    Right,
}

/// A 1.2 × 0.6 mm row tick (a Stage C control point, §8.4).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RowTick {
    /// 0-based question of the row.
    pub question: u16,
    /// Row end.
    pub side: Side,
    /// The printed rectangle.
    pub rect: RectUm,
}

impl RowTick {
    /// Centre in millimetres.
    pub fn centre_mm(&self) -> PointMm {
        self.rect.centre().to_mm()
    }
}

/// An answer bubble. `bubble_id = "Q<question+1>/<option>"` (§6.1).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AnswerBubble {
    /// 0-based question.
    pub question: u16,
    /// 0-based option on the printed form.
    pub option: u8,
    /// Centre.
    pub centre: PtUm,
    /// Radius r₀ (outline centre line).
    pub radius: Um,
    /// Label printed inside.
    pub label: String,
}

/// A student-ID grid bubble.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IdBubble {
    /// Digit column, 0 = leftmost.
    pub column: u8,
    /// The digit value 0–9.
    pub digit: u8,
    /// Centre.
    pub centre: PtUm,
    /// Radius.
    pub radius: Um,
    /// Label printed inside.
    pub label: String,
}

/// A form/version grid bubble.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FormBubble {
    /// Form code (1 = A).
    pub form: u16,
    /// Centre.
    pub centre: PtUm,
    /// Radius.
    pub radius: Um,
    /// Label printed inside.
    pub label: String,
}

macro_rules! bubble_accessors {
    ($t:ty) => {
        impl $t {
            /// Centre in millimetres.
            pub fn centre_mm(&self) -> PointMm {
                self.centre.to_mm()
            }
            /// Radius in millimetres.
            pub fn radius_mm(&self) -> f64 {
                um_to_mm(self.radius)
            }
            /// Bounding square of the outline.
            pub fn rect(&self) -> RectUm {
                RectUm::around(self.centre, self.radius)
            }
        }
    };
}
bubble_accessors!(AnswerBubble);
bubble_accessors!(IdBubble);
bubble_accessors!(FormBubble);

impl AnswerBubble {
    /// The stable bubble ID, e.g. `Q17/2` (§6.1).
    pub fn bubble_id(&self) -> String {
        format!("Q{}/{}", self.question + 1, self.option)
    }
}

/// Kind of a bubble block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BlockKind {
    /// An answer column (logical index: 0 is where reading starts).
    Mcq {
        /// Logical column index.
        column: u8,
    },
    /// The student-ID grid.
    IdGrid,
    /// The form/version grid.
    FormGrid,
}

/// A block's printed extent (quiet zones apply around it).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Block {
    /// Kind.
    pub kind: BlockKind,
    /// Printed extent.
    pub rect: RectUm,
}

/// Where a question sits.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct QuestionGeom {
    /// 0-based question.
    pub question: u16,
    /// 1-based page.
    pub page: u16,
    /// Logical column on the page.
    pub column: u8,
    /// Row within the column.
    pub row: u16,
    /// Number of options.
    pub options: u8,
    /// Row centre line.
    pub row_y: Um,
    /// Index of option 0 in the page's `bubbles`.
    pub first_bubble: u32,
}

/// The hashed geometry of one page side.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PageGeometry {
    /// 1-based page (each side counts).
    pub page: u16,
    /// Corner tags, in [`Corner::ALL`] order.
    pub tags: Vec<FiducialTag>,
    /// The two page-code slots.
    pub codes: Vec<CodeSlot>,
    /// Bubble blocks.
    pub blocks: Vec<Block>,
    /// Answer bubbles, ordered by (question, option).
    pub bubbles: Vec<AnswerBubble>,
    /// ID-grid bubbles, ordered by (column, digit); page 1 only.
    pub id_bubbles: Vec<IdBubble>,
    /// Form-grid bubbles; page 1 only.
    pub form_bubbles: Vec<FormBubble>,
    /// Row ticks, ordered by (question, side).
    pub ticks: Vec<RowTick>,
}

impl PageGeometry {
    /// The 1-based page number.
    pub fn page_no(&self) -> PageNo {
        PageNo(self.page)
    }

    /// Questions on this page (0-based), ascending.
    pub fn questions(&self) -> impl Iterator<Item = u16> + '_ {
        let mut last = None;
        self.bubbles.iter().filter_map(move |b| {
            if last == Some(b.question) {
                None
            } else {
                last = Some(b.question);
                Some(b.question)
            }
        })
    }

    /// Tag of a corner.
    pub fn tag(&self, c: Corner) -> Option<&FiducialTag> {
        self.tags.iter().find(|t| t.corner == c)
    }
}

/// Bubble appearance parameters (hashed).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BubbleSpec {
    /// Shape (`circle`).
    pub shape: String,
    /// Diameter.
    pub diameter: Um,
    /// Outline weight.
    pub stroke: Um,
    /// Measured interior ratio (per mille of r₀).
    pub interior_permille: u16,
    /// Labels printed inside the bubble.
    pub label_inside: bool,
    /// Label gray (0 = black).
    pub label_gray: u8,
}

/// The layout manifest (§6.1): everything a recognizer needs, and exactly what is hashed.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LayoutManifest {
    /// Generator and layout version.
    pub generator: String,
    /// Paper.
    pub paper: Paper,
    /// Page width.
    pub width: Um,
    /// Page height.
    pub height: Um,
    /// Safe box.
    pub safe_box: RectUm,
    /// Marking convention.
    pub cancel_mode: CancelMode,
    /// Option labels.
    pub labels: LabelSet,
    /// Row direction.
    pub direction: Direction,
    /// ID-grid labels.
    pub id_digit_labels: DigitLabels,
    /// Bubble parameters.
    pub bubble: BubbleSpec,
    /// Number of forms.
    pub forms: u16,
    /// ID digits (0 = no ID grid).
    pub id_digits: u8,
    /// Every question's placement, by question.
    pub questions: Vec<QuestionGeom>,
    /// Pages.
    pub pages: Vec<PageGeometry>,
}

impl LayoutManifest {
    /// Canonical JSON of the manifest.
    pub fn canonical_json(&self) -> Result<String, CanonicalError> {
        hash::canonical_json(self)
    }

    /// SHA-384 of the canonical JSON: the template hash.
    pub fn sha384(&self) -> Result<Sha384Digest, CanonicalError> {
        hash::sha384_canonical(self)
    }
}

// ---------------------------------------------------------------------------------------------
// Unhashed art
// ---------------------------------------------------------------------------------------------

/// Physical text anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Anchor {
    /// `x` is the left end.
    Left,
    /// `x` is the centre.
    Center,
    /// `x` is the right end.
    Right,
}

/// Text content: fixed, or resolved per sheet instance.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TextContent {
    /// Fixed text.
    Fixed(String),
    /// "Form B · Page 1/2" for the instance being rendered.
    FormPage,
}

/// What a text item is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TextRole {
    /// Exam title.
    Title,
    /// Course line.
    Course,
    /// Form and page.
    FormPage,
    /// Field captions.
    Caption,
    /// Instructions.
    Instruction,
    /// The cancel-mode instruction.
    Cancel,
    /// A question number.
    QuestionNumber,
}

/// A text item.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextItem {
    /// Content.
    pub content: TextContent,
    /// Anchor x and baseline y.
    pub at: PtUm,
    /// Font size (em).
    pub size: Um,
    /// Anchor.
    pub anchor: Anchor,
    /// Bold weight.
    pub bold: bool,
    /// Monospace face.
    pub mono: bool,
    /// Gray level (0 = black).
    pub gray: u8,
    /// Right-to-left paragraph.
    pub rtl: bool,
    /// Role.
    pub role: TextRole,
    /// Approximate extent (for lint).
    pub bbox: RectUm,
}

/// A straight rule (write-in line).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RuleLine {
    /// Start.
    pub from: PtUm,
    /// End.
    pub to: PtUm,
    /// Stroke width.
    pub width: Um,
}

/// Kind of a "how to mark" example (§6.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ExampleKind {
    /// A correct, complete fill.
    Fill,
    /// A tick.
    Tick,
    /// An X.
    Cross,
    /// A circle around the bubble.
    Circle,
    /// A dot or partial fill.
    Dot,
}

/// A "how to mark" example bubble with its ✓/✗ verdict icon.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Example {
    /// Kind.
    pub kind: ExampleKind,
    /// Bubble centre.
    pub centre: PtUm,
    /// Verdict icon centre.
    pub icon: PtUm,
}

impl Example {
    /// True for the one correct example.
    pub fn ok(&self) -> bool {
        self.kind == ExampleKind::Fill
    }

    /// Printed extent (bubble, mark and icon).
    pub fn rect(&self) -> RectUm {
        RectUm::around(self.centre, BUBBLE_RADIUS + 700).union(&RectUm::around(self.icon, 900))
    }
}

/// A slot for per-sheet personalised text (name or student ID; never a national ID, §6.7).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TextSlot {
    /// Left end and baseline.
    pub at: PtUm,
    /// Font size.
    pub size: Um,
    /// Available width.
    pub max_w: Um,
}

/// The unhashed art of a page.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct PageArt {
    /// Text items (header, captions, question numbers).
    pub texts: Vec<TextItem>,
    /// Write-in rules.
    pub rules: Vec<RuleLine>,
    /// Comb boxes above the ID grid.
    pub frames: Vec<RectUm>,
    /// "How to mark" examples (page 1).
    pub examples: Vec<Example>,
    /// Personalised name slot.
    pub name_slot: Option<TextSlot>,
    /// Personalised student-ID slot.
    pub id_slot: Option<TextSlot>,
}

// ---------------------------------------------------------------------------------------------
// Template
// ---------------------------------------------------------------------------------------------

/// A validated template revision with its computed layout and hash.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Template {
    spec: TemplateSpec,
    cancel_mode: CancelMode,
    manifest: LayoutManifest,
    art: Vec<PageArt>,
    #[serde(with = "hex_digest")]
    hash: Sha384Digest,
    warnings: Vec<LintFinding>,
}

mod hex_digest {
    use serde::Serializer;
    pub fn serialize<S: Serializer>(d: &[u8; 48], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&crate::hash::hex(d))
    }
}

/// Why a template could not be built.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum BuildError {
    /// A specification value is out of range.
    #[error(transparent)]
    Spec(#[from] SpecError),
    /// Blocking lint findings (§6.9), e.g. L14 when `cancel_mode` is unset.
    #[error("blocking lint findings: {}", .0.iter().map(|f| f.to_string()).collect::<Vec<_>>().join("; "))]
    Lint(Vec<LintFinding>),
    /// The manifest could not be canonicalised.
    #[error(transparent)]
    Canonical(#[from] CanonicalError),
    /// No answer column fits on a continuation page.
    #[error("no answer column fits on the page")]
    NoCapacity,
}

impl Template {
    /// Validates `spec`, lays it out, lints it (blocking findings fail the build) and hashes it.
    pub fn build(spec: TemplateSpec) -> Result<Template, BuildError> {
        spec.validate()?;
        let Some(cancel_mode) = spec.cancel_mode else {
            return Err(BuildError::Lint(vec![lint::l14_cancel_mode_unset()]));
        };
        let (manifest, art) = layout(&spec, cancel_mode)?;
        let findings = lint::lint_layout(&manifest, &art, lint::DEFAULT_CAPTURE_DPI);
        let (blocking, warnings): (Vec<_>, Vec<_>) = findings
            .into_iter()
            .partition(|f| f.level == LintLevel::Block);
        if !blocking.is_empty() {
            return Err(BuildError::Lint(blocking));
        }
        let hash = manifest.sha384()?;
        Ok(Template {
            spec,
            cancel_mode,
            manifest,
            art,
            hash,
            warnings,
        })
    }

    /// The specification it was built from.
    pub fn spec(&self) -> &TemplateSpec {
        &self.spec
    }

    /// Marking convention.
    pub fn cancel_mode(&self) -> CancelMode {
        self.cancel_mode
    }

    /// Row direction.
    pub fn direction(&self) -> Direction {
        self.manifest.direction
    }

    /// The hashed layout manifest (what a recognizer reads, §6.1).
    pub fn manifest(&self) -> &LayoutManifest {
        &self.manifest
    }

    /// Pages, in order.
    pub fn pages(&self) -> &[PageGeometry] {
        &self.manifest.pages
    }

    /// Number of page sides.
    pub fn page_count(&self) -> u16 {
        self.manifest.pages.len() as u16
    }

    /// A page by 1-based number.
    pub fn page(&self, p: PageNo) -> Option<&PageGeometry> {
        self.manifest
            .pages
            .get(usize::from(p.0).checked_sub(1)?)
    }

    /// The unhashed art of a page.
    pub fn art(&self, p: PageNo) -> Option<&PageArt> {
        self.art.get(usize::from(p.0).checked_sub(1)?)
    }

    /// Placement of question `q` (0-based).
    pub fn question(&self, q: u16) -> Option<&QuestionGeom> {
        self.manifest.questions.get(usize::from(q))
    }

    /// Bubbles of question `q`, ordered by option.
    pub fn question_bubbles(&self, q: u16) -> &[AnswerBubble] {
        let Some(g) = self.question(q) else {
            return &[];
        };
        let Some(page) = self.page(PageNo(g.page)) else {
            return &[];
        };
        let s = g.first_bubble as usize;
        page.bubbles
            .get(s..s + usize::from(g.options))
            .unwrap_or(&[])
    }

    /// Number of questions.
    pub fn question_count(&self) -> u16 {
        self.spec.questions
    }

    /// Form codes of this exam (1 = A …).
    pub fn forms(&self) -> impl Iterator<Item = FormCode> + use<> {
        (1..=self.spec.forms).map(FormCode)
    }

    /// The template hash: SHA-384 of the canonical layout manifest.
    pub fn hash(&self) -> &Sha384Digest {
        &self.hash
    }

    /// The template hash as lower-case hex (for the engine manifest, §14.6).
    pub fn hash_hex(&self) -> String {
        hash::hex(&self.hash)
    }

    /// The canonical JSON whose SHA-384 is [`Template::hash`].
    pub fn manifest_json(&self) -> String {
        // Canonicalisation succeeded at build time and the manifest is immutable.
        self.manifest.canonical_json().unwrap_or_default()
    }

    /// Non-blocking lint findings recorded at build time.
    pub fn warnings(&self) -> &[LintFinding] {
        &self.warnings
    }

    /// The 40-bit exam ID printed in the page code.
    pub fn exam_id40(&self) -> u64 {
        exam_id40(&self.spec.exam, self.spec.exam_salt)
    }

    /// The page code fields for one printed side.
    pub fn page_code(&self, form: FormCode, page: PageNo, serial: BookletSerial) -> PageCode {
        PageCode {
            exam_id40: self.exam_id40(),
            revision: self.spec.revision,
            form,
            page,
            serial,
        }
    }

    /// Lints again at another capture resolution (L02 depends on it).
    pub fn lint_at_dpi(&self, capture_dpi: u32) -> Vec<LintFinding> {
        lint::lint_layout(&self.manifest, &self.art, capture_dpi)
    }

    /// All art, by page.
    pub fn arts(&self) -> &[PageArt] {
        &self.art
    }
}

// ---------------------------------------------------------------------------------------------
// Layout algorithm
// ---------------------------------------------------------------------------------------------

const TITLE_FONT: Um = 4_233; // 12 pt
const COURSE_FONT: Um = 2_822; // 8 pt
const CAPTION_FONT: Um = 2_469; // 7 pt
const SMALL_FONT: Um = 2_117; // 6 pt
const INSTR_FONT: Um = 2_469; // 7 pt
const INSTR_LINE: Um = 3_100;
const RULE_W: Um = 200;
const COMB_W: Um = 6_000;
const COMB_H: Um = 6_000;
/// Stroke of the comb (write-in) boxes above the ID grid.
pub const COMB_STROKE: Um = 200;
const EXAMPLE_PITCH: Um = 8_500;
const FORM_PITCH: Um = 7_000;

/// Approximate advance width of `text` (lint and wrapping only; rendering uses real metrics).
pub fn approx_text_width(text: &str, size: Um, bold: bool, mono: bool) -> Um {
    let em: f64 = text
        .chars()
        .map(|c| {
            if mono {
                0.6
            } else if c == ' ' {
                0.26
            } else if c.is_ascii_digit() {
                0.58
            } else if c.is_ascii_uppercase() {
                0.68
            } else if c.is_ascii_lowercase() {
                0.54
            } else if ('\u{0600}'..='\u{06FF}').contains(&c) {
                0.48
            } else if c.is_ascii_punctuation() || c == '·' {
                0.32
            } else {
                0.62
            }
        })
        .sum();
    let k = if bold { 1.06 } else { 1.0 };
    (em * k * f64::from(size)).ceil() as Um
}

/// True if the first strong character (Latin or Arabic letter) is Arabic: the paragraph is RTL.
pub fn starts_rtl(s: &str) -> bool {
    s.chars()
        .find(|c| c.is_alphabetic())
        .is_some_and(|c| ('\u{0600}'..='\u{06FF}').contains(&c))
}

fn text_item(
    content: TextContent,
    sample: &str,
    at: PtUm,
    size: Um,
    anchor: Anchor,
    bold: bool,
    role: TextRole,
) -> TextItem {
    let w = approx_text_width(sample, size, bold, false);
    let left = match anchor {
        Anchor::Left => at.x,
        Anchor::Center => at.x - w / 2,
        Anchor::Right => at.x - w,
    };
    let rtl = starts_rtl(sample);
    TextItem {
        content,
        at,
        size,
        anchor,
        bold,
        mono: false,
        gray: 0,
        rtl,
        role,
        bbox: RectUm::new(left, at.y - size * 8 / 10, w, size),
    }
}

fn fixed(
    s: &str,
    at: PtUm,
    size: Um,
    anchor: Anchor,
    bold: bool,
    role: TextRole,
) -> TextItem {
    text_item(TextContent::Fixed(s.to_owned()), s, at, size, anchor, bold, role)
}

/// Truncates `s` with an ellipsis so it fits `max_w`.
fn fit(s: &str, size: Um, bold: bool, max_w: Um) -> String {
    if approx_text_width(s, size, bold, false) <= max_w {
        return s.to_owned();
    }
    let mut out: String = String::new();
    for c in s.chars() {
        let mut t = out.clone();
        t.push(c);
        t.push('…');
        if approx_text_width(&t, size, bold, false) > max_w {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
}

/// Greedy word wrap to `max_w`.
fn wrap(s: &str, size: Um, max_w: Um) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in s.split_whitespace() {
        let cand = if cur.is_empty() {
            word.to_owned()
        } else {
            format!("{cur} {word}")
        };
        if approx_text_width(&cand, size, false, false) <= max_w || cur.is_empty() {
            cur = cand;
        } else {
            lines.push(std::mem::take(&mut cur));
            cur = word.to_owned();
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// The printed instruction for a cancel mode (§6.5 table), English and Arabic.
pub fn cancel_instruction(mode: CancelMode) -> (&'static str, &'static str) {
    match mode {
        CancelMode::EraseOnly => (
            "Fill the bubble completely; erase completely to change an answer.",
            "ظلّل الدائرة بالكامل، وامسحها تمامًا لتغيير الإجابة.",
        ),
        CancelMode::CrossCancels => (
            "Fill the bubble completely; cross it out to cancel an answer.",
            "ظلّل الدائرة بالكامل، وضع عليها علامة × لإلغاء الإجابة.",
        ),
        CancelMode::FillCancelsCheck => (
            "Tick or X the bubble to select; fill it completely to cancel.",
            "ضع علامة صح أو × داخل الدائرة للاختيار، وظلّلها بالكامل للإلغاء.",
        ),
    }
}

const DEFAULT_INSTRUCTIONS_EN: &str =
    "Use a dark HB/2B pencil or a black pen. Mark one bubble per question. Do not fold or staple this sheet.";
const DEFAULT_INSTRUCTIONS_AR: &str =
    "استخدم قلم رصاص HB أو 2B أو قلمًا أسود. ظلّل دائرة واحدة لكل سؤال. لا تطوِ الورقة ولا تدبّسها.";

fn tags() -> Vec<FiducialTag> {
    let s = SAFE_BOX_A4;
    Corner::ALL
        .iter()
        .map(|&corner| {
            let (x, y) = match corner {
                Corner::TopLeft => (s.x, s.y),
                Corner::TopRight => (s.right() - TAG_SIZE, s.y),
                Corner::BottomRight => (s.right() - TAG_SIZE, s.bottom() - TAG_SIZE),
                Corner::BottomLeft => (s.x, s.bottom() - TAG_SIZE),
            };
            let outer = RectUm::new(x, y, TAG_SIZE, TAG_SIZE);
            FiducialTag {
                id: TAG_SET_ANSWER * TAGS_PER_SET + corner.index(),
                corner,
                outer,
                black: outer.inflate(-TAG_MODULE),
            }
        })
        .collect()
}

fn code_slots(diag: CodeDiagonal) -> Vec<CodeSlot> {
    let s = SAFE_BOX_A4;
    let (top, bottom) = match diag {
        CodeDiagonal::TrBl => (Corner::TopRight, Corner::BottomLeft),
        CodeDiagonal::TlBr => (Corner::TopLeft, Corner::BottomRight),
    };
    [top, bottom]
        .iter()
        .enumerate()
        .map(|(i, &corner)| {
            let right = matches!(corner, Corner::TopRight | Corner::BottomRight);
            let upper = matches!(corner, Corner::TopLeft | Corner::TopRight);
            let qx = if right {
                s.right() - TAG_SIZE - QUIET_ZONE - QR_SIZE
            } else {
                s.x + TAG_SIZE + QUIET_ZONE
            };
            let (qy, ey) = if upper {
                (s.y, s.y + QR_SIZE + QUIET_ZONE)
            } else {
                let ey = s.bottom() - ECHO_H;
                (ey - QUIET_ZONE - QR_SIZE, ey)
            };
            let rect = RectUm::new(qx, qy, QR_SIZE, QR_SIZE);
            let (ex, align) = if right {
                (rect.right() - ECHO_W, HAlign::Right)
            } else {
                (rect.x, HAlign::Left)
            };
            CodeSlot {
                copy: i as u8,
                corner,
                rect,
                module: QR_MODULE,
                modules: QR_MODULES,
                echo: RectUm::new(ex, ey, ECHO_W, ECHO_H),
                echo_align: align,
            }
        })
        .collect()
}

/// An obstacle the body flows around: `rect` grown by `quiet`.
#[derive(Clone, Copy, Debug)]
struct Obstacle(RectUm);

fn page_obstacles(tags: &[FiducialTag], codes: &[CodeSlot]) -> Vec<Obstacle> {
    let mut v: Vec<Obstacle> = tags
        .iter()
        .map(|t| Obstacle(t.outer.inflate(QUIET_ZONE)))
        .collect();
    for c in codes {
        v.push(Obstacle(c.rect.inflate(QUIET_ZONE)));
        v.push(Obstacle(c.echo.inflate(QUIET_ZONE)));
    }
    v
}

/// Header items and their obstacles for one page.
struct Header {
    art: PageArt,
    obstacles: Vec<Obstacle>,
    id_bubbles: Vec<IdBubble>,
    form_bubbles: Vec<FormBubble>,
    blocks: Vec<Block>,
}

fn build_header(
    spec: &TemplateSpec,
    cancel_mode: CancelMode,
    first_page: bool,
    tags: &[FiducialTag],
    codes: &[CodeSlot],
) -> Header {
    let s = SAFE_BOX_A4;
    let base = page_obstacles(tags, codes);
    let mut art = PageArt::default();
    let mut blocks = Vec::new();
    let mut id_bubbles = Vec::new();
    let mut form_bubbles = Vec::new();
    let mut obstacles = base.clone();
    let mid_x = s.x + s.w / 2;

    // Left limit: right edge of top-left obstacles; right limit: left edge of top-right ones.
    let top_band = RectUm::from_ltrb(s.x, s.y, s.right(), s.y + 18_000);
    let mut left = s.x;
    let mut right = s.right();
    for o in &base {
        if !o.0.intersects(&top_band) {
            continue;
        }
        if o.0.centre().x < mid_x {
            left = left.max(o.0.right());
        } else {
            right = right.min(o.0.x);
        }
    }

    // Page 1: the student-ID grid at the top left, flowing past top-left obstacles.
    let mut za_x0 = left;
    if first_page && spec.id_digits > 0 {
        let n = Um::from(spec.id_digits);
        let caption = "Student ID · الرقم الجامعي";
        let cap_w = approx_text_width(caption, SMALL_FONT, true, false);
        let w = ((n - 1) * ID_PITCH_X + COMB_W).max(cap_w);
        let h = 2_800 + COMB_H + 1_500 + 9 * ID_PITCH_Y + BUBBLE_DIAMETER;
        let mut rect = RectUm::new(left, s.y, w, h);
        loop {
            let hit = base.iter().find(|o| o.0.intersects(&rect));
            match hit {
                Some(o) => rect.x = o.0.right(),
                None => break,
            }
        }
        let (x0, y0) = (rect.x, rect.y);
        art.texts.push(fixed(
            caption,
            PtUm::new(x0, y0 + 2_000),
            SMALL_FONT,
            Anchor::Left,
            true,
            TextRole::Caption,
        ));
        for c in 0..spec.id_digits {
            let cx = x0 + COMB_W / 2 + Um::from(c) * ID_PITCH_X;
            art.frames
                .push(RectUm::new(cx - COMB_W / 2, y0 + 2_800, COMB_W, COMB_H));
            for d in 0..10u8 {
                let cy = y0 + 2_800 + COMB_H + 1_500 + BUBBLE_RADIUS + Um::from(d) * ID_PITCH_Y;
                id_bubbles.push(IdBubble {
                    column: c,
                    digit: d,
                    centre: PtUm::new(cx, cy),
                    radius: BUBBLE_RADIUS,
                    label: spec.id_digit_labels.label(d).to_owned(),
                });
            }
        }
        blocks.push(Block {
            kind: BlockKind::IdGrid,
            rect,
        });
        obstacles.push(Obstacle(rect.inflate(QUIET_ZONE)));
        za_x0 = rect.right() + QUIET_ZONE + 1_000;
    }

    // Zone A: title, course, form/page, name and student-ID write-in lines (every page).
    let za_x1 = right;
    let za_w = (za_x1 - za_x0).max(10_000);
    let title = fit(&spec.title, TITLE_FONT, true, za_w);
    art.texts.push(fixed(
        &title,
        PtUm::new(za_x0, s.y + 3_600),
        TITLE_FONT,
        Anchor::Left,
        true,
        TextRole::Title,
    ));
    let fp_sample = "Form W · Page 88/88";
    let fp_w = approx_text_width(fp_sample, COURSE_FONT, true, false);
    let course = fit(&spec.course, COURSE_FONT, false, za_w - fp_w - 3_000);
    art.texts.push(fixed(
        &course,
        PtUm::new(za_x0, s.y + 8_600),
        COURSE_FONT,
        Anchor::Left,
        false,
        TextRole::Course,
    ));
    art.texts.push(text_item(
        TextContent::FormPage,
        fp_sample,
        PtUm::new(za_x1, s.y + 8_600),
        COURSE_FONT,
        Anchor::Right,
        true,
        TextRole::FormPage,
    ));
    let mut line_y = s.y + 14_900;
    for (label, is_name) in [("Name · الاسم", true), ("Student ID · الرقم الجامعي", false)] {
        let t = fixed(
            label,
            PtUm::new(za_x0, line_y),
            CAPTION_FONT,
            Anchor::Left,
            false,
            TextRole::Caption,
        );
        let x_rule = t.bbox.right() + 1_500;
        art.rules.push(RuleLine {
            from: PtUm::new(x_rule, line_y + 500),
            to: PtUm::new(za_x1, line_y + 500),
            width: RULE_W,
        });
        let slot = TextSlot {
            at: PtUm::new(x_rule + 500, line_y - 200),
            size: COURSE_FONT,
            max_w: za_x1 - x_rule - 500,
        };
        if is_name {
            art.name_slot = Some(slot);
        } else {
            art.id_slot = Some(slot);
        }
        art.texts.push(t);
        line_y += 5_300;
    }
    let za_bottom = line_y - 5_300 + 500 + RULE_W;

    // Zone B (page 1): instructions, cancel instruction, how-to-mark row, form grid.
    if first_page {
        let zb_x0 = za_x0;
        let zb_x1 = s.right();
        // Start below the upper code's echo line when it overhangs zone B.
        let mut y = za_bottom + 2_000;
        for c in codes {
            if c.echo.y < s.y + 60_000 && c.echo.overlaps_x(zb_x0, zb_x1) {
                y = y.max(c.echo.bottom() + 1_800);
            }
            if c.rect.y < s.y + 60_000 && c.rect.overlaps_x(zb_x0, zb_x1) {
                y = y.max(c.rect.bottom() + QUIET_ZONE);
            }
        }
        let zb_w = zb_x1 - zb_x0;
        let mut lines: Vec<(String, bool, TextRole)> = Vec::new();
        if spec.instructions.trim().is_empty() {
            for l in wrap(DEFAULT_INSTRUCTIONS_EN, INSTR_FONT, zb_w) {
                lines.push((l, false, TextRole::Instruction));
            }
            for l in wrap(DEFAULT_INSTRUCTIONS_AR, INSTR_FONT, zb_w) {
                lines.push((l, false, TextRole::Instruction));
            }
        } else {
            for para in spec.instructions.lines() {
                for l in wrap(para, INSTR_FONT, zb_w) {
                    lines.push((l, false, TextRole::Instruction));
                }
            }
        }
        let (en, ar) = cancel_instruction(cancel_mode);
        for l in wrap(en, INSTR_FONT, zb_w) {
            lines.push((l, true, TextRole::Cancel));
        }
        for l in wrap(ar, INSTR_FONT, zb_w) {
            lines.push((l, true, TextRole::Cancel));
        }
        for (l, bold, role) in lines {
            let (x, anchor) = if starts_rtl(&l) {
                (zb_x1, Anchor::Right)
            } else {
                (zb_x0, Anchor::Left)
            };
            let t = fixed(&l, PtUm::new(x, y + INSTR_FONT * 8 / 10), INSTR_FONT, anchor, bold, role);
            art.texts.push(t);
            y += INSTR_LINE;
        }
        // How-to-mark row: caption, then five examples with verdict icons. The gap keeps a
        // form grid sharing this row 3 mm clear of the text above (block quiet zone).
        y += 1_800;
        let yc = y + BUBBLE_RADIUS + 700;
        let caption = fixed(
            "How to mark · طريقة التظليل:",
            PtUm::new(zb_x0, yc + 800),
            SMALL_FONT,
            Anchor::Left,
            true,
            TextRole::Caption,
        );
        let mut x = caption.bbox.right() + 2_000 + BUBBLE_RADIUS;
        art.texts.push(caption);
        for kind in [
            ExampleKind::Fill,
            ExampleKind::Tick,
            ExampleKind::Cross,
            ExampleKind::Circle,
            ExampleKind::Dot,
        ] {
            art.examples.push(Example {
                kind,
                centre: PtUm::new(x, yc),
                icon: PtUm::new(x + BUBBLE_RADIUS + 1_900, yc),
            });
            x += EXAMPLE_PITCH;
        }
        let examples_right = art
            .examples
            .iter()
            .map(|e| e.rect().right())
            .max()
            .unwrap_or(zb_x0);
        let mut row_bottom = yc + BUBBLE_RADIUS + 700;

        // Form grid: same row if it fits after a quiet zone, else on rows below.
        if spec.form_grid {
            let caption = "Form · النموذج";
            let cap_w = approx_text_width(caption, SMALL_FONT, true, false);
            let avail_same = zb_x1 - (examples_right + QUIET_ZONE) - cap_w - 2_000;
            let per_row_same = if avail_same >= BUBBLE_DIAMETER {
                ((avail_same - BUBBLE_DIAMETER) / FORM_PITCH + 1) as u16
            } else {
                0
            };
            let (mut fy, start_x, per_row) = if per_row_same >= spec.forms {
                (yc, zb_x1 - BUBBLE_RADIUS - (Um::from(spec.forms) - 1) * FORM_PITCH, spec.forms)
            } else {
                let per = (((zb_w - cap_w - 2_000 - BUBBLE_DIAMETER) / FORM_PITCH) + 1).max(1) as u16;
                (row_bottom + QUIET_ZONE + BUBBLE_RADIUS, zb_x0 + cap_w + 2_000 + BUBBLE_RADIUS, per)
            };
            let cap = fixed(
                caption,
                PtUm::new(start_x - BUBBLE_RADIUS - 1_500, fy + 800),
                SMALL_FONT,
                Anchor::Right,
                true,
                TextRole::Caption,
            );
            // The caption belongs to the block, so the block's quiet zone surrounds both.
            let mut rect: Option<RectUm> = Some(cap.bbox);
            art.texts.push(cap);
            for f in 0..spec.forms {
                let col = f % per_row;
                if f > 0 && col == 0 {
                    fy += PITCH_Y;
                }
                let b = FormBubble {
                    form: f + 1,
                    centre: PtUm::new(start_x + Um::from(col) * FORM_PITCH, fy),
                    radius: BUBBLE_RADIUS,
                    label: FormCode(f + 1).letter().to_string(),
                };
                rect = Some(match rect {
                    Some(r) => r.union(&b.rect()),
                    None => b.rect(),
                });
                form_bubbles.push(b);
            }
            if let Some(r) = rect {
                blocks.push(Block {
                    kind: BlockKind::FormGrid,
                    rect: r,
                });
                obstacles.push(Obstacle(r.inflate(QUIET_ZONE)));
                row_bottom = row_bottom.max(r.bottom());
            }
        }
        let _ = row_bottom;
    }

    // Header text, rules and examples are obstacles for answer blocks (block quiet zone).
    for t in &art.texts {
        obstacles.push(Obstacle(t.bbox.inflate(QUIET_ZONE)));
    }
    for r in &art.rules {
        let rr = RectUm::from_ltrb(r.from.x, r.from.y - r.width, r.to.x, r.to.y + r.width);
        obstacles.push(Obstacle(rr.inflate(QUIET_ZONE)));
    }
    for e in &art.examples {
        obstacles.push(Obstacle(e.rect().inflate(QUIET_ZONE)));
    }
    Header {
        art,
        obstacles,
        id_bubbles,
        form_bubbles,
        blocks,
    }
}

/// Physical x ranges of the answer columns, in logical (reading) order.
fn columns(max_options: u8, dir: Direction) -> Vec<(Um, Um)> {
    let s = SAFE_BOX_A4;
    let w = column_width(max_options);
    let n = ((s.w + MIN_GUTTER) / (w + MIN_GUTTER)).max(1);
    let gutter = if n > 1 {
        ((s.w - n * w) / (n - 1)).min(MAX_GUTTER)
    } else {
        0
    };
    let total = n * w + (n - 1) * gutter;
    let x0 = s.x + (s.w - total) / 2;
    let mut v: Vec<(Um, Um)> = (0..n)
        .map(|i| {
            let l = x0 + i * (w + gutter);
            (l, l + w)
        })
        .collect();
    if dir == Direction::Rtl {
        v.reverse();
    }
    v
}

/// Free vertical interval of each column: (top, bottom, capacity).
fn column_intervals(cols: &[(Um, Um)], obstacles: &[Obstacle]) -> Vec<(Um, Um, u16)> {
    let s = SAFE_BOX_A4;
    let mid_y = s.y + s.h / 2;
    cols.iter()
        .map(|&(l, r)| {
            let mut top = s.y;
            let mut bottom = s.bottom();
            for o in obstacles {
                if !o.0.overlaps_x(l, r) {
                    continue;
                }
                if o.0.centre().y < mid_y {
                    top = top.max(o.0.bottom());
                } else {
                    bottom = bottom.min(o.0.y);
                }
            }
            let cap = if bottom > top {
                rows_capacity(bottom - top)
            } else {
                0
            };
            (top, bottom, cap)
        })
        .collect()
}

/// Water-filling: as even as possible, column order, within capacities.
fn fill_columns(n: u16, caps: &[u16]) -> Vec<u16> {
    let total: u32 = caps.iter().map(|c| u32::from(*c)).sum();
    let n = u32::from(n).min(total) as u16;
    let mut t = 0u16;
    while caps.iter().map(|c| u32::from((*c).min(t))).sum::<u32>() < u32::from(n) {
        t += 1;
    }
    let mut rem = n;
    caps.iter()
        .map(|c| {
            let a = (*c).min(t).min(rem);
            rem -= a;
            a
        })
        .collect()
}

fn layout(
    spec: &TemplateSpec,
    cancel_mode: CancelMode,
) -> Result<(LayoutManifest, Vec<PageArt>), BuildError> {
    let dir = spec.direction();
    let tags = tags();
    let codes = code_slots(spec.code_diagonal);
    let cols = columns(spec.options.max(), dir);

    let h1 = build_header(spec, cancel_mode, true, &tags, &codes);
    let hn = build_header(spec, cancel_mode, false, &tags, &codes);
    let iv1 = column_intervals(&cols, &h1.obstacles);
    let ivn = column_intervals(&cols, &hn.obstacles);
    let cap1: u32 = iv1.iter().map(|c| u32::from(c.2)).sum();
    let capn: u32 = ivn.iter().map(|c| u32::from(c.2)).sum();
    if capn == 0 {
        return Err(BuildError::NoCapacity);
    }
    let q = u32::from(spec.questions);
    let page_count = if q <= cap1 {
        1
    } else {
        1 + (q - cap1).div_ceil(capn)
    };

    let mut pages = Vec::new();
    let mut arts = Vec::new();
    let mut questions = Vec::new();
    let mut next_q: u16 = 0;
    let mut headers = Some(h1);
    for p in 0..page_count {
        let first = p == 0;
        let header = if first {
            headers.take().unwrap_or_else(|| build_header(spec, cancel_mode, true, &tags, &codes))
        } else {
            build_header(spec, cancel_mode, false, &tags, &codes)
        };
        let iv = if first { &iv1 } else { &ivn };
        let caps: Vec<u16> = iv.iter().map(|c| c.2).collect();
        let cap_page: u32 = caps.iter().map(|c| u32::from(*c)).sum();
        let rem = q - u32::from(next_q);
        let pages_left = page_count - p;
        let on_page = rem.div_ceil(pages_left).min(cap_page) as u16;
        let per_col = fill_columns(on_page, &caps);

        let Header {
            mut art,
            id_bubbles,
            form_bubbles,
            mut blocks,
            ..
        } = header;
        let mut bubbles = Vec::new();
        let mut ticks = Vec::new();
        for (ci, &rows) in per_col.iter().enumerate() {
            if rows == 0 {
                continue;
            }
            let (l, r) = cols[ci];
            let (top, _, _) = iv[ci];
            let phys = |off: Um| -> Um {
                match dir {
                    Direction::Ltr => l + off,
                    Direction::Rtl => r - off,
                }
            };
            for row in 0..rows {
                let qi = next_q;
                next_q += 1;
                let y = top + row_centre_offset(row);
                let n = spec.options.of(qi);
                questions.push(QuestionGeom {
                    question: qi,
                    page: p as u16 + 1,
                    column: ci as u8,
                    row,
                    options: n,
                    row_y: y,
                    first_bubble: bubbles.len() as u32,
                });
                for k in 0..n {
                    bubbles.push(AnswerBubble {
                        question: qi,
                        option: k,
                        centre: PtUm::new(phys(ROW_FIRST_CENTRE + Um::from(k) * PITCH_X), y),
                        radius: BUBBLE_RADIUS,
                        label: spec.labels.label(k).to_owned(),
                    });
                }
                for side in [Side::Left, Side::Right] {
                    let x = match side {
                        Side::Left => l,
                        Side::Right => r - TICK_W,
                    };
                    ticks.push(RowTick {
                        question: qi,
                        side,
                        rect: RectUm::new(x, y - TICK_H / 2, TICK_W, TICK_H),
                    });
                }
                let label = (qi + 1).to_string();
                let anchor = match dir {
                    Direction::Ltr => Anchor::Right,
                    Direction::Rtl => Anchor::Left,
                };
                let at = PtUm::new(phys(ROW_QNUM_EDGE), y + QNUM_FONT * 36 / 100);
                art.texts
                    .push(fixed(&label, at, QNUM_FONT, anchor, true, TextRole::QuestionNumber));
            }
            blocks.push(Block {
                kind: BlockKind::Mcq { column: ci as u8 },
                rect: RectUm::new(l, top, r - l, rows_extent(rows)),
            });
        }
        pages.push(PageGeometry {
            page: p as u16 + 1,
            tags: tags.clone(),
            codes: codes.clone(),
            blocks,
            bubbles,
            id_bubbles,
            form_bubbles,
            ticks,
        });
        arts.push(art);
    }
    debug_assert_eq!(u32::from(next_q), q);

    let manifest = LayoutManifest {
        generator: GENERATOR.to_owned(),
        paper: spec.paper,
        width: A4_W,
        height: A4_H,
        safe_box: SAFE_BOX_A4,
        cancel_mode,
        labels: spec.labels,
        direction: dir,
        id_digit_labels: spec.id_digit_labels,
        bubble: BubbleSpec {
            shape: "circle".to_owned(),
            diameter: BUBBLE_DIAMETER,
            stroke: BUBBLE_STROKE,
            interior_permille: INTERIOR_RATIO_PERMILLE,
            label_inside: true,
            label_gray: LABEL_GRAY,
        },
        forms: spec.forms,
        id_digits: spec.id_digits,
        questions,
        pages,
    };
    Ok((manifest, arts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::OptionCounts;
    use omr_model::ExamId;

    fn spec(q: u16, opts: u8) -> TemplateSpec {
        TemplateSpec::standard(
            ExamId(uuid::Uuid::from_u128(7)),
            q,
            opts,
            CancelMode::EraseOnly,
        )
    }

    #[test]
    fn fill_columns_balances() {
        assert_eq!(fill_columns(60, &[22, 27, 29, 27]), vec![15, 15, 15, 15]);
        assert_eq!(fill_columns(100, &[22, 27, 29, 27]), vec![22, 26, 26, 26]);
        assert_eq!(fill_columns(3, &[0, 5]), vec![0, 3]);
    }

    #[test]
    fn common_case_hundred_items_fit_one_page_with_a_ten_digit_id() {
        let mut s = spec(100, 5);
        s.id_digits = 10;
        let t = Template::build(s).unwrap();
        assert_eq!(t.page_count(), 1);
        assert_eq!(t.pages()[0].bubbles.len(), 500);
        assert_eq!(t.pages()[0].id_bubbles.len(), 100);
    }

    #[test]
    fn many_items_spill_onto_balanced_pages() {
        let t = Template::build(spec(250, 5)).unwrap();
        assert!(t.page_count() >= 2);
        let per_page: Vec<usize> = t.pages().iter().map(|p| p.questions().count()).collect();
        assert_eq!(per_page.iter().sum::<usize>(), 250);
        let (mn, mx) = (per_page.iter().min().unwrap(), per_page.iter().max().unwrap());
        assert!(mx - mn <= 2, "{per_page:?}");
        // Every question exactly once, in order.
        let all: Vec<u16> = t.pages().iter().flat_map(|p| p.questions()).collect();
        assert_eq!(all, (0..250).collect::<Vec<_>>());
        assert!(t.pages()[1].id_bubbles.is_empty());
    }

    #[test]
    fn question_lookup() {
        let t = Template::build(spec(40, 4)).unwrap();
        let b = t.question_bubbles(17);
        assert_eq!(b.len(), 4);
        assert!(b.iter().enumerate().all(|(i, x)| x.question == 17 && x.option == i as u8));
        assert_eq!(b[0].bubble_id(), "Q18/0");
        assert!(b[1].centre.x - b[0].centre.x == PITCH_X);
    }

    #[test]
    fn rtl_rows_run_right_to_left() {
        let mut s = spec(20, 4);
        s.labels = LabelSet::Arabic;
        let t = Template::build(s).unwrap();
        let b = t.question_bubbles(0);
        assert_eq!(b[0].label, "أ");
        assert!(b[0].centre.x > b[1].centre.x, "option 0 is rightmost");
        let q0 = t.question(0).unwrap();
        let q_last_col = t.manifest().questions.iter().map(|g| g.column).max().unwrap();
        let first_col_x = b[0].centre.x;
        let last = t
            .manifest()
            .questions
            .iter()
            .find(|g| g.column == q_last_col)
            .unwrap();
        assert!(q0.column == 0);
        assert!(t.question_bubbles(last.question)[0].centre.x < first_col_x);
    }

    #[test]
    fn per_item_options_and_ten_option_rows() {
        let mut s = spec(12, 10);
        s.options = OptionCounts::PerItem(vec![2, 3, 4, 5, 6, 7, 8, 9, 10, 10, 4, 2]);
        let t = Template::build(s).unwrap();
        assert_eq!(t.question_bubbles(8).len(), 10);
        assert_eq!(t.question_bubbles(0).len(), 2);
        assert_eq!(t.pages()[0].ticks.len(), 24);
    }

    #[test]
    fn hash_ignores_display_text_but_not_geometry() {
        let a = Template::build(spec(30, 5)).unwrap();
        let mut s = spec(30, 5);
        s.title = "Another title".into();
        s.course = "MATH 203 Linear Algebra".into();
        let b = Template::build(s).unwrap();
        assert_eq!(a.hash(), b.hash(), "title and course are not hashed");
        let c = Template::build(spec(31, 5)).unwrap();
        assert_ne!(a.hash(), c.hash());
        let mut s = spec(30, 5);
        s.cancel_mode = Some(CancelMode::CrossCancels);
        assert_ne!(a.hash(), Template::build(s).unwrap().hash());
        assert_eq!(a.hash_hex().len(), 96);
    }

    #[test]
    fn missing_cancel_mode_is_l14() {
        let mut s = spec(30, 5);
        s.cancel_mode = None;
        match Template::build(s) {
            Err(BuildError::Lint(f)) => assert_eq!(f[0].rule, crate::lint::LintRule::L14),
            other => panic!("expected L14, got {other:?}"),
        }
    }

    #[test]
    fn code_slots_follow_the_diagonal() {
        let t = Template::build(spec(30, 5)).unwrap();
        let p = &t.pages()[0];
        assert_eq!(p.codes[0].corner, Corner::TopRight);
        assert_eq!(p.codes[1].corner, Corner::BottomLeft);
        let mut s = spec(30, 5);
        s.code_diagonal = CodeDiagonal::TlBr;
        let t = Template::build(s).unwrap();
        assert_eq!(t.pages()[0].codes[0].corner, Corner::TopLeft);
        assert_eq!(t.pages()[0].codes[1].corner, Corner::BottomRight);
    }

    #[test]
    fn tags_sit_on_the_safe_box_corners() {
        let t = Template::build(spec(10, 4)).unwrap();
        let p = &t.pages()[0];
        let ids: Vec<u16> = p.tags.iter().map(|t| t.id).collect();
        assert_eq!(ids, vec![0, 1, 2, 3]);
        let tl = p.tag(Corner::TopLeft).unwrap();
        assert_eq!(tl.corners()[0], PtUm::new(7_800, 25_400));
        assert_eq!(tl.black.w, TAG_BLACK);
        assert_eq!(tl.id_in_set(TAG_SET_PROOF), 12);
    }
}
