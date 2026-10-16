//! Renders sample sheets to a directory: `cargo run -p omr-forms --features render --example
//! render_sample -- <out-dir>`. Writes SVG, PNG (200 dpi) and a two-form PDF.

use omr_forms::render::{encode_png, rasterize_sheet, sheet_svg, sheets_pdf};
use omr_forms::{ExamKey, LabelSet, Personalisation, SheetInstance, Template, TemplateSpec};
use omr_model::{BookletSerial, CancelMode, ExamId, FormCode, PageNo};
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "sample-sheets".into()));
    std::fs::create_dir_all(&out)?;
    let key = ExamKey::from_bytes([0x5A; 32]);
    let exam = ExamId(uuid::Uuid::from_u128(0x0190_1234_5678_7abc_8def_0123_4567_89ab));

    let mut latin = TemplateSpec::standard(exam, 100, 5, CancelMode::EraseOnly);
    latin.title = "PHYS 101 Midterm Examination".into();
    latin.course = "PHYS 101 · General Physics I".into();
    latin.id_digits = 10;
    latin.forms = 4;
    latin.form_grid = true;
    let mut arabic = TemplateSpec::standard(exam, 60, 4, CancelMode::CrossCancels);
    arabic.title = "اختبار منتصف الفصل".into();
    arabic.course = "ريض ١٠١ · التفاضل والتكامل".into();
    arabic.labels = LabelSet::Arabic;
    arabic.revision = omr_model::TemplateRevision(2);

    for (name, spec) in [("latin", latin), ("arabic", arabic)] {
        let t = Template::build(spec)?;
        println!("{name}: {} page(s), hash {}", t.page_count(), t.hash_hex());
        let person = Personalisation::new("Sara Al-Qahtani · سارة القحطاني", "441234567")?;
        let insts: Vec<SheetInstance<'_>> = (1..=t.page_count())
            .map(|p| {
                SheetInstance::new(&t, &key, FormCode(1), PageNo(p))
                    .with_serial(BookletSerial(1_234))
                    .with_personal(person.clone())
            })
            .collect();
        for i in &insts {
            std::fs::write(out.join(format!("{name}_p{}.svg", i.page.0)), sheet_svg(i)?)?;
            let img = rasterize_sheet(i, 200.0)?;
            std::fs::write(out.join(format!("{name}_p{}.png", i.page.0)), encode_png(&img)?)?;
        }
        std::fs::write(out.join(format!("{name}.pdf")), sheets_pdf(&insts, &t.spec().title)?)?;
        std::fs::write(out.join(format!("{name}_manifest.json")), t.manifest_json())?;
    }
    Ok(())
}
