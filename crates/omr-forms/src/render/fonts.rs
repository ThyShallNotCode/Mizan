//! Vendored OFL fonts (IBM Plex Sans Arabic, which also covers Latin, and IBM Plex Mono for the
//! echo line, §6.4). They are compiled in, so rendering needs no font files at run time (R4)
//! and every machine renders identical glyphs.

use std::sync::{Arc, OnceLock};

/// IBM Plex Sans Arabic Regular (SIL OFL 1.1).
pub const SANS_REGULAR: &[u8] = include_bytes!("../../assets/fonts/IBMPlexSansArabic-Regular.ttf");
/// IBM Plex Sans Arabic Bold (SIL OFL 1.1).
pub const SANS_BOLD: &[u8] = include_bytes!("../../assets/fonts/IBMPlexSansArabic-Bold.ttf");
/// IBM Plex Mono Regular (SIL OFL 1.1).
pub const MONO_REGULAR: &[u8] = include_bytes!("../../assets/fonts/IBMPlexMono-Regular.ttf");
/// The SIL Open Font License text shipped with the fonts.
pub const OFL_TEXT: &str = include_str!("../../assets/fonts/OFL.txt");

/// Family name of the sans face.
pub const SANS_FAMILY: &str = "IBM Plex Sans Arabic";
/// Family name of the monospace face.
pub const MONO_FAMILY: &str = "IBM Plex Mono";

/// Font database for the rasteriser (usvg 0.48 / fontdb 0.24).
pub(crate) fn raster_fontdb() -> Arc<resvg::usvg::fontdb::Database> {
    static DB: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = resvg::usvg::fontdb::Database::new();
        for f in [SANS_REGULAR, SANS_BOLD, MONO_REGULAR] {
            db.load_font_data(f.to_vec());
        }
        db.set_sans_serif_family(SANS_FAMILY);
        db.set_monospace_family(MONO_FAMILY);
        Arc::new(db)
    })
    .clone()
}

/// Font database for the PDF writer (svg2pdf's usvg 0.45 / fontdb 0.23).
pub(crate) fn pdf_fontdb() -> Arc<svg2pdf::usvg::fontdb::Database> {
    static DB: OnceLock<Arc<svg2pdf::usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = svg2pdf::usvg::fontdb::Database::new();
        for f in [SANS_REGULAR, SANS_BOLD, MONO_REGULAR] {
            db.load_font_data(f.to_vec());
        }
        db.set_sans_serif_family(SANS_FAMILY);
        db.set_monospace_family(MONO_FAMILY);
        Arc::new(db)
    })
    .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendored_faces_load_with_expected_families() {
        let db = raster_fontdb();
        let families: Vec<String> = db
            .faces()
            .flat_map(|f| f.families.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>())
            .collect();
        assert!(families.iter().any(|f| f == SANS_FAMILY), "{families:?}");
        assert!(families.iter().any(|f| f == MONO_FAMILY), "{families:?}");
        assert_eq!(db.len(), 3);
        assert!(OFL_TEXT.contains("SIL Open Font License"));
    }
}
