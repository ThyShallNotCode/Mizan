//! `omr-recog`: Recognition engine (§8): orientation and page/template identification (§8.3), registration
//! (§8.4), normalised darkness (§8.5), per-bubble features (§8.6), classification cascade (§8.7)
//! and per-question decision logic (§8.8) with exceptions (Appendix A).
//!
//! Rule R2 (key-blindness): recognition never sees an answer key.
#![forbid(unsafe_code)]
