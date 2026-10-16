//! Property tests: page-code round trips and forgery rejection (§6.4, A7), and layout invariants
//! over random specifications (§6.3: nothing outside the safe box, no overlaps, pitch rules).

use omr_forms::geometry::{
    BUBBLE_DIAMETER, PITCH_X, PITCH_Y, QUIET_ZONE, SAFE_BOX_A4, TICK_H, TICK_W,
};
use omr_forms::pagecode::{self, ALPHABET, PAYLOAD_LEN, PageCode, PageCodeError, decode_verified};
use omr_forms::{
    BlockKind, CodeDiagonal, ExamKey, LabelSet, OptionCounts, Side, Template, TemplateSpec,
};
use omr_model::{BookletSerial, CancelMode, ExamId, FormCode, PageNo, TemplateRevision};
use proptest::prelude::*;

fn arb_code() -> impl Strategy<Value = PageCode> {
    (
        0u64..(1 << 40),
        0u16..=1023,
        0u16..=1023,
        0u16..=1023,
        0u32..(1 << 30),
    )
        .prop_map(|(e, r, f, p, s)| PageCode {
            exam_id40: e,
            revision: TemplateRevision(r),
            form: FormCode(f),
            page: PageNo(p),
            serial: BookletSerial(s),
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn req_a7_encode_decode_round_trip(code in arb_code(), k in any::<[u8; 32]>()) {
        let key = ExamKey::from_bytes(k);
        let s = code.encode(&key).unwrap();
        prop_assert_eq!(s.len(), PAYLOAD_LEN);
        prop_assert!(s.bytes().all(|b| ALPHABET.contains(&b)));
        prop_assert_eq!(decode_verified(&s, &key).unwrap(), code);
        let parsed = pagecode::parse(&s).unwrap();
        prop_assert_eq!(parsed.short_form(), &s[21..28]);
    }

    #[test]
    fn req_a7_any_single_character_change_is_rejected(
        code in arb_code(),
        k in any::<[u8; 32]>(),
        pos in 0usize..PAYLOAD_LEN,
        delta in 1usize..32,
    ) {
        let key = ExamKey::from_bytes(k);
        let s = code.encode(&key).unwrap();
        let mut b = s.into_bytes();
        let i = ALPHABET.iter().position(|a| *a == b[pos]).unwrap();
        b[pos] = ALPHABET[(i + delta) % 32];
        let t = String::from_utf8(b).unwrap();
        prop_assert!(decode_verified(&t, &key).is_err());
    }

    #[test]
    fn req_a7_other_keys_never_verify(code in arb_code(), a in any::<[u8; 32]>(), b in any::<[u8; 32]>()) {
        prop_assume!(a != b);
        let s = code.encode(&ExamKey::from_bytes(a)).unwrap();
        prop_assert_eq!(decode_verified(&s, &ExamKey::from_bytes(b)), Err(PageCodeError::TagMismatch));
    }

    #[test]
    fn parse_never_panics(s in "[0-9A-Za-z]{0,50}") {
        let _ = pagecode::parse(&s);
        let _ = pagecode::check_echo_code(&s, &ExamKey::from_bytes([0; 32]));
    }
}

fn arb_spec() -> impl Strategy<Value = TemplateSpec> {
    (
        1u16..=260,
        2u8..=10,
        prop_oneof![Just(0u8), 4u8..=12],
        1u16..=6,
        any::<bool>(),
        prop_oneof![
            Just(LabelSet::Latin),
            Just(LabelSet::Arabic),
            Just(LabelSet::Numeric)
        ],
        prop_oneof![Just(CodeDiagonal::TrBl), Just(CodeDiagonal::TlBr)],
        any::<bool>(),
        any::<u64>(),
    )
        .prop_map(|(q, o, id, forms, grid, labels, diag, per_item, seed)| {
            let mut s = TemplateSpec::standard(
                ExamId(uuid::Uuid::from_u128(u128::from(seed))),
                q,
                o,
                CancelMode::EraseOnly,
            );
            s.id_digits = id;
            s.forms = forms;
            s.form_grid = grid;
            s.labels = labels;
            s.code_diagonal = diag;
            if per_item {
                let mut x = seed | 1;
                s.options = OptionCounts::PerItem(
                    (0..q)
                        .map(|_| {
                            x ^= x << 13;
                            x ^= x >> 7;
                            x ^= x << 17;
                            2 + (x % u64::from(o - 1)) as u8
                        })
                        .collect(),
                );
            }
            s
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn layout_invariants(spec in arb_spec()) {
        let q = spec.questions;
        let t = Template::build(spec.clone()).unwrap();
        prop_assert!(t.lint_at_dpi(200).is_empty(), "{:?}", t.lint_at_dpi(200));
        // Every question exactly once, in order, with the right option count.
        let mut seen = Vec::new();
        for p in t.pages() {
            seen.extend(p.questions());
        }
        prop_assert_eq!(seen, (0..q).collect::<Vec<_>>());
        for qi in 0..q {
            prop_assert_eq!(t.question_bubbles(qi).len(), usize::from(spec.options.of(qi)));
        }
        for p in t.pages() {
            let r = BUBBLE_DIAMETER / 2;
            // Safe box and pitch rules.
            for b in &p.bubbles {
                prop_assert!(SAFE_BOX_A4.contains_rect(&b.rect()));
            }
            for w in p.bubbles.windows(2) {
                if w[0].question == w[1].question {
                    prop_assert_eq!((w[1].centre.x - w[0].centre.x).abs(), PITCH_X);
                    prop_assert_eq!(w[1].centre.y, w[0].centre.y);
                }
            }
            // Rows in a column step by the row pitch, plus the band gap every 5 rows.
            let qs: Vec<_> = t.manifest().questions.iter().filter(|g| g.page == p.page).collect();
            for w in qs.windows(2) {
                if w[0].column == w[1].column {
                    let dy = w[1].row_y - w[0].row_y;
                    let expect = if w[1].row % 5 == 0 { PITCH_Y + 2_000 } else { PITCH_Y };
                    prop_assert_eq!(dy, expect);
                }
            }
            // Ticks: both ends of every row, 1.2 × 0.6 mm, centred on the row.
            prop_assert_eq!(p.ticks.len(), 2 * qs.len());
            for (tk, g) in p.ticks.chunks(2).zip(&qs) {
                prop_assert_eq!(tk[0].side, Side::Left);
                prop_assert_eq!(tk[1].side, Side::Right);
                for x in tk {
                    prop_assert_eq!((x.rect.w, x.rect.h), (TICK_W, TICK_H));
                    prop_assert_eq!(x.rect.centre().y, g.row_y);
                }
            }
            // Blocks keep 3 mm quiet zones from each other and from tags and codes.
            for (i, a) in p.blocks.iter().enumerate() {
                prop_assert!(SAFE_BOX_A4.contains_rect(&a.rect));
                for b in p.blocks.iter().skip(i + 1) {
                    prop_assert!(!a.rect.inflate(QUIET_ZONE).intersects(&b.rect), "{:?} vs {:?}", a.kind, b.kind);
                }
                for tg in &p.tags {
                    prop_assert!(!a.rect.inflate(QUIET_ZONE).intersects(&tg.outer));
                }
                for c in &p.codes {
                    prop_assert!(!a.rect.inflate(QUIET_ZONE).intersects(&c.rect));
                    prop_assert!(!a.rect.inflate(QUIET_ZONE).intersects(&c.echo));
                }
            }
            // No two bubbles of any kind overlap.
            let mut all: Vec<_> = p.bubbles.iter().map(|b| b.centre).collect();
            all.extend(p.id_bubbles.iter().map(|b| b.centre));
            all.extend(p.form_bubbles.iter().map(|b| b.centre));
            for (i, a) in all.iter().enumerate() {
                for b in all.iter().skip(i + 1) {
                    prop_assert!(a.dist2(*b) >= i64::from(2 * r) * i64::from(2 * r));
                }
            }
            prop_assert_eq!(p.tags.len(), 4);
            prop_assert_eq!(p.codes.len(), 2);
            if p.page == 1 && spec.id_digits > 0 {
                prop_assert_eq!(p.id_bubbles.len(), 10 * usize::from(spec.id_digits));
                prop_assert!(p.blocks.iter().any(|b| b.kind == BlockKind::IdGrid));
            } else {
                prop_assert!(p.id_bubbles.is_empty());
            }
        }
    }

    #[test]
    fn layout_is_deterministic(spec in arb_spec()) {
        let a = Template::build(spec.clone()).unwrap();
        let b = Template::build(spec).unwrap();
        prop_assert_eq!(a.hash(), b.hash());
        prop_assert_eq!(a.manifest_json(), b.manifest_json());
    }
}

#[test]
fn manifest_round_trips_through_json() {
    let t = Template::build(TemplateSpec::standard(
        ExamId(uuid::Uuid::from_u128(9)),
        50,
        4,
        CancelMode::FillCancelsCheck,
    ))
    .unwrap();
    let json = serde_json::to_string(t.manifest()).unwrap();
    let back: omr_forms::LayoutManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(&back, t.manifest());
    assert_eq!(back.sha384().unwrap(), *t.hash());
}
