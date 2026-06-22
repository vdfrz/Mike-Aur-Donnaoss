//! Minimal DOCX writer + in-place editor.
//!
//! - `markdown_to_docx(title, markdown)` → produces a small but valid .docx
//!   from a Markdown string. Supports headings (#, ##, ###), paragraphs,
//!   bullet/numbered lists, bold/italic emphasis, and code spans.
//! - `apply_text_edits(original, edits)` → reads an existing .docx, walks
//!   `word/document.xml`, performs find/replace inside `<w:t>` runs, and
//!   re-zips the result. Used by the `edit_document` builtin tool.

use anyhow::{anyhow, Result};
use pulldown_cmark::{Event as MdEvent, HeadingLevel, Options, Parser, Tag, TagEnd};
use std::io::{Cursor, Read, Write};

// ---------------------------------------------------------------------------
// generate_docx
// ---------------------------------------------------------------------------

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
  <Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>
  <Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/>
</Types>"#;

const RELS_ROOT: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;

const RELS_DOC: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
  <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/>
</Relationships>"#;

// Indian district court document standards (derived from real HMA/DV Act filings):
// Font: Times New Roman, Body: 12pt (sz=24), Headings: 12pt bold centred CAPS
// Line spacing: 1.5x (line=360), Court header block: double-spaced (line=480)
// Page: A4, left margin 1.25 inch for binding

const STYLES_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:docDefaults>
    <w:rPrDefault><w:rPr>
      <w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman" w:cs="Times New Roman"/>
      <w:sz w:val="24"/><w:szCs w:val="24"/>
      <w:lang w:val="en-IN" w:eastAsia="en-US" w:bidi="hi-IN"/>
    </w:rPr></w:rPrDefault>
    <w:pPrDefault><w:pPr>
      <w:spacing w:line="360" w:lineRule="auto"/>
      <w:jc w:val="both"/>
    </w:pPr></w:pPrDefault>
  </w:docDefaults>
  <w:style w:type="paragraph" w:default="1" w:styleId="Normal">
    <w:name w:val="Normal"/>
    <w:pPr><w:spacing w:line="360" w:lineRule="auto"/><w:ind w:firstLine="360"/><w:jc w:val="both"/></w:pPr>
    <w:rPr><w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman" w:cs="Times New Roman"/><w:sz w:val="24"/><w:szCs w:val="24"/></w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="Heading1">
    <w:name w:val="heading 1"/>
    <w:basedOn w:val="Normal"/><w:next w:val="Normal"/>
    <w:pPr><w:spacing w:before="0" w:after="120" w:line="360" w:lineRule="auto"/><w:ind w:firstLine="0"/><w:jc w:val="center"/><w:outlineLvl w:val="0"/></w:pPr>
    <w:rPr><w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman" w:cs="Times New Roman"/><w:b/><w:bCs/><w:sz w:val="24"/><w:szCs w:val="24"/></w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="Heading2">
    <w:name w:val="heading 2"/>
    <w:basedOn w:val="Normal"/><w:next w:val="Normal"/>
    <w:pPr><w:spacing w:before="360" w:after="120" w:line="360" w:lineRule="auto"/><w:ind w:firstLine="0"/><w:jc w:val="center"/><w:outlineLvl w:val="1"/></w:pPr>
    <w:rPr><w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman" w:cs="Times New Roman"/><w:b/><w:bCs/><w:sz w:val="24"/><w:szCs w:val="24"/><w:u w:val="single"/></w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="Heading3">
    <w:name w:val="heading 3"/>
    <w:basedOn w:val="Normal"/><w:next w:val="Normal"/>
    <w:pPr><w:spacing w:before="240" w:after="60" w:line="360" w:lineRule="auto"/><w:ind w:firstLine="0"/><w:jc w:val="left"/><w:outlineLvl w:val="2"/></w:pPr>
    <w:rPr><w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman" w:cs="Times New Roman"/><w:b/><w:bCs/><w:sz w:val="24"/><w:szCs w:val="24"/></w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="LegalBold">
    <w:name w:val="Legal Bold"/>
    <w:basedOn w:val="Normal"/>
    <w:pPr><w:spacing w:before="240" w:after="60" w:line="360" w:lineRule="auto"/><w:ind w:firstLine="0"/><w:jc w:val="left"/></w:pPr>
    <w:rPr><w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman" w:cs="Times New Roman"/><w:b/><w:bCs/><w:sz w:val="24"/><w:szCs w:val="24"/></w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="PartyBlock">
    <w:name w:val="Party Block"/>
    <w:basedOn w:val="Normal"/>
    <w:pPr><w:spacing w:before="60" w:after="60" w:line="360" w:lineRule="auto"/><w:ind w:firstLine="0"/><w:jc w:val="left"/></w:pPr>
    <w:rPr><w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman" w:cs="Times New Roman"/><w:sz w:val="24"/><w:szCs w:val="24"/></w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="PartyRole">
    <w:name w:val="Party Role"/>
    <w:basedOn w:val="Normal"/>
    <w:pPr><w:spacing w:before="60" w:after="60" w:line="360" w:lineRule="auto"/><w:ind w:firstLine="0"/><w:jc w:val="right"/></w:pPr>
    <w:rPr><w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman" w:cs="Times New Roman"/><w:b/><w:bCs/><w:sz w:val="24"/><w:szCs w:val="24"/></w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="Versus">
    <w:name w:val="Versus"/>
    <w:basedOn w:val="Normal"/>
    <w:pPr><w:spacing w:before="120" w:after="120" w:line="360" w:lineRule="auto"/><w:ind w:firstLine="0"/><w:jc w:val="center"/></w:pPr>
    <w:rPr><w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman" w:cs="Times New Roman"/><w:b/><w:bCs/><w:sz w:val="24"/><w:szCs w:val="24"/></w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="Signature">
    <w:name w:val="Signature"/>
    <w:basedOn w:val="Normal"/>
    <w:pPr><w:spacing w:before="360" w:after="0" w:line="360" w:lineRule="auto"/><w:ind w:firstLine="0"/><w:jc w:val="right"/></w:pPr>
    <w:rPr><w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman" w:cs="Times New Roman"/><w:b/><w:bCs/><w:sz w:val="24"/><w:szCs w:val="24"/></w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="ListBullet">
    <w:name w:val="List Bullet"/>
    <w:basedOn w:val="Normal"/>
    <w:pPr><w:spacing w:line="360" w:lineRule="auto"/><w:ind w:left="720" w:firstLine="0"/><w:jc w:val="both"/><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr>
    <w:rPr><w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman" w:cs="Times New Roman"/><w:sz w:val="24"/></w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="ListNumber">
    <w:name w:val="List Number"/>
    <w:basedOn w:val="Normal"/>
    <w:pPr><w:spacing w:line="360" w:lineRule="auto"/><w:ind w:left="720" w:firstLine="0"/><w:jc w:val="both"/><w:numPr><w:ilvl w:val="0"/><w:numId w:val="2"/></w:numPr></w:pPr>
    <w:rPr><w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman" w:cs="Times New Roman"/><w:sz w:val="24"/></w:rPr>
  </w:style>
</w:styles>"#;

const NUMBERING_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:abstractNum w:abstractNumId="0">
    <w:lvl w:ilvl="0">
      <w:start w:val="1"/>
      <w:numFmt w:val="bullet"/>
      <w:lvlText w:val="•"/>
      <w:lvlJc w:val="left"/>
      <w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr>
    </w:lvl>
  </w:abstractNum>
  <w:abstractNum w:abstractNumId="1">
    <w:lvl w:ilvl="0">
      <w:start w:val="1"/>
      <w:numFmt w:val="decimal"/>
      <w:lvlText w:val="%1."/>
      <w:lvlJc w:val="left"/>
      <w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr>
    </w:lvl>
  </w:abstractNum>
  <w:num w:numId="1">
    <w:abstractNumId w:val="0"/>
  </w:num>
  <w:num w:numId="2">
    <w:abstractNumId w:val="1"/>
  </w:num>
</w:numbering>"#;

/// Legal phrases that should be auto-bolded and given special styling.
const LEGAL_BOLD_PHRASES: &[&str] = &[
    "MOST RESPECTFULLY SHOWETH",
    "IN THE MATTER OF",
    "IN WITNESS WHEREOF",
    "PARAWISE REPLY",
    "BE IT ACKNOWLEDGED",
    "WHEREAS",
];

/// Phrases that indicate a "... COMPLAINANT" / "... RESPONDENT" role label.
const PARTY_ROLE_MARKERS: &[&str] = &[
    "COMPLAINANT", "PETITIONER", "APPELLANT", "APPLICANT",
    "RESPONDENT", "ACCUSED", "OPPOSITE PARTY", "DEFENDANT",
];

/// Post-process the rendered WML to auto-style legal paragraphs.
/// This detects key phrases and applies proper court document styles
/// (bold headings, right-aligned role labels, centered "Versus", etc.)
fn auto_style_legal(wml: &str) -> String {
    let mut result = String::with_capacity(wml.len());
    let mut that_counter: u32 = 0;

    for line in wml.lines() {
        if !line.contains("<w:p>") {
            result.push_str(line);
            result.push('\n');
            continue;
        }
        // Extract plain text from this paragraph for pattern matching
        let plain: String = line
            .split("<w:t")
            .skip(1)
            .filter_map(|s| s.split("</w:t>").next())
            .map(|s| s.split('>').last().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("");
        let upper = plain.trim().to_uppercase();

        // "Versus" / "V/S" → centered bold
        if upper == "VERSUS" || upper == "V/S" || upper == "VS" || upper == "VS." {
            let styled = line
                .replace(r#"w:pStyle w:val="Normal""#, r#"w:pStyle w:val="Versus""#);
            let styled = styled.replace(
                r#"<w:sz w:val="24"/><w:szCs w:val="24"/>"#,
                r#"<w:sz w:val="24"/><w:szCs w:val="24"/><w:b/><w:bCs/>"#,
            );
            result.push_str(&styled);
            result.push('\n');
            continue;
        }

        // "... COMPLAINANT" / "... RESPONDENT" role labels → right-aligned bold
        if PARTY_ROLE_MARKERS.iter().any(|m| upper.ends_with(m))
            && upper.starts_with("...")
        {
            let styled = line
                .replace(r#"w:pStyle w:val="Normal""#, r#"w:pStyle w:val="PartyRole""#);
            let styled = styled.replace(
                r#"<w:sz w:val="24"/><w:szCs w:val="24"/>"#,
                r#"<w:sz w:val="24"/><w:szCs w:val="24"/><w:b/><w:bCs/>"#,
            );
            result.push_str(&styled);
            result.push('\n');
            continue;
        }

        // Section headings: PRAYER, VERIFICATION → centred bold underline (Heading2)
        if upper == "PRAYER" || upper == "PRAYER:" || upper == "VERIFICATION"
            || upper == "VERIFICATION:" || upper.starts_with("PRAYER CLAUSE")
        {
            let styled = line
                .replace(r#"w:pStyle w:val="Normal""#, r#"w:pStyle w:val="Heading2""#);
            let styled = styled.replace(
                r#"<w:sz w:val="24"/><w:szCs w:val="24"/>"#,
                r#"<w:sz w:val="24"/><w:szCs w:val="24"/><w:b/><w:bCs/>"#,
            );
            result.push_str(&styled);
            result.push('\n');
            continue;
        }

        // Legal bold phrases → LegalBold style with bold runs
        if LEGAL_BOLD_PHRASES.iter().any(|p| upper.starts_with(p)) {
            let styled = line
                .replace(r#"w:pStyle w:val="Normal""#, r#"w:pStyle w:val="LegalBold""#);
            let styled = styled.replace(
                r#"<w:sz w:val="24"/><w:szCs w:val="24"/>"#,
                r#"<w:sz w:val="24"/><w:szCs w:val="24"/><w:b/><w:bCs/>"#,
            );
            result.push_str(&styled);
            result.push('\n');
            continue;
        }

        // Signature lines: DEPONENT, COMPLAINANT alone, Through Counsel
        if upper == "DEPONENT" || upper == "COMPLAINANT" || upper == "PETITIONER"
            || upper == "THROUGH COUNSEL" || upper.starts_with("COMPLAINANT\n")
        {
            let styled = line
                .replace(r#"w:pStyle w:val="Normal""#, r#"w:pStyle w:val="Signature""#);
            result.push_str(&styled);
            result.push('\n');
            continue;
        }

        // Auto-number "That, " clauses — but skip if already numbered (e.g. "1. That, ")
        if (upper.starts_with("THAT, ") || upper.starts_with("THAT,"))
            && line.contains(r#"w:pStyle w:val="Normal""#)
        {
            // Check if already numbered: plain text starts with "N. " or bold "N."
            let already_numbered = plain.trim().chars().next().map_or(false, |c| c.is_ascii_digit());
            if !already_numbered {
                that_counter += 1;
                let numbered = line
                    .replacen(">That, ", &format!(">{}. That, ", that_counter), 1);
                let numbered = if numbered == *line {
                    line.replacen(">that, ", &format!(">{}. that, ", that_counter), 1)
                } else {
                    numbered
                };
                result.push_str(&numbered);
                result.push('\n');
                continue;
            }
        }

        // Verification clause text → make it a separate styled paragraph
        if upper.starts_with("THIS AFFIDAVIT IS VERIFIED")
            || upper.starts_with("VERIFIED AT")
            || upper.starts_with("I, THE DEPONENT")
            || upper.starts_with("I SOLEMNLY AFFIRM")
        {
            // Apply LegalBold for the verification text
            let styled = line
                .replace(r#"w:pStyle w:val="Normal""#, r#"w:pStyle w:val="LegalBold""#);
            result.push_str(&styled);
            result.push('\n');
            continue;
        }

        // Multi-line blocks — party/cause-title particulars, addresses and
        // signature blocks — are built with forced line breaks. Justified
        // short lines stretch edge-to-edge and look broken, so left-align
        // any otherwise-Normal paragraph that contains a manual line break.
        if line.contains("<w:br/>")
            && line.contains(r#"w:pStyle w:val="Normal""#)
        {
            let styled = line.replace(
                r#"<w:pStyle w:val="Normal"/>"#,
                r#"<w:pStyle w:val="Normal"/><w:jc w:val="left"/>"#,
            );
            result.push_str(&styled);
            result.push('\n');
            continue;
        }

        // Default — keep as is
        result.push_str(line);
        result.push('\n');
    }
    result
}

/// Produce a DOCX byte buffer from `markdown`. `title` is used as the
/// document heading. Body text must follow Indian court drafting conventions.
pub fn markdown_to_docx(title: &str, markdown: &str) -> Result<Vec<u8>> {
    let raw_wml = render_markdown_to_wml(title, markdown);
    let body_xml = auto_style_legal(&raw_wml);
    let document_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
{body_xml}    <w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1800"/></w:sectPr>
  </w:body>
</w:document>"#
    );

    let buf = Vec::new();
    let cursor = Cursor::new(buf);
    let mut zip = zip::ZipWriter::new(cursor);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    zip.start_file("[Content_Types].xml", opts)?;
    zip.write_all(CONTENT_TYPES.as_bytes())?;
    zip.start_file("_rels/.rels", opts)?;
    zip.write_all(RELS_ROOT.as_bytes())?;
    zip.start_file("word/_rels/document.xml.rels", opts)?;
    zip.write_all(RELS_DOC.as_bytes())?;
    zip.start_file("word/styles.xml", opts)?;
    zip.write_all(STYLES_XML.as_bytes())?;
    zip.start_file("word/numbering.xml", opts)?;
    zip.write_all(NUMBERING_XML.as_bytes())?;
    zip.start_file("word/document.xml", opts)?;
    zip.write_all(document_xml.as_bytes())?;

    let cursor = zip.finish()?;
    Ok(cursor.into_inner())
}

fn render_markdown_to_wml(title: &str, markdown: &str) -> String {
    let mut out = String::new();
    if !title.trim().is_empty() {
        out.push_str(&para("Heading1", &[run(title, false, false, false)]));
    }

    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    let parser = Parser::new_ext(markdown, opts);
    let mut current_runs: Vec<String> = Vec::new();
    let mut current_style: Option<&str> = None;
    let mut bold = false;
    let mut italic = false;
    let mut in_code_block = false;
    let mut in_numbered_list = false;

    // Table state
    let mut in_table = false;
    let mut table_col_count: usize = 0;
    let mut table_header = false;
    let mut cell_runs: Vec<String> = Vec::new();
    let mut row_cells: Vec<Vec<String>> = Vec::new();
    let mut table_rows: Vec<(bool, Vec<Vec<String>>)> = Vec::new(); // (is_header, cells)

    let flush_paragraph = |runs: &mut Vec<String>, style: Option<&str>, out: &mut String| {
        if !runs.is_empty() {
            let style = style.unwrap_or("Normal");
            out.push_str(&para(style, runs));
            runs.clear();
        }
    };

    for ev in parser {
        if in_table {
            match ev {
                MdEvent::Start(Tag::TableHead) => { table_header = true; }
                MdEvent::End(TagEnd::TableHead) => {
                    if !row_cells.is_empty() {
                        table_col_count = row_cells.len();
                        table_rows.push((true, std::mem::take(&mut row_cells)));
                    }
                    table_header = false;
                }
                MdEvent::Start(Tag::TableRow) => {}
                MdEvent::End(TagEnd::TableRow) => {
                    while row_cells.len() < table_col_count {
                        row_cells.push(Vec::new());
                    }
                    table_rows.push((false, std::mem::take(&mut row_cells)));
                }
                MdEvent::Start(Tag::TableCell) => { cell_runs.clear(); }
                MdEvent::End(TagEnd::TableCell) => {
                    row_cells.push(std::mem::take(&mut cell_runs));
                }
                MdEvent::Start(Tag::Strong) => bold = true,
                MdEvent::End(TagEnd::Strong) => bold = false,
                MdEvent::Start(Tag::Emphasis) => italic = true,
                MdEvent::End(TagEnd::Emphasis) => italic = false,
                MdEvent::Text(t) => {
                    cell_runs.push(run(&t, bold || table_header, italic, false));
                }
                MdEvent::Code(t) => {
                    cell_runs.push(run(&t, bold || table_header, italic, true));
                }
                MdEvent::End(TagEnd::Table) => {
                    out.push_str(&render_table(&table_rows, table_col_count));
                    table_rows.clear();
                    in_table = false;
                    table_col_count = 0;
                }
                _ => {}
            }
            continue;
        }

        match ev {
            MdEvent::Start(Tag::Table(_alignments)) => {
                flush_paragraph(&mut current_runs, current_style, &mut out);
                current_style = None;
                in_table = true;
                table_rows.clear();
                row_cells.clear();
                cell_runs.clear();
            }
            MdEvent::Start(Tag::Heading { level, .. }) => {
                flush_paragraph(&mut current_runs, current_style, &mut out);
                current_style = Some(match level {
                    HeadingLevel::H1 => "Heading1",
                    HeadingLevel::H2 => "Heading2",
                    HeadingLevel::H3 => "Heading3",
                    _ => "Heading3",
                });
            }
            MdEvent::End(TagEnd::Heading(_)) => {
                flush_paragraph(&mut current_runs, current_style, &mut out);
                current_style = None;
            }
            MdEvent::Start(Tag::Paragraph) => { current_style = Some("Normal"); }
            MdEvent::End(TagEnd::Paragraph) => {
                flush_paragraph(&mut current_runs, current_style, &mut out);
                current_style = None;
            }
            MdEvent::Start(Tag::List(Some(_))) => { in_numbered_list = true; }
            MdEvent::Start(Tag::List(None))    => { in_numbered_list = false; }
            MdEvent::End(TagEnd::List(_)) => { in_numbered_list = false; }
            MdEvent::Start(Tag::Item) => {
                current_style = Some(if in_numbered_list { "ListNumber" } else { "ListBullet" });
            }
            MdEvent::End(TagEnd::Item) => {
                flush_paragraph(&mut current_runs, current_style, &mut out);
                current_style = None;
            }
            MdEvent::Start(Tag::Strong)   => bold = true,
            MdEvent::End(TagEnd::Strong)  => bold = false,
            MdEvent::Start(Tag::Emphasis) => italic = true,
            MdEvent::End(TagEnd::Emphasis) => italic = false,
            MdEvent::Start(Tag::CodeBlock(_)) => { in_code_block = true; current_style = Some("Normal"); }
            MdEvent::End(TagEnd::CodeBlock)   => {
                flush_paragraph(&mut current_runs, current_style, &mut out);
                in_code_block = false;
                current_style = None;
            }
            MdEvent::Text(t) => {
                current_runs.push(run(&t, bold, italic, in_code_block));
            }
            MdEvent::Code(t) => {
                current_runs.push(run(&t, bold, italic, true));
            }
            MdEvent::SoftBreak | MdEvent::HardBreak => {
                current_runs.push(r#"<w:r><w:br/></w:r>"#.to_string());
            }
            _ => {}
        }
    }
    flush_paragraph(&mut current_runs, current_style, &mut out);
    out
}

/// Render a collected table as WML `<w:tbl>`.
fn render_table(rows: &[(bool, Vec<Vec<String>>)], col_count: usize) -> String {
    if col_count == 0 || rows.is_empty() { return String::new(); }

    // A4 body width minus margins: 11906 - 1800 - 1440 = 8666 twips
    let page_width: u32 = 8666;
    let col_width = page_width / col_count as u32;

    let mut s = String::new();
    s.push_str("    <w:tbl>\n");
    s.push_str("      <w:tblPr>\n");
    s.push_str(&format!(
        "        <w:tblW w:w=\"{}\" w:type=\"dxa\"/>\n", page_width
    ));
    s.push_str("        <w:tblBorders>\
        <w:top w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"000000\"/>\
        <w:left w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"000000\"/>\
        <w:bottom w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"000000\"/>\
        <w:right w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"000000\"/>\
        <w:insideH w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"000000\"/>\
        <w:insideV w:val=\"single\" w:sz=\"4\" w:space=\"0\" w:color=\"000000\"/>\
        </w:tblBorders>\n");
    s.push_str("        <w:tblLook w:val=\"04A0\"/>\n");
    s.push_str("      </w:tblPr>\n");

    // Column widths
    s.push_str("      <w:tblGrid>\n");
    for _ in 0..col_count {
        s.push_str(&format!("        <w:gridCol w:w=\"{}\"/>\n", col_width));
    }
    s.push_str("      </w:tblGrid>\n");

    for (is_header, cells) in rows {
        s.push_str("      <w:tr>\n");
        if *is_header {
            s.push_str("        <w:trPr><w:tblHeader/></w:trPr>\n");
        }
        for (ci, cell) in cells.iter().enumerate() {
            if ci >= col_count { break; }
            s.push_str("        <w:tc>\n");
            s.push_str(&format!(
                "          <w:tcPr><w:tcW w:w=\"{}\" w:type=\"dxa\"/>\
                 <w:tcMar><w:left w:w=\"40\" w:type=\"dxa\"/><w:right w:w=\"40\" w:type=\"dxa\"/></w:tcMar>\
                 </w:tcPr>\n", col_width
            ));
            s.push_str("          <w:p><w:pPr><w:pStyle w:val=\"Normal\"/>\
                <w:spacing w:before=\"40\" w:after=\"40\"/><w:ind w:firstLine=\"0\"/>\
                </w:pPr>");
            if cell.is_empty() {
                s.push_str(&run("", false, false, false));
            } else {
                for r in cell { s.push_str(r); }
            }
            s.push_str("</w:p>\n");
            s.push_str("        </w:tc>\n");
        }
        s.push_str("      </w:tr>\n");
    }

    s.push_str("    </w:tbl>\n");
    s
}

fn para(style: &str, runs: &[String]) -> String {
    let mut s = String::new();
    s.push_str("    <w:p>");
    s.push_str(&format!(r#"<w:pPr><w:pStyle w:val="{style}"/></w:pPr>"#));
    for r in runs { s.push_str(r); }
    s.push_str("</w:p>\n");
    s
}

fn run(text: &str, bold: bool, italic: bool, mono: bool) -> String {
    // Always include explicit font + size so both MS Word and docx-preview
    // render Times New Roman 12pt (sz=24, matching real court filings).
    let mut props = String::new();
    if mono {
        props.push_str(r#"<w:rFonts w:ascii="Courier New" w:hAnsi="Courier New" w:cs="Courier New"/>"#);
    } else {
        props.push_str(r#"<w:rFonts w:ascii="Times New Roman" w:hAnsi="Times New Roman" w:cs="Times New Roman"/>"#);
    }
    props.push_str(r#"<w:sz w:val="24"/><w:szCs w:val="24"/>"#);
    if bold { props.push_str("<w:b/><w:bCs/>"); }
    if italic { props.push_str("<w:i/>"); }
    let rpr = format!("<w:rPr>{props}</w:rPr>");
    format!(
        r#"<w:r>{rpr}<w:t xml:space="preserve">{}</w:t></w:r>"#,
        xml_escape(text)
    )
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// edit_document — find/replace inside <w:t> runs
// ---------------------------------------------------------------------------

pub struct DocxEdit {
    pub find: String,
    pub replace: String,
    pub format: Option<String>,
}

pub struct TrackedChange {
    pub del_w_id: Option<String>,
    pub ins_w_id: Option<String>,
    pub deleted_text: String,
    pub inserted_text: String,
}

pub struct TrackedChangeResult {
    pub bytes: Vec<u8>,
    pub changes: Vec<TrackedChange>,
}

/// Apply text substitutions to a DOCX. Walks `word/document.xml`, replaces
/// occurrences of `find` with `replace` inside text runs, and rezips the
/// archive. Returns the new bytes and a per-edit hit count.
pub fn apply_text_edits(original: &[u8], edits: &[DocxEdit]) -> Result<(Vec<u8>, Vec<usize>)> {
    let cursor = Cursor::new(original.to_vec());
    let mut archive = zip::ZipArchive::new(cursor)?;

    // Collect all entries first (we need to rewrite document.xml, copy others).
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..archive.len() {
        let mut f = archive.by_index(i)?;
        let name = f.name().to_string();
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        entries.push((name, buf));
    }

    let mut counts = vec![0usize; edits.len()];

    for (name, bytes) in entries.iter_mut() {
        if name == "word/document.xml" {
            let xml = String::from_utf8_lossy(bytes).into_owned();
            let (new_xml, hits) = patch_document_xml(&xml, edits);
            for (i, h) in hits.iter().enumerate() {
                counts[i] += h;
            }
            *bytes = new_xml.into_bytes();
        }
    }

    let buf = Vec::new();
    let cursor = Cursor::new(buf);
    let mut zip = zip::ZipWriter::new(cursor);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in entries {
        zip.start_file(name, opts)?;
        zip.write_all(&bytes)?;
    }
    let cursor = zip.finish()?;
    Ok((cursor.into_inner(), counts))
}

/// Apply text edits to a Word document.xml. We extract the *visible text*
/// across `<w:t>…</w:t>` ranges, run each find/replace in order against the
/// concatenated visible text, then write the result back as a single
/// replacement run inside the first text element of each affected paragraph.
///
/// This is intentionally simple — sufficient for word-level substitutions
/// the LLM proposes; not a structured editor for tables/numbering.
fn patch_document_xml(xml: &str, edits: &[DocxEdit]) -> (String, Vec<usize>) {
    let mut counts = vec![0usize; edits.len()];
    let mut working = xml.to_string();

    for (idx, ed) in edits.iter().enumerate() {
        let needle_xml = xml_escape_static(&ed.find);
        let replacement_xml = xml_escape_static(&ed.replace);
        // Try literal escaped match first (exact substring already xml-escaped).
        let mut start = 0usize;
        let mut hits = 0usize;
        while let Some(pos) = working[start..].find(&needle_xml) {
            let abs = start + pos;
            working.replace_range(abs..abs + needle_xml.len(), &replacement_xml);
            hits += 1;
            start = abs + replacement_xml.len();
        }

        // If literal didn't match, fall back to a tolerant search inside
        // visible text only (concatenate <w:t> nodes, find, then patch).
        if hits == 0 {
            if let Some(new_xml) = tolerant_replace_in_runs(&working, &ed.find, &ed.replace) {
                working = new_xml;
                hits = 1;
            }
        }
        counts[idx] = hits;
    }
    (working, counts)
}

fn xml_escape_static(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
    out
}

/// If literal substring fails, try to match across `<w:t>` runs. Best-effort:
/// concatenate visible text, find first occurrence, and replace it by
/// rewriting the affected runs (collapsing them into a single one).
fn tolerant_replace_in_runs(xml: &str, find: &str, replace: &str) -> Option<String> {
    let needle = find.split_whitespace().collect::<Vec<_>>().join(" ");
    if needle.is_empty() { return None; }

    // Build (start, end, text) for every <w:t> ... </w:t>
    let mut runs: Vec<(usize, usize, String)> = Vec::new();
    let mut search_from = 0;
    while let Some(open) = xml[search_from..].find("<w:t") {
        let abs_open = search_from + open;
        // close of the opening tag
        let after_open = xml[abs_open..].find('>').map(|p| abs_open + p + 1)?;
        let close = xml[after_open..].find("</w:t>").map(|p| after_open + p)?;
        let raw = &xml[after_open..close];
        runs.push((after_open, close, html_unescape(raw)));
        search_from = close + 6;
    }

    // Build the concatenated visible text and, in lockstep, a normalized
    // (whitespace-collapsed, lowercased) form together with a map from each
    // normalized byte to the originating byte offset in `combined`. The map
    // lets us project a match found in the normalized form back to a precise
    // byte range in `combined`, which we then splice across the spanned runs.
    let combined: String = runs.iter().map(|(_, _, t)| t.clone()).collect::<Vec<_>>().join("");
    let mut normalized = String::with_capacity(combined.len());
    // norm_to_combined[i] = combined byte offset of the char that produced the
    // normalized byte at index i. Plus a final sentinel = combined.len().
    let mut norm_to_combined: Vec<usize> = Vec::with_capacity(combined.len() + 1);
    let mut last_was_space = false;
    for (i, c) in combined.char_indices() {
        if c.is_whitespace() {
            if !last_was_space {
                norm_to_combined.push(i);
                normalized.push(' ');
                last_was_space = true;
            }
        } else {
            for lc in c.to_lowercase() {
                for _ in 0..lc.len_utf8() {
                    norm_to_combined.push(i);
                }
                normalized.push(lc);
            }
            last_was_space = false;
        }
    }
    norm_to_combined.push(combined.len()); // sentinel for the end position

    let needle_norm = needle.to_lowercase();
    let pos = normalized.find(&needle_norm)?;
    let end = pos + needle_norm.len();

    // Project the normalized match span onto byte offsets in `combined`.
    let combined_start = norm_to_combined[pos];
    let combined_end = norm_to_combined[end];

    // Determine which runs the [combined_start, combined_end) span touches and
    // splice the replacement across exactly those runs: the replacement text
    // goes into the first spanned run (covering its overlapped portion), and
    // the overlapped portion of every other spanned run is cleared. Each run's
    // text before/after the span is preserved. If the span maps onto a single
    // run we still produce a correct in-run substitution.
    let mut run_offset = 0usize; // byte offset of this run's start within combined
    let mut new_xml = String::with_capacity(xml.len() + replace.len());
    let mut last_copied = 0usize; // byte offset in `xml` copied so far
    let mut wrote_replacement = false;
    let mut touched_any = false;

    for (open, close, text) in &runs {
        let run_start = run_offset;
        let run_end = run_offset + text.len();
        run_offset = run_end;

        // Does this run overlap the match span?
        let ov_start = combined_start.max(run_start);
        let ov_end = combined_end.min(run_end);
        if ov_start >= ov_end {
            continue; // no overlap — leave this run untouched
        }
        touched_any = true;

        // Local (within-run) byte range of the overlap.
        let local_start = ov_start - run_start;
        let local_end = ov_end - run_start;
        let before = &text[..local_start];
        let after = &text[local_end..];

        // Rebuild this run's inner text: kept prefix + (replacement only in the
        // first spanned run) + kept suffix.
        let mut inner = String::new();
        inner.push_str(before);
        if !wrote_replacement {
            inner.push_str(replace);
            wrote_replacement = true;
        }
        inner.push_str(after);

        new_xml.push_str(&xml[last_copied..*open]);
        new_xml.push_str(&xml_escape_static(&inner));
        last_copied = *close;
    }

    if !touched_any {
        return None;
    }
    new_xml.push_str(&xml[last_copied..]);
    Some(new_xml)
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

// ---------------------------------------------------------------------------
// tracked changes — w:ins / w:del based editing
// ---------------------------------------------------------------------------

/// Apply edits as Word tracked changes (w:ins/w:del) instead of direct
/// replacement. The resulting docx renders with redlines when opened with
/// `renderChanges: true` in docx-preview.
pub fn apply_tracked_edits(original: &[u8], edits: &[DocxEdit]) -> Result<TrackedChangeResult> {
    let cursor = Cursor::new(original.to_vec());
    let mut archive = zip::ZipArchive::new(cursor)?;

    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..archive.len() {
        let mut f = archive.by_index(i)?;
        let name = f.name().to_string();
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        entries.push((name, buf));
    }

    let mut changes = Vec::new();
    for (name, bytes) in entries.iter_mut() {
        if name == "word/document.xml" {
            let xml = String::from_utf8_lossy(bytes).into_owned();
            let (new_xml, tc) = patch_document_xml_tracked(&xml, edits);
            changes = tc;
            *bytes = new_xml.into_bytes();
        }
    }

    let buf = Vec::new();
    let cursor = Cursor::new(buf);
    let mut zip = zip::ZipWriter::new(cursor);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in entries {
        zip.start_file(name, opts)?;
        zip.write_all(&bytes)?;
    }
    let cursor = zip.finish()?;
    Ok(TrackedChangeResult { bytes: cursor.into_inner(), changes })
}

fn patch_document_xml_tracked(xml: &str, edits: &[DocxEdit]) -> (String, Vec<TrackedChange>) {
    let mut changes = Vec::new();
    let mut working = xml.to_string();
    let mut next_id = find_max_w_id(&working) + 1;
    let timestamp = "2025-01-01T00:00:00Z";

    for ed in edits {
        // Pure insertion: no anchor text, append the clause as a tracked <w:ins>.
        if ed.find.is_empty() {
            if ed.replace.is_empty() { continue; }
            if let Some((new_xml, tcs)) = insert_tracked_paragraph(
                &working, &ed.replace, ed.format.as_deref(), &mut next_id, timestamp,
            ) {
                working = new_xml;
                changes.extend(tcs);
            }
            continue;
        }

        // Format-only reformat (same text, new run properties): a word diff
        // would be a no-op, so keep the whole-run del+ins path.
        if ed.find == ed.replace {
            if let Some((new_xml, change)) = tracked_replace_in_run(
                &working, &ed.find, &ed.replace, ed.format.as_deref(), &mut next_id,
            ) {
                working = new_xml;
                changes.push(change);
            }
            continue;
        }

        // Word-level diff: lands across run splits and marks only changed words.
        if let Some((new_xml, tcs)) = diff_replace_span(
            &working, &ed.find, &ed.replace, ed.format.as_deref(), &mut next_id, timestamp,
        ) {
            working = new_xml;
            changes.extend(tcs);
            continue;
        }

        // Fallback: whole-run tolerant replace (case-insensitive, cross-run).
        // Keeps edits landing when `find` is not an exact substring.
        if let Some((new_xml, change)) = tracked_replace_in_run(
            &working, &ed.find, &ed.replace, ed.format.as_deref(), &mut next_id,
        ) {
            working = new_xml;
            changes.push(change);
        }
        // else: clause not found -> no change pushed (caller surfaces the error)
    }

    (working, changes)
}

/// Word-level diff of `original` -> `replace`, emitted as tracked-change OOXML.
/// Unchanged words become plain <w:r> (preserving `rpr`); deleted words go into
/// <w:del>, inserted words into <w:ins> (with optional `format`). Returns the
/// XML fragment plus one TrackedChange per del/ins hunk. A pure insertion
/// (empty `original`) yields ins-only hunks with no <w:del>.
fn apply_revision_tracked(
    original: &str,
    replace: &str,
    rpr: &str,
    format: Option<&str>,
    next_id: &mut u32,
    timestamp: &str,
) -> (String, Vec<TrackedChange>) {
    use similar::{ChangeTag, TextDiff};

    let ins_rpr = build_ins_rpr(
        &(if rpr.is_empty() { None } else { Some(rpr.to_string()) }),
        format,
    );

    let diff = TextDiff::from_words(original, replace);
    let mut out = String::new();
    let mut changes = Vec::new();
    let mut eq = String::new();
    let mut del = String::new();
    let mut ins = String::new();

    // Emit accumulated unchanged words as a plain run.
    let flush_eq = |out: &mut String, eq: &mut String| {
        if !eq.is_empty() {
            out.push_str(&format!(
                "<w:r>{}<w:t xml:space=\"preserve\">{}</w:t></w:r>",
                rpr,
                xml_escape_static(eq)
            ));
            eq.clear();
        }
    };
    // Emit an accumulated del/ins hunk and record one TrackedChange.
    let flush_hunk = |out: &mut String,
                      changes: &mut Vec<TrackedChange>,
                      del: &mut String,
                      ins: &mut String,
                      next_id: &mut u32| {
        if del.is_empty() && ins.is_empty() {
            return;
        }
        let del_id = if !del.is_empty() {
            let id = *next_id;
            *next_id += 1;
            Some(id)
        } else {
            None
        };
        let ins_id = if !ins.is_empty() {
            let id = *next_id;
            *next_id += 1;
            Some(id)
        } else {
            None
        };
        if let Some(id) = del_id {
            out.push_str(&format!(
                "<w:del w:id=\"{}\" w:author=\"Mike\" w:date=\"{}\"><w:r>{}<w:delText xml:space=\"preserve\">{}</w:delText></w:r></w:del>",
                id, timestamp, rpr, xml_escape_static(del)
            ));
        }
        if let Some(id) = ins_id {
            out.push_str(&format!(
                "<w:ins w:id=\"{}\" w:author=\"Mike\" w:date=\"{}\"><w:r>{}<w:t xml:space=\"preserve\">{}</w:t></w:r></w:ins>",
                id, timestamp, ins_rpr, xml_escape_static(ins)
            ));
        }
        changes.push(TrackedChange {
            del_w_id: del_id.map(|i| i.to_string()),
            ins_w_id: ins_id.map(|i| i.to_string()),
            deleted_text: del.clone(),
            inserted_text: ins.clone(),
        });
        del.clear();
        ins.clear();
    };

    for ch in diff.iter_all_changes() {
        match ch.tag() {
            ChangeTag::Equal => {
                flush_hunk(&mut out, &mut changes, &mut del, &mut ins, next_id);
                eq.push_str(ch.value());
            }
            ChangeTag::Delete => {
                flush_eq(&mut out, &mut eq);
                del.push_str(ch.value());
            }
            ChangeTag::Insert => {
                flush_eq(&mut out, &mut eq);
                ins.push_str(ch.value());
            }
        }
    }
    flush_hunk(&mut out, &mut changes, &mut del, &mut ins, next_id);
    flush_eq(&mut out, &mut eq);

    (out, changes)
}

/// Locate `find` across one or more runs and replace that span with a
/// word-level tracked-change diff. Returns None if `find` is not an exact
/// substring of the concatenated run text (the caller then falls back).
fn diff_replace_span(
    xml: &str,
    find: &str,
    replace: &str,
    format: Option<&str>,
    next_id: &mut u32,
    timestamp: &str,
) -> Option<(String, Vec<TrackedChange>)> {
    // Collect runs as (start, end, rpr, unescaped_text).
    let mut runs: Vec<(usize, usize, String, String)> = Vec::new();
    let mut pos = 0;
    while let Some(r_start) = next_run_start(xml, pos) {
        let r_end = run_end_after(xml, r_start)?;
        let run_xml = &xml[r_start..r_end];
        let rpr = extract_rpr(run_xml).unwrap_or_default();
        let text = html_unescape(&run_visible_text(run_xml));
        runs.push((r_start, r_end, rpr, text));
        pos = r_end;
    }
    if runs.is_empty() {
        return None;
    }

    // Concatenated visible text + per-run start offsets within it.
    let mut combined = String::new();
    let mut starts: Vec<usize> = Vec::with_capacity(runs.len());
    for (_, _, _, t) in &runs {
        starts.push(combined.len());
        combined.push_str(t);
    }

    let mpos = combined.find(find)?;
    let mend = mpos + find.len();

    // First run containing the match start; last run containing the match end.
    let mut first_idx = 0;
    let mut local_start = 0;
    for (i, (_, _, _, t)) in runs.iter().enumerate() {
        let s = starts[i];
        let e = s + t.len();
        if mpos >= s && mpos < e {
            first_idx = i;
            local_start = mpos - s;
            break;
        }
    }
    let mut last_idx = runs.len() - 1;
    let mut local_end = runs[last_idx].3.len();
    for (i, (_, _, _, t)) in runs.iter().enumerate() {
        let s = starts[i];
        let e = s + t.len();
        if mend > s && mend <= e {
            last_idx = i;
            local_end = mend - s;
            break;
        }
    }

    // Safety: never splice across a paragraph boundary. Runs are concatenated
    // doc-wide with no separator, so a `find` straddling two <w:p> would
    // otherwise delete the intervening </w:p><w:p> structure and corrupt the
    // document. Bail to the fallback (which only ever rewrites within a run).
    if last_idx > first_idx
        && xml[runs[first_idx].1..runs[last_idx].0].contains("</w:p>")
    {
        return None;
    }

    let first_rpr = runs[first_idx].2.clone();
    let last_rpr = runs[last_idx].2.clone();
    let before = &runs[first_idx].3[..local_start];
    let after = &runs[last_idx].3[local_end..];

    let mut fragment = String::new();
    if !before.is_empty() {
        fragment.push_str(&format!(
            "<w:r>{}<w:t xml:space=\"preserve\">{}</w:t></w:r>",
            first_rpr,
            xml_escape_static(before)
        ));
    }
    let (diff_xml, changes) =
        apply_revision_tracked(find, replace, &first_rpr, format, next_id, timestamp);
    fragment.push_str(&diff_xml);
    if !after.is_empty() {
        fragment.push_str(&format!(
            "<w:r>{}<w:t xml:space=\"preserve\">{}</w:t></w:r>",
            last_rpr,
            xml_escape_static(after)
        ));
    }
    if changes.is_empty() {
        return None;
    }

    let span_start = runs[first_idx].0;
    let span_end = runs[last_idx].1;
    let mut new_xml = String::with_capacity(xml.len() + fragment.len());
    new_xml.push_str(&xml[..span_start]);
    new_xml.push_str(&fragment);
    new_xml.push_str(&xml[span_end..]);
    Some((new_xml, changes))
}

/// Append `replace` as a new tracked-insertion paragraph before the section
/// properties (or the end of the body). Pure insertion: <w:ins> only, no del.
fn insert_tracked_paragraph(
    xml: &str,
    replace: &str,
    format: Option<&str>,
    next_id: &mut u32,
    timestamp: &str,
) -> Option<(String, Vec<TrackedChange>)> {
    let ins_id = *next_id;
    *next_id += 1;
    let ins_rpr = build_ins_rpr(&None, format);
    let para = format!(
        "<w:p><w:ins w:id=\"{}\" w:author=\"Mike\" w:date=\"{}\"><w:r>{}<w:t xml:space=\"preserve\">{}</w:t></w:r></w:ins></w:p>",
        ins_id, timestamp, ins_rpr, xml_escape_static(replace)
    );
    let insert_at = xml.find("<w:sectPr").or_else(|| xml.find("</w:body>"))?;
    let mut new_xml = String::with_capacity(xml.len() + para.len());
    new_xml.push_str(&xml[..insert_at]);
    new_xml.push_str(&para);
    new_xml.push_str(&xml[insert_at..]);
    Some((
        new_xml,
        vec![TrackedChange {
            del_w_id: None,
            ins_w_id: Some(ins_id.to_string()),
            deleted_text: String::new(),
            inserted_text: replace.to_string(),
        }],
    ))
}

/// Find the highest w:id value in the XML so new IDs don't collide.
fn find_max_w_id(xml: &str) -> u32 {
    let mut max = 0u32;
    let pat = "w:id=\"";
    let mut pos = 0;
    while let Some(idx) = xml[pos..].find(pat) {
        let start = pos + idx + pat.len();
        if let Some(end) = xml[start..].find('"') {
            if let Ok(id) = xml[start..start + end].parse::<u32>() {
                max = max.max(id);
            }
        }
        pos = start;
    }
    max
}

/// Find the next <w:r> (not <w:rPr>) start position at or after `pos`.
fn next_run_start(xml: &str, pos: usize) -> Option<usize> {
    let slice = &xml[pos..];
    let mut search = 0;
    loop {
        let idx = slice[search..].find("<w:r")?;
        let abs = search + idx;
        let next = *slice.as_bytes().get(abs + 4)?;
        if next == b'>' || next == b' ' {
            return Some(pos + abs);
        }
        search = abs + 4;
    }
}

/// Find the </w:r> closing tag after `pos`.
fn run_end_after(xml: &str, pos: usize) -> Option<usize> {
    xml[pos..].find("</w:r>").map(|p| pos + p + 6)
}

/// Extract concatenated <w:t> text from a run's raw XML (still XML-escaped).
fn run_visible_text(run_xml: &str) -> String {
    let mut text = String::new();
    let mut search = 0;
    while let Some(open) = run_xml[search..].find("<w:t") {
        let abs = search + open;
        let after = match run_xml[abs..].find('>') {
            Some(p) => abs + p + 1,
            None => break,
        };
        let close = match run_xml[after..].find("</w:t>") {
            Some(p) => after + p,
            None => break,
        };
        text.push_str(&run_xml[after..close]);
        search = close + 6;
    }
    text
}

/// Extract <w:rPr>...</w:rPr> from a run, if present.
fn extract_rpr(run_xml: &str) -> Option<String> {
    let start = run_xml.find("<w:rPr")?;
    let end_tag = run_xml[start..].find("</w:rPr>")?;
    Some(run_xml[start..start + end_tag + 8].to_string())
}

/// Build the <w:rPr> for the insertion, optionally adding formatting.
fn build_ins_rpr(base_rpr: &Option<String>, format: Option<&str>) -> String {
    let fmt_tag = match format {
        Some("bold") => "<w:b/>",
        Some("italic") => "<w:i/>",
        Some("underline") => "<w:u w:val=\"single\"/>",
        _ => return base_rpr.clone().unwrap_or_default(),
    };
    match base_rpr {
        Some(rpr) if rpr.contains("</w:rPr>") => {
            // Insert format tag right after <w:rPr...>
            let insert_pos = rpr.find('>').unwrap() + 1;
            format!("{}{}{}", &rpr[..insert_pos], fmt_tag, &rpr[insert_pos..])
        }
        _ => format!("<w:rPr>{}</w:rPr>", fmt_tag),
    }
}

/// Core tracked-change replacement. Finds `find` text in a <w:r> run,
/// wraps it in <w:del>, and inserts <w:ins> with the replacement.
fn tracked_replace_in_run(
    xml: &str,
    find: &str,
    replace: &str,
    format: Option<&str>,
    next_id: &mut u32,
) -> Option<(String, TrackedChange)> {
    let needle = xml_escape_static(find);
    let replacement = xml_escape_static(replace);
    let timestamp = "2025-01-01T00:00:00Z";

    // Scan runs for one containing the needle
    let mut pos = 0;
    while let Some(r_start) = next_run_start(xml, pos) {
        let r_end = run_end_after(xml, r_start)?;
        let run_xml = &xml[r_start..r_end];
        let text = run_visible_text(run_xml);

        if let Some(match_pos) = text.find(&needle) {
            let rpr = extract_rpr(run_xml);
            let rpr_str = rpr.as_deref().unwrap_or("");
            let ins_rpr = build_ins_rpr(&rpr, format);

            let del_id = *next_id;
            let ins_id = *next_id + 1;
            *next_id += 2;

            let before = &text[..match_pos];
            let after = &text[match_pos + needle.len()..];

            let mut out = String::new();

            if !before.is_empty() {
                out.push_str(&format!(
                    "<w:r>{}<w:t xml:space=\"preserve\">{}</w:t></w:r>",
                    rpr_str, before
                ));
            }

            out.push_str(&format!(
                "<w:del w:id=\"{}\" w:author=\"Mike\" w:date=\"{}\">\
                 <w:r>{}<w:delText xml:space=\"preserve\">{}</w:delText></w:r>\
                 </w:del>",
                del_id, timestamp, rpr_str, needle
            ));

            if !replace.is_empty() {
                out.push_str(&format!(
                    "<w:ins w:id=\"{}\" w:author=\"Mike\" w:date=\"{}\">\
                     <w:r>{}<w:t xml:space=\"preserve\">{}</w:t></w:r>\
                     </w:ins>",
                    ins_id, timestamp, ins_rpr, replacement
                ));
            }

            if !after.is_empty() {
                out.push_str(&format!(
                    "<w:r>{}<w:t xml:space=\"preserve\">{}</w:t></w:r>",
                    rpr_str, after
                ));
            }

            let mut new_xml = String::with_capacity(xml.len() + out.len());
            new_xml.push_str(&xml[..r_start]);
            new_xml.push_str(&out);
            new_xml.push_str(&xml[r_end..]);

            return Some((new_xml, TrackedChange {
                del_w_id: Some(del_id.to_string()),
                ins_w_id: if replace.is_empty() { None } else { Some(ins_id.to_string()) },
                deleted_text: find.to_string(),
                inserted_text: replace.to_string(),
            }));
        }

        pos = r_end;
    }

    // Fallback: case-insensitive tolerant search across runs
    tolerant_tracked_replace(xml, find, replace, format, next_id, timestamp)
}

/// Tolerant fallback: case-insensitive search across concatenated run text.
fn tolerant_tracked_replace(
    xml: &str,
    find: &str,
    replace: &str,
    format: Option<&str>,
    next_id: &mut u32,
    timestamp: &str,
) -> Option<(String, TrackedChange)> {
    let needle = find.split_whitespace().collect::<Vec<_>>().join(" ");
    if needle.is_empty() { return None; }

    // Find the first <w:r> whose visible text contains the needle
    let mut pos = 0;
    while let Some(r_start) = next_run_start(xml, pos) {
        let r_end = run_end_after(xml, r_start)?;
        let run_xml = &xml[r_start..r_end];
        let text = html_unescape(&run_visible_text(run_xml));

        if text.to_lowercase().contains(&needle.to_lowercase()) {
            let rpr = extract_rpr(run_xml);
            let rpr_str = rpr.as_deref().unwrap_or("");
            let ins_rpr = build_ins_rpr(&rpr, format);
            let del_id = *next_id;
            let ins_id = *next_id + 1;
            *next_id += 2;

            let needle_esc = xml_escape_static(find);
            let replacement_esc = xml_escape_static(replace);

            let mut out = String::new();
            out.push_str(&format!(
                "<w:del w:id=\"{}\" w:author=\"Mike\" w:date=\"{}\">\
                 <w:r>{}<w:delText xml:space=\"preserve\">{}</w:delText></w:r>\
                 </w:del>",
                del_id, timestamp, rpr_str, needle_esc
            ));
            if !replace.is_empty() {
                out.push_str(&format!(
                    "<w:ins w:id=\"{}\" w:author=\"Mike\" w:date=\"{}\">\
                     <w:r>{}<w:t xml:space=\"preserve\">{}</w:t></w:r>\
                     </w:ins>",
                    ins_id, timestamp, ins_rpr, replacement_esc
                ));
            }

            let mut new_xml = String::with_capacity(xml.len() + out.len());
            new_xml.push_str(&xml[..r_start]);
            new_xml.push_str(&out);
            new_xml.push_str(&xml[r_end..]);

            return Some((new_xml, TrackedChange {
                del_w_id: Some(del_id.to_string()),
                ins_w_id: if replace.is_empty() { None } else { Some(ins_id.to_string()) },
                deleted_text: find.to_string(),
                inserted_text: replace.to_string(),
            }));
        }

        pos = r_end;
    }

    None
}

/// Accept or reject a single tracked change by w:id. Returns modified docx
/// bytes or None if the change was not found.
pub fn resolve_tracked_change(
    original: &[u8],
    w_id: &str,
    accept: bool,
) -> Result<Option<Vec<u8>>> {
    let cursor = Cursor::new(original.to_vec());
    let mut archive = zip::ZipArchive::new(cursor)?;

    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..archive.len() {
        let mut f = archive.by_index(i)?;
        let name = f.name().to_string();
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        entries.push((name, buf));
    }

    let mut found = false;
    for (name, bytes) in entries.iter_mut() {
        if name == "word/document.xml" {
            let xml = String::from_utf8_lossy(bytes).into_owned();
            if let Some(new_xml) = resolve_change_in_xml(&xml, w_id, accept) {
                *bytes = new_xml.into_bytes();
                found = true;
            }
        }
    }

    if !found { return Ok(None); }

    let buf = Vec::new();
    let cursor = Cursor::new(buf);
    let mut zip = zip::ZipWriter::new(cursor);
    let opts: zip::write::SimpleFileOptions =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in entries {
        zip.start_file(name, opts)?;
        zip.write_all(&bytes)?;
    }
    let cursor = zip.finish()?;
    Ok(Some(cursor.into_inner()))
}

/// Resolve a tracked change in the XML.
/// Accept: remove <w:del>, unwrap <w:ins> (keep content).
/// Reject: remove <w:ins>, unwrap <w:del> (convert delText→t).
fn resolve_change_in_xml(xml: &str, w_id: &str, accept: bool) -> Option<String> {
    let id_attr = format!("w:id=\"{}\"", w_id);
    let mut working = xml.to_string();

    // Find the element containing this w:id
    let pos = working.find(&id_attr)?;

    // Walk backwards to find the closest opening <w:del or <w:ins
    let before = &working[..pos];
    let del_pos = before.rfind("<w:del ");
    let ins_pos = before.rfind("<w:ins ");
    let tag_start = match (del_pos, ins_pos) {
        (Some(d), Some(i)) => d.max(i),
        (Some(d), None) => d,
        (None, Some(i)) => i,
        (None, None) => return None,
    };
    let tag_name = if working[tag_start..].starts_with("<w:del") { "del" } else { "ins" };
    let close_tag = format!("</w:{}>", tag_name);
    let tag_end = working[tag_start..].find(&close_tag)
        .map(|p| tag_start + p + close_tag.len())?;

    let element = &working[tag_start..tag_end];

    if accept {
        if tag_name == "del" {
            // Accept: remove the entire <w:del> block
            working.replace_range(tag_start..tag_end, "");
        } else {
            // Accept: unwrap <w:ins> — keep inner content
            let inner = extract_inner_content(element, "ins");
            working.replace_range(tag_start..tag_end, &inner);
        }
    } else {
        if tag_name == "ins" {
            // Reject: remove the entire <w:ins> block
            working.replace_range(tag_start..tag_end, "");
        } else {
            // Reject: unwrap <w:del> — keep content, convert delText→t
            let inner = extract_inner_content(element, "del")
                .replace("<w:delText", "<w:t")
                .replace("</w:delText>", "</w:t>");
            working.replace_range(tag_start..tag_end, &inner);
        }
    }

    Some(working)
}

/// Extract the inner content of a tracked change element (everything between
/// the opening and closing tags).
fn extract_inner_content(element: &str, tag_name: &str) -> String {
    let open_end = element.find('>').unwrap_or(0) + 1;
    let close_start = element.rfind(&format!("</w:{}>", tag_name)).unwrap_or(element.len());
    element[open_end..close_start].to_string()
}

/// Extract ordered tracked change IDs from document XML.
pub fn extract_tracked_change_ids(docx_bytes: &[u8]) -> Result<Vec<(String, String)>> {
    let cursor = Cursor::new(docx_bytes.to_vec());
    let mut archive = zip::ZipArchive::new(cursor)?;

    for i in 0..archive.len() {
        let mut f = archive.by_index(i)?;
        if f.name() == "word/document.xml" {
            let mut xml = String::new();
            f.read_to_string(&mut xml)?;
            return Ok(extract_change_ids_from_xml(&xml));
        }
    }
    Ok(Vec::new())
}

fn extract_change_ids_from_xml(xml: &str) -> Vec<(String, String)> {
    let mut ids = Vec::new();
    let mut pos = 0;
    while pos < xml.len() {
        let del_pos = xml[pos..].find("<w:del ");
        let ins_pos = xml[pos..].find("<w:ins ");
        let (kind, found_pos) = match (del_pos, ins_pos) {
            (Some(d), Some(i)) if d < i => ("del", pos + d),
            (Some(_), Some(i)) => ("ins", pos + i),
            (Some(d), None) => ("del", pos + d),
            (None, Some(i)) => ("ins", pos + i),
            (None, None) => break,
        };
        let id_pat = "w:id=\"";
        if let Some(id_start) = xml[found_pos..].find(id_pat) {
            let abs = found_pos + id_start + id_pat.len();
            if let Some(id_end) = xml[abs..].find('"') {
                ids.push((kind.to_string(), xml[abs..abs + id_end].to_string()));
            }
        }
        pos = found_pos + 6;
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracked_edit_wraps_in_del_ins() {
        let xml = r#"<w:body><w:p><w:r><w:t xml:space="preserve">Hello World</w:t></w:r></w:p></w:body>"#;
        let edits = vec![DocxEdit {
            find: "Hello".to_string(),
            replace: "Hi".to_string(),
            format: None,
        }];
        let (result, changes) = patch_document_xml_tracked(xml, &edits);
        assert_eq!(changes.len(), 1);
        assert!(result.contains("<w:del "), "should contain w:del");
        assert!(result.contains("<w:ins "), "should contain w:ins");
        assert!(result.contains("<w:delText"), "should contain delText");
        assert!(result.contains("Hello"), "should keep old text in del");
        assert!(result.contains("Hi"), "should have new text in ins");
    }

    #[test]
    fn tracked_edit_with_format_adds_bold() {
        let xml = r#"<w:body><w:p><w:r><w:t xml:space="preserve">PRAYER</w:t></w:r></w:p></w:body>"#;
        let edits = vec![DocxEdit {
            find: "PRAYER".to_string(),
            replace: "PRAYER".to_string(),
            format: Some("bold".to_string()),
        }];
        let (result, changes) = patch_document_xml_tracked(xml, &edits);
        assert_eq!(changes.len(), 1);
        assert!(result.contains("<w:b/>"), "should contain bold tag in ins");
    }

    #[test]
    fn tracked_edit_empty_replace_is_deletion_only() {
        let xml = r#"<w:body><w:p><w:r><w:t xml:space="preserve">remove this text</w:t></w:r></w:p></w:body>"#;
        let edits = vec![DocxEdit {
            find: "remove this text".to_string(),
            replace: String::new(),
            format: None,
        }];
        let (result, changes) = patch_document_xml_tracked(xml, &edits);
        assert_eq!(changes.len(), 1);
        assert!(result.contains("<w:del "), "should contain w:del");
        assert!(!result.contains("<w:ins "), "should NOT contain w:ins for empty replace");
        assert!(changes[0].ins_w_id.is_none());
    }

    #[test]
    fn resolve_change_accept_removes_del_unwraps_ins() {
        let xml = concat!(
            r#"<w:body><w:p>"#,
            r#"<w:del w:id="1" w:author="Mike" w:date="2025-01-01T00:00:00Z">"#,
            r#"<w:r><w:delText>old</w:delText></w:r></w:del>"#,
            r#"<w:ins w:id="2" w:author="Mike" w:date="2025-01-01T00:00:00Z">"#,
            r#"<w:r><w:t>new</w:t></w:r></w:ins>"#,
            r#"</w:p></w:body>"#,
        );
        // Accept the ins (keep new text)
        let after_accept = resolve_change_in_xml(xml, "2", true).unwrap();
        assert!(!after_accept.contains("<w:ins"), "ins should be unwrapped");
        assert!(after_accept.contains("<w:r><w:t>new</w:t></w:r>"), "new text should remain");

        // Accept also means removing the del
        let after_del = resolve_change_in_xml(xml, "1", true).unwrap();
        assert!(!after_del.contains("<w:del"), "del should be removed");
    }

    #[test]
    fn find_max_w_id_extracts_highest() {
        let xml = r#"<w:del w:id="5"><w:ins w:id="12"><w:r w:id="3">"#;
        assert_eq!(find_max_w_id(xml), 12);
    }

    #[test]
    fn extract_change_ids_ordered() {
        let xml = concat!(
            r#"<w:del w:id="1" w:author="M"><w:r></w:r></w:del>"#,
            r#"<w:ins w:id="2" w:author="M"><w:r></w:r></w:ins>"#,
        );
        let ids = extract_change_ids_from_xml(xml);
        assert_eq!(ids, vec![
            ("del".to_string(), "1".to_string()),
            ("ins".to_string(), "2".to_string()),
        ]);
    }

    // ---- tolerant_replace_in_runs: multi-run correctness, no silent corruption ----

    #[test]
    fn tolerant_replace_single_run_works() {
        // Sanity: the simple single-run case still replaces correctly.
        let xml = r#"<w:p><w:r><w:t>Hello World</w:t></w:r></w:p>"#;
        let out = tolerant_replace_in_runs(xml, "Hello World", "Hi Earth").unwrap();
        assert!(out.contains("Hi Earth"), "should contain replacement, got: {out}");
        assert!(!out.contains("Hello World"), "old text must be gone, got: {out}");
    }

    #[test]
    fn tolerant_replace_across_runs_no_silent_corruption() {
        // The needle "Hello World" spans two <w:t> runs ("Hello " + "World").
        // The buggy implementation wrote the whole replacement into the first
        // matching run and left the second ("World") intact => duplicated/wrong
        // text, yet returned Some(...) so it counted as success.
        let xml = concat!(
            r#"<w:p>"#,
            r#"<w:r><w:t xml:space="preserve">Hello </w:t></w:r>"#,
            r#"<w:r><w:t>World</w:t></w:r>"#,
            r#"</w:p>"#,
        );
        match tolerant_replace_in_runs(xml, "Hello World", "Goodbye Mars") {
            Some(out) => {
                // The visible text after replacement must equal exactly the
                // replacement — no leftover "World", no leftover "Hello".
                let visible = visible_text(&out);
                assert_eq!(
                    visible, "Goodbye Mars",
                    "multi-run replace must produce exactly the replacement, got visible: {visible:?} xml: {out}"
                );
            }
            None => {
                // Returning None is also acceptable: the caller then surfaces
                // "edit not applied" rather than silently corrupting the doc.
            }
        }
    }

    #[test]
    fn tolerant_replace_across_three_runs_no_leftover() {
        // Needle spans three runs.
        let xml = concat!(
            r#"<w:p>"#,
            r#"<w:r><w:t xml:space="preserve">The quick </w:t></w:r>"#,
            r#"<w:r><w:t xml:space="preserve">brown </w:t></w:r>"#,
            r#"<w:r><w:t>fox</w:t></w:r>"#,
            r#"</w:p>"#,
        );
        match tolerant_replace_in_runs(xml, "quick brown fox", "lazy dog") {
            Some(out) => {
                let visible = visible_text(&out);
                assert_eq!(
                    visible, "The lazy dog",
                    "spanned runs must be spliced, got visible: {visible:?} xml: {out}"
                );
            }
            None => {}
        }
    }

    #[test]
    fn tolerant_replace_across_runs_multibyte() {
        // Devanagari needle spanning two runs; replacement also multibyte.
        let xml = concat!(
            r#"<w:p>"#,
            r#"<w:r><w:t xml:space="preserve">नमस्ते </w:t></w:r>"#,
            r#"<w:r><w:t>दुनिया</w:t></w:r>"#,
            r#"</w:p>"#,
        );
        match tolerant_replace_in_runs(xml, "नमस्ते दुनिया", "धन्यवाद") {
            Some(out) => {
                let visible = visible_text(&out);
                assert_eq!(visible, "धन्यवाद", "multibyte multi-run splice, got: {visible:?}");
            }
            None => {}
        }
    }

    #[test]
    fn tolerant_replace_no_match_returns_none() {
        let xml = r#"<w:p><w:r><w:t>Hello World</w:t></w:r></w:p>"#;
        assert!(
            tolerant_replace_in_runs(xml, "absent phrase", "x").is_none(),
            "a needle not present must return None, never a corrupt Some"
        );
    }

    /// Test helper: concatenate the unescaped contents of every <w:t> element.
    fn visible_text(xml: &str) -> String {
        let mut out = String::new();
        let mut from = 0;
        while let Some(open) = xml[from..].find("<w:t") {
            let abs = from + open;
            let after = match xml[abs..].find('>') {
                Some(p) => abs + p + 1,
                None => break,
            };
            let close = match xml[after..].find("</w:t>") {
                Some(p) => after + p,
                None => break,
            };
            out.push_str(&html_unescape(&xml[after..close]));
            from = close + 6;
        }
        out
    }

    // ---- diff-driven tracked changes (apply_revision_tracked) ----

    /// Test helper: zip a minimal .docx around a document.xml body.
    fn make_docx(document_xml: &str) -> Vec<u8> {
        let buf = Vec::new();
        let mut zip = zip::ZipWriter::new(Cursor::new(buf));
        let opts: zip::write::SimpleFileOptions =
            zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("word/document.xml", opts).unwrap();
        zip.write_all(document_xml.as_bytes()).unwrap();
        zip.finish().unwrap().into_inner()
    }

    /// Test helper: visible text of a whole .docx.
    fn docx_visible_text(docx: &[u8]) -> String {
        let mut a = zip::ZipArchive::new(Cursor::new(docx.to_vec())).unwrap();
        let mut xml = String::new();
        for i in 0..a.len() {
            let mut f = a.by_index(i).unwrap();
            if f.name() == "word/document.xml" {
                f.read_to_string(&mut xml).unwrap();
            }
        }
        visible_text(&xml)
    }

    #[test]
    fn diff_minimal_redline_changes_only_one_word() {
        // 12-word clause; only "old" -> "tired" changes. Exactly one del+ins
        // pair; the other 11 words stay in plain runs.
        let xml = r#"<w:body><w:p><w:r><w:t xml:space="preserve">The quick brown fox jumps over the lazy old grey sleepy dog</w:t></w:r></w:p></w:body>"#;
        let edits = vec![DocxEdit {
            find: "The quick brown fox jumps over the lazy old grey sleepy dog".to_string(),
            replace: "The quick brown fox jumps over the lazy tired grey sleepy dog".to_string(),
            format: None,
        }];
        let (result, changes) = patch_document_xml_tracked(xml, &edits);
        assert_eq!(result.matches("<w:del ").count(), 1, "exactly one del: {result}");
        assert_eq!(result.matches("<w:ins ").count(), 1, "exactly one ins: {result}");
        assert!(result.contains("<w:delText xml:space=\"preserve\">old</w:delText>"), "del holds only the old word: {result}");
        assert!(result.contains("tired"), "ins holds the new word: {result}");
        assert_eq!(changes.len(), 1);
        // The unchanged words survive as plain text in the output.
        assert!(visible_text(&result).contains("quick brown fox"), "unchanged words preserved: {result}");
    }

    #[test]
    fn diff_run_split_edit_lands() {
        // "Hello World" is split across two runs with different rPr. The edit
        // must still land as tracked changes.
        let xml = concat!(
            r#"<w:body><w:p>"#,
            r#"<w:r><w:rPr><w:b/></w:rPr><w:t xml:space="preserve">Hello </w:t></w:r>"#,
            r#"<w:r><w:t>World</w:t></w:r>"#,
            r#"</w:p></w:body>"#,
        );
        let edits = vec![DocxEdit {
            find: "Hello World".to_string(),
            replace: "Goodbye Mars".to_string(),
            format: None,
        }];
        let (result, changes) = patch_document_xml_tracked(xml, &edits);
        assert!(result.contains("<w:ins "), "has ins: {result}");
        assert!(result.contains("<w:del "), "has del: {result}");
        assert!(result.contains("<w:delText"), "has delText: {result}");
        assert!(result.contains("Goodbye"), "new words present: {result}");
        assert!(result.contains("Mars"), "new words present: {result}");
        assert!(!changes.is_empty(), "produced at least one hunk");
    }

    #[test]
    fn pure_insertion_emits_ins_no_del() {
        // Empty find = insertion. Append a new clause as <w:ins>, no <w:del>.
        let xml = r#"<w:body><w:p><w:r><w:t>Existing clause.</w:t></w:r></w:p><w:sectPr/></w:body>"#;
        let edits = vec![DocxEdit {
            find: String::new(),
            replace: "A new verification clause.".to_string(),
            format: None,
        }];
        let (result, changes) = patch_document_xml_tracked(xml, &edits);
        assert!(result.contains("<w:ins "), "has ins: {result}");
        assert!(!result.contains("<w:del "), "no del for pure insertion: {result}");
        assert!(result.contains("A new verification clause."), "inserted text present: {result}");
        assert_eq!(changes.len(), 1);
        assert!(changes[0].del_w_id.is_none(), "insertion has no del id");
        assert!(changes[0].ins_w_id.is_some(), "insertion has an ins id");
        let ins_at = result.find("<w:ins ").unwrap();
        let sect_at = result.find("<w:sectPr").unwrap();
        assert!(ins_at < sect_at, "insertion lands before sectPr: {result}");
    }

    #[test]
    fn diff_does_not_corrupt_across_paragraphs() {
        // A find that straddles a paragraph boundary must NOT delete the
        // </w:p><w:p> structure. The writer bails (no change) rather than corrupt.
        let xml = concat!(
            r#"<w:body>"#,
            r#"<w:p><w:r><w:t xml:space="preserve">First paragraph ends here</w:t></w:r></w:p>"#,
            r#"<w:p><w:r><w:t xml:space="preserve">Second paragraph starts here</w:t></w:r></w:p>"#,
            r#"</w:body>"#,
        );
        // "hereSecond" straddles the join (runs concatenate with no separator).
        let edits = vec![DocxEdit {
            find: "hereSecond".to_string(),
            replace: "here. Brand new Second".to_string(),
            format: None,
        }];
        let (result, _changes) = patch_document_xml_tracked(xml, &edits);
        assert_eq!(result.matches("<w:p>").count(), 2, "both paragraphs preserved: {result}");
        assert_eq!(result.matches("</w:p>").count(), 2, "both paragraph closes preserved: {result}");
    }

    #[test]
    fn diff_redline_roundtrips_through_resolve() {
        let body = r#"<w:document><w:body><w:p><w:r><w:t xml:space="preserve">The cat sat on the mat</w:t></w:r></w:p></w:body></w:document>"#;
        let docx = make_docx(body);
        let edits = vec![DocxEdit {
            find: "The cat sat on the mat".to_string(),
            replace: "The dog sat on the rug".to_string(),
            format: None,
        }];
        let res = apply_tracked_edits(&docx, &edits).unwrap();
        let ids = extract_tracked_change_ids(&res.bytes).unwrap();
        assert!(!ids.is_empty(), "produced tracked-change ids");

        // Accept every change -> the revised text.
        let mut accepted = res.bytes.clone();
        for (_kind, id) in &ids {
            if let Some(b) = resolve_tracked_change(&accepted, id, true).unwrap() {
                accepted = b;
            }
        }
        assert_eq!(docx_visible_text(&accepted), "The dog sat on the rug", "accept yields revised text");

        // Reject every change -> the original text.
        let mut rejected = res.bytes.clone();
        for (_kind, id) in &ids {
            if let Some(b) = resolve_tracked_change(&rejected, id, false).unwrap() {
                rejected = b;
            }
        }
        assert_eq!(docx_visible_text(&rejected), "The cat sat on the mat", "reject yields original text");
    }
}
