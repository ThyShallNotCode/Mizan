//! Multi-page PDF output (§6.8): vector bubbles, embedded subset fonts, no page-group
//! transparency, and `/ViewerPreferences << /PrintScaling /None >>` (§6.3).

use super::RenderError;
use super::fonts::{SANS_FAMILY, pdf_fontdb};
use super::svg::sheet_svg;
use crate::geometry::{A4_H, A4_W};
use crate::instance::SheetInstance;
use crate::units::UM_PER_INCH;
use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref, TextStr};
use std::collections::BTreeMap;

/// Points per micrometre.
fn um_to_pt(v: i32) -> f32 {
    (f64::from(v) * 72.0 / UM_PER_INCH) as f32
}

/// Writes one PDF page per instance, in order. `title` goes into the document information.
pub fn sheets_pdf(instances: &[SheetInstance<'_>], title: &str) -> Result<Vec<u8>, RenderError> {
    if instances.is_empty() {
        return Err(RenderError::Pdf("no pages".into()));
    }
    let opt = svg2pdf::usvg::Options {
        fontdb: pdf_fontdb(),
        font_family: SANS_FAMILY.to_owned(),
        ..svg2pdf::usvg::Options::default()
    };
    let conv = svg2pdf::ConversionOptions {
        compress: true,
        raster_scale: 1.5,
        embed_text: true,
        pdfa: false,
    };
    let (w_pt, h_pt) = (um_to_pt(A4_W), um_to_pt(A4_H));

    let mut alloc = Ref::new(1);
    let catalog_id = alloc.bump();
    let tree_id = alloc.bump();
    let info_id = alloc.bump();
    let mut pdf = Pdf::new();
    let mut page_ids = Vec::with_capacity(instances.len());
    let mut chunks = Vec::with_capacity(instances.len());
    for inst in instances {
        let svg = sheet_svg(inst)?;
        let tree = svg2pdf::usvg::Tree::from_str(&svg, &opt)
            .map_err(|e| RenderError::Svg(e.to_string()))?;
        let (chunk, svg_ref) =
            svg2pdf::to_chunk(&tree, conv).map_err(|e| RenderError::Pdf(e.to_string()))?;
        let mut map: BTreeMap<Ref, Ref> = BTreeMap::new();
        let chunk = chunk.renumber(|old| *map.entry(old).or_insert_with(|| alloc.bump()));
        let svg_ref = *map
            .get(&svg_ref)
            .ok_or_else(|| RenderError::Pdf("svg2pdf root missing".into()))?;
        let page_id = alloc.bump();
        let content_id = alloc.bump();
        let name = Name(b"S0");
        let mut page = pdf.page(page_id);
        page.media_box(Rect::new(0.0, 0.0, w_pt, h_pt));
        page.parent(tree_id);
        page.contents(content_id);
        page.resources().x_objects().pair(name, svg_ref);
        page.finish();
        let mut content = Content::new();
        content
            .save_state()
            .transform([w_pt, 0.0, 0.0, h_pt, 0.0, 0.0])
            .x_object(name)
            .restore_state();
        pdf.stream(content_id, &content.finish());
        page_ids.push(page_id);
        chunks.push(chunk);
    }
    for c in &chunks {
        pdf.extend(c);
    }
    pdf.pages(tree_id)
        .kids(page_ids.iter().copied())
        .count(page_ids.len() as i32);
    {
        let mut cat = pdf.catalog(catalog_id);
        cat.pages(tree_id);
        cat.viewer_preferences()
            .pair(Name(b"PrintScaling"), Name(b"None"));
    }
    pdf.document_info(info_id)
        .title(TextStr(title))
        .producer(TextStr("omr-forms"));
    Ok(pdf.finish())
}
