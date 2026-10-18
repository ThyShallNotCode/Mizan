//! Schema creation from empty, forward migration with data, configuration pragmas (§14.2,
//! §14.7).

mod common;

use common::*;
use omr_store::db::{self, APPLICATION_ID};
use omr_store::schema::{self, LATEST_VERSION};
use omr_store::{Project, StoreError};
use rusqlite::Connection;

#[test]
fn migration_from_empty_builds_the_canonical_schema() {
    let mut conn = Connection::open_in_memory().unwrap();
    let r = schema::migrate(&mut conn).unwrap();
    assert_eq!((r.from, r.to), (0, LATEST_VERSION));
    assert_eq!(schema::application_id(&conn).unwrap(), APPLICATION_ID);
    assert_eq!(
        schema::schema_sha384(&conn).unwrap(),
        schema::canonical_schema_sha384(LATEST_VERSION).unwrap()
    );
    // Every table is STRICT (§14.2).
    let mut stmt = conn
        .prepare("SELECT name, strict FROM pragma_table_list WHERE schema = 'main' AND type = 'table' AND name NOT LIKE 'sqlite_%'")
        .unwrap();
    let tables: Vec<(String, bool)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(tables.len() >= 25, "{tables:?}");
    for (t, strict) in &tables {
        assert!(strict, "table {t} is not STRICT");
    }
    drop(stmt);
    // Re-running is a no-op.
    let r = schema::migrate(&mut conn).unwrap();
    assert_eq!((r.from, r.to), (LATEST_VERSION, LATEST_VERSION));
}

#[test]
fn writer_pragmas_follow_section_14_2() {
    let d = tempfile::tempdir().unwrap();
    let p = new_project(&d.path().join("Exam.omrproj"));
    let c = p.conn();
    let q = |sql: &str| -> String {
        c.query_row(sql, [], |r| r.get::<_, rusqlite::types::Value>(0))
            .map(|v| match v {
                rusqlite::types::Value::Integer(i) => i.to_string(),
                rusqlite::types::Value::Text(t) => t,
                other => format!("{other:?}"),
            })
            .unwrap()
    };
    assert_eq!(q("PRAGMA journal_mode"), "wal");
    assert_eq!(q("PRAGMA foreign_keys"), "1");
    assert_eq!(q("PRAGMA secure_delete"), "1");
    assert_eq!(q("PRAGMA trusted_schema"), "0");
    assert_eq!(q("PRAGMA busy_timeout"), "5000");
    assert_eq!(
        q("PRAGMA journal_size_limit"),
        (64 * 1024 * 1024).to_string()
    );
    assert_eq!(q("PRAGMA cache_size"), "-65536");
    assert_eq!(q("PRAGMA synchronous"), "2"); // FULL after the last decision transaction
    assert_eq!(q("PRAGMA application_id"), APPLICATION_ID.to_string());
    assert_eq!(q("PRAGMA user_version"), LATEST_VERSION.to_string());
    assert!(rusqlite::version_number() >= db::MIN_SQLITE_VERSION);
    // Readers are query_only.
    let r = p.open_reader(0).unwrap();
    assert!(r.execute("DELETE FROM project_lease", []).is_err());
    assert_eq!(
        r.query_row("SELECT count(*) FROM events", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn v1_project_migrates_on_open_with_snapshot_and_keeps_its_chain() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("Exam.omrproj");
    {
        let p = project_with_exam(&root);
        p.close().unwrap();
    }
    // Turn the database back into a version-1 database (drop the v4, v3 and v2 objects).
    {
        let c = Connection::open(root.join("project.db")).unwrap();
        c.execute_batch(
            "DROP TABLE review_note; DROP TABLE identity_decision_log; \
             DROP INDEX page_by_batch; DROP INDEX events_by_entity; \
             DROP INDEX exception_by_state; DROP INDEX question_result_by_page; \
             DROP TABLE export; PRAGMA user_version = 1;",
        )
        .unwrap();
        assert_eq!(
            schema::schema_sha384(&c).unwrap(),
            schema::canonical_schema_sha384(1).unwrap()
        );
    }
    let p = Project::open(&root, opts()).unwrap();
    assert_eq!(schema::user_version(p.conn()).unwrap(), LATEST_VERSION);
    let snaps: Vec<_> = std::fs::read_dir(root.join("snapshots"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(snaps.len(), 1);
    assert!(snaps[0].starts_with("pre-migration-v1-"));
    // Data and chain survive; the snapshot is a valid version-1 database.
    assert_eq!(p.reader().exam(&exam_id()).unwrap().title, "CS 101 Midterm");
    let rep = p.verify_chain(None, &[]).unwrap();
    assert_eq!(rep.events, 2);
    let s = Connection::open(root.join("snapshots").join(&snaps[0])).unwrap();
    assert_eq!(schema::user_version(&s).unwrap(), 1);
}

#[test]
fn newer_schema_is_refused_with_sys_016() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("Exam.omrproj");
    new_project(&root).close().unwrap();
    Connection::open(root.join("project.db"))
        .unwrap()
        .execute_batch("PRAGMA user_version = 99;")
        .unwrap();
    let e = Project::open(&root, opts()).unwrap_err();
    assert!(
        matches!(e, StoreError::SchemaTooNew { found: 99, .. }),
        "{e}"
    );
    assert_eq!(e.exception().unwrap().code, "SYS-016");
}

#[test]
fn foreign_database_is_refused() {
    let mut c = Connection::open_in_memory().unwrap();
    c.execute_batch("CREATE TABLE t (x INTEGER);").unwrap();
    assert!(matches!(
        schema::migrate(&mut c),
        Err(StoreError::NotAProject(0))
    ));
}

#[test]
fn altered_schema_is_refused_on_open() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("Exam.omrproj");
    new_project(&root).close().unwrap();
    // An extra trigger (e.g. one that would rewrite rows) changes the schema hash.
    Connection::open(root.join("project.db"))
        .unwrap()
        .execute_batch("CREATE TRIGGER sneaky AFTER INSERT ON exam BEGIN SELECT 1; END;")
        .unwrap();
    let e = Project::open(&root, opts()).unwrap_err();
    assert!(matches!(e, StoreError::Integrity(_)), "{e}");
    assert_eq!(e.exception().unwrap().code, "SYS-010");
}

#[test]
fn second_writer_is_locked_out() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("Exam.omrproj");
    let _p = new_project(&root);
    let e = Project::open(&root, opts()).unwrap_err();
    assert!(matches!(e, StoreError::Locked(_)), "{e}");
}

#[test]
fn creating_over_an_existing_project_is_refused() {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().join("Exam.omrproj");
    new_project(&root).close().unwrap();
    assert!(matches!(
        Project::create(&root, Default::default(), opts()),
        Err(StoreError::Conflict(_))
    ));
}
