//! Reversible PII anonymizer for LLM calls.
//!
//! Unlike `scrubber.rs` (which does one-way redaction for training data),
//! this module replaces PII with **semantic placeholders** (`PERSON_01`,
//! `AADHAAR_01`, …) and keeps a bidirectional mapping so the originals
//! can be restored in LLM responses.
//!
//! Name detection can optionally be boosted by a GLiNER sidecar service
//! (a small Python/FastAPI app). When the sidecar is unreachable the
//! anonymizer falls back to regex-only — it never blocks on the sidecar.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::LazyLock;

// ── regex bank (shared with scrubber.rs patterns) ────────────────────

static RE_AADHAAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d{4}[\s\-]?\d{4}[\s\-]?\d{4}\b").unwrap());

static RE_PAN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[A-Z]{5}\d{4}[A-Z]\b").unwrap());

static RE_PHONE_PLUS91: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\+91[\s\-]?[6-9]\d{9}\b").unwrap());

static RE_PHONE_BARE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[6-9]\d{9}\b").unwrap());

static RE_EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}\b").unwrap()
});

static RE_BANK_ACCOUNT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:account|a/c|bank\s*a/c|saving|current|acct)[\s.:No\-]*(\d{9,18})")
        .unwrap()
});

static RE_IFSC: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[A-Z]{4}0[A-Z0-9]{6}\b").unwrap());

static RE_PINCODE_KEYWORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)pin\s*(?:code)?[\s.:\-/]*(\d{6})\b").unwrap()
});

static RE_TITLE_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i:Sh\.?|Shri\.?|Smt\.?|Mr\.?|Mrs\.?|Ms\.?|Km\.?|Kumari|Dr\.?|Adv\.?)\s+([A-Z][a-zA-Z']+(?:\s+[A-Z][a-zA-Z']+){0,4})").unwrap()
});

static RE_RELATION_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i:s/o|d/o|w/o|son\s+of|daughter\s+of|wife\s+of)\s+(?i:Late\s+)?(?i:Sh\.?|Shri\.?|Smt\.?|Mr\.?|Mrs\.?|Ms\.?|Km\.?|Kumari)?\s*([A-Z][a-zA-Z']+(?:\s+[A-Z][a-zA-Z']+){0,4})").unwrap()
});

// ── GLiNER sidecar types ─────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct GlinerRequest {
    text: String,
    labels: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct GlinerEntity {
    text: String,
    start: usize,
    end: usize,
    label: String,
    #[allow(dead_code)]
    score: f64,
}

// ── detected span ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Span {
    start: usize,
    end: usize,
    original: String,
    category: PiiCategory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum PiiCategory {
    Person,
    Aadhaar,
    Pan,
    Phone,
    Email,
    BankAccount,
    Ifsc,
    Pincode,
    Organization,
    Address,
}

impl PiiCategory {
    fn prefix(self) -> &'static str {
        match self {
            Self::Person => "PERSON",
            Self::Aadhaar => "AADHAAR",
            Self::Pan => "PAN",
            Self::Phone => "PHONE",
            Self::Email => "EMAIL",
            Self::BankAccount => "BANK_ACCT",
            Self::Ifsc => "IFSC",
            Self::Pincode => "PINCODE",
            Self::Organization => "ORG",
            Self::Address => "ADDRESS",
        }
    }
}

// ── public types ─────────────────────────────────────────────────────

/// Holds the placeholder ↔ original mapping for one anonymization pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiiMapping {
    /// placeholder → original  (e.g. "PERSON_01" → "Rajesh Kumar")
    pub to_original: HashMap<String, String>,
    /// original (lowercased) → placeholder
    #[serde(skip)]
    to_placeholder: HashMap<String, String>,
}

impl PiiMapping {
    fn new() -> Self {
        Self {
            to_original: HashMap::new(),
            to_placeholder: HashMap::new(),
        }
    }

    /// Get or create a placeholder for `original` under `category`.
    fn placeholder_for(&mut self, original: &str, category: PiiCategory) -> String {
        let key = original.to_lowercase();
        if let Some(existing) = self.to_placeholder.get(&key) {
            return existing.clone();
        }
        let prefix = category.prefix();
        let n = self
            .to_original
            .keys()
            .filter(|k| k.starts_with(prefix))
            .count()
            + 1;
        let placeholder = format!("{prefix}_{n:02}");
        self.to_original
            .insert(placeholder.clone(), original.to_string());
        self.to_placeholder.insert(key, placeholder.clone());
        placeholder
    }
}

// ── GLiNER client ────────────────────────────────────────────────────

const GLINER_LABELS: &[&str] = &[
    "person name",
    "organization",
    "address",
];

/// Call the GLiNER sidecar. Returns detected entities, or empty vec on
/// failure (timeout, sidecar down, etc.) — never blocks the pipeline.
async fn query_gliner(text: &str, base_url: &str) -> Vec<GlinerEntity> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    let req = GlinerRequest {
        text: text.to_string(),
        labels: GLINER_LABELS.iter().map(|s| s.to_string()).collect(),
    };

    match client.post(format!("{base_url}/detect")).json(&req).send().await {
        Ok(resp) => resp.json::<Vec<GlinerEntity>>().await.unwrap_or_default(),
        Err(e) => {
            tracing::debug!("[pii] GLiNER sidecar unreachable: {e}");
            Vec::new()
        }
    }
}

// ── span collectors ──────────────────────────────────────────────────

/// Words that look like names after a title but are actually legal terms.
const NON_NAME_WORDS: &[&str] = &[
    "petitioner", "respondent", "complainant", "accused", "plaintiff",
    "defendant", "applicant", "appellant", "court", "judge", "justice",
    "advocate", "counsel", "learned", "section", "act", "code", "order",
    "india", "indian", "union", "state", "government", "tribunal",
    "commission", "authority", "police", "hospital", "bank", "university",
];

fn is_non_name(w: &str) -> bool {
    NON_NAME_WORDS.contains(&w.to_lowercase().as_str())
}

fn clean_name(raw: &str) -> Option<String> {
    let words: Vec<&str> = raw.split_whitespace().collect();
    let meaningful: Vec<&&str> = words.iter().filter(|w| !is_non_name(w)).collect();
    if meaningful.is_empty() || raw.len() < 3 {
        return None;
    }
    // Drop trailing non-name words
    let mut end = words.len();
    while end > 0 && is_non_name(words[end - 1]) {
        end -= 1;
    }
    let mut start = 0;
    while start < end && is_non_name(words[start]) {
        start += 1;
    }
    if start >= end {
        return None;
    }
    Some(words[start..end].join(" "))
}

fn collect_regex_spans(text: &str) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();

    // Aadhaar
    for m in RE_AADHAAR.find_iter(text) {
        let digits: String = m.as_str().chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.len() == 12 {
            spans.push(Span {
                start: m.start(),
                end: m.end(),
                original: m.as_str().to_string(),
                category: PiiCategory::Aadhaar,
            });
        }
    }

    // PAN
    for m in RE_PAN.find_iter(text) {
        spans.push(Span {
            start: m.start(),
            end: m.end(),
            original: m.as_str().to_string(),
            category: PiiCategory::Pan,
        });
    }

    // Phone (+91 first, then bare)
    for m in RE_PHONE_PLUS91.find_iter(text) {
        spans.push(Span {
            start: m.start(),
            end: m.end(),
            original: m.as_str().to_string(),
            category: PiiCategory::Phone,
        });
    }
    for m in RE_PHONE_BARE.find_iter(text) {
        // Skip if already covered by +91 pattern
        if spans.iter().any(|s| s.start <= m.start() && s.end >= m.end()) {
            continue;
        }
        spans.push(Span {
            start: m.start(),
            end: m.end(),
            original: m.as_str().to_string(),
            category: PiiCategory::Phone,
        });
    }

    // Email
    for m in RE_EMAIL.find_iter(text) {
        spans.push(Span {
            start: m.start(),
            end: m.end(),
            original: m.as_str().to_string(),
            category: PiiCategory::Email,
        });
    }

    // Bank account (capture group 1 is the number)
    for caps in RE_BANK_ACCOUNT.captures_iter(text) {
        if let Some(m) = caps.get(1) {
            spans.push(Span {
                start: m.start(),
                end: m.end(),
                original: m.as_str().to_string(),
                category: PiiCategory::BankAccount,
            });
        }
    }

    // IFSC
    for m in RE_IFSC.find_iter(text) {
        spans.push(Span {
            start: m.start(),
            end: m.end(),
            original: m.as_str().to_string(),
            category: PiiCategory::Ifsc,
        });
    }

    // Pincode (capture group 1)
    for caps in RE_PINCODE_KEYWORD.captures_iter(text) {
        if let Some(m) = caps.get(1) {
            spans.push(Span {
                start: m.start(),
                end: m.end(),
                original: m.as_str().to_string(),
                category: PiiCategory::Pincode,
            });
        }
    }

    // Names via title prefix (Shri X, Mr. Y)
    for caps in RE_TITLE_NAME.captures_iter(text) {
        if let Some(m) = caps.get(1) {
            if let Some(name) = clean_name(m.as_str()) {
                spans.push(Span {
                    start: m.start(),
                    end: m.start() + name.len().min(m.len()),
                    original: name,
                    category: PiiCategory::Person,
                });
            }
        }
    }

    // Names via relation (s/o, d/o, w/o)
    for caps in RE_RELATION_NAME.captures_iter(text) {
        if let Some(m) = caps.get(1) {
            if let Some(name) = clean_name(m.as_str()) {
                spans.push(Span {
                    start: m.start(),
                    end: m.start() + name.len().min(m.len()),
                    original: name,
                    category: PiiCategory::Person,
                });
            }
        }
    }

    spans
}

fn merge_gliner_spans(spans: &mut Vec<Span>, entities: Vec<GlinerEntity>) {
    for ent in entities {
        // Skip if this span is already covered by regex detection
        let dominated = spans
            .iter()
            .any(|s| s.start <= ent.start && s.end >= ent.end);
        if dominated {
            continue;
        }

        let category = match ent.label.as_str() {
            "person name" => PiiCategory::Person,
            "organization" => PiiCategory::Organization,
            "address" => PiiCategory::Address,
            _ => continue,
        };

        spans.push(Span {
            start: ent.start,
            end: ent.end,
            original: ent.text,
            category,
        });
    }
}

/// Remove overlapping spans, keeping the longer/higher-priority one.
fn dedupe_spans(spans: &mut Vec<Span>) {
    // Sort by start position, then longer spans first
    spans.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));

    let mut keep = Vec::with_capacity(spans.len());
    let mut last_end = 0usize;
    for span in spans.drain(..) {
        if span.start >= last_end {
            last_end = span.end;
            keep.push(span);
        }
        // else: overlapping with previous → skip (previous was longer or started earlier)
    }
    *spans = keep;
}

// ── public API ───────────────────────────────────────────────────────

/// URL of the GLiNER sidecar. Set `GLINER_URL` env var to override.
fn gliner_url() -> String {
    std::env::var("GLINER_URL").unwrap_or_else(|_| "http://127.0.0.1:4010".to_string())
}

/// Anonymize `text`, replacing PII with semantic placeholders.
///
/// Returns the sanitized text and the mapping needed to restore originals.
/// If the GLiNER sidecar is unreachable, falls back to regex-only.
pub async fn anonymize(text: &str) -> (String, PiiMapping) {
    let mut spans = collect_regex_spans(text);

    // Boost with GLiNER (best-effort)
    let gliner_entities = query_gliner(text, &gliner_url()).await;
    if !gliner_entities.is_empty() {
        merge_gliner_spans(&mut spans, gliner_entities);
    }

    dedupe_spans(&mut spans);

    let mut mapping = PiiMapping::new();
    let mut result = String::with_capacity(text.len());
    let mut cursor = 0usize;

    for span in &spans {
        if span.start > cursor {
            result.push_str(&text[cursor..span.start]);
        }
        let placeholder = mapping.placeholder_for(&span.original, span.category);
        result.push_str(&placeholder);
        cursor = span.end;
    }
    // Remainder
    if cursor < text.len() {
        result.push_str(&text[cursor..]);
    }

    (result, mapping)
}

/// Returns `true` if PII anonymization is enabled via `PII_ANONYMIZE=1`.
pub fn is_enabled() -> bool {
    std::env::var("PII_ANONYMIZE")
        .map(|v| matches!(v.trim(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

/// Anonymize all message contents in a conversation, sharing a single
/// mapping so the same entity gets the same placeholder across turns.
///
/// Only user and assistant messages are anonymized (tool/system are left
/// intact — they contain internal data, not client PII).
pub async fn anonymize_messages(
    messages: &[crate::llm::Message],
) -> (Vec<crate::llm::Message>, PiiMapping) {
    // Build a combined text block so GLiNER sees cross-message context.
    let combined: String = messages
        .iter()
        .filter(|m| matches!(m.role, crate::llm::Role::User | crate::llm::Role::Assistant))
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n---\n");

    // Collect spans + GLiNER in one pass over the combined text.
    let mut spans = collect_regex_spans(&combined);
    let gliner_entities = query_gliner(&combined, &gliner_url()).await;
    if !gliner_entities.is_empty() {
        merge_gliner_spans(&mut spans, gliner_entities);
    }
    dedupe_spans(&mut spans);

    // Build a shared mapping from the combined spans.
    let mut mapping = PiiMapping::new();
    for span in &spans {
        mapping.placeholder_for(&span.original, span.category);
    }

    // Now apply the mapping to each message individually via simple
    // string replacement (the mapping already knows every entity).
    let anonymized = messages
        .iter()
        .map(|m| {
            if !matches!(m.role, crate::llm::Role::User | crate::llm::Role::Assistant) {
                return m.clone();
            }
            let mut content = m.content.clone();
            // Replace longest originals first to avoid partial clobbering
            let mut entries: Vec<_> = mapping.to_placeholder.iter().collect();
            entries.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
            for (original_lower, placeholder) in entries {
                // Case-insensitive replacement: find the original in content
                if let Ok(re) = Regex::new(&format!(r"(?i){}", regex::escape(original_lower))) {
                    content = re.replace_all(&content, placeholder.as_str()).into_owned();
                }
            }
            crate::llm::Message { content, ..m.clone() }
        })
        .collect();

    (anonymized, mapping)
}

/// Restore placeholders in `text` with the originals from `mapping`.
pub fn deanonymize(text: &str, mapping: &PiiMapping) -> String {
    let mut result = text.to_string();
    // Replace longest placeholders first to avoid partial matches
    let mut entries: Vec<_> = mapping.to_original.iter().collect();
    entries.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    for (placeholder, original) in entries {
        result = result.replace(placeholder.as_str(), original);
    }
    result
}

// ── tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: run anonymize without GLiNER (regex-only).
    fn anon_sync(text: &str) -> (String, PiiMapping) {
        let mut spans = collect_regex_spans(text);
        dedupe_spans(&mut spans);
        let mut mapping = PiiMapping::new();
        let mut result = String::with_capacity(text.len());
        let mut cursor = 0usize;
        for span in &spans {
            if span.start > cursor {
                result.push_str(&text[cursor..span.start]);
            }
            let placeholder = mapping.placeholder_for(&span.original, span.category);
            result.push_str(&placeholder);
            cursor = span.end;
        }
        if cursor < text.len() {
            result.push_str(&text[cursor..]);
        }
        (result, mapping)
    }

    #[test]
    fn roundtrip_aadhaar() {
        let (anon, map) = anon_sync("His Aadhaar is 1234 5678 9012.");
        assert!(anon.contains("AADHAAR_01"));
        assert!(!anon.contains("1234"));
        let restored = deanonymize(&anon, &map);
        assert!(restored.contains("1234 5678 9012"));
    }

    #[test]
    fn roundtrip_pan() {
        let (anon, map) = anon_sync("PAN: ABCDE1234F on file.");
        assert!(anon.contains("PAN_01"));
        let restored = deanonymize(&anon, &map);
        assert!(restored.contains("ABCDE1234F"));
    }

    #[test]
    fn roundtrip_phone() {
        let (anon, map) = anon_sync("Call +91 9876543210 or 8765432109.");
        assert!(anon.contains("PHONE_01"));
        assert!(anon.contains("PHONE_02"));
        let restored = deanonymize(&anon, &map);
        assert!(restored.contains("9876543210"));
        assert!(restored.contains("8765432109"));
    }

    #[test]
    fn roundtrip_person_name() {
        let (anon, map) =
            anon_sync("Shri Rajesh Kumar filed against Smt. Anita Sharma.");
        assert!(anon.contains("PERSON_01"));
        assert!(anon.contains("PERSON_02"));
        assert!(!anon.contains("Rajesh"));
        let restored = deanonymize(&anon, &map);
        assert!(restored.contains("Rajesh Kumar"));
        assert!(restored.contains("Anita Sharma"));
    }

    #[test]
    fn same_entity_reuses_placeholder() {
        let (anon, map) = anon_sync(
            "Shri Rajesh Kumar stated that Shri Rajesh Kumar was present.",
        );
        // Both occurrences should map to the same placeholder
        let count = anon.matches("PERSON_01").count();
        assert_eq!(count, 2);
        assert_eq!(map.to_original.len(), 1);
    }

    #[test]
    fn combined_doc_roundtrip() {
        let input = "Shri Arun Patel, PAN BQRPN4321A, Aadhaar 4321 8765 0912, \
                     Mobile: +91 9988776655, Email: arun@example.com";
        let (anon, map) = anon_sync(input);
        assert!(!anon.contains("Arun Patel"));
        assert!(!anon.contains("BQRPN4321A"));
        assert!(!anon.contains("9988776655"));
        assert!(!anon.contains("arun@example.com"));
        let restored = deanonymize(&anon, &map);
        assert!(restored.contains("Arun Patel"));
        assert!(restored.contains("BQRPN4321A"));
        assert!(restored.contains("arun@example.com"));
    }

    #[test]
    fn ifsc_detected() {
        let (anon, _) = anon_sync("IFSC: SBIN0001234 for the branch.");
        assert!(anon.contains("IFSC_01"));
        assert!(!anon.contains("SBIN0001234"));
    }

    #[test]
    fn legal_terms_preserved() {
        let input = "Filed under Section 138 of the Negotiable Instruments Act.";
        let (anon, _) = anon_sync(input);
        assert_eq!(anon, input); // nothing should be redacted
    }
}
