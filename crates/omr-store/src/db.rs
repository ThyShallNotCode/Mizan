//! SQLite configuration (spec §14.2): version floor, pragmas, durability lanes and checks.

use crate::error::{Result, StoreError};
use rusqlite::config::DbConfig;
use rusqlite::{Connection, OpenFlags};
use std::path::Path;
use std::time::Duration;

/// `application_id` of project databases: "OMR1" (proposed, §14.2).
pub const APPLICATION_ID: i64 = 0x4F4D_5231;

/// SQLite version floor 3.51.3 (`sqlite_version()` as `3_051_003`); 3.7.0–3.51.2 carry the
/// WAL-reset corruption bug triggered by the writer-plus-readers design (§14.2).
pub const MIN_SQLITE_VERSION: i32 = 3_051_003;

/// `journal_size_limit`: 64 MiB.
pub const JOURNAL_SIZE_LIMIT: i64 = 64 * 1024 * 1024;

/// `busy_timeout`: 5,000 ms.
pub const BUSY_TIMEOUT: Duration = Duration::from_millis(5000);

/// Writer page cache: 64 MiB (negative `cache_size` = KiB).
pub const WRITER_CACHE_KIB: i64 = 64 * 1024;

/// Reader page cache: 16 MiB.
pub const READER_CACHE_KIB: i64 = 16 * 1024;

/// Upper bound of a reader's `mmap_size`: 256 MiB (§21.5).
pub const MAX_READER_MMAP: i64 = 256 * 1024 * 1024;

/// Durability lane of a write transaction (§14.2 "Writer actor").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    /// Human decisions, state changes, events: `synchronous = FULL`, acknowledged after commit.
    Decision,
    /// Recognition output in groups of ≤ 50 pages: `synchronous = NORMAL`.
    Bulk,
}

/// Fails unless the linked SQLite is at least 3.51.3.
pub fn check_sqlite_version() -> Result<()> {
    let found = rusqlite::version_number();
    if found < MIN_SQLITE_VERSION {
        return Err(StoreError::SqliteTooOld {
            found: rusqlite::version().to_owned(),
            required: "3.51.3".into(),
        });
    }
    Ok(())
}

fn common(conn: &Connection) -> Result<()> {
    conn.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.pragma_update(None, "trusted_schema", "OFF")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(())
}

/// Opens the project's single read-write connection with the §14.2 writer settings.
pub fn open_writer(path: &Path) -> Result<Connection> {
    check_sqlite_version()?;
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI,
    )?;
    configure_writer(&conn)?;
    Ok(conn)
}

/// Applies the writer pragmas: WAL, FULL, foreign keys, busy timeout, journal size limit,
/// secure delete, 64 MiB cache, `fullfsync` (effective on macOS only), `trusted_schema = OFF`
/// and `SQLITE_DBCONFIG_DEFENSIVE`.
pub fn configure_writer(conn: &Connection) -> Result<()> {
    common(conn)?;
    let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") && !mode.eq_ignore_ascii_case("memory") {
        return Err(StoreError::Invalid(format!(
            "journal_mode WAL refused (got {mode})"
        )));
    }
    conn.pragma_update(None, "synchronous", "FULL")?;
    conn.pragma_update(None, "journal_size_limit", JOURNAL_SIZE_LIMIT)?;
    conn.pragma_update(None, "secure_delete", "ON")?;
    conn.pragma_update(None, "cache_size", -WRITER_CACHE_KIB)?;
    conn.pragma_update(None, "fullfsync", "ON")?;
    conn.pragma_update(None, "checkpoint_fullfsync", "ON")?;
    Ok(())
}

/// Opens a `query_only` reader connection (16 MiB cache, `mmap_size` clamped to 256 MiB;
/// pass 0 on 4 GB machines and removable media, §21.5).
pub fn open_reader(path: &Path, mmap_size: i64) -> Result<Connection> {
    check_sqlite_version()?;
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_URI,
    )?;
    common(&conn)?;
    conn.pragma_update(None, "query_only", "ON")?;
    conn.pragma_update(None, "cache_size", -READER_CACHE_KIB)?;
    conn.pragma_update(None, "mmap_size", mmap_size.clamp(0, MAX_READER_MMAP))?;
    Ok(conn)
}

/// Sets the durability of the next transaction (must be called outside a transaction).
pub fn set_lane(conn: &Connection, lane: Lane) -> Result<()> {
    let v = match lane {
        Lane::Decision => "FULL",
        Lane::Bulk => "NORMAL",
    };
    conn.pragma_update(None, "synchronous", v)?;
    Ok(())
}

fn check_rows(conn: &Connection, sql: &str, what: &str) -> Result<()> {
    let mut stmt = conn.prepare(sql)?;
    let rows: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    if rows.len() == 1 && rows[0] == "ok" {
        Ok(())
    } else {
        Err(StoreError::Integrity(format!("{what}: {}", rows.join("; "))))
    }
}

/// `PRAGMA quick_check` (SYS-010 on failure).
pub fn quick_check(conn: &Connection) -> Result<()> {
    check_rows(conn, "PRAGMA quick_check", "quick_check")
}

/// `PRAGMA integrity_check` plus `foreign_key_check` (SYS-010 on failure).
pub fn integrity_check(conn: &Connection) -> Result<()> {
    check_rows(conn, "PRAGMA integrity_check", "integrity_check")?;
    foreign_key_check(conn)
}

/// `PRAGMA foreign_key_check` (SYS-010 on any violation).
pub fn foreign_key_check(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
    let bad: Vec<String> = stmt
        .query_map([], |r| {
            Ok(format!(
                "{} row {:?} → {}",
                r.get::<_, String>(0)?,
                r.get::<_, Option<i64>>(1)?,
                r.get::<_, String>(2)?
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    if bad.is_empty() {
        Ok(())
    } else {
        Err(StoreError::Integrity(format!(
            "foreign_key_check: {}",
            bad.join("; ")
        )))
    }
}

/// `wal_checkpoint(TRUNCATE)` at batch end and on close (§14.2).
pub fn wal_checkpoint_truncate(conn: &Connection) -> Result<()> {
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
    Ok(())
}

/// A consistent copy of the live database via `VACUUM INTO` (snapshots, pre-migration
/// copies, archives: never the live file plus WAL, §14.1, §14.7). `dest` must not exist.
pub fn snapshot_into(conn: &Connection, dest: &Path) -> Result<()> {
    let dest = dest
        .to_str()
        .ok_or_else(|| StoreError::Invalid("snapshot path is not UTF-8".into()))?;
    conn.execute("VACUUM INTO ?1", [dest])?;
    Ok(())
}
