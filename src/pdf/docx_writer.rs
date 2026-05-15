//! Minimal DOCX writer + in-place editor.
//!
//! - `markdown_to_docx(title, markdown)` → produces a small but valid .docx
//!   from a Markdown string. Supports headings (#, ##, ###), paragraphs,
//!   bullet/numbered lists, bold/italic emphasis, and code spans.
//! - `apply_text_edits(original, edits)` → reads an existing .docx, walks
//!   `word/document.xml`, performs find/replace inside `<w:t>` runs, and
//!   re-zips the result. Used by the `edit_document` builtin tool.

use anyhow::{anyhow, Result};
use pulldown_cmark::{Event as MdEvent, HeadingLevel, Parser, Tag, TagEnd};
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
</Types>"#;

const RELS_ROOT: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;

const RELS_DOC: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
</Relationships>"#;

const STYLES_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
  <w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:spacing w:before="240" w:after="120"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="36"/></w:rPr></w:style>
  <w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:spacing w:before="200" w:after="80"/><w:outlineLvl w:val="1"/></w:pPr><w:rPr><w:b/><w:sz w:val="30"/></w:rPr></w:style>
  <w:style w:type="paragraph" w:styleId="Heading3"><w:name w:val="heading 3"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:spacing w:before="160" w:after="60"/><w:outlineLvl w:val="2"/></w:pPr><w:rPr><w:b/><w:sz w:val="26"/></w:rPr></w:style>
  <w:style w:type="paragraph" w:styleId="ListBullet"><w:name w:val="List Bullet"/><w:basedOn w:val="Normal"/><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr></w:style>
  <w:style w:type="paragraph" w:styleId="ListNumber"><w:name w:val="List Number"/><w:basedOn w:val="Normal"/><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="2"/></w:numPr></w:pPr></w:style>
</w:styles>"#;

/// Produce a DOCX byte buffer from `markdown`. `title` is currently used
/// only as the implicit Heading 1 prepended at the top.
pub fn markdown_to_docx(title: &str, markdown: &str) -> Result<Vec<u8>> {
    let body_xml = render_markdown_to_wml(title, markdown);
    let document_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
{body_xml}    <w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"/></w:sectPr>
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

    let parser = Parser::new(markdown);
    let mut current_runs: Vec<String> = Vec::new();
    let mut current_style: Option<&str> = None;
    let mut bold = false;
    let mut italic = false;
    let mut in_code_block = false;

    let flush_paragraph = |runs: &mut Vec<String>, style: Option<&str>, out: &mut String| {
        if !runs.is_empty() {
            let style = style.unwrap_or("Normal");
            out.push_str(&para(style, runs));
            runs.clear();
        }
    };

    for ev in parser {
        match ev {
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
            MdEvent::Start(Tag::List(Some(_))) => { /* numbered */ }
            MdEvent::Start(Tag::List(None))    => { /* bullet */ }
            MdEvent::End(TagEnd::List(_)) => {}
            MdEvent::Start(Tag::Item) => { current_style = Some("ListBullet"); }
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

fn para(style: &str, runs: &[String]) -> String {
    let mut s = String::new();
    s.push_str("    <w:p>");
    s.push_str(&format!(r#"<w:pPr><w:pStyle w:val="{style}"/></w:pPr>"#));
    for r in runs { s.push_str(r); }
    s.push_str("</w:p>\n");
    s
}

fn run(text: &str, bold: bool, italic: bool, mono: bool) -> String {
    let mut props = String::new();
    if bold { props.push_str("<w:b/>"); }
    if italic { props.push_str("<w:i/>"); }
    if mono { props.push_str(r#"<w:rFonts w:ascii="Courier New" w:hAnsi="Courier New"/>"#); }
    let rpr = if !props.is_empty() { format!("<w:rPr>{props}</w:rPr>") } else { String::new() };
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

    let combined: String = runs.iter().map(|(_, _, t)| t.clone()).collect::<Vec<_>>().join("");
    let normalized: String = combined.split_whitespace().collect::<Vec<_>>().join(" ");
    let pos = normalized.to_lowercase().find(&needle.to_lowercase())?;

    // Map pos in normalized back to position in combined (approximate by
    // removing one whitespace at a time until lengths align).
    let mut combined_pos = 0usize;
    let mut norm_walk = 0usize;
    let mut last_was_space = false;
    for (i, c) in combined.char_indices() {
        if norm_walk == pos {
            combined_pos = i;
            break;
        }
        if c.is_whitespace() {
            if !last_was_space {
                norm_walk += 1;
                last_was_space = true;
            }
        } else {
            norm_walk += c.len_utf8();
            last_was_space = false;
        }
    }
    let _ = combined_pos; // we don't need exact byte-precision below

    // Pragmatic: replace first whole run that contains a substring of the
    // needle, write `replace` into it, and clear the others involved.
    // Acceptable for the LLM-proposed edits which usually fit in one run.
    let needle_lower = needle.to_lowercase();
    for (open, close, text) in &runs {
        if text.to_lowercase().contains(&needle_lower)
            || (text.len() < needle.len() && needle_lower.contains(&text.to_lowercase()))
        {
            let mut new_xml = String::with_capacity(xml.len());
            new_xml.push_str(&xml[..*open]);
            new_xml.push_str(&xml_escape_static(replace));
            new_xml.push_str(&xml[*close..]);
            return Some(new_xml);
        }
    }
    None
}

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}
