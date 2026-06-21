#!/usr/bin/env python3
"""Scrub PII from extracted_texts.jsonl -> extracted_texts_clean.jsonl."""

import json
import re
import sys
from collections import Counter
from pathlib import Path

INPUT = Path(__file__).parent / "extracted_texts.jsonl"
OUTPUT = Path(__file__).parent / "extracted_texts_clean.jsonl"

REDACTED = "________"

TITLE_RE = r"(?:Sh\.?|Shri\.?|Smt\.?|Mr\.?|Mrs\.?|Ms\.?|Km\.?|Kumari)"

NON_NAME_WORDS = {
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
}

CITIES_STATES = {
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
}


# ---------------------------------------------------------------------------
# Party name extraction
# ---------------------------------------------------------------------------

def _clean_name(name: str) -> str:
    words = name.split()
    while words and words[-1].lower().rstrip(".") in NON_NAME_WORDS:
        words.pop()
    while words and words[0].lower().rstrip(".") in NON_NAME_WORDS:
        words.pop(0)
    return " ".join(words)


def _is_valid_name(name: str) -> bool:
    if not name or len(name) < 3:
        return False
    words = name.split()
    meaningful = [w for w in words if w.lower().rstrip(".") not in NON_NAME_WORDS]
    if not meaningful:
        return False
    # Single short all-caps word is likely an abbreviation, not a person name
    if len(words) == 1 and len(name) <= 4 and name.isupper():
        return False
    return True


def extract_party_names(filename: str, text: str) -> list[str]:
    names = []
    header = text[:4000]

    # From filename: "X V. Y" or "X Vs Y"
    stem = re.sub(r"\.[^.]+$", "", filename)
    stem = re.sub(r"^\([^)]*\)\s*", "", stem)  # remove court prefix like "(THC)"
    parts = re.split(r"\s+[Vv][Ss]?\.?\s+", stem, maxsplit=1)
    if len(parts) == 2:
        for part in parts:
            cleaned = re.sub(
                r"\s+(?:affidavit|rejoinder|evidence|ws|reply|application|petition|synopsis|order|notice|web_?\d*).*$",
                "", part, flags=re.IGNORECASE
            ).strip().rstrip(" -–—")
            cleaned = re.sub(r"\s*&\s*[Oo]rs\.?$", "", cleaned).strip()
            cleaned = _clean_name(cleaned)
            if _is_valid_name(cleaned):
                names.append(cleaned)

    # Title + Name in text header (case-insensitive title, case-sensitive name)
    NAME_RE = r"([A-Z][a-zA-Z']+(?:\s+[A-Z][a-zA-Z']+){0,4})"
    for m in re.finditer(rf"(?i:{TITLE_RE})\s+" + NAME_RE, header):
        name = _clean_name(m.group(1).strip())
        if _is_valid_name(name):
            names.append(name)

    # After s/o, d/o, w/o, son of, daughter of, wife of
    for m in re.finditer(
        r"(?i:s/o|d/o|w/o|son\s+of|daughter\s+of|wife\s+of)\s+(?i:Late\s+)?(?i:{TITLE_RE})\s*".format(TITLE_RE=TITLE_RE)
        + NAME_RE,
        header,
    ):
        name = _clean_name(m.group(1).strip())
        if _is_valid_name(name):
            names.append(name)

    # "I, Name, wife/son/daughter of"
    for m in re.finditer(
        r"I,\s+([A-Z][a-zA-Z']+(?:\s+[A-Z][a-zA-Z']+){0,3}),\s+(?:wife|son|daughter|husband)\s+of",
        header,
    ):
        name = _clean_name(m.group(1).strip())
        if _is_valid_name(name):
            names.append(name)

    # Deduplicate, longest first (avoids partial-match clobbering)
    seen = set()
    unique = []
    for n in names:
        key = n.lower()
        if key not in seen:
            seen.add(key)
            unique.append(n)
    unique.sort(key=len, reverse=True)
    return unique


# ---------------------------------------------------------------------------
# PII scrubbers
# ---------------------------------------------------------------------------

def scrub_names(text: str, names: list[str]) -> tuple[str, int]:
    count = 0
    for name in names:
        pat = re.compile(re.escape(name), re.IGNORECASE)
        found = len(pat.findall(text))
        if found:
            text = pat.sub(REDACTED, text)
            count += found
    return text, count


def scrub_aadhaar(text: str) -> tuple[str, int]:
    count = 0
    def repl(m):
        nonlocal count
        if len(re.sub(r"[\s-]", "", m.group())) == 12:
            count += 1
            return REDACTED
        return m.group()
    text = re.sub(r"\b\d{4}[\s-]?\d{4}[\s-]?\d{4}\b", repl, text)
    return text, count


def scrub_pan(text: str) -> tuple[str, int]:
    pat = r"\b[A-Z]{5}\d{4}[A-Z]\b"
    count = len(re.findall(pat, text))
    return re.sub(pat, REDACTED, text), count


def scrub_phone(text: str) -> tuple[str, int]:
    count = 0
    # +91 prefixed first (prevents Aadhaar false-match on 12-digit +91 strings)
    pat1 = r"\+91[\s-]?[6-9]\d{9}\b"
    count += len(re.findall(pat1, text))
    text = re.sub(pat1, REDACTED, text)
    # Standalone 10-digit Indian mobile
    pat2 = r"\b[6-9]\d{9}\b"
    count += len(re.findall(pat2, text))
    text = re.sub(pat2, REDACTED, text)
    return text, count


def scrub_email(text: str) -> tuple[str, int]:
    pat = r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b"
    count = len(re.findall(pat, text))
    return re.sub(pat, REDACTED, text), count


def scrub_bank_account(text: str) -> tuple[str, int]:
    count = 0
    def repl(m):
        nonlocal count
        count += 1
        return m.group(0).replace(m.group(1), REDACTED)
    text = re.sub(
        r"(?:account|a/c|bank\s*a/c|saving|current|acct)[\s.:No-]*(\d{9,18})",
        repl, text, flags=re.IGNORECASE,
    )
    return text, count


def scrub_address(text: str) -> tuple[str, int]:
    count = 0

    addr_re = re.compile(
        r"((?:r/o|resident\s+of|residing\s+at|address\s+(?:at|:)|located\s+at)\s+)"
        r"(.{5,200}\S*)",
        re.IGNORECASE,
    )

    def repl(m):
        nonlocal count
        prefix = m.group(1)
        raw = m.group(2)

        # Trim captured text at common post-address phrases
        addr = raw
        for stop in [r",?\s*hereinafter", r",?\s*which\s+expression", r",?\s*\(which", r",?\s*who\s+is"]:
            sm = re.search(stop, addr, re.IGNORECASE)
            if sm:
                addr = addr[:sm.start()]
                break
        addr = addr.strip().rstrip(",").strip()
        if len(addr) < 5:
            return m.group(0)

        # Preserve city/state names
        preserved = []
        for city in sorted(CITIES_STATES, key=len, reverse=True):
            cm = re.search(r"\b" + re.escape(city) + r"\b", addr, re.IGNORECASE)
            if cm and cm.group().lower() not in [p.lower() for p in preserved]:
                preserved.append(cm.group())

        count += 1
        result = prefix + REDACTED
        if preserved:
            result += ", " + ", ".join(preserved)
        result += raw[len(addr):]  # keep text after address
        return result

    text = addr_re.sub(repl, text)

    # Inline "at <number>/<details> <locality keyword>"
    def repl_inline(m):
        nonlocal count
        count += 1
        return m.group(1) + REDACTED
    text = re.sub(
        r"(\bat\s+)(\d+[/\-]\w[^.\n;]*?(?:colony|road|street|lane|nagar|vihar|enclave|gali|mohalla|block|sector)\b[^,.\n;]*)",
        repl_inline, text, flags=re.IGNORECASE,
    )

    return text, count


# Precompiled once at import: known city/state followed by a 6-digit pincode.
_CITY_ALT = "|".join(sorted((re.escape(c) for c in CITIES_STATES), key=len, reverse=True))
_CITY_PIN_RE = re.compile(rf"(\b(?:{_CITY_ALT}),?\s+)(\d{{6}})\b", re.IGNORECASE)


def scrub_pincode(text: str) -> tuple[str, int]:
    count = 0

    # Near "pin"/"pincode" keyword
    def repl1(m):
        nonlocal count
        count += 1
        return m.group(0).replace(m.group(1), REDACTED)
    text = re.sub(r"(?:pin\s*(?:code)?[\s.:/-]*)(\d{6})\b", repl1, text, flags=re.IGNORECASE)

    # City/area-dash-pincode: "Delhi-110032"
    def repl2(m):
        nonlocal count
        count += 1
        return m.group(1) + REDACTED
    text = re.sub(r"(\b[A-Za-z]+-)\s*(\d{6})\b", repl2, text)

    # Known city/state immediately followed by a 6-digit pin: "Delhi, 110059".
    # Skip 6-digit numbers ending in 000 — almost always monetary amounts, not PINs.
    def repl3(m):
        nonlocal count
        pin = m.group(2)
        if pin.endswith("000"):
            return m.group(0)
        count += 1
        return m.group(1) + REDACTED
    text = _CITY_PIN_RE.sub(repl3, text)

    return text, count


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def scrub_org_names(text: str) -> tuple[str, int]:
    """Redact organisation / firm names ending in a company-type suffix."""
    count = 0
    org_re = re.compile(
        r"\b(?:M/?S\.?\s+)?"
        r"([A-Z][\w&.'\-]*(?:\s+[A-Z0-9][\w&.'\-]*){0,5}\s+"
        r"(?i:pvt\.?\s*ltd\.?|private\s+limited|ltd\.?|l\.?l\.?p\.?|"
        r"&\s*co\.?|&\s*sons|&\s*associates|nursing\s+home|"
        r"insurance\s+co(?:mpany)?(?:\s+ltd\.?)?))\b"
    )

    def repl(m):
        nonlocal count
        count += 1
        return REDACTED

    return org_re.sub(repl, text), count


def scrub_case_number(text: str) -> tuple[str, int]:
    """Redact case / diary numbers, keeping the case-type label."""
    count = 0

    def repl(m):
        nonlocal count
        count += 1
        return m.group(1) + REDACTED

    text = re.sub(
        r"\b((?:Consumer\s+Case|Case|Complaint|Appeal|Second\s+Appeal|O\.?A\.?|"
        r"C\.?C\.?|W\.?P\.?(?:\s*\([A-Z]\))?|HMA|Crl\.?(?:\s*\w+)?|CS|FAO|RFA|SLP|"
        r"Suit|Bail\s+Application|Misc\.?(?:\s*\w+)?)\s*No\.?\s*)"
        r"(\d+(?:\s*/\s*\d+)*(?:\s+of\s+\d{4})?)",
        repl, text, flags=re.IGNORECASE,
    )

    # Slash-coded reference numbers, e.g. CIC/KVSAN/A/2017/164356-BJ
    def repl2(m):
        nonlocal count
        count += 1
        return REDACTED

    # Require at least one digit in the slash-chain so plain acronym chains
    # ("AND/OR/NOT", "DNA/RNA/ATP") are not redacted — case numbers have digits.
    text = re.sub(r"\b(?=[A-Z0-9/.\-]*\d)[A-Z]{2,}(?:/[A-Z0-9.\-]+){2,}\b", repl2, text)
    return text, count


def scrub_document(doc: dict) -> tuple[dict, Counter]:
    text = doc["text"]
    counts = Counter()

    # 1. Party names (extract before any text modifications)
    party_names = extract_party_names(doc.get("filename", ""), text)
    text, c = scrub_names(text, party_names)
    counts["party_names"] = c

    # 2-10. Structured PII patterns
    for label, fn in [
        ("phone", scrub_phone),
        ("aadhaar", scrub_aadhaar),
        ("pan", scrub_pan),
        ("email", scrub_email),
        ("bank_account", scrub_bank_account),
        ("org_names", scrub_org_names),
        ("case_number", scrub_case_number),
        ("address", scrub_address),
        ("pincode", scrub_pincode),
    ]:
        text, c = fn(text)
        counts[label] = c

    # Scrub the filename + filepath too — they often embed party names ("X v Y").
    def _scrub_path(s: str) -> str:
        s, _ = scrub_names(s, party_names)
        s = re.sub(r"\b[\w.'\-]+\s+[Vv][Ss]?\.?\s+[\w.'\-]+", f"{REDACTED} v {REDACTED}", s)
        return s

    out = {**doc, "text": text}
    if "filename" in doc:
        out["filename"] = _scrub_path(doc["filename"])
    if "filepath" in doc:
        out["filepath"] = _scrub_path(doc["filepath"])
    return out, counts


def main():
    if not INPUT.exists():
        print(f"Error: {INPUT} not found.", file=sys.stderr)
        sys.exit(1)

    docs = []
    with open(INPUT) as f:
        for line in f:
            if line.strip():
                docs.append(json.loads(line))

    total = Counter()
    pii_docs = 0

    with open(OUTPUT, "w") as out:
        for doc in docs:
            cleaned, counts = scrub_document(doc)
            out.write(json.dumps(cleaned, ensure_ascii=False) + "\n")
            if sum(counts.values()):
                pii_docs += 1
            total += counts

    print(f"\nPII Scrubbing Summary")
    print("=" * 40)
    print(f"Documents processed: {len(docs)}")
    print(f"Documents with PII:  {pii_docs}")
    print(f"\nReplacements by type:")
    for t in ["party_names", "org_names", "case_number", "aadhaar", "pan", "phone", "email", "bank_account", "address", "pincode"]:
        print(f"  {t:20s} {total.get(t, 0):5d}")
    print(f"  {'TOTAL':20s} {sum(total.values()):5d}")
    print(f"\nOutput: {OUTPUT}")


if __name__ == "__main__":
    main()
