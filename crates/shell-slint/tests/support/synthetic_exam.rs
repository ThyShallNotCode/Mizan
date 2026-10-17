//! A synthetic IAU exam for the desktop shell (no real data anywhere): the committed blank
//! `iau-ccsit-v1` form (layout only, no PII) with pencil marks painted by omr-sim's mark painter,
//! a light scanner model (skew, shift, paper tone, noise), an invented bilingual roster and a CLO
//! mapping, graded end to end through omr-app into a project package.
//!
//! The mix is the review work of a real midterm: light fills (MRK-001), erasures with residue
//! (MRK-003), a double mark (MRK-002), a blank version bubble (SCO-005, the form inferred from
//! the answers), a student number that is not on the roster and a faint ID digit (IDN-004), and
//! a probable mis-key on form A Q12 (the key says D, the class learned B). Student numbers start
//! with 9 (never an IAU number) and names are invented.
//!
//! Shared by the real-backend A11 test and the `synthetic_exam` example (screenshots).

#![allow(dead_code)]

use image::GrayImage as Raster;
use omr_app::exam::{
    CloDefinition, CloMapping, DomainLetter, ExamDefinition, FormKey, IdSchemeChoice, KeySource,
    MultiReviewPolicy, OutcomeSetup, PolicyChoice, RosterStudent, TemplateChoice,
};
use omr_app::{ExamProject, GradeOptions};
use omr_forms::legacy::LegacyField;
use omr_model::{CancelMode, MarkClass};
use omr_recog::LegacyEngine;
use omr_sim::paint::Canvas;
use omr_sim::script::{BubbleMark, Instrument};
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::path::{Path, PathBuf};

pub const QUESTIONS: u16 = 30;
pub const OPTIONS: u8 = 5;
/// Report snapshot time of the midterm's grading run (reproducible outputs).
pub const SNAPSHOT: &str = "2026-10-15T09:20:00Z";
/// Working resolution of the committed blank form.
const DPI: f64 = 200.0;
/// The number printed key for form A Q12 says D; the class was taught B (a mis-key).
pub const MISKEY_Q: u16 = 11;

/// One synthetic exam.
#[derive(Clone, Debug)]
pub struct Spec {
    pub course_code: &'static str,
    pub course_title: &'static str,
    pub course_title_ar: &'static str,
    pub title: &'static str,
    pub date: &'static str,
    /// When it was graded (RFC 3339 UTC; the newest exam opens first).
    pub snapshot: &'static str,
    /// Sheets scanned (the roster has two more students, who are absent).
    pub sheets: usize,
    pub seed: u64,
    /// Provided keys (else the key is inferred, never releasable).
    pub keyed: bool,
    /// A roster and CLOs (else the exam has neither: no identities to match, no outcomes).
    pub roster_and_clos: bool,
    /// Folder name under the workspace.
    pub folder: &'static str,
}

/// The exam the screenshots and the A11 test use: CS 211, 60 sheets, two versions, keyed.
pub const MIDTERM: Spec = Spec {
    course_code: "CS 211",
    course_title: "Data Structures",
    course_title_ar: "هياكل البيانات",
    title: "Midterm 1",
    date: "2026-10-12",
    snapshot: SNAPSHOT,
    sheets: 60,
    seed: 20_261_012,
    keyed: true,
    roster_and_clos: true,
    folder: "cs211-midterm-1",
};

/// A second, smaller exam graded without a key (the dashboard shows an INFERRED key).
pub const QUIZ: Spec = Spec {
    course_code: "CS 212",
    course_title: "Algorithms",
    course_title_ar: "الخوارزميات",
    title: "Quiz 2",
    date: "2026-10-08",
    snapshot: "2026-10-09T11:40:00Z",
    sheets: 24,
    seed: 20_261_014,
    keyed: false,
    roster_and_clos: true,
    folder: "cs212-quiz-2",
};

/// A bare exam: no roster, no CLOs, the key inferred (edge-state screenshots and tests).
pub const BARE: Spec = Spec {
    course_code: "CS 213",
    course_title: "Discrete Structures",
    course_title_ar: "التراكيب المتقطعة",
    title: "Quiz 1",
    date: "2026-10-14",
    snapshot: "2026-10-14T08:10:00Z",
    sheets: 12,
    seed: 20_261_016,
    keyed: false,
    roster_and_clos: false,
    folder: "cs213-quiz-1",
};

/// What [`build`] produced.
#[derive(Clone, Debug)]
pub struct Built {
    /// The exam folder (`<workspace>/<folder>`).
    pub exam_dir: PathBuf,
    /// Its `project.omrproj`.
    pub project: PathBuf,
}

/// The key printed for `form` (0 = A) on question `q` (0-based).
pub fn key(spec: &Spec, form: usize, q: u16) -> u8 {
    if form == 0 && q == MISKEY_Q && spec.keyed {
        return 3;
    }
    taught(spec, form, q)
}

/// What the class was taught for `form`, question `q`.
pub fn taught(spec: &Spec, form: usize, q: u16) -> u8 {
    let mut h = spec.seed ^ (u64::from(q) << 8) ^ (form as u64 * 0x9E37);
    h = h.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    ((h >> 33) % u64::from(OPTIONS)) as u8
}

const FIRST: [(&str, &str); 20] = [
    ("Noura", "نورة"),
    ("Sara", "سارة"),
    ("Lama", "لمى"),
    ("Haya", "هيا"),
    ("Reem", "ريم"),
    ("Dana", "دانة"),
    ("Joud", "جود"),
    ("Raghad", "رغد"),
    ("Shahad", "شهد"),
    ("Lulwa", "لولوة"),
    ("Abdullah", "عبدالله"),
    ("Faisal", "فيصل"),
    ("Turki", "تركي"),
    ("Saud", "سعود"),
    ("Khalid", "خالد"),
    ("Nawaf", "نواف"),
    ("Yazeed", "يزيد"),
    ("Omar", "عمر"),
    ("Rakan", "راكان"),
    ("Fahad", "فهد"),
];

const FAMILY: [(&str, &str); 12] = [
    ("Al-Harbi", "الحربي"),
    ("Al-Qahtani", "القحطاني"),
    ("Al-Otaibi", "العتيبي"),
    ("Al-Dossary", "الدوسري"),
    ("Al-Shehri", "الشهري"),
    ("Al-Mutairi", "المطيري"),
    ("Al-Zahrani", "الزهراني"),
    ("Al-Ghamdi", "الغامدي"),
    ("Al-Anazi", "العنزي"),
    ("Al-Subaie", "السبيعي"),
    ("Al-Shammari", "الشمري"),
    ("Al-Malki", "المالكي"),
];

/// The invented roster: `sheets + 2` students (the last two are absent), IDs starting with 9.
pub fn roster(spec: &Spec) -> Vec<RosterStudent> {
    (0..spec.sheets + 2)
        .map(|k| {
            let (f, f_ar) = FIRST[(k * 7 + spec.seed as usize) % FIRST.len()];
            let (l, l_ar) = FAMILY[(k * 5 + k / 3) % FAMILY.len()];
            RosterStudent {
                id: student_id(spec, k),
                name: format!("{f} {l}"),
                name_ar: format!("{f_ar} {l_ar}"),
                section: if k % 2 == 0 { "171" } else { "172" }.into(),
                numeric_cell: false,
            }
        })
        .collect()
}

fn student_id(spec: &Spec, k: usize) -> String {
    format!("92{}{:07}", spec.seed % 10, 41_003 + k * 37)
}

/// The sheet of roster student `k` bubbles a number that is not on the roster.
fn unknown_id_sheet(spec: &Spec) -> usize {
    spec.sheets * 2 / 3 + 1
}

/// The sheet whose version bubble is left blank.
fn blank_version_sheet(spec: &Spec) -> usize {
    spec.sheets / 4 + 2
}

/// The sheet with one faint ID digit.
fn faint_id_sheet(spec: &Spec) -> usize {
    spec.sheets / 3 + 3
}

/// The exam definition (keys, roster, CLOs).
pub fn definition(spec: &Spec) -> ExamDefinition {
    let forms = vec!["A".to_string(), "B".to_string()];
    let mut d = ExamDefinition::new(
        spec.title,
        TemplateChoice::Legacy("iau-ccsit-v1".into()),
        QUESTIONS,
        OPTIONS,
        forms.clone(),
        CancelMode::EraseOnly,
    );
    d.course_code = spec.course_code.into();
    d.course_title = spec.course_title.into();
    d.course_title_ar = spec.course_title_ar.into();
    d.term = "1448 H, 1st term".into();
    d.date = spec.date.into();
    // double marks go to review (the instructor decides, never the engine)
    d.multi_review = MultiReviewPolicy::Always;
    d.points = vec!["1".into(); usize::from(QUESTIONS)];
    d.key = if spec.keyed {
        KeySource::Provided {
            forms: forms
                .iter()
                .enumerate()
                .map(|(fi, f)| FormKey {
                    form: f.clone(),
                    answers: (0..QUESTIONS)
                        .map(|q| char::from(b'A' + key(spec, fi, q)).to_string())
                        .collect(),
                })
                .collect(),
        }
    } else {
        KeySource::Inferred
    };
    d.roster = Some(roster(spec));
    d.id_scheme = IdSchemeChoice::Digits(10);
    let clo = |code: &str, title: &str, title_ar: &str, domain, plo: &str| CloDefinition {
        code: code.into(),
        title: title.into(),
        title_ar: title_ar.into(),
        domain,
        plo: plo.into(),
    };
    let mut mapping = Vec::new();
    for q in 1..=QUESTIONS {
        let main = match q {
            1..=8 => "CLO1",
            9..=15 => "CLO2",
            16..=24 => "CLO3",
            _ => "CLO4",
        };
        let (weight, note) = match q {
            15 => ("0.5", "Big-O of a BST insertion"),
            24 => ("0.5", "يجمع بين الأشجار وتحليل التعقيد"),
            _ => ("1", ""),
        };
        mapping.push(CloMapping {
            question: q,
            clo: main.into(),
            weight: weight.into(),
            note: note.into(),
        });
        if q == 15 || q == 24 {
            mapping.push(CloMapping {
                question: q,
                clo: if q == 15 { "CLO3" } else { "CLO4" }.into(),
                weight: "0.5".into(),
                note: String::new(),
            });
        }
    }
    d.outcomes = Some(OutcomeSetup {
        clos: vec![
            clo(
                "CLO1",
                "Describe linear data structures and their operations",
                "وصف هياكل البيانات الخطية وعملياتها",
                DomainLetter::K,
                "K1",
            ),
            clo(
                "CLO2",
                "Explain the complexity of standard algorithms",
                "شرح تعقيد الخوارزميات القياسية",
                DomainLetter::K,
                "K2",
            ),
            clo(
                "CLO3",
                "Apply trees and hashing to solve problems",
                "تطبيق الأشجار والتجزئة لحل المسائل",
                DomainLetter::S,
                "S1",
            ),
            clo(
                "CLO4",
                "Analyse the trade-offs between data structures",
                "تحليل المفاضلة بين هياكل البيانات",
                DomainLetter::S,
                "S2",
            ),
        ],
        mapping,
        split: false,
        policy: PolicyChoice::Default,
    });
    if !spec.roster_and_clos {
        d.roster = None;
        d.outcomes = None;
    }
    d.exam_id = d.derived_exam_id();
    d.validate().expect("the synthetic definition is valid");
    d
}

/// One painted mark: option, class, darkness and coverage.
type Mark = (u8, MarkClass, f32, f32);

/// What one sheet carries: its form, whether the version is bubbled, the bubbled ID, and per
/// question the marks to paint.
struct Sheet {
    form: usize,
    version_bubbled: bool,
    id: String,
    faint_digit: Option<usize>,
    marks: Vec<(u16, Vec<Mark>)>,
}

fn sheet(spec: &Spec, k: usize) -> Sheet {
    let mut rng = ChaCha8Rng::seed_from_u64(spec.seed ^ (k as u64).wrapping_mul(0x5851_F42D));
    let form = k % 2;
    // ability on a logit scale (about normal, mean 0.8); item difficulty spread over the paper;
    // a five-option guessing floor
    let u: f64 = (0..3).map(|_| rng.random::<f64>()).sum();
    let theta = 0.8 + (u - 1.5) * 1.8;
    let mut marks = Vec::new();
    for q in 0..QUESTIONS {
        let b = -1.6 + 3.0 * f64::from((q * 11) % QUESTIONS) / f64::from(QUESTIONS);
        let p = 0.2 + 0.8 / (1.0 + (-1.5 * (theta - b)).exp());
        if rng.random_bool(0.012) {
            marks.push((q, Vec::new()));
            continue;
        }
        let truth = taught(spec, form, q);
        let answer = if rng.random_bool(p.clamp(0.0, 1.0)) {
            truth
        } else {
            (truth + 1 + rng.random_range(0..OPTIONS - 1)) % OPTIONS
        };
        let dark = rng.random_range(0.62f32..0.9);
        let cover = rng.random_range(0.82f32..0.97);
        let mut m = vec![(answer, MarkClass::Filled, dark, cover)];
        let pick = (k * 7 + 3) % usize::from(QUESTIONS);
        let other = (answer + 2) % OPTIONS;
        if usize::from(q) == pick && k % 6 == 3 {
            // a light fill: soft pencil, low pressure
            m = vec![(answer, MarkClass::LightFill, 0.34, 0.8)];
        } else if usize::from(q) == (k * 5 + 1) % usize::from(QUESTIONS) && k % 7 == 5 {
            // a changed answer: residue of the erased option beside the final fill
            let residue = if k % 14 == 5 { 0.3 } else { 0.16 };
            m.insert(0, (other, MarkClass::ErasedResidue, residue, 0.75));
        } else if q == 7 && k == spec.sheets / 3 {
            // two firm marks on one question
            m.push((other, MarkClass::Filled, 0.78, 0.9));
        } else if q == 20 && k == spec.sheets * 3 / 5 {
            // a pencil dot beside the answer
            m.push((other, MarkClass::PartialOrDot, 0.6, 0.08));
        }
        marks.push((q, m));
    }
    let unknown = k == unknown_id_sheet(spec);
    Sheet {
        form,
        version_bubbled: k != blank_version_sheet(spec),
        id: if unknown {
            format!("92{}9990{:03}", spec.seed % 10, k)
        } else {
            student_id(spec, k)
        },
        faint_digit: (k == faint_id_sheet(spec)).then_some(6),
        marks,
    }
}

fn bubble(engine: &LegacyEngine, field: LegacyField, group: u16, option: u8) -> (f64, f64) {
    let b = engine
        .template()
        .bubbles()
        .iter()
        .find(|b| b.field == field && b.group == group && b.option == option)
        .expect("bubble of the IAU form");
    (b.centre.x, b.centre.y)
}

/// Paints the sheet's marks onto a copy of the blank form, then "scans" it.
fn render(engine: &LegacyEngine, spec: &Spec, k: usize) -> Raster {
    let s = sheet(spec, k);
    let blank = engine.blank();
    let (w, h) = (blank.width() as u32, blank.height() as u32);
    let mut page = Raster::from_raw(w, h, blank.as_raw().to_vec()).expect("blank raster");
    let mut rng = ChaCha8Rng::seed_from_u64(spec.seed.wrapping_add(k as u64 * 977));
    let ppm = DPI / 25.4;
    let r_mm = engine.template().bubble_radius_px() / ppm;
    {
        let mut canvas = Canvas::new(&mut page, DPI);
        let mut paint = |(x, y): (f64, f64), class, darkness, coverage, rng: &mut ChaCha8Rng| {
            let m = BubbleMark {
                option: 0,
                class,
                darkness,
                coverage,
                instrument: Instrument::Pencil,
            };
            canvas.paint_mark(x / ppm, y / ppm, r_mm, &m, rng);
        };
        for (q, marks) in &s.marks {
            for &(o, class, d, c) in marks {
                paint(
                    bubble(engine, LegacyField::Answers, *q, o),
                    class,
                    d,
                    c,
                    &mut rng,
                );
            }
        }
        for (col, ch) in s.id.bytes().enumerate() {
            let faint = s.faint_digit == Some(col);
            paint(
                bubble(engine, LegacyField::StudentNumber, col as u16, ch - b'0'),
                if faint {
                    MarkClass::LightFill
                } else {
                    MarkClass::Filled
                },
                if faint { 0.22 } else { 0.78 },
                if faint { 0.55 } else { 0.92 },
                &mut rng,
            );
        }
        if s.version_bubbled {
            paint(
                bubble(engine, LegacyField::Version, 0, s.form as u8),
                MarkClass::Filled,
                0.8,
                0.92,
                &mut rng,
            );
        }
    }
    scan(&page, &mut rng)
}

/// A light scanner model: small skew and shift on a slightly larger bed, paper and toner
/// levels, sensor noise.
fn scan(page: &Raster, rng: &mut ChaCha8Rng) -> Raster {
    let (sw, sh) = (page.width() as usize, page.height() as usize);
    let (w, h) = (sw + 40, sh + 30);
    let angle: f64 = rng.random_range(-0.0045..0.0045);
    let (tx, ty): (f64, f64) = (rng.random_range(12.0..26.0), rng.random_range(8.0..20.0));
    let (c, s) = (angle.cos(), angle.sin());
    let paper: f32 = rng.random_range(238.0..250.0);
    let toner: f32 = rng.random_range(14.0..30.0);
    let src = page.as_raw();
    let sample = |x: f64, y: f64| -> f32 {
        let (fx, fy) = (x - 0.5, y - 0.5);
        let (x0, y0) = (fx.floor(), fy.floor());
        let (ax, ay) = ((fx - x0) as f32, (fy - y0) as f32);
        let px = |xi: f64, yi: f64| -> f32 {
            if xi < 0.0 || yi < 0.0 || xi >= sw as f64 || yi >= sh as f64 {
                255.0
            } else {
                f32::from(src[yi as usize * sw + xi as usize])
            }
        };
        let top = px(x0, y0) * (1.0 - ax) + px(x0 + 1.0, y0) * ax;
        let bot = px(x0, y0 + 1.0) * (1.0 - ax) + px(x0 + 1.0, y0 + 1.0) * ax;
        top * (1.0 - ay) + bot * ay
    };
    let mut out = vec![0u8; w * h];
    let mut noise = rng.random::<u64>() | 1;
    for v in 0..h {
        for u in 0..w {
            // inverse of x' = R·x + t
            let (dx, dy) = (u as f64 + 0.5 - tx, v as f64 + 0.5 - ty);
            let (x, y) = (c * dx + s * dy, -s * dx + c * dy);
            let g = sample(x, y) / 255.0;
            noise ^= noise << 13;
            noise ^= noise >> 7;
            noise ^= noise << 17;
            let n = ((noise >> 40) as f32 / (1u64 << 24) as f32 - 0.5) * 7.0;
            out[v * w + u] = (toner + (paper - toner) * g + n).clamp(0.0, 255.0) as u8;
        }
    }
    Raster::from_raw(w as u32, h as u32, out).expect("scan raster")
}

/// Writes the sheets as 200-dpi JPEG files into `dir` (in parallel), returning `dir`.
pub fn write_scans(spec: &Spec, dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).expect("scan folder");
    let engine = LegacyEngine::iau_ccsit_v1().expect("committed IAU template");
    let workers = std::thread::available_parallelism().map_or(2, |n| n.get().min(4));
    std::thread::scope(|scope| {
        for t in 0..workers {
            let engine = &engine;
            scope.spawn(move || {
                for k in (t..spec.sheets).step_by(workers) {
                    let img = render(engine, spec, k);
                    let jpeg = omr_sim::capture::encode_jpeg(&img, 88, DPI as u32).expect("jpeg");
                    std::fs::write(dir.join(format!("scan-{k:03}.jpg")), jpeg).expect("write scan");
                }
            });
        }
    });
    dir.to_path_buf()
}

/// As [`build`], in two scan batches: sheets `0..first` are graded on day one; the rest come
/// on day two (`scans-<folder>-day2`) with the last sheet scanned in portrait and a blank back,
/// and are added to the exam ([`ExamProject::add_scans`]), so the Scan screen shows a second
/// batch with its REG-002 and CAP-012 page notices.
pub fn build_in_two_batches(spec: &Spec, workspace: &Path, first: usize) -> Built {
    let day1 = write_scans(spec, &workspace.join(format!("scans-{}", spec.folder)));
    let day2 = workspace.join(format!("scans-{}-day2", spec.folder));
    std::fs::create_dir_all(&day2).expect("day-two folder");
    for k in first..spec.sheets {
        let name = format!("scan-{k:03}.jpg");
        std::fs::rename(day1.join(&name), day2.join(&name)).expect("move scan");
    }
    let last = day2.join(format!("scan-{:03}.jpg", spec.sheets - 1));
    let page = image::load_from_memory(&std::fs::read(&last).expect("scan"))
        .expect("decode")
        .to_luma8();
    let portrait = image::imageops::rotate90(&page);
    let jpeg = |img: &Raster, path: &Path| {
        let bytes = omr_sim::capture::encode_jpeg(img, 88, DPI as u32).expect("jpeg");
        std::fs::write(path, bytes).expect("write scan");
    };
    jpeg(&portrait, &last);
    let (w, h) = page.dimensions();
    let blank = Raster::from_fn(w, h, |x, y| {
        image::Luma([243 + ((x * 7 + y * 13) % 5) as u8])
    });
    jpeg(
        &blank,
        &day2.join(format!("scan-{:03}-back.jpg", spec.sheets - 1)),
    );
    let exam_dir = workspace.join(spec.folder);
    std::fs::create_dir_all(&exam_dir).expect("exam folder");
    let project = exam_dir.join("project.omrproj");
    let opts = GradeOptions {
        threads: 0,
        project_dir: Some(project.clone()),
        snapshot_utc: Some(spec.snapshot.into()),
        operator: "principal:synthetic".into(),
        max_pages: None,
    };
    ExamProject::grade(&definition(spec), &[day1], &opts, &|_| {})
        .expect("grading")
        .close()
        .expect("close");
    let mut p = ExamProject::open(&project, "principal:synthetic").expect("reopen");
    let add = GradeOptions {
        project_dir: None,
        snapshot_utc: None,
        ..opts
    };
    p.add_scans(&[day2], &add, &|_| {}).expect("day-two batch");
    p.close().expect("close");
    Built { exam_dir, project }
}

/// Renders the sheets into `<workspace>/scans-<folder>` and grades them into
/// `<workspace>/<folder>/project.omrproj`.
pub fn build(spec: &Spec, workspace: &Path) -> Built {
    let scans = write_scans(spec, &workspace.join(format!("scans-{}", spec.folder)));
    let exam_dir = workspace.join(spec.folder);
    std::fs::create_dir_all(&exam_dir).expect("exam folder");
    let project = exam_dir.join("project.omrproj");
    let opts = GradeOptions {
        threads: 0,
        project_dir: Some(project.clone()),
        snapshot_utc: Some(spec.snapshot.into()),
        operator: "principal:synthetic".into(),
        max_pages: None,
    };
    let p = ExamProject::grade(&definition(spec), &[scans], &opts, &|_| {}).expect("grading");
    p.close().expect("close");
    Built { exam_dir, project }
}
