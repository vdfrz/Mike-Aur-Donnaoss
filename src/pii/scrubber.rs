//! PII scrubber for Indian legal documents.
//!
//! Detects and replaces: party names, Aadhaar, PAN, phone, email,
//! bank account numbers, addresses, and pin codes with "________".

use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

const REDACTED: &str = "________";

#[derive(Debug, Clone, Default)]
pub struct ScrubResult {
    pub scrubbed_text: String,
    pub counts: HashMap<String, usize>,
}

// ---------------------------------------------------------------------------
// Static regex patterns (compiled once)
// ---------------------------------------------------------------------------

static RE_AADHAAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b\d{4}[\s\-]?\d{4}[\s\-]?\d{4}\b").unwrap());

static RE_PAN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[A-Z]{5}\d{4}[A-Z]\b").unwrap());

static RE_PHONE_PLUS91: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\+91[\s\-]?[6-9]\d{9}\b").unwrap());

static RE_PHONE_BARE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[6-9]\d{9}\b").unwrap());

static RE_EMAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}\b").unwrap());

static RE_BANK_ACCOUNT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:account|a/c|bank\s*a/c|saving|current|acct)[\s.:No\-]*(\d{9,18})").unwrap()
});

static RE_ADDRESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)((?:r/o|resident\s+of|residing\s+at|address\s+(?:at|:)|located\s+at)\s+)(.{5,200}\S*)").unwrap()
});

static RE_ADDRESS_INLINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(\bat\s+)(\d+[/\-]\w[^.\n;]*?(?:colony|road|street|lane|nagar|vihar|enclave|gali|mohalla|block|sector)\b[^,.\n;]*)").unwrap()
});

static RE_PINCODE_KEYWORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)pin\s*(?:code)?[\s.:\-/]*(\d{6})\b").unwrap()
});

static RE_PINCODE_CITY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([A-Za-z]+\-)\s*(\d{6})\b").unwrap());

static RE_TITLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i:Sh\.?|Shri\.?|Smt\.?|Mr\.?|Mrs\.?|Ms\.?|Km\.?|Kumari)\s+([A-Z][a-zA-Z']+(?:\s+[A-Z][a-zA-Z']+){0,4})").unwrap()
});

static RE_RELATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i:s/o|d/o|w/o|son\s+of|daughter\s+of|wife\s+of)\s+(?i:Late\s+)?(?i:Sh\.?|Shri\.?|Smt\.?|Mr\.?|Mrs\.?|Ms\.?|Km\.?|Kumari)?\s*([A-Z][a-zA-Z']+(?:\s+[A-Z][a-zA-Z']+){0,4})").unwrap()
});

static RE_I_COMMA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"I,\s+([A-Z][a-zA-Z']+(?:\s+[A-Z][a-zA-Z']+){0,3}),\s+(?:wife|son|daughter|husband)\s+of").unwrap()
});

// ---------------------------------------------------------------------------
// City/State list (preserved during address scrubbing)
// ---------------------------------------------------------------------------

const CITIES_STATES: &[&str] = &[
    "delhi", "new delhi", "mumbai", "kolkata", "chennai", "bangalore", "bengaluru",
    "hyderabad", "pune", "ahmedabad", "jaipur", "lucknow", "chandigarh",
    "gurugram", "gurgaon", "noida", "faridabad", "ghaziabad",
    "bhopal", "indore", "patna", "ranchi", "dehradun", "shimla",
    "nagpur", "surat", "vadodara", "kochi",
    "maharashtra", "karnataka", "tamil nadu", "telangana", "andhra pradesh",
    "uttar pradesh", "rajasthan", "madhya pradesh", "bihar", "jharkhand",
    "west bengal", "odisha", "kerala", "gujarat", "punjab",
    "haryana", "uttarakhand", "himachal pradesh", "goa", "chhattisgarh",
    "jammu", "kashmir",
];

// Words that are NOT personal names (used to filter extracted "names")
const NON_NAME_WORDS: &[&str] = &[
    "petitioner", "respondent", "complainant", "accused", "plaintiff",
    "defendant", "applicant", "appellant", "revisionist", "licensor", "licensee",
    "court", "judge", "justice", "advocate", "counsel", "learned",
    "section", "act", "code", "article", "rule", "order", "petition",
    "affidavit", "application", "complaint", "notice", "reply", "evidence",
    "written", "statement", "rejoinder", "synopsis", "settlement", "agreement",
    "power", "attorney", "versus", "behalf", "opposite", "party",
    "india", "indian", "union", "state", "government", "govt", "tribunal",
    "commission", "committee", "authority", "board", "police", "hospital",
    "bank", "limited", "ltd", "pvt", "private", "public", "university",
    "the", "and", "or", "of", "in", "at", "to", "for", "by", "from", "with",
    "that", "this", "is", "was", "are", "were", "has", "have", "had",
    "shall", "will", "would", "may", "might", "can", "could", "should",
    "hereinafter", "called", "referred", "above", "said", "present",
    "between", "part", "other", "also", "further", "male", "female", "adult",
    "hindu", "muslim", "sikh", "christian", "inhabitants",
    "resident", "residing", "aged", "years", "having", "address", "bearing",
    "working", "whereas", "therefore", "submitted", "filed",
    "sh", "shri", "smt", "mr", "mrs", "ms", "dr", "late", "km", "kumari",
    "ors", "anr", "oic", "misc", "criminal", "civil", "writ", "special",
    "case", "matter", "no", "persons",
];

// ---------------------------------------------------------------------------
// Name extraction (from header text)
// ---------------------------------------------------------------------------

fn is_non_name_word(w: &str) -> bool {
    NON_NAME_WORDS.contains(&w.to_lowercase().trim_end_matches('.'))
}

fn clean_name(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let mut start = 0;
    let mut end = words.len();
    while start < end && is_non_name_word(words[start]) {
        start += 1;
    }
    while end > start && is_non_name_word(words[end - 1]) {
        end -= 1;
    }
    words[start..end].join(" ")
}

fn is_valid_name(name: &str) -> bool {
    if name.len() < 3 {
        return false;
    }
    let words: Vec<&str> = name.split_whitespace().collect();
    let meaningful: Vec<&&str> = words.iter().filter(|w| !is_non_name_word(w)).collect();
    if meaningful.is_empty() {
        return false;
    }
    if words.len() == 1 && name.len() <= 4 && name.chars().all(|c| c.is_uppercase()) {
        return false;
    }
    true
}

fn extract_party_names(text: &str) -> Vec<String> {
    let header = if text.len() > 4000 {
        // Snap to a char boundary so multibyte input (₹, Devanagari,
        // smart quotes) straddling byte 4000 doesn't panic the slice.
        let cut = (0..=4000).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0);
        &text[..cut]
    } else {
        text
    };
    let mut names: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    let mut try_add = |raw: &str| {
        let cleaned = clean_name(raw.trim());
        if is_valid_name(&cleaned) {
            let key = cleaned.to_lowercase();
            if !seen.contains(&key) {
                seen.insert(key);
                names.push(cleaned);
            }
        }
    };

    for cap in RE_TITLE.captures_iter(header) {
        if let Some(m) = cap.get(1) {
            try_add(m.as_str());
        }
    }

    for cap in RE_RELATION.captures_iter(header) {
        if let Some(m) = cap.get(1) {
            try_add(m.as_str());
        }
    }

    for cap in RE_I_COMMA.captures_iter(header) {
        if let Some(m) = cap.get(1) {
            try_add(m.as_str());
        }
    }

    // Sort longest first so replacement doesn't clobber substrings
    names.sort_by(|a, b| b.len().cmp(&a.len()));
    names
}

// ---------------------------------------------------------------------------
// Individual scrubbers
// ---------------------------------------------------------------------------

fn scrub_names(text: &mut String) -> usize {
    let names = extract_party_names(text);
    let mut count = 0;
    for name in &names {
        let pat = Regex::new(&format!(r"(?i){}", regex::escape(name))).unwrap();
        let found = pat.find_iter(text).count();
        if found > 0 {
            *text = pat.replace_all(text, REDACTED).into_owned();
            count += found;
        }
    }
    count
}

fn scrub_aadhaar(text: &mut String) -> usize {
    let mut count = 0;
    *text = RE_AADHAAR
        .replace_all(text, |caps: &regex::Captures| {
            let digits: String = caps[0].chars().filter(|c| c.is_ascii_digit()).collect();
            if digits.len() == 12 {
                count += 1;
                REDACTED.to_string()
            } else {
                caps[0].to_string()
            }
        })
        .into_owned();
    count
}

fn scrub_pan(text: &mut String) -> usize {
    let count = RE_PAN.find_iter(text).count();
    *text = RE_PAN.replace_all(text, REDACTED).into_owned();
    count
}

fn scrub_phone(text: &mut String) -> usize {
    let mut count = 0;
    // +91 prefixed first
    count += RE_PHONE_PLUS91.find_iter(text).count();
    *text = RE_PHONE_PLUS91.replace_all(text, REDACTED).into_owned();
    // Bare 10-digit
    count += RE_PHONE_BARE.find_iter(text).count();
    *text = RE_PHONE_BARE.replace_all(text, REDACTED).into_owned();
    count
}

fn scrub_email(text: &mut String) -> usize {
    let count = RE_EMAIL.find_iter(text).count();
    *text = RE_EMAIL.replace_all(text, REDACTED).into_owned();
    count
}

fn scrub_bank_account(text: &mut String) -> usize {
    let mut count = 0;
    *text = RE_BANK_ACCOUNT
        .replace_all(text, |caps: &regex::Captures| {
            count += 1;
            caps[0].replace(&caps[1], REDACTED)
        })
        .into_owned();
    count
}

fn scrub_address(text: &mut String) -> usize {
    let mut count = 0;

    // Primary address pattern (r/o, residing at, etc.)
    *text = RE_ADDRESS
        .replace_all(text, |caps: &regex::Captures| {
            let prefix = &caps[1];
            let raw = &caps[2];

            // Trim at common post-address phrases
            let mut addr = raw.to_string();
            let stop_patterns = [
                r"(?i),?\s*hereinafter",
                r"(?i),?\s*which\s+expression",
                r"(?i),?\s*\(which",
                r"(?i),?\s*who\s+is",
            ];
            for stop in &stop_patterns {
                if let Ok(re) = Regex::new(stop) {
                    if let Some(m) = re.find(&addr) {
                        addr = addr[..m.start()].to_string();
                        break;
                    }
                }
            }
            let addr = addr.trim().trim_end_matches(',').trim();
            if addr.len() < 5 {
                return caps[0].to_string();
            }

            // Preserve city/state names
            let mut preserved: Vec<String> = Vec::new();
            for city in CITIES_STATES.iter().rev() {
                let city_re = Regex::new(&format!(r"(?i)\b{}\b", regex::escape(city))).unwrap();
                if let Some(m) = city_re.find(addr) {
                    let found = m.as_str().to_string();
                    if !preserved.iter().any(|p| p.eq_ignore_ascii_case(&found)) {
                        preserved.push(found);
                    }
                }
            }

            count += 1;
            let mut result = format!("{prefix}{REDACTED}");
            if !preserved.is_empty() {
                result.push_str(", ");
                result.push_str(&preserved.join(", "));
            }
            // Append text after the address portion
            if addr.len() < raw.len() {
                result.push_str(&raw[addr.len()..]);
            }
            result
        })
        .into_owned();

    // Inline "at <number>/<details> <locality keyword>"
    *text = RE_ADDRESS_INLINE
        .replace_all(text, |caps: &regex::Captures| {
            count += 1;
            format!("{}{REDACTED}", &caps[1])
        })
        .into_owned();

    count
}

fn scrub_pincode(text: &mut String) -> usize {
    let mut count = 0;

    // Near "pin"/"pincode" keyword
    *text = RE_PINCODE_KEYWORD
        .replace_all(text, |caps: &regex::Captures| {
            count += 1;
            caps[0].replace(&caps[1], REDACTED)
        })
        .into_owned();

    // City-dash-pincode: "Delhi-110032"
    *text = RE_PINCODE_CITY
        .replace_all(text, |caps: &regex::Captures| {
            count += 1;
            format!("{}{REDACTED}", &caps[1])
        })
        .into_owned();

    count
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

pub fn scrub_pii(text: &str) -> ScrubResult {
    let mut out = text.to_string();
    let mut counts = HashMap::new();

    // Order matters: names first (before text is modified), then structured patterns.
    // Pincode before address (address regex can swallow "located at pin code XXXXXX").
    counts.insert("party_names".into(), scrub_names(&mut out));
    counts.insert("phone".into(), scrub_phone(&mut out));
    counts.insert("aadhaar".into(), scrub_aadhaar(&mut out));
    counts.insert("pan".into(), scrub_pan(&mut out));
    counts.insert("email".into(), scrub_email(&mut out));
    counts.insert("bank_account".into(), scrub_bank_account(&mut out));
    counts.insert("pincode".into(), scrub_pincode(&mut out));
    counts.insert("address".into(), scrub_address(&mut out));

    ScrubResult {
        scrubbed_text: out,
        counts,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aadhaar_with_spaces() {
        let r = scrub_pii("His Aadhaar number is 1234 5678 9012 on record.");
        assert_eq!(r.counts["aadhaar"], 1);
        assert!(r.scrubbed_text.contains(REDACTED));
        assert!(!r.scrubbed_text.contains("1234"));
    }

    #[test]
    fn test_aadhaar_with_dashes() {
        let r = scrub_pii("Aadhaar: 9876-5432-1098");
        assert_eq!(r.counts["aadhaar"], 1);
        assert!(!r.scrubbed_text.contains("9876"));
    }

    #[test]
    fn test_aadhaar_continuous() {
        let r = scrub_pii("ID 123456789012 filed.");
        assert_eq!(r.counts["aadhaar"], 1);
    }

    #[test]
    fn test_pan_card() {
        let r = scrub_pii("PAN: ABCDE1234F belongs to accused.");
        assert_eq!(r.counts["pan"], 1);
        assert!(!r.scrubbed_text.contains("ABCDE1234F"));
    }

    #[test]
    fn test_phone_with_plus91() {
        let r = scrub_pii("Contact: +91 9876543210 for details.");
        assert_eq!(r.counts["phone"], 1);
        assert!(!r.scrubbed_text.contains("9876543210"));
    }

    #[test]
    fn test_phone_with_plus91_dash() {
        let r = scrub_pii("Phone: +91-7654321098");
        assert_eq!(r.counts["phone"], 1);
        assert!(!r.scrubbed_text.contains("7654321098"));
    }

    #[test]
    fn test_phone_bare() {
        let r = scrub_pii("Mobile 9812345678 registered.");
        assert_eq!(r.counts["phone"], 1);
        assert!(!r.scrubbed_text.contains("9812345678"));
    }

    #[test]
    fn test_email() {
        let r = scrub_pii("Email: sharma.rajesh@gmail.com for communication.");
        assert_eq!(r.counts["email"], 1);
        assert!(!r.scrubbed_text.contains("sharma.rajesh@gmail.com"));
    }

    #[test]
    fn test_bank_account() {
        let r = scrub_pii("His account no. 12345678901234 at SBI Connaught Place.");
        assert_eq!(r.counts["bank_account"], 1);
        assert!(!r.scrubbed_text.contains("12345678901234"));
    }

    #[test]
    fn test_bank_account_ac() {
        let r = scrub_pii("Credited to a/c 987654321012345");
        assert_eq!(r.counts["bank_account"], 1);
        assert!(!r.scrubbed_text.contains("987654321012345"));
    }

    #[test]
    fn test_address_residing_at() {
        let r = scrub_pii("Smt. Sunita Devi, residing at H.No. 45, Sector 12, Rohini, New Delhi, hereinafter called");
        assert_eq!(r.counts["address"], 1);
        assert!(r.scrubbed_text.contains(REDACTED));
        // City should be preserved
        assert!(r.scrubbed_text.contains("New Delhi") || r.scrubbed_text.contains("Delhi"));
    }

    #[test]
    fn test_address_r_o() {
        let r = scrub_pii("Sh. Ramesh Kumar, r/o Village Khanpur, Faridabad, Haryana");
        assert_eq!(r.counts["address"], 1);
        assert!(r.scrubbed_text.contains("Faridabad") || r.scrubbed_text.contains("Haryana"));
    }

    #[test]
    fn test_pincode_keyword() {
        let r = scrub_pii("Located at pin code 110032 within jurisdiction.");
        assert_eq!(r.counts["pincode"], 1);
        assert!(!r.scrubbed_text.contains("110032"));
    }

    #[test]
    fn test_pincode_city_dash() {
        let r = scrub_pii("Residing in Delhi-110001");
        assert_eq!(r.counts["pincode"], 1);
        assert!(!r.scrubbed_text.contains("110001"));
    }

    #[test]
    fn test_party_name_title() {
        let r = scrub_pii("Shri Rajesh Kumar filed the petition against Smt. Anita Sharma.");
        assert!(r.counts["party_names"] >= 2);
        assert!(!r.scrubbed_text.contains("Rajesh Kumar"));
        assert!(!r.scrubbed_text.contains("Anita Sharma"));
    }

    #[test]
    fn test_party_name_relation() {
        let r = scrub_pii("Accused Sh. Vikram Singh s/o Shri Baldev Singh, aged 35 years.");
        assert!(r.counts["party_names"] >= 2);
        assert!(!r.scrubbed_text.contains("Vikram Singh"));
        assert!(!r.scrubbed_text.contains("Baldev Singh"));
    }

    #[test]
    fn test_does_not_scrub_court_names() {
        let input = "Filed before the Delhi High Court under Section 138 of the Negotiable Instruments Act.";
        let r = scrub_pii(input);
        assert!(r.scrubbed_text.contains("Delhi High Court"));
        assert!(r.scrubbed_text.contains("Negotiable Instruments Act"));
    }

    #[test]
    fn test_does_not_scrub_dates() {
        let input = "The hearing is scheduled for 15.03.2026 at 10:30 AM.";
        let r = scrub_pii(input);
        assert!(r.scrubbed_text.contains("15.03.2026"));
    }

    #[test]
    fn test_multibyte_at_header_boundary_does_not_panic() {
        // A multibyte char (₹) straddling byte 4000 must not panic the
        // header slice in extract_party_names. (Before the fix this slice
        // panicked with "byte index 4000 is not a char boundary".)
        for offset in 3998..=4001 {
            // Name BEFORE the boundary so it's inside the 4000-byte header.
            let mut s = String::from("Shri Rajesh Kumar filed the petition. ");
            while s.len() < offset {
                s.push('a');
            }
            s.push('₹'); // 3-byte char crosses byte 4000 for most offsets
            s.push_str(" trailing text continues.");
            let r = scrub_pii(&s); // must not panic
            assert!(!r.scrubbed_text.contains("Rajesh Kumar"));
        }
    }

    #[test]
    fn test_devanagari_and_smart_quotes_no_panic() {
        // Devanagari + smart quotes straddling the 4000-byte cut must not panic.
        let mut s = String::from("“Shri Rajesh Kumar” filed. ");
        s.push_str(&"धारा ".repeat(900)); // multibyte, pushes well past 4000 bytes
        let r = scrub_pii(&s); // must not panic
        assert!(r.scrubbed_text.contains("धारा"));
        assert!(!r.scrubbed_text.contains("Rajesh Kumar"));
    }

    #[test]
    fn test_combined_document() {
        let input = "\
Shri Arun Patel s/o Shri Dinesh Patel, aged 42 years, \
residing at 12/B Nehru Nagar, Lucknow, Uttar Pradesh, Pin Code: 226001, \
Aadhaar No. 4321 8765 0912, PAN BQRPN4321A, \
Mobile: +91 9988776655, Email: arun.patel@yahoo.co.in, \
A/c No. 123456789012345 at PNB Main Branch.";

        let r = scrub_pii(input);
        assert!(!r.scrubbed_text.contains("Arun Patel"));
        assert!(!r.scrubbed_text.contains("Dinesh Patel"));
        assert!(!r.scrubbed_text.contains("4321 8765 0912"));
        assert!(!r.scrubbed_text.contains("BQRPN4321A"));
        assert!(!r.scrubbed_text.contains("9988776655"));
        assert!(!r.scrubbed_text.contains("arun.patel@yahoo.co.in"));
        assert!(!r.scrubbed_text.contains("123456789012345"));
        assert!(!r.scrubbed_text.contains("226001"));
        // Preserved
        assert!(r.scrubbed_text.contains("Lucknow") || r.scrubbed_text.contains("Uttar Pradesh"));
    }
}
