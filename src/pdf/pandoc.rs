//! Pandoc-backed structured-Markdown view of a `.docx`, for the redline path.
//!
//! This is an OPTIONAL enhancement. When pandoc is on the system it renders a
//! Word document to structured Markdown — clause numbering, tables, and any
//! existing tracked changes — which the pure-Rust hot extractor
//! (`super::extract_docx_text`) deliberately flattens away. When pandoc is
//! absent the caller falls back to that pure-Rust extractor; pandoc is never a
//! hard dependency and a missing binary must never error the read path.

use anyhow::{anyhow, Result};
use std::process::Command;

/// Resolve the pandoc binary to use, in priority order:
///
///   1. `$PANDOC_PATH`, if it points at a file that exists (explicit override).
///   2. `"pandoc"`, if `pandoc --version` succeeds on `PATH`.
///   3. `None` — no usable pandoc.
pub fn pandoc_bin() -> Option<String> {
    // 1. Explicit override — only honoured when the path actually exists, so a
    //    stale/typo'd PANDOC_PATH falls through to PATH rather than failing.
    if let Ok(p) = std::env::var("PANDOC_PATH") {
        if !p.is_empty() && std::path::Path::new(&p).exists() {
            return Some(p);
        }
    }

    // 2. Probe PATH. `pandoc --version` exits 0 only if the binary runs.
    let on_path = Command::new("pandoc")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if on_path {
        return Some("pandoc".to_string());
    }

    // 3. Nothing usable.
    None
}

/// Whether a usable pandoc binary is available.
pub fn pandoc_available() -> bool {
    pandoc_bin().is_some()
}

/// Convert `.docx` bytes to structured Markdown via pandoc.
///
/// pandoc reads `.docx` from a *file path* (not stdin), so the bytes are first
/// written to a temp file. Tracked changes are preserved (`--track-changes=all`)
/// and hard-wrapping is disabled (`--wrap=none`) so each clause stays on one
/// line. The temp file is always removed before returning. Returns `Err` if
/// pandoc is missing, fails to spawn, exits non-zero, or emits non-UTF-8.
pub fn docx_to_markdown(bytes: &[u8]) -> Result<String> {
    let bin = pandoc_bin().ok_or_else(|| anyhow!("pandoc not available"))?;

    let tmp = unique_tmp_path();
    std::fs::write(&tmp, bytes).map_err(|e| anyhow!("write temp docx: {e}"))?;

    // Run pandoc, capturing the result so we can clean up the temp file on
    // every exit path (success or failure) before returning.
    let result = (|| {
        let output = Command::new(&bin)
            .arg(&tmp)
            .args(["-f", "docx", "-t", "markdown", "--track-changes=all", "--wrap=none"])
            .output()
            .map_err(|e| anyhow!("spawn pandoc: {e}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("pandoc exited {}: {}", output.status, stderr.trim()));
        }
        String::from_utf8(output.stdout).map_err(|e| anyhow!("pandoc output not UTF-8: {e}"))
    })();

    let _ = std::fs::remove_file(&tmp);
    result
}

/// A collision-resistant temp path: `<tmpdir>/mike-pandoc-<pid>-<n>.docx`.
/// The `.docx` suffix also lets pandoc infer the format if `-f docx` were ever
/// dropped. Dependency-free (the `tempfile` crate is dev-only here).
fn unique_tmp_path() -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("mike-pandoc-{}-{n}.docx", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Serializes the few tests that mutate `PANDOC_PATH`. `set_var`/`remove_var`
    /// are `unsafe` on edition 2024 (process-global), so we hold this lock to
    /// keep concurrent tests from racing on the environment.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A minimal valid-enough `.docx` (a ZIP carrying only `word/document.xml`)
    /// — exactly what the pure-Rust `extract_docx_text` reads. Not a fully
    /// pandoc-loadable package; it only feeds the fallback / pandoc-missing
    /// paths, which never actually invoke pandoc on it.
    fn minimal_docx(text: &str) -> Vec<u8> {
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut zipw = zip::ZipWriter::new(&mut cursor);
            zipw.start_file("word/document.xml", zip::write::SimpleFileOptions::default())
                .unwrap();
            let xml = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
                 <w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
                 <w:body><w:p><w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r></w:p></w:body>\
                 </w:document>"
            );
            zipw.write_all(xml.as_bytes()).unwrap();
            zipw.finish().unwrap();
        }
        cursor.into_inner()
    }

    /// Build a real, fully-formed `.docx` from Markdown using pandoc itself, so
    /// the round-trip test reads a document with genuine numbering + table
    /// definitions (what Word would produce). Only called when pandoc exists.
    fn pandoc_make_docx(bin: &str, md: &str) -> Vec<u8> {
        let md_in = super::unique_tmp_path().with_extension("md");
        let docx_out = super::unique_tmp_path();
        std::fs::write(&md_in, md).unwrap();
        let status = Command::new(bin)
            .arg(&md_in)
            .args(["-f", "markdown", "-t", "docx", "-o"])
            .arg(&docx_out)
            .status()
            .unwrap();
        // Clean up the input unconditionally so a failed fixture build does not
        // leak it, then assert before reading the output pandoc produced.
        let _ = std::fs::remove_file(&md_in);
        assert!(status.success(), "pandoc md->docx fixture build failed");
        let bytes = std::fs::read(&docx_out).unwrap();
        let _ = std::fs::remove_file(&docx_out);
        bytes
    }

    #[test]
    fn nonexistent_pandoc_path_is_ignored() {
        // A bad PANDOC_PATH must NOT be returned as the binary; pandoc_bin()
        // falls through to PATH (Some("pandoc")) or None. Deterministic on any
        // machine regardless of whether pandoc is installed.
        let _g = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("PANDOC_PATH", "/nonexistent/definitely/not/here") };
        let bin = pandoc_bin();
        unsafe { std::env::remove_var("PANDOC_PATH") };
        assert_ne!(
            bin.as_deref(),
            Some("/nonexistent/definitely/not/here"),
            "a non-existent PANDOC_PATH must be ignored, not returned"
        );
    }

    #[test]
    fn docx_to_markdown_errors_when_pandoc_missing() {
        // Spec check: "with PANDOC_PATH=/nonexistent and no pandoc on PATH,
        // docx_to_markdown errors." We force the override to miss; the
        // assertion is only meaningful when pandoc is also absent from PATH, so
        // it skips on machines that have pandoc installed.
        let _g = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("PANDOC_PATH", "/nonexistent/pandoc") };
        let missing = !pandoc_available();
        if missing {
            let docx = minimal_docx("Clause 1. The term of this Agreement is 12 months.");
            let got = docx_to_markdown(&docx);
            assert!(got.is_err(), "expected Err when pandoc unavailable, got {got:?}");
        }
        unsafe { std::env::remove_var("PANDOC_PATH") };
        if !missing {
            eprintln!("skipping missing-pandoc assertion: pandoc is present on PATH");
        }
    }

    #[test]
    fn numbered_list_and_table_round_trip_with_pandoc() {
        // Spec check: "with pandoc present, a sample .docx with a numbered list
        // + a table round-trips to markdown containing list markers and a pipe
        // table." Skips cleanly when pandoc is not installed.
        let Some(bin) = pandoc_bin() else {
            eprintln!("skipping round-trip: pandoc not installed");
            return;
        };

        let source_md = "\
1.  First clause about the term.
2.  Second clause about payment.

| Item | Value |
|------|-------|
| Term | 12 months |
| Fee  | 50000 |
";
        let docx = pandoc_make_docx(&bin, source_md);
        let out = docx_to_markdown(&docx).expect("docx_to_markdown should succeed with pandoc");

        // Clause text + ordered-list markers survive.
        assert!(out.contains("First clause"), "missing list item text in: {out}");
        assert!(out.contains("Second clause"), "missing list item text in: {out}");
        assert!(out.contains("1."), "missing ordered-list marker in: {out}");

        // Table survives as a (pipe) table: cells present and pipe delimiters
        // emitted (pandoc's markdown writer renders this simple table with `|`).
        assert!(out.contains('|'), "missing pipe-table delimiter in: {out}");
        assert!(out.contains("Term") && out.contains("12 months"), "missing table cells in: {out}");
    }
}
