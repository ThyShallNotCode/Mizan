//! What the desktop binary hands the shell from the operating system: native file dialogs,
//! showing a folder in the file manager, and the desktop preferences with a way to save them.
//! Plain data and callbacks only: the library opens no dialog and starts no program itself
//! (`main.rs` supplies them through [`crate::bind::Shell::set_host`], tests a fake). Without a
//! host the window has no Browse buttons and every path field takes typed paths alone.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// What a file dialog picks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickKind {
    File,
    Files,
    Folder,
}

/// The path field a file dialog fills: each Browse… and Folder… button names one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PickTarget {
    /// New exam: the scan files, or a folder of them.
    NewScans,
    NewScanFolder,
    NewKey,
    NewKeySheets,
    NewRoster,
    NewClo,
    /// New exam: the spec file of an own sheet (the sheet field).
    NewSpec,
    /// New exam: the exam's own folder (Advanced options).
    NewOut,
    /// Setup: replace the key from a key file.
    SetupKey,
    SetupKeySheets,
    SetupClo,
    /// Setup: the roster given to a graded exam.
    SetupRoster,
    /// Scan: a later batch of scan files, or a folder of them.
    AddScans,
    AddScanFolder,
    /// Remark's graded workbooks to import (New exam and Setup) and their points file.
    RemarkBooks,
    RemarkPoints,
    /// The parallel run: Remark's workbooks to compare, and the folder of its report.
    CompareBooks,
    ParallelDir,
    /// Another exams folder to open (Exams, Settings): no path field, the folder opens.
    Workspace,
}

/// The scan files a dialog offers (`omr_app::ingest` reads these).
const SCANS: &[&str] = omr_app::ingest::EXTENSIONS;

impl PickTarget {
    pub const ALL: [PickTarget; 19] = [
        PickTarget::NewScans,
        PickTarget::NewScanFolder,
        PickTarget::NewKey,
        PickTarget::NewKeySheets,
        PickTarget::NewRoster,
        PickTarget::NewClo,
        PickTarget::NewSpec,
        PickTarget::NewOut,
        PickTarget::SetupKey,
        PickTarget::SetupKeySheets,
        PickTarget::SetupClo,
        PickTarget::SetupRoster,
        PickTarget::AddScans,
        PickTarget::AddScanFolder,
        PickTarget::RemarkBooks,
        PickTarget::RemarkPoints,
        PickTarget::CompareBooks,
        PickTarget::ParallelDir,
        PickTarget::Workspace,
    ];

    /// The name the window uses for it (`App.browse("new-scans")`).
    pub fn name(self) -> &'static str {
        match self {
            PickTarget::NewScans => "new-scans",
            PickTarget::NewScanFolder => "new-scan-folder",
            PickTarget::NewKey => "new-key",
            PickTarget::NewKeySheets => "new-key-sheets",
            PickTarget::NewRoster => "new-roster",
            PickTarget::NewClo => "new-clo",
            PickTarget::NewSpec => "new-spec",
            PickTarget::NewOut => "new-out",
            PickTarget::SetupKey => "setup-key",
            PickTarget::SetupKeySheets => "setup-key-sheets",
            PickTarget::SetupClo => "setup-clo",
            PickTarget::SetupRoster => "setup-roster",
            PickTarget::AddScans => "add-scans",
            PickTarget::AddScanFolder => "add-scan-folder",
            PickTarget::RemarkBooks => "remark-books",
            PickTarget::RemarkPoints => "remark-points",
            PickTarget::CompareBooks => "compare-books",
            PickTarget::ParallelDir => "parallel-dir",
            PickTarget::Workspace => "workspace",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.name() == name)
    }

    /// The New-exam card's field it fills (`None`: not a field of the card).
    pub fn card_field(self) -> Option<crate::backend::CardField> {
        use crate::backend::CardField as F;
        Some(match self {
            PickTarget::NewScans | PickTarget::NewScanFolder => F::Scans,
            PickTarget::NewKey => F::Key,
            PickTarget::NewKeySheets => F::KeySheets,
            PickTarget::NewRoster => F::Roster,
            PickTarget::NewClo => F::Clo,
            PickTarget::NewSpec => F::Sheet,
            PickTarget::NewOut => F::Folder,
            _ => return None,
        })
    }

    /// The field it fills: a Folder… button fills the field of its Browse… button.
    pub fn field(self) -> Self {
        match self {
            PickTarget::NewScanFolder => PickTarget::NewScans,
            PickTarget::AddScanFolder => PickTarget::AddScans,
            t => t,
        }
    }

    pub fn kind(self) -> PickKind {
        match self {
            PickTarget::NewScans
            | PickTarget::NewKeySheets
            | PickTarget::SetupKeySheets
            | PickTarget::AddScans
            | PickTarget::RemarkBooks
            | PickTarget::CompareBooks => PickKind::Files,
            PickTarget::NewScanFolder
            | PickTarget::NewOut
            | PickTarget::AddScanFolder
            | PickTarget::ParallelDir
            | PickTarget::Workspace => PickKind::Folder,
            PickTarget::NewKey
            | PickTarget::NewRoster
            | PickTarget::NewClo
            | PickTarget::NewSpec
            | PickTarget::SetupKey
            | PickTarget::SetupClo
            | PickTarget::SetupRoster
            | PickTarget::RemarkPoints => PickKind::File,
        }
    }

    /// The dialog's title.
    pub fn title(self) -> &'static str {
        match self {
            PickTarget::NewScans => "Choose the scans to grade",
            PickTarget::NewScanFolder => "Choose the folder of scans to grade",
            PickTarget::NewKey | PickTarget::SetupKey => "Choose the answer key file",
            PickTarget::NewKeySheets | PickTarget::SetupKeySheets => {
                "Choose the scans of the filled key sheets"
            }
            PickTarget::NewRoster | PickTarget::SetupRoster => "Choose the roster",
            PickTarget::NewClo | PickTarget::SetupClo => "Choose the CLO mapping file",
            PickTarget::NewSpec => "Choose the spec file of your sheet",
            PickTarget::NewOut => "Choose the folder for the new exam",
            PickTarget::AddScans => "Choose the scans to add",
            PickTarget::AddScanFolder => "Choose the folder of scans to add",
            PickTarget::RemarkBooks => "Choose Remark's graded Excel exports",
            PickTarget::RemarkPoints => "Choose the points file",
            PickTarget::CompareBooks => "Choose Remark's graded Excel exports to compare",
            PickTarget::ParallelDir => "Choose the folder for the report",
            PickTarget::Workspace => "Choose the folder that holds your exams",
        }
    }

    /// The file types offered, the first shown first; a folder dialog has none.
    pub fn filters(self) -> Vec<(String, Vec<String>)> {
        let one = |name: &str, exts: &[&str]| {
            (
                name.to_string(),
                exts.iter().map(|e| e.to_string()).collect::<Vec<_>>(),
            )
        };
        let all = one("All files", &["*"]);
        match self {
            PickTarget::NewScans
            | PickTarget::NewKeySheets
            | PickTarget::SetupKeySheets
            | PickTarget::AddScans => vec![one("Scans", SCANS), all],
            // keys are read as delimited text: an Excel workbook is saved as CSV first
            PickTarget::NewKey | PickTarget::SetupKey => {
                vec![one("Key file", &["csv", "tsv", "txt"]), all]
            }
            PickTarget::NewRoster | PickTarget::SetupRoster => {
                vec![one("Roster", &["csv", "xlsx"]), all]
            }
            PickTarget::NewClo | PickTarget::SetupClo => vec![one("CLO file", &["csv"]), all],
            PickTarget::NewSpec => vec![one("Sheet spec", &["json"]), all],
            // only the graded exports: Remark's own files are never opened
            PickTarget::RemarkBooks => vec![one("Remark exports", &["xlsx"])],
            PickTarget::RemarkPoints => vec![one("Points file", &["csv"]), all],
            PickTarget::CompareBooks => vec![one(
                "Remark exports and handwritten student numbers",
                &["xlsx", "csv"],
            )],
            PickTarget::NewScanFolder
            | PickTarget::NewOut
            | PickTarget::AddScanFolder
            | PickTarget::ParallelDir
            | PickTarget::Workspace => Vec::new(),
        }
    }

    /// The [`Prefs::last_dirs`] entry its dialog starts in and updates.
    pub fn dir_key(self) -> &'static str {
        match self {
            PickTarget::NewScans
            | PickTarget::NewScanFolder
            | PickTarget::NewKeySheets
            | PickTarget::SetupKeySheets
            | PickTarget::AddScans
            | PickTarget::AddScanFolder => "scans",
            PickTarget::NewOut | PickTarget::ParallelDir | PickTarget::Workspace => "folders",
            _ => "files",
        }
    }
}

/// One file dialog to open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickRequest {
    pub target: PickTarget,
    pub kind: PickKind,
    pub title: String,
    /// (name, extensions without the dot; `*` for all files).
    pub filters: Vec<(String, Vec<String>)>,
    /// The folder it opens in (it may no longer exist).
    pub start: Option<PathBuf>,
}

/// Opens a file dialog and gives its answer, on the UI thread, once it is closed: the paths
/// picked, or `None` when nothing was (cancelled, or no dialog could open). The answer may
/// come during the call or later; a dialog is never modal to the event loop.
pub type Pick = Rc<dyn Fn(PickRequest, Box<dyn FnOnce(Option<Vec<PathBuf>>)>)>;

/// Shows an existing folder in the system's file manager.
pub type Reveal = Rc<dyn Fn(&Path) -> Result<(), String>>;

/// Saves the preferences (best effort).
pub type SavePrefs = Rc<dyn Fn(&Prefs)>;

/// What the binary supplies; every part is optional.
#[derive(Clone, Default)]
pub struct Host {
    pub pick: Option<Pick>,
    pub reveal: Option<Reveal>,
    pub save_prefs: Option<SavePrefs>,
    /// The preferences read at start.
    pub prefs: Prefs,
}

/// The desktop's preferences (`desktop.json` in the private data folder), schema version 1.
/// Read tolerantly: a missing, unknown or mistyped entry takes its default, and the file of
/// earlier versions (`{"workspace": "…"}` only) reads as version 1.
#[derive(Clone, Debug, PartialEq)]
pub struct Prefs {
    /// The exams folder opened last.
    pub workspace: Option<PathBuf>,
    /// Exams folders opened before, the latest first (at most [`Prefs::RECENT`]).
    pub recent: Vec<PathBuf>,
    /// The name recorded in the audit trail.
    pub operator: Option<String>,
    /// `light` or `dark`.
    pub theme: Option<String>,
    pub reduce_motion: bool,
    pub single_key: bool,
    pub show_all_default: bool,
    /// Write the exports when a grading run ends.
    pub auto_export: bool,
    /// The folder each kind of file dialog picked from last (`scans`, `files`, `folders`).
    pub last_dirs: BTreeMap<String, PathBuf>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            workspace: None,
            recent: Vec::new(),
            operator: None,
            theme: None,
            reduce_motion: false,
            single_key: true,
            show_all_default: false,
            auto_export: true,
            last_dirs: BTreeMap::new(),
        }
    }
}

impl Prefs {
    pub const VERSION: u64 = 1;
    pub const RECENT: usize = 5;

    /// `dir` is the exams folder opened last: the one to open next time, and the latest of
    /// the recent folders (each folder once, at most [`Prefs::RECENT`]).
    pub fn remember_workspace(&mut self, dir: PathBuf) {
        self.recent.retain(|r| *r != dir);
        self.recent.insert(0, dir.clone());
        self.recent.truncate(Self::RECENT);
        self.workspace = Some(dir);
    }

    /// The preferences in `bytes`; anything unreadable takes its default.
    pub fn from_json(bytes: &[u8]) -> Self {
        use serde_json::Value;
        let d = Self::default();
        let Ok(Value::Object(m)) = serde_json::from_slice::<Value>(bytes) else {
            return d;
        };
        let text = |k: &str| {
            m.get(k)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
        };
        let flag = |k: &str, default: bool| m.get(k).and_then(Value::as_bool).unwrap_or(default);
        let path = |v: &Value| {
            v.as_str()
                .filter(|t| !t.trim().is_empty())
                .map(PathBuf::from)
        };
        Self {
            workspace: m.get("workspace").and_then(path),
            recent: m
                .get("recent")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(path).take(Self::RECENT).collect())
                .unwrap_or_default(),
            operator: text("operator"),
            theme: text("theme").filter(|t| t == "light" || t == "dark"),
            reduce_motion: flag("reduce_motion", d.reduce_motion),
            single_key: flag("single_key", d.single_key),
            show_all_default: flag("show_all_default", d.show_all_default),
            auto_export: flag("auto_export", d.auto_export),
            last_dirs: m
                .get("last_dirs")
                .and_then(Value::as_object)
                .map(|o| {
                    o.iter()
                        .filter_map(|(k, v)| Some((k.clone(), path(v)?)))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// The preferences as the file holds them (pretty JSON, with the schema version).
    pub fn to_json(&self) -> Vec<u8> {
        let path = |p: &Path| serde_json::Value::from(p.to_string_lossy().into_owned());
        let v = serde_json::json!({
            "version": Self::VERSION,
            "workspace": self.workspace.as_deref().map(path),
            "recent": self.recent.iter().map(|p| path(p)).collect::<Vec<_>>(),
            "operator": self.operator,
            "theme": self.theme,
            "reduce_motion": self.reduce_motion,
            "single_key": self.single_key,
            "show_all_default": self.show_all_default,
            "auto_export": self.auto_export,
            "last_dirs": self
                .last_dirs
                .iter()
                .map(|(k, p)| (k.clone(), path(p)))
                .collect::<serde_json::Map<_, _>>(),
        });
        serde_json::to_vec_pretty(&v).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefs_reads_the_old_workspace_only_file() {
        let p = Prefs::from_json(br#"{"workspace": "/home/huda/OMR Exams"}"#);
        assert_eq!(p.workspace, Some(PathBuf::from("/home/huda/OMR Exams")));
        assert_eq!(
            p,
            Prefs {
                workspace: p.workspace.clone(),
                ..Prefs::default()
            }
        );
        // what the defaults are: one-key review and exports after grading are on
        assert!(p.single_key && p.auto_export);
        assert!(!p.reduce_motion && !p.show_all_default);
    }

    #[test]
    fn prefs_ignores_unknown_keys_and_bad_json() {
        for bad in [
            &b""[..],
            b"not json",
            b"[1, 2]",
            b"\"text\"",
            b"{\"workspace\": ",
        ] {
            assert_eq!(Prefs::from_json(bad), Prefs::default(), "{bad:?}");
        }
        let p = Prefs::from_json(
            br#"{"version": 7, "colour": "teal", "workspace": 3, "operator": "  ",
                 "theme": "purple", "auto_export": "yes", "single_key": false,
                 "recent": ["/a", 4, "", "/b", "/c", "/d", "/e", "/f"],
                 "last_dirs": {"scans": "/s", "files": 9}}"#,
        );
        assert_eq!(p.workspace, None, "a number is no folder");
        assert_eq!(p.operator, None, "a blank name is none");
        assert_eq!(p.theme, None, "only light or dark");
        assert!(p.auto_export, "a mistyped switch keeps its default");
        assert!(!p.single_key, "a known entry of a newer file is read");
        let recent: Vec<_> = ["/a", "/b", "/c", "/d", "/e"].map(PathBuf::from).into();
        assert_eq!(p.recent, recent, "five at most, unreadable entries dropped");
        assert_eq!(
            p.last_dirs,
            BTreeMap::from([("scans".to_string(), PathBuf::from("/s"))])
        );
    }

    #[test]
    fn prefs_round_trip() {
        let p = Prefs {
            workspace: Some(PathBuf::from("/exams/فصل ١")),
            recent: vec![PathBuf::from("/exams/old"), PathBuf::from("/other")],
            operator: Some("Huda A".into()),
            theme: Some("dark".into()),
            reduce_motion: true,
            single_key: false,
            show_all_default: true,
            auto_export: false,
            last_dirs: BTreeMap::from([
                ("scans".into(), PathBuf::from("/scans/batch;1")),
                ("files".into(), PathBuf::from("/keys")),
            ]),
        };
        let json = p.to_json();
        assert_eq!(Prefs::from_json(&json), p);
        let v: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(
            Prefs::from_json(&Prefs::default().to_json()),
            Prefs::default()
        );
    }

    #[test]
    fn the_folder_opened_last_leads_the_recent_folders() {
        let mut p = Prefs::default();
        for d in ["/a", "/b", "/c", "/d", "/e", "/f", "/c"] {
            p.remember_workspace(PathBuf::from(d));
        }
        assert_eq!(p.workspace, Some(PathBuf::from("/c")));
        let recent: Vec<_> = ["/c", "/f", "/e", "/d", "/b"].map(PathBuf::from).into();
        assert_eq!(
            p.recent, recent,
            "each once, the latest first, five at most"
        );
    }

    #[test]
    fn every_target_has_a_unique_name_and_a_dialog() {
        let mut names: Vec<_> = PickTarget::ALL.iter().map(|t| t.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), PickTarget::ALL.len());
        for t in PickTarget::ALL {
            assert_eq!(PickTarget::from_name(t.name()), Some(t));
            assert!(!t.title().is_empty());
            // file dialogs filter, folder dialogs do not
            assert_eq!(
                t.filters().is_empty(),
                t.kind() == PickKind::Folder,
                "{t:?}"
            );
            assert_eq!(t.field().field(), t.field());
        }
        assert_eq!(PickTarget::from_name("nowhere"), None);
    }
}
