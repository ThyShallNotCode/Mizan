//! The project package `Exam.omrproj/` and its single writer (spec §14.1, §14.2).
//!
//! ```text
//! Exam.omrproj/
//!   project.omr              marker + header (format, project UUID, workspace/offering/assessment)
//!   project.db (-wal, -shm)  SQLite: metadata, events, small blobs
//!   objects/ab/cd/<blake3>   immutable originals (BLAKE3 = dedup address, non-security)
//!   spool/tmp/               in-flight files
//!   snapshots/               DB snapshots (VACUUM INTO)
//!   cache/                   derived, never backed up
//!   exports/                 artefacts + SHA-384 manifests
//!   .lock                    OS advisory lock of the single writer process
//! ```
//!
//! [`Project`] owns the only read-write connection. Every write runs in one transaction on a
//! durability lane ([`Lane::Decision`] = `synchronous FULL`, [`Lane::Bulk`] = `NORMAL`) under
//! `catch_unwind`: an error or a panic rolls the whole transaction back, so a crash or a failed
//! command never leaves a partial batch or an event without its state change. A `command_id`
//! makes a retried command a no-op. The writer *thread* (actor) belongs to `omr-app` (R4).

use crate::checkpoint::{CHECKPOINT_EVERY, Checkpoint, CheckpointSigner, CheckpointVerifier};
use crate::clock::{Clock, SystemClock, hlc_next, rfc3339_utc};
use crate::db::{self, Lane};
use crate::error::{Result, StoreError};
use crate::events::{self, Actor, ChainReport, EventCtx, EventRecord, NewEvent, genesis_hash};
use crate::jcs;
use crate::location;
use crate::model::Reader;
use crate::objects::{ObjectManifest, ObjectStore, SweepReport};
use crate::schema;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions as FsOpenOptions};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use uuid::Uuid;

/// Package format major version (§14.7 read-compat ≥ 2 majors).
pub const FORMAT_MAJOR: u32 = 1;
/// Package format minor version.
pub const FORMAT_MINOR: u32 = 0;
/// Default classification label of a project (§14.3 `label`, §16.3).
pub const DEFAULT_LABEL: &str = "restricted";
/// Recommended maximum pages per bulk-lane transaction (§14.2).
pub const BULK_GROUP_PAGES: usize = 50;

/// Paths inside a package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackagePaths {
    /// Package root (`Exam.omrproj`).
    pub root: PathBuf,
    /// `project.omr` marker.
    pub marker: PathBuf,
    /// `project.db`.
    pub db: PathBuf,
    /// `objects/`.
    pub objects: PathBuf,
    /// `spool/tmp/`.
    pub spool: PathBuf,
    /// `snapshots/`.
    pub snapshots: PathBuf,
    /// `cache/`.
    pub cache: PathBuf,
    /// `exports/`.
    pub exports: PathBuf,
    /// `.lock`.
    pub lock: PathBuf,
}

impl PackagePaths {
    /// The layout under `root`.
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            marker: root.join("project.omr"),
            db: root.join("project.db"),
            objects: root.join("objects"),
            spool: root.join("spool").join("tmp"),
            snapshots: root.join("snapshots"),
            cache: root.join("cache"),
            exports: root.join("exports"),
            lock: root.join(".lock"),
        }
    }
}

/// The `project.omr` header (also the `project_header` row).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectHeader {
    /// Always `omr-project`.
    pub magic: String,
    /// Format major.
    pub format_major: u32,
    /// Format minor.
    pub format_minor: u32,
    /// Project UUID (genesis of the chain).
    pub project_uuid: Uuid,
    /// Workspace (§14.9).
    pub workspace_id: Option<String>,
    /// Offering.
    pub offering_id: Option<String>,
    /// Assessment.
    pub assessment_id: Option<String>,
    /// DMO classification label (default `restricted`).
    pub label: String,
}

/// Inputs to [`Project::create`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewProject {
    /// Project UUID; a fresh UUIDv7 when `None`.
    pub project_uuid: Option<Uuid>,
    /// Workspace ID.
    pub workspace_id: Option<String>,
    /// Offering ID.
    pub offering_id: Option<String>,
    /// Assessment ID.
    pub assessment_id: Option<String>,
    /// Label; [`DEFAULT_LABEL`] when `None`.
    pub label: Option<String>,
}

/// How to open a project.
pub struct OpenOptions {
    /// This device's ID (events carry it with a per-device sequence).
    pub device_id: String,
    /// Clock (system clock by default; a step clock in tests).
    pub clock: Box<dyn Clock>,
    /// Refuse network and sync-folder locations (SYS-015).
    pub check_location: bool,
    /// Take the `.lock` OS lock.
    pub lock: bool,
    /// Verify the whole chain on open (SYS-011 on failure).
    pub verify_chain: bool,
    /// Signer for automatic checkpoints (every [`CHECKPOINT_EVERY`] events, batch end, close).
    pub signer: Option<Box<dyn CheckpointSigner + Send>>,
    /// Verifies the signatures of stored and external checkpoints on open (`None`: head
    /// hashes only).
    pub verifier: Option<Box<dyn CheckpointVerifier + Send>>,
    /// Checkpoints held outside the package (e.g. the device's latest checkpoint of this
    /// project): a truncated or rewritten chain fails against them on open (SYS-011).
    pub external: Vec<Checkpoint>,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            device_id: "device:local".into(),
            clock: Box::new(SystemClock),
            check_location: true,
            lock: true,
            verify_chain: true,
            signer: None,
            verifier: None,
            external: Vec::new(),
        }
    }
}

impl std::fmt::Debug for OpenOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenOptions")
            .field("device_id", &self.device_id)
            .field("check_location", &self.check_location)
            .field("lock", &self.lock)
            .field("verify_chain", &self.verify_chain)
            .field("signer", &self.signer.is_some())
            .field("verifier", &self.verifier.is_some())
            .field("external", &self.external.len())
            .finish_non_exhaustive()
    }
}

/// Result of a command with an idempotency key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandOutcome<T> {
    /// Committed now.
    Applied(T),
    /// The command ID was committed before (a post-crash retry); nothing was done.
    AlreadyApplied {
        /// Chain head when it was first committed.
        committed_seq: i64,
    },
}

/// One write transaction: SQL access plus event appends with the writer's context.
pub struct Tx<'a> {
    pub(crate) tx: rusqlite::Transaction<'a>,
    pub(crate) ctx: &'a mut EventCtx,
    pub(crate) objects: &'a ObjectStore,
    pub(crate) project: Uuid,
}

impl Tx<'_> {
    /// The underlying connection (inside the transaction).
    pub fn conn(&self) -> &Connection {
        &self.tx
    }

    /// Appends an event in this transaction.
    pub fn append_event(&mut self, ev: &NewEvent) -> Result<EventRecord> {
        events::append(&self.tx, self.ctx, ev)
    }

    /// The object store (files are staged before the transaction).
    pub fn objects(&self) -> &ObjectStore {
        self.objects
    }

    /// Reads through this transaction.
    pub fn reader(&self) -> Reader<'_> {
        Reader::new(&self.tx)
    }

    /// Head (seq, hash) of the chain.
    pub fn head(&self) -> Result<(i64, String)> {
        events::head(&self.tx, &self.ctx.genesis)
    }

    pub(crate) fn next_hlc(&mut self) -> i64 {
        let now = self.ctx.clock.now_unix_ms();
        self.ctx.hlc_last = hlc_next(self.ctx.hlc_last, now);
        self.ctx.hlc_last
    }

    pub(crate) fn now_utc(&self) -> String {
        rfc3339_utc(self.ctx.clock.now_unix_ms())
    }

    /// Signs the current head and records the checkpoint plus its `ChainCheckpoint` event.
    /// Idempotent for an unchanged head.
    pub fn write_checkpoint(&mut self, signer: &dyn CheckpointSigner) -> Result<Checkpoint> {
        let (seq, head_hash) = self.head()?;
        if seq == 0 {
            return Err(StoreError::Invalid(
                "cannot checkpoint an empty chain".into(),
            ));
        }
        let project = self.project.to_string();
        if let Some(cp) = events::read_checkpoints(&self.tx)?
            .into_iter()
            .find(|c| c.seq == seq)
        {
            return Ok(cp);
        }
        let hlc_ts = self.next_hlc();
        let signature = signer.sign(&Checkpoint::message(&project, seq, &head_hash, hlc_ts))?;
        let cp = Checkpoint {
            project,
            seq,
            head_hash,
            hlc_ts,
            alg: signer.algorithm().to_owned(),
            key_id: signer.key_id(),
            public_key: signer.public_key(),
            signature,
        };
        self.tx.execute(
            "INSERT INTO checkpoint (seq, project, head_hash, hlc_ts, alg, key_id, public_key, \
             signature) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                cp.seq,
                cp.project,
                cp.head_hash,
                cp.hlc_ts,
                cp.alg,
                cp.key_id,
                cp.public_key,
                cp.signature
            ],
        )?;
        self.append_event(&NewEvent::new(
            "ChainCheckpoint",
            "chain",
            &cp.project,
            Actor::system(),
            json!({
                "seq": cp.seq,
                "head_hash": cp.head_hash,
                "alg": cp.alg,
                "key_id": cp.key_id,
                "signature_sha384": jcs::sha384_hex(&cp.signature),
            }),
        ))?;
        Ok(cp)
    }
}

/// A project opened by [`Project::open_read_only`]: a `query_only` connection, no lock.
pub struct ReadOnlyProject {
    conn: Connection,
    paths: PackagePaths,
    header: ProjectHeader,
}

impl std::fmt::Debug for ReadOnlyProject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadOnlyProject")
            .field("root", &self.paths.root)
            .field("project_uuid", &self.header.project_uuid)
            .finish_non_exhaustive()
    }
}

impl ReadOnlyProject {
    /// The header.
    pub fn header(&self) -> &ProjectHeader {
        &self.header
    }

    /// Package paths.
    pub fn paths(&self) -> &PackagePaths {
        &self.paths
    }

    /// Typed reads.
    pub fn reader(&self) -> Reader<'_> {
        Reader::new(&self.conn)
    }
}

/// An open project with its single writer connection.
pub struct Project {
    conn: Connection,
    paths: PackagePaths,
    objects: ObjectStore,
    header: ProjectHeader,
    ctx: EventCtx,
    signer: Option<Box<dyn CheckpointSigner + Send>>,
    auto_checkpoint_error: Option<String>,
    _lock: Option<File>,
}

impl std::fmt::Debug for Project {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Project")
            .field("root", &self.paths.root)
            .field("project_uuid", &self.header.project_uuid)
            .finish_non_exhaustive()
    }
}

/// Makes a package folder private to its owner (mode 0700 on Unix): the database, its WAL,
/// the originals, the snapshot and every cache file hold Restricted data (PDPL), and SQLite
/// creates its files under the process umask, so the folder is the gate (§19.3). On other
/// platforms the folder keeps the owner-only ACL of the user's profile (an explicit ACL is
/// deferred to the platform layer).
pub fn restrict_to_owner(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(dir)?.permissions().mode();
        if mode & 0o077 != 0 {
            fs::set_permissions(dir, fs::Permissions::from_mode(mode & 0o700 | 0o700))?;
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

fn take_lock(path: &Path) -> Result<File> {
    let f = FsOpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    match f.try_lock() {
        Ok(()) => Ok(f),
        Err(std::fs::TryLockError::WouldBlock) => {
            Err(StoreError::Locked(path.display().to_string()))
        }
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

impl Project {
    /// Creates a new package at `root` (which must not already hold a project).
    pub fn create(root: &Path, new: NewProject, opts: OpenOptions) -> Result<Self> {
        let paths = PackagePaths::new(root);
        if paths.marker.exists() || paths.db.exists() {
            return Err(StoreError::Conflict(format!(
                "{} already holds a project",
                root.display()
            )));
        }
        fs::create_dir_all(&paths.root)?;
        restrict_to_owner(&paths.root)?;
        if opts.check_location {
            location::check_location(&paths.root)?;
        }
        for d in [
            &paths.objects,
            &paths.spool,
            &paths.snapshots,
            &paths.cache,
            &paths.exports,
        ] {
            fs::create_dir_all(d)?;
            restrict_to_owner(d)?;
        }
        let header = ProjectHeader {
            magic: "omr-project".into(),
            format_major: FORMAT_MAJOR,
            format_minor: FORMAT_MINOR,
            project_uuid: new.project_uuid.unwrap_or_else(Uuid::now_v7),
            workspace_id: new.workspace_id,
            offering_id: new.offering_id,
            assessment_id: new.assessment_id,
            label: new.label.unwrap_or_else(|| DEFAULT_LABEL.into()),
        };
        let lock = if opts.lock {
            Some(take_lock(&paths.lock)?)
        } else {
            None
        };
        let mut conn = db::open_writer(&paths.db)?;
        schema::migrate(&mut conn)?;
        let objects = ObjectStore::new(&paths.objects, &paths.spool)?;
        let ctx = EventCtx {
            device_id: opts.device_id,
            clock: opts.clock,
            hlc_last: 0,
            genesis: genesis_hash(&header.project_uuid),
        };
        let mut p = Self {
            conn,
            paths,
            objects,
            header,
            ctx,
            signer: opts.signer,
            auto_checkpoint_error: None,
            _lock: lock,
        };
        let h = p.header.clone();
        p.decision(|tx| {
            let now = tx.now_utc();
            tx.conn().execute(
                "INSERT INTO project_header (id, project_uuid, workspace_id, offering_id, \
                 assessment_id, format_major, format_minor, label, created_utc) \
                 VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    h.project_uuid.to_string(),
                    h.workspace_id,
                    h.offering_id,
                    h.assessment_id,
                    h.format_major,
                    h.format_minor,
                    h.label,
                    now
                ],
            )?;
            tx.append_event(&NewEvent::new(
                "ProjectCreated",
                "project",
                &h.project_uuid.to_string(),
                Actor::system(),
                json!({
                    "format": format!("{}.{}", h.format_major, h.format_minor),
                    "schema": schema::LATEST_VERSION,
                    "label": h.label,
                    "workspace_id": h.workspace_id,
                    "offering_id": h.offering_id,
                    "assessment_id": h.assessment_id,
                }),
            ))?;
            Ok(())
        })?;
        // The marker is written last: a crash before this point leaves no openable project.
        let marker = jcs::to_jcs(&p.header)?;
        let tmp = p.paths.root.join("project.omr.tmp");
        {
            // Synced through the writing handle: Windows refuses FlushFileBuffers on a
            // read-only one.
            use std::io::Write;
            let mut f = File::create(&tmp)?;
            f.write_all(marker.as_bytes())?;
            f.sync_all()?;
        }
        fs::rename(&tmp, &p.paths.marker)?;
        crate::objects::sync_dir(&p.paths.root)?;
        p.touch_lease()?;
        Ok(p)
    }

    /// Opens an existing package: location check, lock, migration (after a `VACUUM INTO`
    /// snapshot), canonical-schema check, `quick_check` and chain verification.
    pub fn open(root: &Path, opts: OpenOptions) -> Result<Self> {
        let paths = PackagePaths::new(root);
        if opts.check_location {
            location::check_location(&paths.root)?;
        }
        let header: ProjectHeader = serde_json::from_slice(&fs::read(&paths.marker)?)?;
        if header.magic != "omr-project" {
            return Err(StoreError::Invalid(
                "project.omr is not an OMR marker".into(),
            ));
        }
        if header.format_major > FORMAT_MAJOR {
            return Err(StoreError::SchemaTooNew {
                found: i64::from(header.format_major),
                supported: i64::from(FORMAT_MAJOR),
            });
        }
        let lock = if opts.lock {
            Some(take_lock(&paths.lock)?)
        } else {
            None
        };
        // A package created before this rule keeps working; it becomes private on its first
        // writable open.
        restrict_to_owner(&paths.root)?;
        let mut conn = db::open_writer(&paths.db)?;
        let version = schema::user_version(&conn)?;
        if version > schema::LATEST_VERSION {
            return Err(StoreError::SchemaTooNew {
                found: version,
                supported: schema::LATEST_VERSION,
            });
        }
        if version < schema::LATEST_VERSION {
            fs::create_dir_all(&paths.snapshots)?;
            let snap = paths.snapshots.join(format!(
                "pre-migration-v{version}-{}.db",
                Uuid::now_v7().simple()
            ));
            db::snapshot_into(&conn, &snap)?;
            schema::migrate(&mut conn)?;
        }
        schema::verify_schema(&conn)?;
        db::quick_check(&conn)?;
        let stored_uuid: String =
            conn.query_row("SELECT project_uuid FROM project_header", [], |r| r.get(0))?;
        if stored_uuid != header.project_uuid.to_string() {
            return Err(StoreError::Integrity(
                "project.omr and project.db name different projects".into(),
            ));
        }
        if opts.verify_chain {
            events::verify_chain(
                &conn,
                &header.project_uuid,
                opts.verifier
                    .as_deref()
                    .map(|v| v as &dyn CheckpointVerifier),
                &opts.external,
            )?;
            Reader::new(&conn).verify_projections()?;
        }
        let hlc_last: i64 =
            conn.query_row("SELECT coalesce(max(hlc_ts), 0) FROM events", [], |r| {
                r.get(0)
            })?;
        let objects = ObjectStore::new(&paths.objects, &paths.spool)?;
        let ctx = EventCtx {
            device_id: opts.device_id,
            clock: opts.clock,
            hlc_last,
            genesis: genesis_hash(&header.project_uuid),
        };
        let p = Self {
            conn,
            paths,
            objects,
            header,
            ctx,
            signer: opts.signer,
            auto_checkpoint_error: None,
            _lock: lock,
        };
        p.touch_lease()?;
        Ok(p)
    }

    /// Opens a package for reading only: no lock, no lease, no migration, no writes (a
    /// dashboard, a preview, or a viewer while another process holds the writer). The checks
    /// of [`Project::open`] run first: location (when asked), marker, canonical schema,
    /// project identity, and with `verify_chain` the chain against stored and `external`
    /// checkpoints and the decision projections (SYS-011). A project whose schema is older
    /// than this app's must be opened for writing once (it is migrated then).
    pub fn open_read_only(root: &Path, opts: OpenOptions) -> Result<ReadOnlyProject> {
        let paths = PackagePaths::new(root);
        if opts.check_location {
            location::check_location(&paths.root)?;
        }
        let header: ProjectHeader = serde_json::from_slice(&fs::read(&paths.marker)?)?;
        if header.magic != "omr-project" {
            return Err(StoreError::Invalid(
                "project.omr is not an OMR marker".into(),
            ));
        }
        let conn = db::open_reader(&paths.db, 0)?;
        let version = schema::user_version(&conn)?;
        if version > schema::LATEST_VERSION {
            return Err(StoreError::SchemaTooNew {
                found: version,
                supported: schema::LATEST_VERSION,
            });
        }
        // An older schema is read as it is (checked against its own canonical form); the
        // decision replay needs version 3 (the identity decision log).
        if version < 3 {
            return Err(StoreError::Invalid(format!(
                "project schema version {version} predates this app's {}; open the project \
                 for writing once so it is migrated",
                schema::LATEST_VERSION
            )));
        }
        schema::verify_schema(&conn)?;
        let stored_uuid: String =
            conn.query_row("SELECT project_uuid FROM project_header", [], |r| r.get(0))?;
        if stored_uuid != header.project_uuid.to_string() {
            return Err(StoreError::Integrity(
                "project.omr and project.db name different projects".into(),
            ));
        }
        if opts.verify_chain {
            events::verify_chain(
                &conn,
                &header.project_uuid,
                opts.verifier
                    .as_deref()
                    .map(|v| v as &dyn CheckpointVerifier),
                &opts.external,
            )?;
            Reader::new(&conn).verify_projections()?;
        }
        Ok(ReadOnlyProject {
            conn,
            paths,
            header,
        })
    }

    fn touch_lease(&self) -> Result<()> {
        let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "localhost".into());
        self.conn.execute(
            "INSERT INTO project_lease (id, holder, host, heartbeat_at) VALUES (1, ?1, ?2, ?3) \
             ON CONFLICT (id) DO UPDATE SET holder = ?1, host = ?2, heartbeat_at = ?3",
            params![
                self.ctx.device_id,
                host,
                rfc3339_utc(self.ctx.clock.now_unix_ms())
            ],
        )?;
        Ok(())
    }

    /// The header.
    pub fn header(&self) -> &ProjectHeader {
        &self.header
    }

    /// Package paths.
    pub fn paths(&self) -> &PackagePaths {
        &self.paths
    }

    /// The object store.
    pub fn objects(&self) -> &ObjectStore {
        &self.objects
    }

    /// Read access to the writer connection (for ad-hoc queries; writes go through
    /// [`Project::decision`] and [`Project::bulk`]).
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// Typed reads.
    pub fn reader(&self) -> Reader<'_> {
        Reader::new(&self.conn)
    }

    /// A separate `query_only` reader connection (§14.2 readers).
    pub fn open_reader(&self, mmap_size: i64) -> Result<Connection> {
        db::open_reader(&self.paths.db, mmap_size)
    }

    /// Sets the signer used for automatic checkpoints.
    pub fn set_signer(&mut self, signer: Option<Box<dyn CheckpointSigner + Send>>) {
        self.signer = signer;
    }

    fn run<T>(
        &mut self,
        lane: Lane,
        command_id: Option<&str>,
        f: impl FnOnce(&mut Tx<'_>) -> Result<T>,
    ) -> Result<CommandOutcome<T>> {
        db::set_lane(&self.conn, lane)?;
        let project = self.header.project_uuid;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(cid) = command_id {
            let done: Option<i64> = tx
                .query_row(
                    "SELECT committed_seq FROM commands WHERE command_id = ?1",
                    [cid],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(committed_seq) = done {
                return Ok(CommandOutcome::AlreadyApplied { committed_seq });
            }
        }
        let out = {
            let mut t = Tx {
                tx,
                ctx: &mut self.ctx,
                objects: &self.objects,
                project,
            };
            let out = match catch_unwind(AssertUnwindSafe(|| f(&mut t))) {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => return Err(e), // dropping `t` rolls back
                Err(panic) => {
                    let msg = panic
                        .downcast_ref::<&str>()
                        .map(|s| (*s).to_owned())
                        .or_else(|| panic.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "panic".into());
                    return Err(StoreError::Panicked(msg));
                }
            };
            if let Some(cid) = command_id {
                let (seq, _) = t.head()?;
                t.tx.execute(
                    "INSERT INTO commands (command_id, committed_seq) VALUES (?1, ?2)",
                    params![cid, seq],
                )?;
            }
            t.tx.commit()?;
            out
        };
        // The command is committed: a failed automatic checkpoint (e.g. the key store is
        // unavailable) must not report it as unsaved, or the caller would apply it twice. The
        // failure is kept for `auto_checkpoint_error`, retried after the next commit, and
        // `batch_end`/`close` report it.
        self.auto_checkpoint_error = self.maybe_auto_checkpoint().err().map(|e| e.to_string());
        Ok(CommandOutcome::Applied(out))
    }

    /// The last automatic-checkpoint failure, if the most recent commit could not write its
    /// due checkpoint (the commit itself stands; the checkpoint is retried after the next
    /// commit, and [`Project::batch_end`] and [`Project::close`] return the error).
    pub fn auto_checkpoint_error(&self) -> Option<&str> {
        self.auto_checkpoint_error.as_deref()
    }

    /// One decision-lane command (`synchronous = FULL`): acknowledged only after commit.
    pub fn decision<T>(&mut self, f: impl FnOnce(&mut Tx<'_>) -> Result<T>) -> Result<T> {
        match self.run(Lane::Decision, None, f)? {
            CommandOutcome::Applied(v) => Ok(v),
            CommandOutcome::AlreadyApplied { .. } => unreachable!("no command id"),
        }
    }

    /// A decision-lane command with an idempotency key: a retry after a crash is a no-op.
    pub fn command<T>(
        &mut self,
        command_id: &str,
        f: impl FnOnce(&mut Tx<'_>) -> Result<T>,
    ) -> Result<CommandOutcome<T>> {
        self.run(Lane::Decision, Some(command_id), f)
    }

    /// One bulk-lane transaction (`synchronous = NORMAL`), e.g. ≤ [`BULK_GROUP_PAGES`] pages of
    /// recognition output. All or nothing.
    pub fn bulk<T>(&mut self, f: impl FnOnce(&mut Tx<'_>) -> Result<T>) -> Result<T> {
        match self.run(Lane::Bulk, None, f)? {
            CommandOutcome::Applied(v) => Ok(v),
            CommandOutcome::AlreadyApplied { .. } => unreachable!("no command id"),
        }
    }

    fn maybe_auto_checkpoint(&mut self) -> Result<()> {
        if self.signer.is_none() {
            return Ok(());
        }
        let (head, _) = events::head(&self.conn, &self.ctx.genesis)?;
        let last: i64 =
            self.conn
                .query_row("SELECT coalesce(max(seq), 0) FROM checkpoint", [], |r| {
                    r.get(0)
                })?;
        if head - last >= CHECKPOINT_EVERY {
            self.checkpoint_with_own_signer()?;
        }
        Ok(())
    }

    fn checkpoint_with_own_signer(&mut self) -> Result<Option<Checkpoint>> {
        let Some(signer) = self.signer.take() else {
            return Ok(None);
        };
        let r = self.checkpoint(signer.as_ref());
        self.signer = Some(signer);
        r.map(Some)
    }

    /// Signs and records a checkpoint over the current head.
    pub fn checkpoint(&mut self, signer: &dyn CheckpointSigner) -> Result<Checkpoint> {
        // Run without the auto-checkpoint hook (the signer may be the project's own).
        db::set_lane(&self.conn, Lane::Decision)?;
        let project = self.header.project_uuid;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut t = Tx {
            tx,
            ctx: &mut self.ctx,
            objects: &self.objects,
            project,
        };
        let cp = t.write_checkpoint(signer)?;
        t.tx.commit()?;
        Ok(cp)
    }

    /// Full chain verification against stored checkpoints, optionally their signatures, and
    /// any externally held checkpoints (SYS-011 on failure).
    pub fn verify_chain(
        &self,
        verifier: Option<&dyn CheckpointVerifier>,
        external: &[Checkpoint],
    ) -> Result<ChainReport> {
        events::verify_chain(&self.conn, &self.header.project_uuid, verifier, external)
    }

    /// `integrity_check` + `foreign_key_check` + canonical schema (SYS-010 on failure).
    pub fn integrity_check(&self) -> Result<()> {
        db::integrity_check(&self.conn)?;
        schema::verify_schema(&self.conn)
    }

    /// A consistent snapshot of the database at `dest` (`VACUUM INTO`).
    pub fn snapshot_to(&self, dest: &Path) -> Result<()> {
        db::snapshot_into(&self.conn, dest)
    }

    /// Batch end: checkpoint (if a signer is set) and `wal_checkpoint(TRUNCATE)`.
    pub fn batch_end(&mut self) -> Result<Option<Checkpoint>> {
        let cp = self.checkpoint_with_own_signer()?;
        self.auto_checkpoint_error = None;
        db::wal_checkpoint_truncate(&self.conn)?;
        Ok(cp)
    }

    /// Closes the project: checkpoint (if a signer is set) and `wal_checkpoint(TRUNCATE)`.
    pub fn close(mut self) -> Result<Option<Checkpoint>> {
        self.batch_end()
    }

    /// The SHA-384 object manifest of every stored object.
    pub fn object_manifest(&self) -> Result<ObjectManifest> {
        Ok(ObjectManifest {
            algorithms: ["sha384".into(), "blake3-dedup-non-security".into()],
            entries: self.reader().objects()?,
        })
    }

    /// Verifies every object file against its row: SHA-384 and length (integrity) and the
    /// BLAKE3 placement. Returns the number verified.
    pub fn verify_objects(&self) -> Result<usize> {
        let entries = self.reader().objects()?;
        for e in &entries {
            self.objects.verify(&e.blake3, &e.sha384, e.len)?;
        }
        Ok(entries.len())
    }

    /// Sweeps orphan objects (no row) and stale spool files older than `grace`.
    pub fn sweep_orphans(&self, grace: Duration) -> Result<SweepReport> {
        let referenced: BTreeSet<String> = self
            .reader()
            .objects()?
            .into_iter()
            .map(|e| e.blake3)
            .collect();
        self.objects.sweep(&referenced, grace, SystemTime::now())
    }

    /// Appends one standalone event in its own decision transaction.
    pub fn append_event(&mut self, ev: &NewEvent) -> Result<EventRecord> {
        self.decision(|tx| tx.append_event(ev))
    }
}
