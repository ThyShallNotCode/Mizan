//! `omr-app`: Application services (§4.3, §4.5): the exam project model, the ingest→recognise→identify→
//! score→outcomes pipeline with its scheduler (rayon), the review queue, and view-models consumed
//! by the shells and the CLI. Owns parallelism (rule R4).
#![forbid(unsafe_code)]
