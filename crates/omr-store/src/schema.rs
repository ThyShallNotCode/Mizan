//! Project schema (spec §14.3 MVP subset) and forward-only migrations (§14.7).
//!
//! Every table is `STRICT`; hash-keyed tables are also `WITHOUT ROWID` (§14.2). The schema
//! version is `PRAGMA user_version`; each migration runs in its own transaction and sets it.
//! Events are never rewritten by a migration.

use crate::db::{self, APPLICATION_ID};
use crate::error::{Result, StoreError};
use crate::jcs;
use rusqlite::Connection;

/// One forward-only migration.
#[derive(Clone, Copy, Debug)]
pub struct Migration {
    /// `user_version` after this migration.
    pub version: i64,
    /// Short name.
    pub name: &'static str,
    /// DDL, run in one transaction.
    pub sql: &'static str,
}

/// Version 1: the §14.3 MVP entities, the event log and checkpoints.
const V1: &str = r#"
CREATE TABLE project_header (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    project_uuid TEXT NOT NULL,
    workspace_id TEXT,
    offering_id TEXT,
    assessment_id TEXT,
    format_major INTEGER NOT NULL,
    format_minor INTEGER NOT NULL,
    label TEXT NOT NULL,
    created_utc TEXT NOT NULL
) STRICT;

CREATE TABLE commands (
    command_id TEXT PRIMARY KEY,
    committed_seq INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE events (
    seq INTEGER PRIMARY KEY,
    event_id TEXT NOT NULL UNIQUE,
    type TEXT NOT NULL,
    schema_ver INTEGER NOT NULL,
    entity_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    actor_id TEXT NOT NULL,
    actor_role TEXT NOT NULL,
    payload TEXT NOT NULL,
    device_id TEXT NOT NULL,
    device_seq INTEGER NOT NULL,
    hlc_ts INTEGER NOT NULL,
    wall_ts_utc TEXT NOT NULL,
    reason_code TEXT,
    reason_note_ref TEXT,
    engine_manifest_hash TEXT,
    prev_hash TEXT NOT NULL,
    hash TEXT NOT NULL UNIQUE,
    UNIQUE (device_id, device_seq)
) STRICT;

CREATE TRIGGER events_append_only_update BEFORE UPDATE ON events
BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;
CREATE TRIGGER events_append_only_delete BEFORE DELETE ON events
BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;

CREATE TABLE checkpoint (
    seq INTEGER PRIMARY KEY,
    project TEXT NOT NULL,
    head_hash TEXT NOT NULL,
    hlc_ts INTEGER NOT NULL,
    alg TEXT NOT NULL,
    key_id TEXT NOT NULL,
    public_key BLOB NOT NULL,
    signature BLOB NOT NULL
) STRICT;

CREATE TRIGGER checkpoint_append_only_update BEFORE UPDATE ON checkpoint
BEGIN SELECT RAISE(ABORT, 'checkpoints are append-only'); END;
CREATE TRIGGER checkpoint_append_only_delete BEFORE DELETE ON checkpoint
BEGIN SELECT RAISE(ABORT, 'checkpoints are append-only'); END;

CREATE TABLE project_lease (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    holder TEXT NOT NULL,
    host TEXT NOT NULL,
    heartbeat_at TEXT NOT NULL
) STRICT;

CREATE TABLE pinned (
    kind TEXT NOT NULL CHECK (kind IN ('roster_snapshot', 'clo_set', 'course_policy',
                                       'attainment_policy', 'grade_scale')),
    sha384 TEXT NOT NULL,
    body TEXT NOT NULL,
    PRIMARY KEY (kind, sha384)
) STRICT, WITHOUT ROWID;

CREATE TABLE exam (
    exam_id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('exam', 'imported', 'survey')),
    anonymity TEXT NOT NULL CHECK (anonymity IN ('anonymous', 'identified')),
    identity_mode TEXT NOT NULL CHECK (identity_mode IN ('roster', 'candidate_number', 'none')),
    title TEXT NOT NULL,
    course_code TEXT,
    term TEXT,
    weight TEXT,
    cancel_mode TEXT,
    high_stakes INTEGER NOT NULL CHECK (high_stakes IN (0, 1)),
    label TEXT NOT NULL,
    created_seq INTEGER NOT NULL,
    version INTEGER NOT NULL DEFAULT 1
) STRICT;

CREATE TABLE template_family (
    family_id TEXT PRIMARY KEY,
    origin TEXT NOT NULL CHECK (origin IN ('native', 'imported', 'converted')),
    name TEXT NOT NULL
) STRICT;

CREATE TABLE template_revision (
    sha384 TEXT PRIMARY KEY,
    family_id TEXT NOT NULL REFERENCES template_family (family_id),
    revision INTEGER NOT NULL,
    body TEXT NOT NULL,
    frozen_at TEXT NOT NULL,
    UNIQUE (family_id, revision)
) STRICT, WITHOUT ROWID;

CREATE TABLE master_item (
    exam_id TEXT NOT NULL REFERENCES exam (exam_id),
    item_index INTEGER NOT NULL,
    label TEXT NOT NULL,
    body TEXT NOT NULL,
    PRIMARY KEY (exam_id, item_index)
) STRICT, WITHOUT ROWID;

CREATE TABLE form (
    exam_id TEXT NOT NULL REFERENCES exam (exam_id),
    code INTEGER NOT NULL,
    lang TEXT NOT NULL,
    template_sha384 TEXT REFERENCES template_revision (sha384),
    positions TEXT NOT NULL,
    PRIMARY KEY (exam_id, code)
) STRICT, WITHOUT ROWID;

CREATE TABLE form_map (
    exam_id TEXT PRIMARY KEY REFERENCES exam (exam_id),
    sha384 TEXT NOT NULL,
    per_form_normalization INTEGER NOT NULL CHECK (per_form_normalization IN (0, 1))
) STRICT, WITHOUT ROWID;

CREATE TABLE key_version (
    exam_id TEXT NOT NULL REFERENCES exam (exam_id),
    n INTEGER NOT NULL,
    parent INTEGER,
    reason TEXT NOT NULL,
    payload BLOB NOT NULL,
    payload_sha384 TEXT NOT NULL,
    key_id TEXT NOT NULL,
    created_seq INTEGER NOT NULL,
    PRIMARY KEY (exam_id, n)
) STRICT, WITHOUT ROWID;

CREATE TRIGGER key_version_append_only BEFORE UPDATE ON key_version
BEGIN SELECT RAISE(ABORT, 'key versions are append-only'); END;

CREATE TABLE policy_version (
    sha384 TEXT PRIMARY KEY,
    exam_id TEXT NOT NULL REFERENCES exam (exam_id),
    version INTEGER NOT NULL,
    body TEXT NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE student (
    student_id TEXT PRIMARY KEY,
    elevated INTEGER NOT NULL DEFAULT 0 CHECK (elevated IN (0, 1))
) STRICT, WITHOUT ROWID;

CREATE TABLE pii_vault (
    student_id TEXT PRIMARY KEY REFERENCES student (student_id),
    fields BLOB NOT NULL,
    key_id TEXT NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE roster_snapshot (
    sha384 TEXT PRIMARY KEY,
    parent TEXT REFERENCES roster_snapshot (sha384),
    source TEXT NOT NULL,
    fetched_at TEXT NOT NULL,
    id_scheme_version TEXT NOT NULL,
    row_count INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE roster_row (
    snapshot TEXT NOT NULL REFERENCES roster_snapshot (sha384),
    student_id TEXT NOT NULL REFERENCES student (student_id),
    section TEXT NOT NULL,
    sis_status TEXT NOT NULL,
    fields BLOB NOT NULL,
    key_id TEXT NOT NULL,
    attributes TEXT NOT NULL,
    PRIMARY KEY (snapshot, student_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE batch (
    batch_id TEXT PRIMARY KEY,
    exam_id TEXT NOT NULL REFERENCES exam (exam_id),
    header TEXT,
    campus TEXT,
    operator TEXT,
    device_profile TEXT,
    expected_count INTEGER,
    state TEXT NOT NULL CHECK (state IN ('open', 'closed')),
    created_seq INTEGER NOT NULL,
    version INTEGER NOT NULL DEFAULT 1
) STRICT;

CREATE TABLE object (
    blake3 TEXT PRIMARY KEY,
    sha384 TEXT NOT NULL UNIQUE,
    len INTEGER NOT NULL CHECK (len >= 0),
    kind TEXT NOT NULL CHECK (kind IN ('original', 'evidence_crop', 'exemplar')),
    wrap_key_id TEXT,
    pack_id TEXT,
    pack_offset INTEGER
) STRICT, WITHOUT ROWID;

CREATE TABLE source_file (
    source_id TEXT PRIMARY KEY,
    batch_id TEXT NOT NULL REFERENCES batch (batch_id),
    object TEXT NOT NULL REFERENCES object (blake3),
    name TEXT NOT NULL,
    media_type TEXT NOT NULL
) STRICT;

CREATE TABLE booklet (
    booklet_id TEXT PRIMARY KEY,
    exam_id TEXT NOT NULL REFERENCES exam (exam_id),
    serial INTEGER NOT NULL,
    form_code INTEGER,
    bound_student TEXT REFERENCES student (student_id),
    state TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    UNIQUE (exam_id, serial)
) STRICT;

CREATE TABLE page (
    page_id TEXT PRIMARY KEY,
    batch_id TEXT NOT NULL REFERENCES batch (batch_id),
    source_id TEXT REFERENCES source_file (source_id),
    page_index INTEGER NOT NULL,
    object TEXT NOT NULL REFERENCES object (blake3),
    source_sha384 TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('spooled', 'decoded', 'registered', 'read', 'final',
                                         'quarantined', 'superseded')),
    quarantine_reason TEXT,
    booklet_id TEXT REFERENCES booklet (booklet_id),
    page_no INTEGER,
    imprint TEXT,
    dpi_x INTEGER,
    dpi_y INTEGER,
    capture_kind TEXT,
    registration TEXT,
    version INTEGER NOT NULL DEFAULT 1
) STRICT;

CREATE TABLE recognition_run (
    run_id TEXT PRIMARY KEY,
    exam_id TEXT NOT NULL REFERENCES exam (exam_id),
    kind TEXT NOT NULL CHECK (kind IN ('provisional', 'final')),
    parent_run TEXT REFERENCES recognition_run (run_id),
    engine_manifest TEXT NOT NULL,
    engine_manifest_sha384 TEXT NOT NULL,
    created_seq INTEGER NOT NULL
) STRICT;

CREATE TABLE question_result (
    run_id TEXT NOT NULL REFERENCES recognition_run (run_id),
    page_id TEXT NOT NULL REFERENCES page (page_id),
    question INTEGER NOT NULL,
    response TEXT NOT NULL,
    auto_accepted INTEGER NOT NULL CHECK (auto_accepted IN (0, 1)),
    logit_milli INTEGER NOT NULL,
    codes TEXT NOT NULL,
    PRIMARY KEY (run_id, page_id, question)
) STRICT, WITHOUT ROWID;

CREATE TABLE exception (
    exception_id INTEGER PRIMARY KEY,
    code TEXT NOT NULL,
    sub_reason TEXT,
    severity TEXT NOT NULL,
    scope TEXT NOT NULL,
    subject TEXT NOT NULL,
    detail TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('open', 'resolved')),
    run_id TEXT,
    page_id TEXT REFERENCES page (page_id),
    resolved_seq INTEGER
) STRICT;

CREATE TABLE identity_run (
    run_id TEXT PRIMARY KEY,
    exam_id TEXT NOT NULL REFERENCES exam (exam_id),
    roster_snapshot TEXT REFERENCES roster_snapshot (sha384),
    models TEXT NOT NULL,
    thresholds TEXT NOT NULL,
    created_seq INTEGER NOT NULL
) STRICT;

CREATE TABLE identity_decision (
    run_id TEXT NOT NULL REFERENCES identity_run (run_id),
    booklet_id TEXT NOT NULL REFERENCES booklet (booklet_id),
    student_id TEXT,
    status TEXT NOT NULL CHECK (status IN ('auto_accepted', 'needs_confirmation', 'review',
                                           'confirmed', 'not_on_roster', 'duplicate')),
    logit_q INTEGER,
    margin_q INTEGER,
    evidence TEXT NOT NULL,
    commit_nonce TEXT NOT NULL,
    version INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (run_id, booklet_id)
) STRICT, WITHOUT ROWID;

CREATE TABLE response_correction (
    correction_id INTEGER PRIMARY KEY,
    booklet_id TEXT NOT NULL REFERENCES booklet (booklet_id),
    position INTEGER NOT NULL,
    old_response TEXT NOT NULL,
    new_response TEXT NOT NULL,
    reason_code TEXT NOT NULL,
    note_ref TEXT,
    actor_id TEXT NOT NULL,
    commit_nonce TEXT NOT NULL,
    event_seq INTEGER NOT NULL REFERENCES events (seq)
) STRICT;

CREATE TABLE score_run (
    run_id TEXT PRIMARY KEY,
    exam_id TEXT NOT NULL REFERENCES exam (exam_id),
    key_version INTEGER NOT NULL,
    policy_version INTEGER NOT NULL,
    results BLOB NOT NULL,
    results_sha384 TEXT NOT NULL,
    created_seq INTEGER NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE mapping_set (
    sha384 TEXT PRIMARY KEY,
    version INTEGER NOT NULL,
    mode TEXT NOT NULL,
    body TEXT NOT NULL
) STRICT, WITHOUT ROWID;

CREATE TABLE item_clo_map (
    mapping_set TEXT NOT NULL REFERENCES mapping_set (sha384),
    row_index INTEGER NOT NULL,
    assessment TEXT NOT NULL,
    item INTEGER NOT NULL,
    clo TEXT NOT NULL,
    clo_version INTEGER NOT NULL,
    weight TEXT NOT NULL,
    note TEXT NOT NULL,
    evidence_uri TEXT,
    rationale TEXT,
    bloom_item INTEGER,
    status TEXT NOT NULL,
    source TEXT NOT NULL,
    approved_by TEXT,
    version INTEGER NOT NULL,
    supersedes INTEGER,
    PRIMARY KEY (mapping_set, row_index)
) STRICT, WITHOUT ROWID;
"#;

/// Version 2: the export registry (§16.3 `ExportCreated`) and query indexes.
const V2: &str = r#"
CREATE TABLE export (
    export_id TEXT PRIMARY KEY,
    purpose TEXT NOT NULL,
    destination TEXT NOT NULL,
    recipient TEXT,
    pii INTEGER NOT NULL CHECK (pii IN (0, 1)),
    label TEXT NOT NULL,
    manifest_sha384 TEXT NOT NULL,
    event_seq INTEGER NOT NULL REFERENCES events (seq)
) STRICT;

CREATE INDEX page_by_batch ON page (batch_id, page_index);
CREATE INDEX events_by_entity ON events (entity_type, entity_id);
CREATE INDEX exception_by_state ON exception (state, code);
CREATE INDEX question_result_by_page ON question_result (page_id);
"#;

/// All migrations, in order.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "mvp_schema",
        sql: V1,
    },
    Migration {
        version: 2,
        name: "export_registry_and_indexes",
        sql: V2,
    },
];

/// The newest schema version this build knows.
pub const LATEST_VERSION: i64 = 2;

/// What a migration call did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MigrationReport {
    /// `user_version` before.
    pub from: i64,
    /// `user_version` after.
    pub to: i64,
}

/// `PRAGMA user_version`.
pub fn user_version(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA user_version", [], |r| r.get(0))?)
}

/// `PRAGMA application_id`.
pub fn application_id(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row("PRAGMA application_id", [], |r| r.get(0))?)
}

/// Migrates to [`LATEST_VERSION`].
pub fn migrate(conn: &mut Connection) -> Result<MigrationReport> {
    migrate_to(conn, LATEST_VERSION)
}

/// Migrates forward to `target` (tests and tooling use intermediate targets). Refuses a
/// foreign database (`application_id`) and a newer schema (SYS-016). Each migration is one
/// transaction that also sets `user_version`; `foreign_key_check` and `quick_check` follow.
pub fn migrate_to(conn: &mut Connection, target: i64) -> Result<MigrationReport> {
    let from = user_version(conn)?;
    let app = application_id(conn)?;
    if app != APPLICATION_ID {
        let empty: i64 =
            conn.query_row("SELECT count(*) FROM sqlite_schema", [], |r| r.get(0))?;
        if app != 0 || from != 0 || empty != 0 {
            return Err(StoreError::NotAProject(app));
        }
        conn.pragma_update(None, "application_id", APPLICATION_ID)?;
    }
    if from > LATEST_VERSION {
        return Err(StoreError::SchemaTooNew {
            found: from,
            supported: LATEST_VERSION,
        });
    }
    let target = target.min(LATEST_VERSION);
    for m in MIGRATIONS
        .iter()
        .filter(|m| m.version > from && m.version <= target)
    {
        let tx = conn.transaction()?;
        tx.execute_batch(m.sql)?;
        tx.pragma_update(None, "user_version", m.version)?;
        tx.commit()?;
    }
    let to = user_version(conn)?;
    if to != from {
        db::foreign_key_check(conn)?;
        db::quick_check(conn)?;
    }
    Ok(MigrationReport { from, to })
}

/// SHA-384 over `sqlite_schema` (type, name, tbl_name, sql), the §14.1 import check: a received
/// database must hash-match the canonical schema for its `user_version`.
pub fn schema_sha384(conn: &Connection) -> Result<String> {
    let mut stmt = conn.prepare(
        "SELECT type, name, tbl_name, coalesce(sql, '') FROM sqlite_schema \
         WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
    )?;
    let rows: Vec<[String; 4]> = stmt
        .query_map([], |r| Ok([r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?]))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(jcs::jcs_sha384_hex(&rows)?)
}

/// The canonical schema hash of `version`, built in memory from this build's migrations.
pub fn canonical_schema_sha384(version: i64) -> Result<String> {
    let mut conn = Connection::open_in_memory()?;
    migrate_to(&mut conn, version)?;
    schema_sha384(&conn)
}

/// Refuses a database whose schema differs from the canonical one for its `user_version`
/// (any extra or altered table, index, trigger or view; SYS-010).
pub fn verify_schema(conn: &Connection) -> Result<()> {
    let v = user_version(conn)?;
    let want = canonical_schema_sha384(v)?;
    let got = schema_sha384(conn)?;
    if want != got {
        return Err(StoreError::Integrity(format!(
            "schema of user_version {v} does not match the canonical schema"
        )));
    }
    Ok(())
}
