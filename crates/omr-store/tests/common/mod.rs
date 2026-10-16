//! Shared fixtures for the omr-store integration tests (synthetic data only).
#![allow(dead_code)]

use omr_model::{CancelMode, ExamId};
use omr_store::{
    Actor, Anonymity, AssessmentKind, EcdsaP384Signer, ExamRecord, IdentityMode, NewProject,
    OpenOptions, Project, StepClock,
};
use std::path::Path;
use uuid::Uuid;

pub const PROJECT: u128 = 0x0192_0000_0000_7000_8000_0000_0000_0001;
pub const EXAM: u128 = 0x0192_0000_0000_7000_8000_0000_0000_00e1;

pub fn opts() -> OpenOptions {
    OpenOptions {
        device_id: "device:test".into(),
        clock: Box::new(StepClock::new(1_792_054_800_000, 1000)),
        check_location: false,
        ..OpenOptions::default()
    }
}

pub fn exam_id() -> ExamId {
    ExamId(Uuid::from_u128(EXAM))
}

pub fn instructor() -> Actor {
    Actor::user("principal:instructor-1", "Instructor")
}

pub fn exam_record() -> ExamRecord {
    ExamRecord {
        exam_id: exam_id(),
        kind: AssessmentKind::Exam,
        anonymity: Anonymity::Identified,
        identity_mode: IdentityMode::Roster,
        title: "CS 101 Midterm".into(),
        course_code: Some("CS 101".into()),
        term: Some("1448-1".into()),
        weight: Some("0.3".into()),
        cancel_mode: Some(CancelMode::EraseOnly),
        high_stakes: false,
        label: "restricted".into(),
    }
}

pub fn new_project(dir: &Path) -> Project {
    Project::create(
        dir,
        NewProject {
            project_uuid: Some(Uuid::from_u128(PROJECT)),
            assessment_id: Some("cs101-mt".into()),
            ..NewProject::default()
        },
        opts(),
    )
    .unwrap()
}

/// A project with its exam created.
pub fn project_with_exam(dir: &Path) -> Project {
    let mut p = new_project(dir);
    p.decision(|tx| tx.create_exam(&exam_record(), &instructor()))
        .unwrap();
    p
}

/// A fixed test signer (NOT a production key: keys come from the key store, §19.2).
pub fn test_signer() -> EcdsaP384Signer {
    EcdsaP384Signer::from_secret_bytes(&[0x42; 48]).unwrap()
}

/// An attacker's key.
pub fn other_signer() -> EcdsaP384Signer {
    EcdsaP384Signer::from_secret_bytes(&[0x24; 48]).unwrap()
}
