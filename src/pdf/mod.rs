pub mod docx_writer;

#[cfg(feature = "pdf")]
pub mod md_pdf;

use anyhow::Result;

// ---------------------------------------------------------------------------
// PDF extraction via pdfium — requires feature "pdf" + bundled pdfium DLL
// ---------------------------------------------------------------------------

#[cfg(feature = "pdf")]
use anyhow::anyhow;
#[cfg(feature = "pdf")]
use pdfium_render::prelude::*;
#[cfg(feature = "pdf")]
use std::path::Path;

#[cfg(feature = "pdf")]
const OCR_FALLBACK_THRESHOLD: usize = 10;

/// A page "needs OCR" when its extractable text has fewer alphanumeric
/// characters than the fallback threshold — i.e. there is effectively no
/// usable text layer (a scanned/image-only page). Used both to decide whether
/// to *attempt* OCR and, after OCR, to record whether usable text was actually
/// recovered so downstream consumers can detect a still-scanned document.
#[cfg(feature = "pdf")]
fn text_needs_ocr(text: &str) -> bool {
    text.chars().filter(|c| c.is_alphanumeric()).count() < OCR_FALLBACK_THRESHOLD
}

/// Load pdfium from the bundled DLL in libs/pdfium/.
///
/// Looks (in order):
///   1. `$PDFIUM_DYNAMIC_LIB_PATH` if set (explicit override).
///   2. `<exe_dir>/libs/pdfium/`
///   3. `<cwd>/libs/pdfium/`
///   4. Each ancestor of `<cwd>` ending in `libs/pdfium/`, up to filesystem root.
///   5. Each ancestor of `<exe_dir>` ending in `libs/pdfium/`, up to filesystem root.
///
/// Steps 4-5 cover the common Tauri-dev layout where the binary lives at
/// `target/debug/mike-tauri.exe` and the DLL sits at `<workspace>/libs/pdfium/`,
/// neither directly under cwd nor exe_dir.
#[cfg(feature = "pdf")]
fn load_pdfium() -> Result<Pdfium> {
    #[cfg(target_os = "windows")]
    const DLL_NAME: &str = "pdfium.dll";
    #[cfg(target_os = "linux")]
    const DLL_NAME: &str = "libpdfium.so";
    #[cfg(target_os = "macos")]
    const DLL_NAME: &str = "libpdfium.dylib";

    fn try_load(dir: &std::path::Path) -> Option<Pdfium> {
        let dll = dir.join(DLL_NAME);
        if dll.exists() {
            tracing::info!("[pdf] loading pdfium from {}", dll.display());
            Pdfium::bind_to_library(dll)
                .map_err(|e| anyhow!("pdfium bind: {e}"))
                .ok()
                .map(Pdfium::new)
        } else {
            None
        }
    }

    // 1. Explicit override.
    if let Ok(path) = std::env::var("PDFIUM_DYNAMIC_LIB_PATH") {
        let p = std::path::PathBuf::from(&path);
        let dir = if p.is_file() { p.parent().map(|x| x.to_path_buf()) } else { Some(p) };
        if let Some(d) = dir {
            if let Some(p) = try_load(&d) {
                return Ok(p);
            }
        }
    }

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|x| x.to_path_buf()));
    let cwd = std::env::current_dir().ok();

    // 2 & 3.
    for base in exe_dir.iter().chain(cwd.iter()) {
        if let Some(p) = try_load(&base.join("libs").join("pdfium")) {
            return Ok(p);
        }
    }

    // 4 & 5: walk ancestors of cwd, then exe_dir.
    for base in cwd.iter().chain(exe_dir.iter()) {
        for ancestor in base.ancestors() {
            if let Some(p) = try_load(&ancestor.join("libs").join("pdfium")) {
                return Ok(p);
            }
        }
    }

    Err(anyhow!(
        "pdfium library not found. Download {DLL_NAME} from \
         https://github.com/bblanchon/pdfium-binaries/releases (use the \
         windows-arm64 build on Snapdragon X Elite) and place it under \
         libs/pdfium/ at the project root, or set $PDFIUM_DYNAMIC_LIB_PATH."
    ))
}

#[cfg(feature = "pdf")]
pub struct PageText {
    pub page: usize,
    pub text: String,
    pub needs_ocr: bool,
}

/// Pass 2: Tesseract OCR fallback for scanned/image-only PDF pages.
///
/// Renders the page to a PNG at 300 DPI via pdfium, then runs
/// Tesseract on the rendered image. Accepts a &PdfPage reference
/// from the already-loaded document so we avoid re-parsing the PDF.
///
/// Requires `brew install tesseract` on macOS (installs libtesseract
/// + libleptonica + tessdata at /opt/homebrew/share/tessdata/).
#[cfg(all(feature = "pdf", feature = "tesseract"))]
fn ocr_page(pdfium: &Pdfium, page: &PdfPage, page_index: usize) -> Result<String> {
    use anyhow::anyhow;

    // 150 DPI — faster render + smaller image for Tesseract while still readable.
    let scale = 150.0 / 72.0;
    let config = PdfRenderConfig::new().scale_page_by_factor(scale);
    let bitmap = page
        .render_with_config(&config)
        .map_err(|e| anyhow!("render page {page_index} for OCR: {e}"))?;
    let dyn_img = bitmap.as_image();

    let mut png_buf = Vec::new();
    dyn_img
        .write_to(&mut std::io::Cursor::new(&mut png_buf), image::ImageFormat::Png)
        .map_err(|e| anyhow!("encode PNG page {page_index}: {e}"))?;

    // Probe Homebrew tessdata paths — Apple Silicon vs Intel.
    let tessdata: Option<&str> = if std::path::Path::new("/opt/homebrew/share/tessdata/").exists() {
        Some("/opt/homebrew/share/tessdata/")
    } else if std::path::Path::new("/usr/local/share/tessdata/").exists() {
        Some("/usr/local/share/tessdata/")
    } else if let Ok(env) = std::env::var("TESSDATA_PREFIX") {
        Some(Box::leak(env.into_boxed_str()) as &str)
    } else {
        None
    };

    // Indian case files are routinely a mix of English and Hindi
    // (Devanagari). Tesseract supports multi-language OCR via "lang1+lang2"
    // syntax, so we prefer "eng+hin" when Hindi traineddata is available
    // and fall back to "eng" alone if not. Detect availability by probing
    // for hin.traineddata in the tessdata directory.
    let want_hindi = tessdata
        .map(|d| std::path::Path::new(d).join("hin.traineddata").exists())
        .unwrap_or(false);
    let langs = if want_hindi { "eng+hin" } else { "eng" };

    let mut lt = leptess::LepTess::new(tessdata, langs)
        .or_else(|_| leptess::LepTess::new(tessdata, "eng"))
        .map_err(|e| anyhow!("leptess init (langs={langs}): {e}"))?;
    lt.set_image_from_mem(&png_buf)
        .map_err(|e| anyhow!("leptess set_image page {page_index}: {e}"))?;
    let text = lt
        .get_utf8_text()
        .map_err(|e| anyhow!("leptess ocr page {page_index}: {e}"))?;

    Ok(text.trim().to_string())
}

/// Pass 1: native text extraction via pdfium content stream.
/// Returns per-page text + flag for pages that need OCR fallback.
///
/// Logs progress every 10 pages on documents larger than that — for a
/// 200-page brief the user wants to see "10/200, 20/200…" rather than
/// silence punctuated by the final result.
#[cfg(feature = "pdf")]
pub fn extract_text(path: &Path) -> Result<Vec<PageText>> {
    let pdfium = load_pdfium()?;
    let doc = pdfium
        .load_pdf_from_file(path, None)
        .map_err(|e| anyhow!("pdfium load error: {e}"))?;

    let total = doc.pages().len() as usize;
    if total > 0 {
        tracing::info!(
            "[pdf] {}: extracting {} pages",
            path.file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default(),
            total
        );
    }

    let mut pages = Vec::new();
    for (i, page) in doc.pages().iter().enumerate() {
        let text = page.text().map_err(|e| anyhow!("page text error: {e}"))?.all();
        let needs_ocr = text_needs_ocr(&text);

        let final_text = if needs_ocr {
            #[cfg(feature = "tesseract")]
            {
                match ocr_page(&pdfium, &page, i) {
                    Ok(ocr_text) if !ocr_text.is_empty() => {
                        tracing::info!(
                            "[pdf] page {}: OCR recovered {} chars",
                            i + 1,
                            ocr_text.len()
                        );
                        ocr_text
                    }
                    Ok(_) | Err(_) => {
                        tracing::warn!(
                            "[pdf] page {}: OCR returned empty; using pdfium text",
                            i + 1
                        );
                        text
                    }
                }
            }
            #[cfg(not(feature = "tesseract"))]
            {
                text
            }
        } else {
            text
        };

        // Preserve the OCR signal: a page still counts as needing OCR if, after
        // any OCR attempt, the final text is *still* below the threshold. This
        // is true for scanned/image-only pages when OCR recovered nothing or
        // when the `tesseract` feature is not compiled in. Downstream consumers
        // (`is_scanned_pdf`, the case-prep orchestrator, the chat vision
        // fallback) rely on this to detect scanned PDFs instead of silently
        // shipping empty text as a successful extraction.
        let still_needs_ocr = text_needs_ocr(&final_text);
        pages.push(PageText {
            page: i + 1,
            text: final_text,
            needs_ocr: still_needs_ocr,
        });
        if total > 10 && (i + 1) % 10 == 0 {
            tracing::info!(
                "[pdf] {}: {}/{} pages",
                path.file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default(),
                i + 1,
                total
            );
        }
    }
    Ok(pages)
}

/// Convenience: join all pages into a single string with [Page N] markers.
#[cfg(feature = "pdf")]
pub fn extract_full_text(path: &Path) -> Result<String> {
    let pages = extract_text(path)?;
    let mut out = String::new();
    for p in pages {
        out.push_str(&format!("[Page {}]\n{}\n", p.page, p.text));
    }
    Ok(out)
}

/// Heuristic: a PDF is "scanned" (image-only) when the *majority* of pages
/// have almost no extractable text. Used to decide whether to fall back to
/// rendering pages as images for a vision-capable model.
#[cfg(feature = "pdf")]
pub fn is_scanned_pdf(pages: &[PageText]) -> bool {
    if pages.is_empty() { return false; }
    let scanned = pages.iter().filter(|p| p.needs_ocr).count();
    scanned * 2 >= pages.len()
}

/// Render PDF pages as PNG bytes at the given DPI.
/// Used for vision-capable models when text extraction fails (scanned PDFs).
#[cfg(feature = "pdf")]
pub fn render_pdf_pages(path: &Path, dpi: f32, max_pages: usize) -> Result<Vec<Vec<u8>>> {
    use image::ImageFormat;
    use pdfium_render::prelude::PdfRenderConfig;
    use std::io::Cursor;

    let pdfium = load_pdfium()?;
    let doc = pdfium
        .load_pdf_from_file(path, None)
        .map_err(|e| anyhow!("pdfium load error: {e}"))?;

    // PDF base resolution is 72 DPI; scale = target / 72.
    let scale = dpi / 72.0;
    let config = PdfRenderConfig::new().scale_page_by_factor(scale);

    let mut out = Vec::new();
    for (i, page) in doc.pages().iter().enumerate() {
        if i >= max_pages { break; }
        let bitmap = page
            .render_with_config(&config)
            .map_err(|e| anyhow!("render page {i}: {e}"))?;
        let dyn_img = bitmap.as_image();
        let mut buf = Vec::new();
        dyn_img
            .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
            .map_err(|e| anyhow!("encode png page {i}: {e}"))?;
        out.push(buf);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// TIFF → JPEG conversion (handles single and multi-page TIFFs)
// ---------------------------------------------------------------------------

/// Decode every frame in a TIFF and re-encode each as JPEG (quality 85).
/// Single-page TIFFs return a 1-element Vec; multi-page TIFFs return one
/// JPEG per frame in source order. Used for vision-capable models that
/// cannot consume TIFF natively.
pub fn convert_tiff_to_jpegs(data: &[u8]) -> Result<Vec<Vec<u8>>> {
    use anyhow::anyhow;
    use std::io::Cursor;
    use tiff::decoder::{Decoder, DecodingResult};
    use tiff::ColorType;

    let mut decoder = Decoder::new(Cursor::new(data.to_vec()))
        .map_err(|e| anyhow!("tiff decoder init: {e}"))?;

    let mut out = Vec::new();
    loop {
        let (w, h) = decoder
            .dimensions()
            .map_err(|e| anyhow!("tiff dimensions: {e}"))?;
        let color = decoder
            .colortype()
            .map_err(|e| anyhow!("tiff colortype: {e}"))?;
        let pixels = decoder
            .read_image()
            .map_err(|e| anyhow!("tiff read frame: {e}"))?;

        let dyn_img: image::DynamicImage = match (color, pixels) {
            (ColorType::RGB(8), DecodingResult::U8(buf)) => {
                let img = image::RgbImage::from_raw(w, h, buf)
                    .ok_or_else(|| anyhow!("tiff RGB frame buffer mismatch"))?;
                image::DynamicImage::ImageRgb8(img)
            }
            (ColorType::RGBA(8), DecodingResult::U8(buf)) => {
                let img = image::RgbaImage::from_raw(w, h, buf)
                    .ok_or_else(|| anyhow!("tiff RGBA frame buffer mismatch"))?;
                image::DynamicImage::ImageRgba8(img)
            }
            (ColorType::Gray(8), DecodingResult::U8(buf)) => {
                let img = image::GrayImage::from_raw(w, h, buf)
                    .ok_or_else(|| anyhow!("tiff Gray frame buffer mismatch"))?;
                image::DynamicImage::ImageLuma8(img)
            }
            (ColorType::GrayA(8), DecodingResult::U8(buf)) => {
                let img = image::GrayAlphaImage::from_raw(w, h, buf)
                    .ok_or_else(|| anyhow!("tiff GrayA frame buffer mismatch"))?;
                image::DynamicImage::ImageLumaA8(img)
            }
            // 16-bit channels — downscale to 8-bit by truncating low byte.
            (ColorType::RGB(16), DecodingResult::U16(buf)) => {
                let bytes: Vec<u8> = buf.into_iter().map(|v| (v >> 8) as u8).collect();
                let img = image::RgbImage::from_raw(w, h, bytes)
                    .ok_or_else(|| anyhow!("tiff RGB16 frame buffer mismatch"))?;
                image::DynamicImage::ImageRgb8(img)
            }
            (ColorType::Gray(16), DecodingResult::U16(buf)) => {
                let bytes: Vec<u8> = buf.into_iter().map(|v| (v >> 8) as u8).collect();
                let img = image::GrayImage::from_raw(w, h, bytes)
                    .ok_or_else(|| anyhow!("tiff Gray16 frame buffer mismatch"))?;
                image::DynamicImage::ImageLuma8(img)
            }
            (ct, _) => {
                return Err(anyhow!("Unsupported TIFF color type: {:?}", ct));
            }
        };

        let mut jpeg_buf = Vec::new();
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_buf, 85);
        dyn_img
            .write_with_encoder(encoder)
            .map_err(|e| anyhow!("jpeg encode: {e}"))?;
        out.push(jpeg_buf);

        if !decoder.more_images() {
            break;
        }
        decoder
            .next_image()
            .map_err(|e| anyhow!("tiff next frame: {e}"))?;
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// XLSX extraction — calamine, pure Rust
// ---------------------------------------------------------------------------

pub fn extract_xlsx_text(data: &[u8]) -> Result<String> {
    use anyhow::anyhow;
    use calamine::{Reader, Xlsx};
    use std::io::Cursor;

    let cursor = Cursor::new(data.to_vec());
    let mut workbook: Xlsx<_> = calamine::open_workbook_from_rs(cursor)
        .map_err(|e| anyhow!("xlsx open error: {e}"))?;

    let mut out = String::new();
    let sheet_names = workbook.sheet_names();
    for name in &sheet_names {
        if let Ok(range) = workbook.worksheet_range(name) {
            out.push_str(&format!("=== Sheet: {name} ===\n"));
            for row in range.rows() {
                let cells: Vec<String> = row
                    .iter()
                    .map(|c| c.to_string())
                    .collect();
                out.push_str(&cells.join("\t"));
                out.push('\n');
            }
            out.push('\n');
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// DOCX extraction — pure Rust ZIP+XML, no external process
// ---------------------------------------------------------------------------

/// Extract the body text of a DOCX, surfacing two classes of "removed"
/// content that legal redlines depend on:
///
///   * Tracked deletions — `<w:del>…<w:delText>X</w:delText>…</w:del>`
///     blocks. Word emits these when the doc was edited with track-
///     changes on; X is the literal text the author marked for removal.
///   * Strike-through formatting — runs whose `<w:rPr>` carries
///     `<w:strike/>` or `<w:dstrike/>`. This is purely visual styling
///     (no track-changes session needed) but is the convention some
///     contracts use to signal "this clause is no longer in force."
///
/// Both kinds are wrapped in `[removed by author: …]` markers so the
/// LLM can reason about the redline structure, e.g.:
///
///     The contract clauses are: clause 1, [removed by author: clause 2],
///     clause 3.
///
/// Paragraph boundaries (`<w:p>`) and line breaks (`<w:br/>`) are
/// emitted as newlines so the output keeps some shape; tab elements
/// (`<w:tab/>`) become single spaces.
pub fn extract_docx_text(data: &[u8]) -> Result<String> {
    use anyhow::anyhow;
    use std::io::Cursor;

    let cursor = Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor)?;

    let xml = {
        let mut file = archive
            .by_name("word/document.xml")
            .map_err(|_| anyhow!("Not a valid DOCX: missing word/document.xml"))?;
        let mut buf = String::new();
        use std::io::Read;
        file.read_to_string(&mut buf)?;
        buf
    };

    Ok(extract_docx_body_text(&xml))
}

/// Pure-string entry point for the docx XML → annotated plain text
/// conversion. Kept separate from `extract_docx_text` so unit tests
/// can exercise the extraction without packaging a real ZIP.
fn extract_docx_body_text(xml: &str) -> String {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);

    let mut out = String::with_capacity(xml.len() / 4);
    // Stack-style depth counter for tracked deletions. <w:del> blocks
    // can in principle nest; the depth lets us notice the *outermost*
    // close to flip the "removed" flag back off.
    let mut del_depth: usize = 0;
    // Strike-through is run-scoped: <w:strike/> appears inside the
    // run's <w:rPr>, applies to the run's text, ends at </w:r>.
    let mut current_run_struck = false;
    let mut in_run = false;
    let mut in_rpr = false;
    // True while we have an unclosed "[removed by author: " in the
    // output — closes on whichever event ends the removed region first.
    let mut removal_open = false;

    let close_removal = |out: &mut String, removal_open: &mut bool| {
        if *removal_open {
            out.push(']');
            *removal_open = false;
        }
    };

    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                match local_name_str(e.name()).as_deref() {
                    Some("p") => {
                        close_removal(&mut out, &mut removal_open);
                        if !out.ends_with('\n') && !out.is_empty() {
                            out.push('\n');
                        }
                    }
                    Some("r") => {
                        in_run = true;
                        current_run_struck = false;
                    }
                    Some("rPr") => {
                        if in_run {
                            in_rpr = true;
                        }
                    }
                    Some("del") => del_depth += 1,
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => match local_name_str(e.name()).as_deref() {
                Some("strike") | Some("dstrike") => {
                    if in_rpr {
                        current_run_struck = true;
                    }
                }
                Some("br") => {
                    close_removal(&mut out, &mut removal_open);
                    out.push('\n');
                }
                Some("tab") => out.push(' '),
                _ => {}
            },
            Ok(Event::End(e)) => match local_name_str(e.name()).as_deref() {
                Some("r") => {
                    if removal_open && current_run_struck && del_depth == 0 {
                        close_removal(&mut out, &mut removal_open);
                    }
                    in_run = false;
                    current_run_struck = false;
                }
                Some("rPr") => {
                    in_rpr = false;
                }
                Some("del") => {
                    if del_depth > 0 {
                        del_depth -= 1;
                    }
                    if del_depth == 0 {
                        close_removal(&mut out, &mut removal_open);
                    }
                }
                Some("p") => {
                    close_removal(&mut out, &mut removal_open);
                }
                _ => {}
            },
            Ok(Event::Text(t)) => {
                let raw = t.unescape().unwrap_or_default().into_owned();
                if raw.is_empty() {
                    continue;
                }
                let removed = del_depth > 0 || current_run_struck;
                if removed {
                    if !removal_open {
                        if !out.is_empty() && !out.ends_with(char::is_whitespace) {
                            out.push(' ');
                        }
                        out.push_str("[removed by author: ");
                        removal_open = true;
                    }
                    out.push_str(&raw);
                } else {
                    close_removal(&mut out, &mut removal_open);
                    out.push_str(&raw);
                }
            }
            Ok(Event::CData(c)) => {
                let raw = String::from_utf8_lossy(c.as_ref()).into_owned();
                if raw.is_empty() {
                    continue;
                }
                let removed = del_depth > 0 || current_run_struck;
                if removed {
                    if !removal_open {
                        if !out.is_empty() && !out.ends_with(char::is_whitespace) {
                            out.push(' ');
                        }
                        out.push_str("[removed by author: ");
                        removal_open = true;
                    }
                    out.push_str(&raw);
                } else {
                    close_removal(&mut out, &mut removal_open);
                    out.push_str(&raw);
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    close_removal(&mut out, &mut removal_open);
    collapse_inline_whitespace(&out).trim().to_string()
}

fn local_name_str(name: quick_xml::name::QName) -> Option<String> {
    let bytes = name.local_name().into_inner();
    std::str::from_utf8(bytes).ok().map(|s| s.to_string())
}

fn collapse_inline_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_was_inline_space = false;
    for ch in s.chars() {
        if ch == '\n' {
            while out.ends_with(' ') {
                out.pop();
            }
            out.push('\n');
            last_was_inline_space = false;
        } else if ch.is_whitespace() {
            if !last_was_inline_space && !out.is_empty() && !out.ends_with('\n') {
                out.push(' ');
                last_was_inline_space = true;
            }
        } else {
            out.push(ch);
            last_was_inline_space = false;
        }
    }
    out
}

#[cfg(test)]
mod docx_tests {
    use super::extract_docx_body_text;

    fn wrap(body: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
             <w:body>{body}</w:body></w:document>",
        )
    }

    fn run(text: &str) -> String {
        format!("<w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r>")
    }

    fn struck_run(text: &str) -> String {
        format!(
            "<w:r><w:rPr><w:strike/></w:rPr><w:t xml:space=\"preserve\">{text}</w:t></w:r>"
        )
    }

    fn deleted_run(author: &str, text: &str) -> String {
        format!(
            "<w:del w:id=\"1\" w:author=\"{author}\" w:date=\"2024-01-01T00:00:00Z\">\
             <w:r><w:delText xml:space=\"preserve\">{text}</w:delText></w:r></w:del>",
        )
    }

    #[test]
    fn plain_paragraphs_extract_unchanged() {
        let xml = wrap(&format!(
            "<w:p>{}</w:p><w:p>{}</w:p>",
            run("Hello world."),
            run("Second line.")
        ));
        let got = extract_docx_body_text(&xml);
        assert_eq!(got, "Hello world.\nSecond line.");
    }

    #[test]
    fn tracked_deletion_is_marked() {
        let xml = wrap(&format!(
            "<w:p>{}{}{}</w:p>",
            run("Keep "),
            deleted_run("Alice", "this part"),
            run(" then more.")
        ));
        let got = extract_docx_body_text(&xml);
        assert!(
            got.contains("[removed by author: this part]"),
            "expected del marker, got {got:?}"
        );
        assert!(got.contains("Keep"));
        assert!(got.contains("then more."));
    }

    #[test]
    fn strike_through_run_is_marked() {
        let xml = wrap(&format!(
            "<w:p>{}{}{}</w:p>",
            run("Before "),
            struck_run("STRUCK"),
            run(" after.")
        ));
        let got = extract_docx_body_text(&xml);
        assert!(
            got.contains("[removed by author: STRUCK]"),
            "expected strike marker, got {got:?}"
        );
        assert!(got.contains("Before"));
        assert!(got.contains("after."));
    }

    #[test]
    fn dstrike_run_is_marked() {
        let xml = wrap(&format!(
            "<w:p><w:r><w:rPr><w:dstrike/></w:rPr><w:t>X</w:t></w:r></w:p>"
        ));
        let got = extract_docx_body_text(&xml);
        assert!(
            got.contains("[removed by author: X]"),
            "expected dstrike marker, got {got:?}"
        );
    }

    #[test]
    fn removal_does_not_span_paragraphs() {
        // A pathological case: a deletion followed by a paragraph
        // close should not leave the bracket open across <w:p>.
        let xml = wrap(&format!(
            "<w:p>{}{}</w:p><w:p>{}</w:p>",
            run("a "),
            deleted_run("X", "b"),
            run("c")
        ));
        let got = extract_docx_body_text(&xml);
        // No stray '[' without a matching ']' on either line.
        for line in got.lines() {
            let opens = line.matches("[removed by author:").count();
            let closes = line.matches(']').count();
            assert!(
                opens <= closes,
                "unbalanced brackets on line {line:?} (full: {got:?})"
            );
        }
    }

    #[test]
    fn run_without_strike_is_plain() {
        let xml = wrap(&format!("<w:p>{}</w:p>", run("not struck")));
        let got = extract_docx_body_text(&xml);
        assert_eq!(got, "not struck");
        assert!(!got.contains("removed by author"));
    }

    #[test]
    fn empty_document_returns_empty_string() {
        let xml = wrap("");
        assert_eq!(extract_docx_body_text(&xml), "");
    }
}

#[cfg(all(test, feature = "pdf"))]
mod ocr_tests {
    use super::{is_scanned_pdf, text_needs_ocr, PageText};

    #[test]
    fn empty_page_text_needs_ocr() {
        assert!(text_needs_ocr(""), "an image-only page with no text layer needs OCR");
        assert!(text_needs_ocr("   \n\t "), "whitespace-only page needs OCR");
        assert!(text_needs_ocr("###"), "punctuation-only page (no alphanumerics) needs OCR");
    }

    #[test]
    fn real_text_does_not_need_ocr() {
        assert!(
            !text_needs_ocr("This page has a real, extractable text layer."),
            "a page with a genuine text layer must NOT be flagged for OCR"
        );
    }

    #[test]
    fn scanned_pdf_detected_from_image_only_pages() {
        // Simulates extract_text's output for a scanned/image-only PDF: every
        // page came back below the OCR threshold and OCR recovered nothing, so
        // needs_ocr stays true. Before the fix needs_ocr was hardcoded false,
        // so is_scanned_pdf could never return true and the vision fallback
        // never fired.
        let pages = vec![
            PageText { page: 1, text: String::new(), needs_ocr: text_needs_ocr("") },
            PageText { page: 2, text: " ".to_string(), needs_ocr: text_needs_ocr(" ") },
        ];
        assert!(
            is_scanned_pdf(&pages),
            "an image-only PDF (every page below threshold) must be reported as scanned"
        );
        assert!(
            pages.iter().any(|p| p.needs_ocr),
            "the orchestrator's `any(needs_ocr)` check must see the scanned pages"
        );
    }

    #[test]
    fn text_pdf_not_scanned() {
        let body = "A paragraph with plenty of real extractable text on the page.";
        let pages = vec![
            PageText { page: 1, text: body.to_string(), needs_ocr: text_needs_ocr(body) },
            PageText { page: 2, text: body.to_string(), needs_ocr: text_needs_ocr(body) },
        ];
        assert!(!is_scanned_pdf(&pages), "a normal text PDF must not be flagged as scanned");
    }

    #[test]
    fn multibyte_text_layer_not_scanned() {
        // Devanagari text must count as a real text layer (is_alphanumeric is
        // true for these code points), not be mistaken for an image-only page.
        let body = "यह एक वास्तविक पाठ परत है जिसमें पर्याप्त अक्षर हैं।";
        assert!(
            !text_needs_ocr(body),
            "a multibyte (Devanagari) text layer must NOT be flagged for OCR"
        );
    }
}
