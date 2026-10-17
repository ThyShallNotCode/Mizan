//! The device's checkpoint key and the checkpoints it keeps outside every package (§14.4,
//! §19.2, in the MVP's software form).
//!
//! A per-user ECDSA P-384 key (`device-key.p384`, mode 0600 in a 0700 folder) signs the chain
//! checkpoints omr-app writes at batch end, export and close (and every 1,000 events). The
//! latest checkpoint of each project is also kept in that folder (`checkpoints/<project
//! UUID>.json`), outside the package: a chain cut short or rewritten behind the app's back no
//! longer verifies on open (SYS-011), even when the checkpoints stored in the package were
//! removed with it. The folder is `OMR_KEY_DIR` when set, else the platform data folder
//! (`$XDG_DATA_HOME/omr` or `~/.local/share/omr`; `%APPDATA%\omr` on Windows).
//!
//! Deferred (§19.2): the OS key store or TPM, rotation, and certification of device keys by
//! the institution. A checkpoint signed on another device is checked for a consistent
//! signature and its head hash only.

use crate::AppError;
use omr_store::{
    ChainError, Checkpoint, CheckpointSigner, CheckpointVerifier, EcdsaP384Signer, TrustedKeys,
};
use std::path::{Path, PathBuf};

/// File name of the device key (hex of the 48-byte secret scalar).
pub const DEVICE_KEY_FILE: &str = "device-key.p384";

/// What to do when the key folder cannot be used (R10).
const REMEDY_KEY_DIR: &str = "make the folder writable by this user only, or set OMR_KEY_DIR \
                              to a private local folder (not a sync or network folder)";

/// The folder holding the device key and the external checkpoints.
pub fn key_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("OMR_KEY_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d));
    }
    if cfg!(windows) {
        return std::env::var_os("APPDATA")
            .filter(|d| !d.is_empty())
            .map(|d| PathBuf::from(d).join("omr"));
    }
    std::env::var_os("XDG_DATA_HOME")
        .filter(|d| !d.is_empty())
        .map(|d| PathBuf::from(d).join("omr"))
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|d| !d.is_empty())
                .map(|h| PathBuf::from(h).join(".local").join("share").join("omr"))
        })
}

fn dir_or_err() -> Result<PathBuf, AppError> {
    key_dir().ok_or_else(|| {
        AppError::Store(format!(
            "no folder for the device's checkpoint key (HOME is not set): {REMEDY_KEY_DIR}"
        ))
    })
}

/// Creates `dir` (and its parents) private to this user.
pub fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

/// Writes `bytes` to a new private file (mode 0600 on Unix) at `path`; refuses an existing
/// file.
pub fn write_private_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// The device's checkpoint signer, created on first use. Two processes creating it at once
/// agree on one key (the file is linked into place without replacing).
pub fn device_signer() -> Result<EcdsaP384Signer, AppError> {
    let dir = dir_or_err()?;
    let fail = |what: &str, e: &dyn std::fmt::Display| {
        AppError::Store(format!(
            "device checkpoint key in {}: {what}: {e}; {REMEDY_KEY_DIR}",
            dir.display()
        ))
    };
    let path = dir.join(DEVICE_KEY_FILE);
    for _ in 0..50 {
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let secret = hex::decode(text.trim()).map_err(|e| fail("unreadable key", &e))?;
                return EcdsaP384Signer::from_secret_bytes(&secret)
                    .map_err(|e| fail("unusable key", &e));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(fail("cannot read the key", &e)),
        }
        create_private_dir(&dir).map_err(|e| fail("cannot create the folder", &e))?;
        // A fresh scalar (a random 48-byte value is a valid P-384 scalar with overwhelming
        // probability; an invalid one is drawn again).
        let secret = loop {
            let mut s = [0u8; 48];
            rand::fill(&mut s);
            if EcdsaP384Signer::from_secret_bytes(&s).is_ok() {
                break s;
            }
        };
        let tmp = dir.join(format!(
            "{DEVICE_KEY_FILE}.{}.tmp",
            uuid::Uuid::now_v7().simple()
        ));
        write_private_new(&tmp, hex::encode(secret).as_bytes())
            .map_err(|e| fail("cannot write the key", &e))?;
        let linked = std::fs::hard_link(&tmp, &path);
        let _ = std::fs::remove_file(&tmp);
        match linked {
            Ok(()) => {}
            // Another process created it first: use theirs.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(fail("cannot store the key", &e)),
        }
    }
    Err(AppError::Store(format!(
        "device checkpoint key in {}: could not be created; {REMEDY_KEY_DIR}",
        dir.display()
    )))
}

/// Verifies stored checkpoints: those of this device's key fully; those of another device's
/// key for a signature consistent with the key they carry (device certification is
/// deferred, §19.2).
pub struct DeviceVerifier {
    own: TrustedKeys,
    own_id: String,
}

impl DeviceVerifier {
    /// Trusts `signer`'s key.
    pub fn new(signer: &dyn CheckpointSigner) -> Result<Self, AppError> {
        let mut own = TrustedKeys::new();
        let own_id = own
            .trust_signer(signer)
            .map_err(|e| AppError::Internal(format!("device key: {e}")))?;
        Ok(Self { own, own_id })
    }
}

impl CheckpointVerifier for DeviceVerifier {
    fn verify(&self, cp: &Checkpoint) -> Result<(), ChainError> {
        if cp.key_id == self.own_id {
            return self.own.verify(cp);
        }
        let mut theirs = TrustedKeys::new();
        theirs
            .trust_sec1(&cp.public_key)
            .map_err(|_| ChainError::BadSignature { seq: cp.seq })?;
        theirs.verify(cp)
    }
}

fn checkpoint_path(project: &str) -> Option<PathBuf> {
    // Project UUIDs only (no path separators can reach the file name).
    if project.is_empty() || !project.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return None;
    }
    key_dir().map(|d| d.join("checkpoints").join(format!("{project}.json")))
}

/// The latest checkpoint this device holds for `project`, its signature verified (see
/// [`DeviceVerifier`]; a key replaced since keeps its checkpoints usable). `Ok(None)` when it
/// holds none; an error when the file was altered (SYS-011).
pub fn external_checkpoint(
    project: &str,
    verifier: &DeviceVerifier,
) -> Result<Option<Checkpoint>, AppError> {
    let Some(path) = checkpoint_path(project) else {
        return Ok(None);
    };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(AppError::Store(format!(
                "{}: {e}; {REMEDY_KEY_DIR}",
                path.display()
            )));
        }
    };
    let altered = |why: String| {
        AppError::Store(format!(
            "SYS-011 audit chain: this device's checkpoint of the project ({}) {why}; the \
             project is not opened. Report it to the exam's QA officer",
            path.display()
        ))
    };
    let cp: Checkpoint =
        serde_json::from_slice(&bytes).map_err(|e| altered(format!("is unreadable ({e})")))?;
    if cp.project != project {
        return Err(altered("names another project".into()));
    }
    verifier
        .verify(&cp)
        .map_err(|e| altered(format!("does not verify ({e})")))?;
    Ok(Some(cp))
}

/// Keeps `cp` as the device's latest checkpoint of its project (an older one never replaces
/// a newer one).
pub fn record_checkpoint(cp: &Checkpoint) -> Result<(), AppError> {
    let Some(path) = checkpoint_path(&cp.project) else {
        return Ok(());
    };
    let fail = |e: &dyn std::fmt::Display| {
        AppError::Store(format!(
            "cannot keep the checkpoint in {}: {e}; {REMEDY_KEY_DIR}",
            path.display()
        ))
    };
    if let Ok(b) = std::fs::read(&path)
        && let Ok(old) = serde_json::from_slice::<Checkpoint>(&b)
        && old.seq > cp.seq
    {
        return Ok(());
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    create_private_dir(dir).map_err(|e| fail(&e))?;
    let tmp = dir.join(format!(
        "{}.{}.tmp",
        cp.project,
        uuid::Uuid::now_v7().simple()
    ));
    let bytes = serde_json::to_vec(cp).map_err(|e| fail(&e))?;
    write_private_new(&tmp, &bytes).map_err(|e| fail(&e))?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        fail(&e)
    })
}

fn registry_path(exam: &omr_model::ExamId) -> Option<PathBuf> {
    key_dir().map(|d| d.join("exams").join(format!("{exam}.json")))
}

/// The other project this device graded exam `exam` into, if that project still exists
/// (SEC-04): sheets printed for one exam must not be accepted by a second exam that reuses
/// their spec and key, e.g. another sitting.
pub fn exam_claimed_by(exam: &omr_model::ExamId, project_dir: &Path) -> Option<PathBuf> {
    let bytes = std::fs::read(registry_path(exam)?).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let other = PathBuf::from(v["project"].as_str()?);
    let me = std::path::absolute(project_dir).ok()?;
    (other != me && other.join("project.omr").exists()).then_some(other)
}

/// Records `project_dir` as the project of exam `exam` on this device.
pub fn claim_exam(exam: &omr_model::ExamId, project_dir: &Path) -> Result<(), AppError> {
    let Some(path) = registry_path(exam) else {
        return Ok(());
    };
    let me = std::path::absolute(project_dir)?;
    let dir = path.parent().unwrap_or(Path::new("."));
    create_private_dir(dir)?;
    let tmp = dir.join(format!("{exam}.{}.tmp", uuid::Uuid::now_v7().simple()));
    let body = serde_json::json!({"project": me.to_string_lossy()});
    write_private_new(&tmp, body.to_string().as_bytes())?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        AppError::from(e)
    })
}

/// Writes `bytes` to `path`, replacing it, as a file private to the user (mode 0600 on Unix,
/// also when it existed with a wider mode): exports, crops and reports hold Restricted data
/// (PDPL, §16.3).
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        o.mode(0o600);
        if let Ok(m) = std::fs::metadata(path)
            && m.permissions().mode() & 0o077 != 0
        {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    let mut f = o.open(path)?;
    f.write_all(bytes)
}

/// Prepares an export destination (§16.3, SYS-015): refused inside a sync folder or on a
/// network mount (Restricted data must stay on university devices); created private to the
/// user when it does not exist yet (an existing folder keeps its permissions).
pub fn export_dir(dir: &Path) -> Result<(), AppError> {
    if !dir.exists() {
        create_private_dir(dir)?;
    }
    omr_store::location::check_location(dir).map_err(crate::store::store_error)
}
