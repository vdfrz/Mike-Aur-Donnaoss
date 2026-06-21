//! Markdown → A4 PDF renderer, Indian court drafting style.
//!
//! Renders a Markdown string into a fresh `PdfDocument` using pdfium's
//! built-in Times fonts (no font files). Mirrors the feature set of
//! `docx_writer::render_markdown_to_wml`: H1/H2/H3 headings, paragraphs,
//! bullet/numbered lists, bold/italic spans, code spans, and simple pipe
//! tables. Word-wrapping and pagination are hand-rolled; this is a v1 that
//! prioritises correct flow over typographic perfection.
//!
//! Lifetime note: `PdfPage<'a>` is parameterized only by the `Pdfium` instance
//! lifetime, not by a borrow of the `PdfDocument` variable, so a page can be
//! held while we keep mutating the document (creating more pages). Fonts are
//! captured as lifetime-free `PdfFontToken`s. No `unsafe` is required.

use anyhow::{anyhow, Result};
use pdfium_render::prelude::*;
use pulldown_cmark::{Event as MdEvent, HeadingLevel, Options, Parser, Tag, TagEnd};

// A4 portrait body box: ~1 inch (72pt) margins on every side.
const PAGE_H: f32 = 842.0; // A4 height in points (297mm)
const MARGIN: f32 = 72.0; // 1 inch
const BODY_W: f32 = 595.0 - 2.0 * MARGIN; // A4 width (210mm) minus margins

const BODY_SIZE: f32 = 12.0;
const LINE_SPACING: f32 = 1.5; // 1.5x leading
const PARA_GAP: f32 = 6.0; // extra vertical gap after a block

/// One styled inline span within a block. `mono` selects a Courier face;
/// otherwise the Times face is chosen from the bold/italic flags.
#[derive(Clone)]
struct Span {
    text: String,
    bold: bool,
    italic: bool,
    mono: bool,
}

/// The four Times faces + Courier as lifetime-free font tokens, so wrapping
/// can switch face per span. The fonts themselves are owned by the document's
/// font map; we only carry their `Copy` handles. Built once per document.
struct Fonts {
    regular: PdfFontToken,
    bold: PdfFontToken,
    italic: PdfFontToken,
    bold_italic: PdfFontToken,
    mono: PdfFontToken,
}

impl Fonts {
    fn new(doc: &mut PdfDocument) -> Self {
        let f = doc.fonts_mut();
        Fonts {
            regular: f.new_built_in(PdfFontBuiltin::TimesRoman),
            bold: f.new_built_in(PdfFontBuiltin::TimesBold),
            italic: f.new_built_in(PdfFontBuiltin::TimesItalic),
            bold_italic: f.new_built_in(PdfFontBuiltin::TimesBoldItalic),
            mono: f.new_built_in(PdfFontBuiltin::Courier),
        }
    }

    fn pick(&self, bold: bool, italic: bool, mono: bool) -> PdfFontToken {
        if mono {
            self.mono
        } else {
            match (bold, italic) {
                (true, true) => self.bold_italic,
                (true, false) => self.bold,
                (false, true) => self.italic,
                (false, false) => self.regular,
            }
        }
    }
}

/// Approximate the rendered width of `text` at the given size. pdfium's
/// built-in fonts expose no metrics API in this crate, so we use a per-char
/// average of size*0.5 — comfortably inside the body box for Times/Courier.
fn text_width(text: &str, size: f32) -> f32 {
    text.chars().count() as f32 * size * 0.5
}

/// Inter-word space width.
fn space_width(size: f32) -> f32 {
    size * 0.25
}

/// Mutable rendering cursor: the page being filled and the baseline y.
/// y is measured in PDF points from the page bottom (pdfium origin).
struct Cursor<'a> {
    page: PdfPage<'a>,
    y: f32,
}

impl<'a> Cursor<'a> {
    /// True if a line of `height` still fits above the bottom margin.
    fn fits(&self, height: f32) -> bool {
        self.y - height >= MARGIN
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Align {
    Left,
    Center,
}

/// Render `markdown` to a fresh A4 PDF document, prefixed with `title` as a
/// centered bold all-caps H1. Returns the document for the caller to merge.
pub fn markdown_to_pdf_pages<'a>(
    pdfium: &'a Pdfium,
    title: &str,
    markdown: &str,
) -> Result<PdfDocument<'a>> {
    let mut doc = pdfium
        .create_new_pdf()
        .map_err(|e| anyhow!("create pdf: {e}"))?;

    let fonts = Fonts::new(&mut doc);
    let mut cursor = new_page(&mut doc)?;

    // Title as a centered, bold, all-caps heading.
    if !title.trim().is_empty() {
        let spans = vec![Span {
            text: title.trim().to_uppercase(),
            bold: true,
            italic: false,
            mono: false,
        }];
        render_block(&mut doc, &mut cursor, &fonts, &spans, BODY_SIZE, Align::Center, 0.0)?;
        cursor.y -= PARA_GAP;
    }

    render_markdown(&mut doc, &mut cursor, &fonts, markdown)?;
    Ok(doc)
}

/// Create a fresh A4 page and return a cursor positioned at the top margin.
fn new_page<'a>(doc: &mut PdfDocument<'a>) -> Result<Cursor<'a>> {
    let page = doc
        .pages_mut()
        .create_page_at_end(PdfPagePaperSize::a4())
        .map_err(|e| anyhow!("create page: {e}"))?;
    Ok(Cursor { page, y: PAGE_H - MARGIN })
}

/// Walk the Markdown event stream, mirroring the structure of
/// `docx_writer::render_markdown_to_wml`. Inline spans are buffered per block
/// and flushed (wrapped + paginated) on block end.
fn render_markdown<'a>(
    doc: &mut PdfDocument<'a>,
    cursor: &mut Cursor<'a>,
    fonts: &Fonts,
    markdown: &str,
) -> Result<()> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    let parser = Parser::new_ext(markdown, opts);

    let mut spans: Vec<Span> = Vec::new();
    let mut bold = false;
    let mut italic = false;

    // Block context.
    let mut block_size = BODY_SIZE;
    let mut block_align = Align::Left;
    let mut block_indent = 0.0f32;
    let mut list_prefix: Option<String> = None; // bullet / number marker
    let mut in_numbered_list = false;
    let mut list_counter: u64 = 0;
    let mut in_code_block = false;

    // Table state — collected then rendered as plain rows.
    let mut in_table = false;
    let mut cell_text = String::new();
    let mut row_cells: Vec<String> = Vec::new();
    let mut table_rows: Vec<Vec<String>> = Vec::new();

    for ev in parser {
        if in_table {
            match ev {
                MdEvent::Start(Tag::TableHead) => {}
                MdEvent::End(TagEnd::TableHead) => {
                    if !row_cells.is_empty() {
                        table_rows.push(std::mem::take(&mut row_cells));
                    }
                }
                MdEvent::Start(Tag::TableRow) => {}
                MdEvent::End(TagEnd::TableRow) => {
                    table_rows.push(std::mem::take(&mut row_cells));
                }
                MdEvent::Start(Tag::TableCell) => cell_text.clear(),
                MdEvent::End(TagEnd::TableCell) => {
                    row_cells.push(std::mem::take(&mut cell_text));
                }
                MdEvent::Text(t) | MdEvent::Code(t) => cell_text.push_str(&t),
                MdEvent::End(TagEnd::Table) => {
                    render_table(doc, cursor, fonts, &table_rows)?;
                    table_rows.clear();
                    in_table = false;
                }
                _ => {}
            }
            continue;
        }

        match ev {
            MdEvent::Start(Tag::Table(_)) => {
                in_table = true;
                table_rows.clear();
                row_cells.clear();
                cell_text.clear();
            }
            MdEvent::Start(Tag::Heading { level, .. }) => {
                block_align = match level {
                    HeadingLevel::H1 | HeadingLevel::H2 => Align::Center,
                    _ => Align::Left,
                };
                block_size = match level {
                    HeadingLevel::H1 => 16.0,
                    HeadingLevel::H2 => 14.0,
                    _ => 12.0,
                };
                bold = true; // all headings render bold
            }
            MdEvent::End(TagEnd::Heading(level)) => {
                // H1 renders all-caps (court style).
                if matches!(level, HeadingLevel::H1) {
                    for s in spans.iter_mut() {
                        s.text = s.text.to_uppercase();
                    }
                }
                flush_block(doc, cursor, fonts, &mut spans, block_size, block_align, 0.0)?;
                cursor.y -= PARA_GAP;
                bold = false;
                block_size = BODY_SIZE;
                block_align = Align::Left;
            }
            MdEvent::Start(Tag::Paragraph) => {}
            MdEvent::End(TagEnd::Paragraph) => {
                flush_block(doc, cursor, fonts, &mut spans, BODY_SIZE, Align::Left, block_indent)?;
                cursor.y -= PARA_GAP;
            }
            MdEvent::Start(Tag::List(start)) => {
                in_numbered_list = start.is_some();
                list_counter = start.unwrap_or(1);
            }
            MdEvent::End(TagEnd::List(_)) => {
                in_numbered_list = false;
                cursor.y -= PARA_GAP;
            }
            MdEvent::Start(Tag::Item) => {
                block_indent = 24.0;
                list_prefix = Some(if in_numbered_list {
                    let p = format!("{}. ", list_counter);
                    list_counter += 1;
                    p
                } else {
                    "•  ".to_string()
                });
            }
            MdEvent::End(TagEnd::Item) => {
                // Prepend the list marker as a leading span.
                if let Some(prefix) = list_prefix.take() {
                    spans.insert(
                        0,
                        Span { text: prefix, bold: false, italic: false, mono: false },
                    );
                }
                flush_block(doc, cursor, fonts, &mut spans, BODY_SIZE, Align::Left, block_indent)?;
                block_indent = 0.0;
            }
            MdEvent::Start(Tag::Strong) => bold = true,
            MdEvent::End(TagEnd::Strong) => bold = false,
            MdEvent::Start(Tag::Emphasis) => italic = true,
            MdEvent::End(TagEnd::Emphasis) => italic = false,
            MdEvent::Start(Tag::CodeBlock(_)) => in_code_block = true,
            MdEvent::End(TagEnd::CodeBlock) => {
                flush_block(doc, cursor, fonts, &mut spans, BODY_SIZE, Align::Left, block_indent)?;
                cursor.y -= PARA_GAP;
                in_code_block = false;
            }
            MdEvent::Text(t) => spans.push(Span {
                text: t.to_string(),
                bold,
                italic,
                mono: in_code_block,
            }),
            MdEvent::Code(t) => spans.push(Span {
                text: t.to_string(),
                bold,
                italic,
                mono: true,
            }),
            MdEvent::SoftBreak | MdEvent::HardBreak => spans.push(Span {
                text: " ".to_string(),
                bold,
                italic,
                mono: false,
            }),
            _ => {}
        }
    }

    // Flush any trailing block.
    if !spans.is_empty() {
        flush_block(doc, cursor, fonts, &mut spans, BODY_SIZE, Align::Left, 0.0)?;
    }
    Ok(())
}

/// Flush buffered spans as one block, clearing the buffer.
#[allow(clippy::too_many_arguments)]
fn flush_block<'a>(
    doc: &mut PdfDocument<'a>,
    cursor: &mut Cursor<'a>,
    fonts: &Fonts,
    spans: &mut Vec<Span>,
    size: f32,
    align: Align,
    indent: f32,
) -> Result<()> {
    if spans.is_empty() {
        return Ok(());
    }
    let block = std::mem::take(spans);
    render_block(doc, cursor, fonts, &block, size, align, indent)
}

/// Word-wrap `spans` into lines that fit the body box (minus `indent`),
/// emitting each line as a row of text objects, paginating as needed.
#[allow(clippy::too_many_arguments)]
fn render_block<'a>(
    doc: &mut PdfDocument<'a>,
    cursor: &mut Cursor<'a>,
    fonts: &Fonts,
    spans: &[Span],
    size: f32,
    align: Align,
    indent: f32,
) -> Result<()> {
    let line_height = size * LINE_SPACING;
    let avail = BODY_W - indent;
    let space_w = space_width(size);

    // Tokenize spans into per-word styled spans.
    let mut words: Vec<Span> = Vec::new();
    for s in spans {
        for w in s.text.split(' ') {
            if w.is_empty() {
                continue;
            }
            words.push(Span {
                text: w.to_string(),
                bold: s.bold,
                italic: s.italic,
                mono: s.mono,
            });
        }
    }
    if words.is_empty() {
        return Ok(());
    }

    // Greedy line breaking.
    let mut line: Vec<Span> = Vec::new();
    let mut line_w = 0.0f32;

    for w in words {
        let ww = text_width(&w.text, size);
        let add = if line.is_empty() { ww } else { space_w + ww };
        if !line.is_empty() && line_w + add > avail {
            emit_line(doc, cursor, fonts, &line, size, line_height, align, indent)?;
            line.clear();
            line_w = 0.0;
        }
        line_w += if line.is_empty() { ww } else { space_w + ww };
        line.push(w);
    }
    if !line.is_empty() {
        emit_line(doc, cursor, fonts, &line, size, line_height, align, indent)?;
    }
    Ok(())
}

/// Emit one wrapped line: paginate first, then place each styled word as a
/// text object left-to-right at the current baseline.
#[allow(clippy::too_many_arguments)]
fn emit_line<'a>(
    doc: &mut PdfDocument<'a>,
    cursor: &mut Cursor<'a>,
    fonts: &Fonts,
    words: &[Span],
    size: f32,
    line_height: f32,
    align: Align,
    indent: f32,
) -> Result<()> {
    if !cursor.fits(line_height) {
        // Start a new page. The old page is dropped by the assignment,
        // releasing its content for regeneration.
        let fresh = new_page(doc)?;
        *cursor = fresh;
    }
    cursor.y -= line_height;
    let baseline = cursor.y;

    let space_w = space_width(size);
    let total: f32 = words.iter().map(|w| text_width(&w.text, size)).sum::<f32>()
        + space_w * words.len().saturating_sub(1) as f32;

    let mut x = match align {
        Align::Left => MARGIN + indent,
        Align::Center => MARGIN + (BODY_W - total).max(0.0) / 2.0,
    };

    for (i, w) in words.iter().enumerate() {
        if i > 0 {
            x += space_w;
        }
        let font = fonts.pick(w.bold, w.italic, w.mono);
        cursor
            .page
            .objects_mut()
            .create_text_object(
                PdfPoints::new(x),
                PdfPoints::new(baseline),
                &w.text,
                font,
                PdfPoints::new(size),
            )
            .map_err(|e| anyhow!("text object: {e}"))?;
        x += text_width(&w.text, size);
    }
    Ok(())
}

/// Render a pipe table as plain rows: each row becomes one paragraph with
/// cells joined by " — ". Wide rows wrap like any other paragraph.
/// Render a pipe table as aligned columns: each column gets a share of the
/// body width proportional to its widest cell, cells word-wrap within their
/// column, and the first row is bold (the header). This is what makes the
/// court-bundle INDEX read as a real "S.No | Document | Pages" table rather
/// than a run-on line.
fn render_table<'a>(
    doc: &mut PdfDocument<'a>,
    cursor: &mut Cursor<'a>,
    fonts: &Fonts,
    rows: &[Vec<String>],
) -> Result<()> {
    let ncols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if ncols == 0 {
        return Ok(());
    }
    let size = BODY_SIZE;
    let line_height = size * LINE_SPACING;
    let col_gap = size; // ~one em between columns

    // Natural width = widest cell per column; share the body box proportionally.
    let mut natural = vec![1.0f32; ncols];
    for row in rows {
        for (c, cell) in row.iter().enumerate() {
            natural[c] = natural[c].max(text_width(cell.trim(), size));
        }
    }
    let total: f32 = natural.iter().sum();
    let avail = (BODY_W - col_gap * (ncols as f32 - 1.0)).max(size);
    let widths: Vec<f32> = natural.iter().map(|w| (w / total) * avail).collect();
    let mut col_x = vec![MARGIN; ncols];
    for c in 1..ncols {
        col_x[c] = col_x[c - 1] + widths[c - 1] + col_gap;
    }

    for (ri, row) in rows.iter().enumerate() {
        let header = ri == 0;
        // Wrap each cell to its column width; the row is as tall as its
        // tallest cell.
        let wrapped: Vec<Vec<String>> = (0..ncols)
            .map(|c| wrap_to_width(row.get(c).map(|s| s.trim()).unwrap_or(""), size, widths[c]))
            .collect();
        let row_lines = wrapped.iter().map(|w| w.len()).max().unwrap_or(1).max(1);

        if !cursor.fits(row_lines as f32 * line_height) {
            *cursor = new_page(doc)?;
        }
        for li in 0..row_lines {
            cursor.y -= line_height;
            let baseline = cursor.y;
            for c in 0..ncols {
                let Some(text) = wrapped[c].get(li) else { continue };
                if text.is_empty() {
                    continue;
                }
                let font = fonts.pick(header, false, false);
                cursor
                    .page
                    .objects_mut()
                    .create_text_object(
                        PdfPoints::new(col_x[c]),
                        PdfPoints::new(baseline),
                        text,
                        font,
                        PdfPoints::new(size),
                    )
                    .map_err(|e| anyhow!("table cell: {e}"))?;
            }
        }
    }
    cursor.y -= PARA_GAP;
    Ok(())
}

/// Greedy word-wrap of one cell's text into lines no wider than `width`.
fn wrap_to_width(text: &str, size: f32, width: f32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let candidate = if line.is_empty() {
            word.to_string()
        } else {
            format!("{line} {word}")
        };
        if !line.is_empty() && text_width(&candidate, size) > width {
            lines.push(std::mem::take(&mut line));
            line = word.to_string();
        } else {
            line = candidate;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}
