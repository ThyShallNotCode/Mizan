//! Content-addressed object store for page images and other originals (spec §14.5).
//!
//! Address = BLAKE3 of the plaintext, used **only** for deduplication and file placement (a
//! non-security use under the §19.2 BLAKE3 scope rule). Integrity rests on SHA-384: the
//! plaintext SHA-384 is the evidence hash recorded in the `object` row and in manifests, and
//! every verified read recomputes it.
//!
//! Write order (a database row never points at a missing file; a crash leaves at most an
//! orphan): stream to `spool/tmp/`, hashing BLAKE3 and SHA-384 in one pass → `fsync` the file →
//! link into `objects/ab/cd/<blake3>` without replacing (a hard link, which fails if the
//! target exists, then unlink of the temporary) → `fsync` the directory → the caller inserts
//! the row. Envelope encryption (AES-256-GCM STREAM, §14.5) is deferred in the MVP
//! (docs/IMPLEMENTATION.md §2): objects are stored as plaintext originals.

use crate::error::{Result, StoreError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha384};
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Orphans and stale spool files are swept after 24 h (§14.5).
pub const ORPHAN_GRACE: Duration = Duration::from_secs(24 * 3600);

/// What an object holds (§14.3 `object.kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectKind {
    /// An ingested original (scan or PDF page image).
    Original,
    /// A retained evidence crop (§14.5 retention).
    EvidenceCrop,
    /// A course-file exemplar.
    Exemplar,
}

impl ObjectKind {
    /// Column value.
    pub fn as_str(self) -> &'static str {
        match self {
            ObjectKind::Original => "original",
            ObjectKind::EvidenceCrop => "evidence_crop",
            ObjectKind::Exemplar => "exemplar",
        }
    }

    /// Parses a column value.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "original" => Some(ObjectKind::Original),
            "evidence_crop" => Some(ObjectKind::EvidenceCrop),
            "exemplar" => Some(ObjectKind::Exemplar),
            _ => None,
        }
    }
}

/// An object made durable under its address, ready for its database row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedObject {
    /// BLAKE3 of the plaintext (hex): dedup address, non-security.
    pub blake3: String,
    /// SHA-384 of the plaintext (hex): the integrity/evidence hash.
    pub sha384: String,
    /// Length in bytes.
    pub len: u64,
    /// The address already held these exact bytes (an exact duplicate, CAP-011).
    pub already_present: bool,
}

/// One entry of the SHA-384 object manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// BLAKE3 dedup address (non-security).
    pub blake3: String,
    /// Plaintext SHA-384.
    pub sha384: String,
    /// Length.
    pub len: u64,
    /// Kind.
    pub kind: ObjectKind,
}

/// The object manifest of a project (archives, exports, `omr-verify`, §14.1): entries sorted by
/// SHA-384; its own SHA-384 is taken over its JCS form.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectManifest {
    /// Hash algorithm IDs: integrity and dedup.
    pub algorithms: [String; 2],
    /// Entries.
    pub entries: Vec<ManifestEntry>,
}

impl ObjectManifest {
    /// SHA-384 (hex) of the manifest's JCS form.
    pub fn sha384(&self) -> Result<String> {
        Ok(crate::jcs::jcs_sha384_hex(self)?)
    }
}

/// What a sweep removed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Loose objects without a database row, older than the grace period.
    pub orphans_removed: Vec<String>,
    /// Stale spool files.
    pub spool_removed: usize,
}

/// The `objects/` tree and its `spool/tmp/` staging directory.
#[derive(Clone, Debug)]
pub struct ObjectStore {
    objects: PathBuf,
    spool: PathBuf,
}

pub(crate) fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(dir)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        Ok(())
    }
}

fn is_hex_address(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

impl ObjectStore {
    /// A store rooted at `objects` with staging in `spool` (both created if missing).
    pub fn new(objects: impl Into<PathBuf>, spool: impl Into<PathBuf>) -> Result<Self> {
        let s = Self {
            objects: objects.into(),
            spool: spool.into(),
        };
        fs::create_dir_all(&s.objects)?;
        fs::create_dir_all(&s.spool)?;
        Ok(s)
    }

    /// Path of the object at `address`: `objects/ab/cd/<address>`.
    pub fn path_of(&self, address: &str) -> Result<PathBuf> {
        if !is_hex_address(address) {
            return Err(StoreError::Invalid(format!(
                "not a BLAKE3 address: {address:?}"
            )));
        }
        Ok(self
            .objects
            .join(&address[..2])
            .join(&address[2..4])
            .join(address))
    }

    /// Streams `reader` into the store (steps 1–4 of the write order).
    pub fn stage(&self, mut reader: impl Read) -> Result<StagedObject> {
        let tmp = self
            .spool
            .join(format!("{}.part", uuid::Uuid::now_v7().simple()));
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        let result = (|| -> Result<StagedObject> {
            let mut b3 = blake3::Hasher::new();
            let mut s384 = Sha384::new();
            let mut len = 0u64;
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e.into()),
                };
                b3.update(&buf[..n]);
                s384.update(&buf[..n]);
                file.write_all(&buf[..n])?;
                len += n as u64;
            }
            file.sync_all()?;
            let blake3 = b3.finalize().to_hex().to_string();
            let sha384 = hex::encode(s384.finalize());
            let target = self.path_of(&blake3)?;
            let parent = target
                .parent()
                .ok_or_else(|| StoreError::Invalid("object path has no parent".into()))?;
            fs::create_dir_all(parent)?;
            let already_present = match fs::hard_link(&tmp, &target) {
                Ok(()) => false,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => true,
                Err(_) if !target.exists() => {
                    // File systems without hard links (FAT32, some exFAT): rename. The single
                    // writer holds the project lock, so no other process races for the name.
                    fs::rename(&tmp, &target)?;
                    false
                }
                Err(_) => true,
            };
            if already_present {
                let (_, existing, _) = hash_file(&target)?;
                if existing != sha384 {
                    return Err(StoreError::ObjectCorrupt {
                        address: blake3,
                        detail: "existing object differs from the ingested bytes".into(),
                    });
                }
            }
            sync_dir(parent)?;
            Ok(StagedObject {
                blake3,
                sha384,
                len,
                already_present,
            })
        })();
        drop(file);
        if tmp.exists() {
            let _ = fs::remove_file(&tmp);
            let _ = sync_dir(&self.spool);
        }
        result
    }

    /// Stages an in-memory buffer.
    pub fn stage_bytes(&self, bytes: &[u8]) -> Result<StagedObject> {
        self.stage(bytes)
    }

    /// Reads an object without verification (callers that need evidence use
    /// [`ObjectStore::read_verified`]).
    pub fn read(&self, address: &str) -> Result<Vec<u8>> {
        Ok(fs::read(self.path_of(address)?)?)
    }

    /// Reads an object and checks its SHA-384 against the recorded value.
    pub fn read_verified(&self, address: &str, sha384: &str) -> Result<Vec<u8>> {
        let bytes = self.read(address)?;
        let got = hex::encode(Sha384::digest(&bytes));
        if got != sha384 {
            return Err(StoreError::ObjectCorrupt {
                address: address.into(),
                detail: "SHA-384 mismatch".into(),
            });
        }
        Ok(bytes)
    }

    /// Recomputes SHA-384 (integrity), the length and the BLAKE3 address (placement check).
    pub fn verify(&self, address: &str, sha384: &str, len: u64) -> Result<()> {
        let path = self.path_of(address)?;
        let (b3, s, n) = hash_file(&path).map_err(|e| match e {
            StoreError::Io(io) if io.kind() == io::ErrorKind::NotFound => {
                StoreError::ObjectCorrupt {
                    address: address.into(),
                    detail: "file missing".into(),
                }
            }
            other => other,
        })?;
        let fail = |d: &str| StoreError::ObjectCorrupt {
            address: address.into(),
            detail: d.into(),
        };
        if s != sha384 {
            return Err(fail("SHA-384 mismatch"));
        }
        if n != len {
            return Err(fail("length mismatch"));
        }
        if b3 != address {
            return Err(fail("not at its dedup address"));
        }
        Ok(())
    }

    /// Every loose object address present on disk.
    pub fn list(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for a in read_dir_sorted(&self.objects)? {
            for b in read_dir_sorted(&a)? {
                for f in read_dir_sorted(&b)? {
                    if let Some(name) = f.file_name().and_then(|n| n.to_str())
                        && is_hex_address(name)
                    {
                        out.push(name.to_owned());
                    }
                }
            }
        }
        Ok(out)
    }

    /// Removes loose objects that have no row in `referenced` and spool leftovers, both older
    /// than `grace` at `now` (§14.5: orphans swept after 24 h).
    pub fn sweep(
        &self,
        referenced: &BTreeSet<String>,
        grace: Duration,
        now: SystemTime,
    ) -> Result<SweepReport> {
        let old = |p: &Path| -> bool {
            fs::metadata(p)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .is_some_and(|age| age >= grace)
        };
        let mut report = SweepReport::default();
        for address in self.list()? {
            if referenced.contains(&address) {
                continue;
            }
            let p = self.path_of(&address)?;
            if old(&p) {
                fs::remove_file(&p)?;
                report.orphans_removed.push(address);
            }
        }
        for f in read_dir_sorted(&self.spool)? {
            if f.is_file() && old(&f) {
                fs::remove_file(&f)?;
                report.spool_removed += 1;
            }
        }
        Ok(report)
    }
}

fn read_dir_sorted(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut v: Vec<PathBuf> = fs::read_dir(dir)?
        .map(|e| e.map(|e| e.path()))
        .collect::<io::Result<_>>()?;
    v.sort();
    Ok(v)
}

/// (BLAKE3 hex, SHA-384 hex, length) of a file.
fn hash_file(path: &Path) -> Result<(String, String, u64)> {
    let mut f = File::open(path)?;
    let mut b3 = blake3::Hasher::new();
    let mut s = Sha384::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut len = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        b3.update(&buf[..n]);
        s.update(&buf[..n]);
        len += n as u64;
    }
    Ok((
        b3.finalize().to_hex().to_string(),
        hex::encode(s.finalize()),
        len,
    ))
}
