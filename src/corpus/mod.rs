//! Firm-knowledge corpus: ingest uploaded firm documents (past cases,
//! skeleton arguments, deeds, templates), chunk them structurally, tag
//! them with light metadata, and expose two agent tools the drafting
//! model uses to search the firm's own prior work instead of relying on
//! blind top-k injection.
//!
//! * `chunker` — pure structure-aware splitting (no DB, no LLM).
//! * `ingest`  — the pipeline: extract → chunk → store → LLM-tag.
//! * `tools`   — `search_firm_corpus` + `expand_chunk` exec fns.

pub mod chunker;
pub mod ingest;
pub mod tools;

// ---------------------------------------------------------------------------
// Firm-knowledge bulk-upload limits — ONE source of truth, read by the server
// (upload handler + ingest) AND surfaced to the client via GET /corpus/limits
// so the browser preflight and the server enforce the exact same numbers.
// ---------------------------------------------------------------------------

/// Hard cap on documents per folder drop. A folder over this is not silently
/// truncated: the user is told and chooses to take the first N or cancel.
pub const FIRM_UPLOAD_MAX_DOCS: usize = 500;

/// Per-file size cap. A 500-page scanned court bundle at ~300 KB/page is ~150 MB,
/// so 200 MB covers the largest realistic legal document with headroom.
pub const FIRM_UPLOAD_MAX_FILE_BYTES: u64 = 200 * 1024 * 1024;

/// Extensions we can actually extract text from (incl. images via OCR).
/// Legacy binary `.doc` and `.odt` are intentionally absent — they have no
/// extractor here, so they surface as an itemized "unsupported type" skip
/// rather than a silent failure.
pub const FIRM_SUPPORTED_EXTS: &[&str] = &[
    // text + office
    "pdf", "docx", "rtf", "txt", "md", "csv", "xlsx", "xls", "xlsb", "ods",
    // scans / images (OCR)
    "png", "jpg", "jpeg", "tif", "tiff", "webp", "bmp",
];

/// Lowercased extension of a filename, or "" when there is none.
pub fn file_ext(filename: &str) -> String {
    match filename.rsplit_once('.') {
        Some((_, ext)) if !ext.is_empty() => ext.to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// True for image extensions that must go through OCR rather than text extraction.
pub fn is_image_ext(ext: &str) -> bool {
    matches!(ext, "png" | "jpg" | "jpeg" | "tif" | "tiff" | "webp" | "bmp")
}

/// Why a file would be skipped at upload, or `None` if it is acceptable.
/// Pure (no IO) so the same rule backs the client preflight and the server
/// upload handler. Does NOT cover the 500 cap (that needs the DB count).
pub fn upload_skip_reason(filename: &str, size_bytes: u64) -> Option<String> {
    let ext = file_ext(filename);
    if !FIRM_SUPPORTED_EXTS.contains(&ext.as_str()) {
        let shown = if ext.is_empty() { "none".to_string() } else { format!(".{ext}") };
        return Some(format!("unsupported type ({shown})"));
    }
    if size_bytes > FIRM_UPLOAD_MAX_FILE_BYTES {
        return Some(format!(
            "file is {:.1} MB, limit is {} MB",
            size_bytes as f64 / (1024.0 * 1024.0),
            FIRM_UPLOAD_MAX_FILE_BYTES / (1024 * 1024)
        ));
    }
    None
}

/// Whether one more file must be skipped because the folder batch is already
/// at the 500 cap, plus the message to surface. `None` means there is room.
/// Only folder (batched) uploads are capped; ad-hoc single uploads pass.
/// Surfacing the skip here is what keeps an over-cap drop from silently
/// truncating.
pub fn batch_cap_skip(in_batch: bool, current_count: usize) -> Option<String> {
    if in_batch && current_count >= FIRM_UPLOAD_MAX_DOCS {
        Some(format!("folder cap of {FIRM_UPLOAD_MAX_DOCS} reached"))
    } else {
        None
    }
}

#[cfg(test)]
mod limit_tests {
    use super::*;

    #[test]
    fn unsupported_type_is_skipped_with_reason() {
        let r = upload_skip_reason("brief.doc", 1000).unwrap();
        assert!(r.contains("unsupported type"), "got: {r}");
        assert!(r.contains(".doc"), "reason names the extension: {r}");
        // no extension at all
        assert!(upload_skip_reason("README", 1000).unwrap().contains("none"));
    }

    #[test]
    fn oversize_file_is_skipped_with_actual_size_vs_limit() {
        let too_big = FIRM_UPLOAD_MAX_FILE_BYTES + 1;
        let r = upload_skip_reason("scan.pdf", too_big).unwrap();
        assert!(r.contains("limit is 200 MB"), "names the limit: {r}");
        assert!(r.contains("MB"), "states the actual size: {r}");
    }

    #[test]
    fn supported_within_size_is_accepted() {
        assert!(upload_skip_reason("petition.pdf", 5 * 1024 * 1024).is_none());
        assert!(upload_skip_reason("photo.JPG", 1024).is_none(), "ext match is case-insensitive");
        assert!(upload_skip_reason("notes.txt", 0).is_none());
    }

    #[test]
    fn images_route_through_ocr() {
        for e in ["png", "jpg", "jpeg", "tiff", "tif", "webp", "bmp"] {
            assert!(is_image_ext(e), "{e} should be an OCR image type");
        }
        for e in ["pdf", "docx", "txt", ""] {
            assert!(!is_image_ext(e), "{e} is not an image type");
        }
    }

    #[test]
    fn over_cap_is_surfaced_not_silently_truncated() {
        // Under the cap: room remains, no skip.
        assert!(batch_cap_skip(true, FIRM_UPLOAD_MAX_DOCS - 1).is_none());
        // At/over the cap: a skip WITH a reason (never a silent drop).
        let r = batch_cap_skip(true, FIRM_UPLOAD_MAX_DOCS).expect("at cap must skip");
        assert!(r.contains("500"), "the reason names the cap: {r}");
        assert!(batch_cap_skip(true, FIRM_UPLOAD_MAX_DOCS + 10).is_some());
        // Single (non-batch) uploads are never capped.
        assert!(batch_cap_skip(false, FIRM_UPLOAD_MAX_DOCS + 999).is_none());
    }
}
