#!/usr/bin/env python3
"""Read extracted_texts.jsonl, detect doc type / court / parties, generate instruction-response pairs."""

import json
import re
import sys
from collections import Counter
from pathlib import Path

INPUT = Path(__file__).parent / "extracted_texts_clean.jsonl"
OUTPUT = Path(__file__).parent / "raw_pairs.jsonl"

MIN_TEXT_LEN = 50  # skip docs with barely any extracted text

# ---------------------------------------------------------------------------
# Document type detection
# ---------------------------------------------------------------------------

DOC_TYPE_KEYWORDS = [
    # order matters: more specific first
    ("written_statement", [r"\bwritten\s+statement\b", r"\bw\.?\s*s\b\.?\s+on\s+behalf", r"\bw\.?\s*s\b\.?\s+opposite", r"\bws\s+on\s+behalf", r"\bws\s+opposite"]),
    ("rejoinder", [r"\brejoinder\b"]),
    ("affidavit", [r"\baffidavit\b"]),
    ("reply", [r"\breply\b"]),
    ("petition", [r"\bpetition\b", r"\bwrit\s+petition\b", r"\bclaim\s+petition\b"]),
    ("application", [r"\bapplication\b"]),
    ("settlement", [r"\bsettlement\b", r"\bmemorandum\s+of\s+understanding\b", r"\bmou\b"]),
    ("poa", [r"\bpower\s+of\s+attorney\b", r"\bpoa\b"]),
    ("complaint", [r"\bcomplaint\b", r"\bct\.?\s*case\b"]),
    ("notice", [r"\bnotice\b"]),
    ("evidence", [r"\bevidence\b"]),
    ("synopsis", [r"\bsynopsis\b"]),
    ("order", [r"\border\b.*\bcourt\b", r"\bcopy\s+of\s+the\s+order\b"]),
]


def detect_doc_type(filename: str, text: str) -> str:
    combined = (filename + " " + text[:1000]).lower()
    for dtype, patterns in DOC_TYPE_KEYWORDS:
        for pat in patterns:
            if re.search(pat, combined):
                return dtype
    return "other"


# ---------------------------------------------------------------------------
# Court detection
# ---------------------------------------------------------------------------

COURT_PREFIXES = {
    "AFT": "Armed Forces Tribunal",
    "DHC": "Delhi High Court",
    "THC": "Tis Hazari Courts",
    "PHC": "Patiala House Courts",
    "KKD": "Karkardooma Courts",
    "TDSAT": "TDSAT",
    "LABOUR COURT": "Labour Court",
    "SOUTH DELHI": "South Delhi District Court",
    "SOUTH-WEST": "South-West District Court",
    "SOUTH II": "South-II District Court",
    "ROHINI": "Rohini Courts",
    "DWARKA": "Dwarka Courts",
}

COURT_CONTENT_PATTERNS = [
    (r"high\s+court\s+of\s+delhi", "Delhi High Court"),
    (r"armed\s+forces\s+tribunal", "Armed Forces Tribunal"),
    (r"family\s+court", "Family Court"),
    (r"labour\s+court", "Labour Court"),
    (r"consumer\s+disputes?\s+redressal", "Consumer Court"),
    (r"district\s+court", "District Court"),
    (r"sessions?\s+court", "Sessions Court"),
    (r"metropolitan\s+magistrate", "Metropolitan Magistrate Court"),
    (r"tis\s+hazari", "Tis Hazari Courts"),
    (r"patiala\s+house", "Patiala House Courts"),
    (r"karkardooma", "Karkardooma Courts"),
    (r"rohini", "Rohini Courts"),
    (r"dwarka", "Dwarka Courts"),
    (r"saket", "Saket Courts"),
    (r"supreme\s+court", "Supreme Court of India"),
    (r"central\s+information\s+commission", "Central Information Commission"),
]


def detect_court(filepath: str, filename: str, text: str) -> str:
    combined_path = (filepath + " " + filename).upper()
    # Check for parenthesized prefix in filename, e.g. "(AFT)", "(DHC)"
    paren_match = re.search(r"\(([^)]+)\)", filename)
    if paren_match:
        tag = paren_match.group(1).strip().upper()
        for prefix, court in COURT_PREFIXES.items():
            if prefix.upper() in tag:
                return court

    # Check path for court prefix without parens
    for prefix, court in COURT_PREFIXES.items():
        if prefix.upper() in combined_path:
            return court

    # Scan first 2000 chars of content
    text_lower = text[:2000].lower()
    for pat, court in COURT_CONTENT_PATTERNS:
        if re.search(pat, text_lower):
            return court

    return "Unknown"


# ---------------------------------------------------------------------------
# Party name extraction
# ---------------------------------------------------------------------------

def extract_parties(filename: str) -> dict:
    """Extract petitioner and respondent from 'X V. Y' or 'X Vs Y' in filename."""
    stem = re.sub(r"\.[^.]+$", "", filename)  # strip extension
    # Remove parenthesized court prefix
    stem = re.sub(r"^\([^)]*\)\s*", "", stem)

    match = re.split(r"\s+[Vv][Ss]?\.?\s+", stem, maxsplit=1)
    if len(match) == 2:
        petitioner = match[0].strip().rstrip(" -–—")
        respondent = match[1].strip().rstrip(" -–—")
        # Clean trailing doc-type words from respondent
        respondent = re.sub(
            r"\s+(affidavit|rejoinder|evidence|ws|w\.s|reply|application|petition|synopsis|order|notice).*$",
            "", respondent, flags=re.IGNORECASE
        ).strip()
        if petitioner and respondent:
            return {"petitioner": petitioner, "respondent": respondent}
    return {}


# ---------------------------------------------------------------------------
# Instruction generation
# ---------------------------------------------------------------------------

DOC_TYPE_LABELS = {
    "petition": "petition",
    "written_statement": "written statement",
    "rejoinder": "rejoinder",
    "affidavit": "affidavit",
    "application": "application",
    "settlement": "settlement agreement",
    "poa": "power of attorney",
    "complaint": "complaint",
    "notice": "legal notice",
    "reply": "reply",
    "evidence": "evidence affidavit",
    "synopsis": "synopsis",
    "order": "court order",
    "other": "legal document",
}


def first_sentences(text: str, n: int = 2) -> str:
    """Extract first n meaningful sentences from text."""
    cleaned = re.sub(r"\s+", " ", text.strip())
    # Skip leading whitespace / page headers
    cleaned = re.sub(r"^(page\s+\d+\s+of\s+\d+\s*)+", "", cleaned, flags=re.IGNORECASE).strip()
    sentences = re.split(r"(?<=[.!?])\s+", cleaned)
    meaningful = [s for s in sentences if len(s) > 20][:n]
    return " ".join(meaningful) if meaningful else cleaned[:300]


def build_instruction(doc_type: str, court: str, parties: dict, text: str) -> str:
    label = DOC_TYPE_LABELS.get(doc_type, "legal document")
    context = first_sentences(text)

    article = "an" if label[0] in "aeiou" else "a"
    parts = [f"Draft {article} {label}"]
    if parties:
        parts.append(f"on behalf of {parties['petitioner']} in the case of {parties['petitioner']} v. {parties['respondent']}")
    if court and court != "Unknown":
        parts.append(f"before the {court}")

    instruction = " ".join(parts) + "."
    if context:
        instruction += f" The matter involves: {context}"
    return instruction


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    if not INPUT.exists():
        print(f"Error: {INPUT} not found. Run prepare_data.py first.", file=sys.stderr)
        sys.exit(1)

    docs = []
    with open(INPUT) as f:
        for line in f:
            if line.strip():
                docs.append(json.loads(line))

    type_counts = Counter()
    court_counts = Counter()
    skipped = 0
    written = 0

    with open(OUTPUT, "w") as out:
        for doc in docs:
            text = doc["text"].strip()
            if len(text) < MIN_TEXT_LEN:
                skipped += 1
                continue

            filepath = doc.get("filepath", "")
            filename = doc.get("filename", "")

            doc_type = detect_doc_type(filename, text)
            court = detect_court(filepath, filename, text)
            parties = extract_parties(filename)

            instruction = build_instruction(doc_type, court, parties, text)

            record = {
                "instruction": instruction,
                "response": text,
                "doc_type": doc_type,
                "court": court,
                "parties": parties,
                "source_file": filename,
            }
            out.write(json.dumps(record, ensure_ascii=False) + "\n")
            type_counts[doc_type] += 1
            court_counts[court] += 1
            written += 1

    # Summary
    print(f"\nProcessed {len(docs)} documents, wrote {written} pairs, skipped {skipped} (text too short)\n")

    print("Document types:")
    for dtype, count in type_counts.most_common():
        print(f"  {dtype:20s} {count}")

    print(f"\nCourts:")
    for court, count in court_counts.most_common():
        print(f"  {court:40s} {count}")

    print(f"\nOutput: {OUTPUT}")


if __name__ == "__main__":
    main()
