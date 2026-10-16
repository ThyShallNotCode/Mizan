use kornia_apriltag::{AprilTagDecoder, DecodeTagsConfig, family::TagFamilyKind};
use kornia_image::{Image, ImageSize};
use omr_forms::{ExamKey, Template, TemplateSpec};
use omr_model::{CancelMode, ExamId};
use omr_sim::{CorpusConfig, Profile, Simulator};
use std::time::Instant;

fn main() {
    let t = Template::build(TemplateSpec::standard(
        ExamId(uuid::Uuid::from_u128(0x0190_aaaa_bbbb_7ccc_8ddd_eeee_ffff_0001)),
        100,
        5,
        CancelMode::EraseOnly,
    ))
    .unwrap();
    let key = ExamKey::from_bytes([7; 32]);
    let sim = Simulator::new(&t, &key, 200);
    let mut cfg = CorpusConfig::standard(10, 1);
    cfg.profile = Profile::Hard;
    for i in 0..4 {
        let (script, mut rng) = omr_sim::corpus::sheet_script(&t, &cfg, &[], i).unwrap();
        let pages = sim.simulate(&script, cfg.profile, &mut rng).unwrap();
        let p = &pages[0];
        let jpg = omr_sim::capture::encode_jpeg(&p.image, p.truth.capture.jpeg_quality, 200).unwrap();
        let dec = omr_imaging::decode(&jpg).unwrap();
        let img = dec.image;
        let size = ImageSize { width: img.width(), height: img.height() };
        let k = Image::<u8, 1>::new(size, img.as_raw().to_vec()).unwrap();
        for ds in [1usize, 2, 3] {
            let mut cfgk = DecodeTagsConfig::new(vec![TagFamilyKind::Tag36H11]).unwrap();
            cfgk.downscale_factor = ds;
            let mut d = AprilTagDecoder::new(cfgk, size).unwrap();
            let t0 = Instant::now();
            let det = d.decode(&k).unwrap();
            let el = t0.elapsed();
            println!("sheet {i} ds {ds}: {} dets in {:?}: {:?}", det.len(), el, det.iter().map(|d| (d.id, d.hamming, d.decision_margin as i32)).collect::<Vec<_>>());
        }
        // truth corners
        let h = p.truth.mm_to_px;
        let g = t.page(omr_model::PageNo(1)).unwrap();
        for tag in &g.tags {
            let c = tag.corners_mm();
            let pts: Vec<(f64,f64)> = c.iter().map(|c| omr_sim::profile::apply_h(&h, c.x, c.y)).collect();
            println!("  tag {} truth {:?}", tag.id, pts.iter().map(|(x,y)| (x.round(), y.round())).collect::<Vec<_>>());
        }
    }
}
