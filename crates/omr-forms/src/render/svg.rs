//! SVG generation of a sheet instance (§6.2 MVP: SVG behind the same API as the Typst renderer).
//!
//! User units are millimetres (`viewBox="0 0 210 297"`, `width="210mm"`). Everything is 100 % K
//! except the 45 % K option labels; there is no transparency (§6.8).

use super::RenderError;
use super::fonts::{MONO_FAMILY, SANS_FAMILY};
use crate::apriltag;
use crate::geometry::*;
use crate::instance::SheetInstance;
use crate::layout::{
    Anchor, COMB_STROKE, ExampleKind, HAlign, PageArt, PageGeometry, TextContent, TextItem,
};
use crate::pagecode;
use crate::units::{PtUm, RectUm, Um};
use std::fmt::Write as _;

/// Which parts of a page to emit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Layer {
    /// The complete sheet.
    All,
    /// Everything that does not depend on serial, key or personalisation (cacheable per form
    /// and page): paper, tags, bubbles, labels, ticks, header art.
    Static,
    /// Only the per-sheet parts: both page codes, both echo lines, personalised text. The
    /// background is transparent.
    Variable,
}

/// The complete sheet as SVG.
pub fn sheet_svg(inst: &SheetInstance<'_>) -> Result<String, RenderError> {
    write_svg(inst, Layer::All)
}

/// One layer of the sheet as SVG.
pub fn layer_svg(inst: &SheetInstance<'_>, layer: Layer) -> Result<String, RenderError> {
    write_svg(inst, layer)
}

/// Page regions the [`Layer::Variable`] layer may draw into (codes, echo lines, name slots).
pub fn variable_regions(inst: &SheetInstance<'_>) -> Result<Vec<RectUm>, RenderError> {
    let g = inst.geometry()?;
    let mut v: Vec<RectUm> = g.codes.iter().flat_map(|c| [c.rect, c.echo]).collect();
    if inst.personal.is_some()
        && let Some(art) = inst.template.art(inst.page)
    {
        for s in [&art.name_slot, &art.id_slot].into_iter().flatten() {
            v.push(RectUm::new(
                s.at.x,
                s.at.y - s.size,
                s.max_w,
                s.size * 13 / 10,
            ));
        }
    }
    Ok(v)
}

fn mm(out: &mut String, v: Um) {
    if v < 0 {
        out.push('-');
    }
    let a = v.unsigned_abs();
    let (i, f) = (a / 1000, a % 1000);
    if f == 0 {
        let _ = write!(out, "{i}");
    } else {
        let s = format!("{f:03}");
        let _ = write!(out, "{i}.{}", s.trim_end_matches('0'));
    }
}

fn attr_mm(out: &mut String, name: &str, v: Um) {
    let _ = write!(out, " {name}=\"");
    mm(out, v);
    out.push('"');
}

fn escape(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
}

fn gray_hex(g: u8) -> String {
    format!("#{g:02x}{g:02x}{g:02x}")
}

/// A path of axis-aligned module runs (one subpath per horizontal run of dark modules).
fn module_path(
    out: &mut String,
    origin: PtUm,
    module: Um,
    n: usize,
    dark: impl Fn(usize, usize) -> bool,
) {
    out.push_str("<path fill=\"#000\" d=\"");
    for r in 0..n {
        let mut c = 0;
        while c < n {
            if !dark(r, c) {
                c += 1;
                continue;
            }
            let start = c;
            while c < n && dark(r, c) {
                c += 1;
            }
            out.push('M');
            mm(out, origin.x + start as Um * module);
            out.push(' ');
            mm(out, origin.y + r as Um * module);
            out.push('h');
            mm(out, (c - start) as Um * module);
            out.push('v');
            mm(out, module);
            out.push('h');
            mm(out, -((c - start) as Um) * module);
            out.push('z');
        }
    }
    out.push_str("\"/>\n");
}

fn text(out: &mut String, t: &TextItem, content: &str, mono: bool) {
    let anchor = match t.anchor {
        Anchor::Left => "start",
        Anchor::Center => "middle",
        Anchor::Right => "end",
    };
    out.push_str("<text");
    attr_mm(out, "x", t.at.x);
    attr_mm(out, "y", t.at.y);
    let fam = if mono || t.mono {
        MONO_FAMILY
    } else {
        SANS_FAMILY
    };
    let _ = write!(out, " font-family=\"{fam}\"");
    attr_mm(out, "font-size", t.size);
    if t.bold {
        out.push_str(" font-weight=\"bold\"");
    }
    let _ = write!(
        out,
        " fill=\"{}\" text-anchor=\"{anchor}\">",
        gray_hex(t.gray)
    );
    // usvg lays every paragraph out left to right; an RLE … PDF embedding gives Arabic-first
    // lines their right-to-left run order.
    if t.rtl {
        out.push('\u{202B}');
    }
    escape(out, content);
    if t.rtl {
        out.push('\u{202C}');
    }
    out.push_str("</text>\n");
}

fn write_svg(inst: &SheetInstance<'_>, layer: Layer) -> Result<String, RenderError> {
    inst.check()?;
    let g = inst.geometry()?;
    let empty = PageArt::default();
    let art = inst.template.art(inst.page).unwrap_or(&empty);
    let mut out = String::with_capacity(96 * 1024);
    out.push_str(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"210mm\" height=\"297mm\" viewBox=\"0 0 210 297\">\n",
    );
    if layer != Layer::Variable {
        out.push_str("<rect width=\"210\" height=\"297\" fill=\"#fff\"/>\n");
        write_static(&mut out, inst, g, art);
    }
    if layer != Layer::Static {
        write_variable(&mut out, inst, g, art)?;
    }
    out.push_str("</svg>\n");
    Ok(out)
}

fn write_static(out: &mut String, inst: &SheetInstance<'_>, g: &PageGeometry, art: &PageArt) {
    if inst.proof {
        let _ = writeln!(
            out,
            "<text x=\"105\" y=\"160\" transform=\"rotate(-35 105 148.5)\" font-family=\"{SANS_FAMILY}\" font-size=\"38\" font-weight=\"bold\" fill=\"#d0d0d0\" text-anchor=\"middle\">PROOF · مسودة</text>"
        );
    }
    // Corner tags.
    for t in &g.tags {
        let id = t.id_in_set(inst.tag_set());
        if let Some(m) = apriltag::modules(id) {
            module_path(
                out,
                PtUm::new(t.outer.x, t.outer.y),
                TAG_MODULE,
                10,
                |r, c| m[r][c],
            );
        }
    }
    // Bubble outlines.
    let _ = writeln!(
        out,
        "<g fill=\"none\" stroke=\"#000\" stroke-width=\"{}\">",
        fmt_mm(BUBBLE_STROKE)
    );
    let circles = g
        .bubbles
        .iter()
        .map(|b| (b.centre, b.radius))
        .chain(g.id_bubbles.iter().map(|b| (b.centre, b.radius)))
        .chain(g.form_bubbles.iter().map(|b| (b.centre, b.radius)));
    for (c, r) in circles {
        out.push_str("<circle");
        attr_mm(out, "cx", c.x);
        attr_mm(out, "cy", c.y);
        attr_mm(out, "r", r);
        out.push_str("/>\n");
    }
    out.push_str("</g>\n");
    // Labels inside bubbles (6 pt, 45 % K).
    let _ = writeln!(
        out,
        "<g font-family=\"{SANS_FAMILY}\" font-size=\"{}\" fill=\"{}\" text-anchor=\"middle\">",
        fmt_mm(LABEL_FONT),
        gray_hex(LABEL_GRAY)
    );
    let labels = g
        .bubbles
        .iter()
        .map(|b| (b.centre, b.label.as_str()))
        .chain(g.id_bubbles.iter().map(|b| (b.centre, b.label.as_str())))
        .chain(g.form_bubbles.iter().map(|b| (b.centre, b.label.as_str())));
    for (c, l) in labels {
        out.push_str("<text");
        attr_mm(out, "x", c.x);
        attr_mm(out, "y", c.y + LABEL_FONT * 35 / 100);
        out.push('>');
        escape(out, l);
        out.push_str("</text>\n");
    }
    out.push_str("</g>\n");
    // Row ticks.
    if !g.ticks.is_empty() {
        out.push_str("<path fill=\"#000\" d=\"");
        for t in &g.ticks {
            out.push('M');
            mm(out, t.rect.x);
            out.push(' ');
            mm(out, t.rect.y);
            out.push('h');
            mm(out, t.rect.w);
            out.push('v');
            mm(out, t.rect.h);
            out.push('h');
            mm(out, -t.rect.w);
            out.push('z');
        }
        out.push_str("\"/>\n");
    }
    // Comb boxes and write-in rules.
    for f in &art.frames {
        out.push_str("<rect fill=\"none\" stroke=\"#000\"");
        attr_mm(out, "stroke-width", COMB_STROKE);
        attr_mm(out, "x", f.x);
        attr_mm(out, "y", f.y);
        attr_mm(out, "width", f.w);
        attr_mm(out, "height", f.h);
        out.push_str("/>\n");
    }
    for r in &art.rules {
        out.push_str("<line stroke=\"#000\"");
        attr_mm(out, "stroke-width", r.width);
        attr_mm(out, "x1", r.from.x);
        attr_mm(out, "y1", r.from.y);
        attr_mm(out, "x2", r.to.x);
        attr_mm(out, "y2", r.to.y);
        out.push_str("/>\n");
    }
    // Header text and question numbers.
    let form_page = inst.form_page_text();
    for t in &art.texts {
        let content = match &t.content {
            TextContent::Fixed(s) => s.as_str(),
            TextContent::FormPage => form_page.as_str(),
        };
        text(out, t, content, false);
    }
    // "How to mark" examples with verdict icons.
    for e in &art.examples {
        let (cx, cy) = (e.centre.x, e.centre.y);
        let _ = write!(
            out,
            "<circle fill=\"none\" stroke=\"#000\" stroke-width=\"{}\"",
            fmt_mm(BUBBLE_STROKE)
        );
        attr_mm(out, "cx", cx);
        attr_mm(out, "cy", cy);
        attr_mm(out, "r", BUBBLE_RADIUS);
        out.push_str("/>\n");
        match e.kind {
            ExampleKind::Fill => circle(out, e.centre, BUBBLE_RADIUS - 100, None),
            ExampleKind::Dot => circle(out, e.centre, 600, None),
            ExampleKind::Circle => circle(out, e.centre, BUBBLE_RADIUS + 800, Some(350)),
            ExampleKind::Tick => polyline(
                out,
                &[
                    (cx - 1_300, cy),
                    (cx - 300, cy + 1_100),
                    (cx + 1_500, cy - 1_400),
                ],
                450,
            ),
            ExampleKind::Cross => {
                polyline(
                    out,
                    &[(cx - 1_400, cy - 1_400), (cx + 1_400, cy + 1_400)],
                    450,
                );
                polyline(
                    out,
                    &[(cx - 1_400, cy + 1_400), (cx + 1_400, cy - 1_400)],
                    450,
                );
            }
        }
        let (ix, iy) = (e.icon.x, e.icon.y);
        if e.ok() {
            polyline(
                out,
                &[(ix - 700, iy), (ix - 200, iy + 600), (ix + 800, iy - 700)],
                350,
            );
        } else {
            polyline(out, &[(ix - 600, iy - 600), (ix + 600, iy + 600)], 350);
            polyline(out, &[(ix - 600, iy + 600), (ix + 600, iy - 600)], 350);
        }
    }
}

fn write_variable(
    out: &mut String,
    inst: &SheetInstance<'_>,
    g: &PageGeometry,
    art: &PageArt,
) -> Result<(), RenderError> {
    let payload = inst.payload()?;
    let (n, dark) = qr_modules(&payload)?;
    let echo_code = pagecode::echo_code(&payload)?;
    let prefix = inst.echo_prefix();
    for c in &g.codes {
        module_path(out, PtUm::new(c.rect.x, c.rect.y), c.module, n, |r, col| {
            dark[r * n + col]
        });
        let (x, anchor) = match c.echo_align {
            HAlign::Left => (c.echo.x, Anchor::Left),
            HAlign::Right => (c.echo.right(), Anchor::Right),
        };
        for (i, (s, mono)) in [(prefix.as_str(), false), (echo_code.as_str(), true)]
            .into_iter()
            .enumerate()
        {
            let t = TextItem {
                content: TextContent::Fixed(String::new()),
                at: PtUm::new(x, c.echo.y + ECHO_LINE_H * (i as Um) + ECHO_LINE_H * 3 / 4),
                size: ECHO_FONT,
                anchor,
                bold: false,
                mono,
                gray: 0,
                rtl: false,
                role: crate::layout::TextRole::Caption,
                bbox: c.echo,
            };
            text(out, &t, s, mono);
        }
    }
    if let Some(p) = &inst.personal {
        for (slot, s) in [(&art.name_slot, p.name()), (&art.id_slot, p.student_id())] {
            if let Some(slot) = slot {
                let t = TextItem {
                    content: TextContent::Fixed(String::new()),
                    at: slot.at,
                    size: slot.size,
                    anchor: Anchor::Left,
                    bold: true,
                    mono: false,
                    gray: 0,
                    rtl: false,
                    role: crate::layout::TextRole::Caption,
                    bbox: RectUm::new(slot.at.x, slot.at.y - slot.size, slot.max_w, slot.size),
                };
                text(out, &t, s, false);
            }
        }
    }
    Ok(())
}

fn fmt_mm(v: Um) -> String {
    let mut s = String::new();
    mm(&mut s, v);
    s
}

fn circle(out: &mut String, c: PtUm, r: Um, stroke: Option<Um>) {
    out.push_str("<circle");
    match stroke {
        Some(w) => {
            out.push_str(" fill=\"none\" stroke=\"#000\"");
            attr_mm(out, "stroke-width", w);
        }
        None => out.push_str(" fill=\"#000\""),
    }
    attr_mm(out, "cx", c.x);
    attr_mm(out, "cy", c.y);
    attr_mm(out, "r", r);
    out.push_str("/>\n");
}

fn polyline(out: &mut String, pts: &[(Um, Um)], w: Um) {
    out.push_str(
        "<path fill=\"none\" stroke=\"#000\" stroke-linecap=\"round\" stroke-linejoin=\"round\"",
    );
    attr_mm(out, "stroke-width", w);
    out.push_str(" d=\"");
    for (i, (x, y)) in pts.iter().enumerate() {
        out.push(if i == 0 { 'M' } else { 'L' });
        mm(out, *x);
        out.push(' ');
        mm(out, *y);
    }
    out.push_str("\"/>\n");
}

/// QR v3-M modules of a payload: (modules per side, row-major dark flags).
pub fn qr_modules(payload: &str) -> Result<(usize, Vec<bool>), RenderError> {
    let code = qrcode::QrCode::with_version(
        payload.as_bytes(),
        qrcode::Version::Normal(QR_VERSION),
        qrcode::EcLevel::M,
    )
    .map_err(|e| RenderError::Qr(e.to_string()))?;
    let n = code.width();
    if n != usize::from(QR_MODULES) {
        return Err(RenderError::Qr(format!("unexpected QR width {n}")));
    }
    let dark = code
        .to_colors()
        .iter()
        .map(|c| *c == qrcode::Color::Dark)
        .collect();
    Ok((n, dark))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn millimetre_formatting() {
        let f = |v| fmt_mm(v);
        assert_eq!(f(7_000), "7");
        assert_eq!(f(24_600), "24.6");
        assert_eq!(f(212), "0.212");
        assert_eq!(f(-1_050), "-1.05");
    }

    #[test]
    fn qr_is_version_3() {
        let (n, d) = qr_modules("1X7K2QF3M030202000CH1K9QF2ZA4M8RT0VXH6C3G").unwrap();
        assert_eq!(n, 29);
        assert_eq!(d.len(), 29 * 29);
        // Finder pattern corner module is dark.
        assert!(d[0] && d[28] && d[28 * 29]);
    }
}
