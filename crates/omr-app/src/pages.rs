//! Stored pages read and decoded away from their project (the desktop's page thumbnails,
//! §9.3). A shell's worker thread cannot borrow an [`ExamProject`]: a [`PageBatch`] carries what
//! [`ExamProject::page_image`] uses for some pages (the object store's folder, the pages'
//! sources and the exam definition) and reads, verifies and decodes them wherever it runs.

use crate::engine::Recognizer;
use crate::exam::ExamDefinition;
use crate::ingest::{self, OpenSource, SourceFile, SourceKind};
use crate::model::SourceInfo;
use crate::{AppError, ExamProject};
use omr_imaging::GrayImage;
use omr_store::objects::ObjectStore;
use std::collections::BTreeMap;
use std::path::Path;

/// Stored pages of a project to decode on another thread ([`ExamProject::page_batch`]).
pub struct PageBatch {
    objects: ObjectStore,
    definition: ExamDefinition,
    /// Per page asked for: its source, the page within the source and its page ID (or why
    /// there is no such page).
    pages: Vec<Result<(usize, u32, String), String>>,
    /// The sources of those pages.
    sources: BTreeMap<usize, SourceInfo>,
}

impl ExamProject {
    /// The pages `pages` (indices into the session's pages) to decode on another thread with
    /// [`PageBatch::decode`]. Nothing is read here. `None` without the project store (a project
    /// opened read-only cannot read its originals).
    pub fn page_batch(&self, pages: &[usize]) -> Option<PageBatch> {
        let objects = self.store()?.objects();
        let mut sources = BTreeMap::new();
        let pages = pages
            .iter()
            .map(|&page| {
                let p = self
                    .session
                    .pages
                    .get(page)
                    .ok_or_else(|| format!("page {page}"))?;
                let info = self
                    .session
                    .sources
                    .get(p.source)
                    .ok_or_else(|| format!("source {}", p.source))?;
                sources.entry(p.source).or_insert_with(|| info.clone());
                Ok((p.source, p.index, p.page_id.clone()))
            })
            .collect();
        Some(PageBatch {
            objects,
            definition: self.session.definition.clone(),
            pages,
            sources,
        })
    }
}

impl PageBatch {
    /// The number of pages asked for.
    pub fn len(&self) -> usize {
        self.pages.len()
    }

    /// True when no page was asked for.
    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    /// Decodes the pages in the order asked and hands each one's working-resolution image (what
    /// [`ExamProject::page_image`] gives) to `each` with its position, as soon as it is decoded.
    /// Each source file is read, verified and opened once.
    pub fn decode(self, mut each: impl FnMut(usize, Result<GrayImage, AppError>)) {
        let engine = Recognizer::new(&self.definition).map_err(|e| e.to_string());
        let mut opened: BTreeMap<usize, Result<(SourceFile, OpenSource), String>> = BTreeMap::new();
        for (i, page) in self.pages.iter().enumerate() {
            let image = (|| {
                let (s, index, page_id) =
                    page.as_ref().map_err(|e| AppError::NotFound(e.clone()))?;
                let rec = engine
                    .as_ref()
                    .map_err(|e| AppError::Definition(e.clone()))?;
                let (src, open) = opened
                    .entry(*s)
                    .or_insert_with(|| self.open(*s))
                    .as_ref()
                    .map_err(|e| AppError::NotFound(e.clone()))?;
                let d = ingest::decode_page(src, open, *index, page_id)
                    .map_err(|e| AppError::Input(e.detail))?;
                Ok(rec.to_working(d.image, d.capture, d.known_dpi).image)
            })();
            each(i, image);
        }
    }

    /// Reads, verifies and opens source `s` (as the project does for its page images).
    fn open(&self, s: usize) -> Result<(SourceFile, OpenSource), String> {
        let info = self.sources.get(&s).ok_or_else(|| format!("source {s}"))?;
        let blake = info
            .blake3
            .as_deref()
            .ok_or_else(|| format!("{}: original not stored", info.name))?;
        let bytes = self
            .objects
            .read_verified(blake, &info.sha384)
            .map_err(|e| crate::store::store_error(e).to_string())?;
        let ext = match info.kind {
            SourceKind::Pdf => "pdf",
            SourceKind::Image => "png",
        };
        let src = SourceFile::from_bytes(Path::new(&format!("x.{ext}")), bytes)
            .ok_or_else(|| "source kind".to_string())?;
        let (open, _) = ingest::open_source(&src);
        Ok((src, open))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_batches_are_sent_to_other_threads() {
        fn send<T: Send>() {}
        send::<PageBatch>();
    }
}
