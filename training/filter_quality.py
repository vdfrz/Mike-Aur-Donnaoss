#!/usr/bin/env python3
"""Filter raw_pairs.jsonl to remove low-quality entries."""

import json
import os
import re
import unicodedata
from difflib import SequenceMatcher

INPUT_FILE = "training/raw_pairs.jsonl"
OUTPUT_FILE = "training/raw_pairs_clean.jsonl"

CJK_PATTERN = re.compile(r'[一-鿿㐀-䶿぀-ゟ゠-ヿ가-힯]')
CONSONANTS_RUN = re.compile(r'[bcdfghjklmnpqrstvwxyz]{3,}', re.IGNORECASE)
OCR_JUNK = re.compile(r'[|}{)\]\[(<>]{2,}')
MIXED_GARBAGE = re.compile(r'[A-Za-z][0-9|/\\]{2,}[A-Za-z]')


def has_cjk(text):
    return bool(CJK_PATTERN.search(text))


def is_garbled_word(word):
    clean = word.strip(".,;:!?\"'()-")
    if not clean:
        return False
    non_ascii = sum(1 for ch in clean if ord(ch) > 127 and unicodedata.category(ch) not in ('Ll', 'Lu', 'Lt', 'Lo', 'Nd', 'Pc', 'Pd', 'Po', 'Ps', 'Pe'))
    if non_ascii > 0:
        return True
    if CONSONANTS_RUN.search(clean):
        return True
    if OCR_JUNK.search(clean):
        return True
    if MIXED_GARBAGE.search(clean):
        return True
    return False


def garbled_ratio(text):
    words = text.split()
    if not words:
        return 0.0
    garbled = sum(1 for w in words if is_garbled_word(w))
    return garbled / len(words)


def has_repeated_sentences(text, threshold=3):
    sentences = [s.strip() for s in re.split(r'[.!?\n]', text) if len(s.strip()) > 20]
    seen = {}
    for s in sentences:
        seen[s] = seen.get(s, 0) + 1
        if seen[s] >= threshold:
            return True
    return False


def has_fuzzy_repeats(text, similarity=0.75, threshold=3):
    sentences = [s.strip() for s in re.split(r'[.!?\n]', text) if len(s.strip()) > 30]
    if len(sentences) < threshold:
        return False
    for i, s1 in enumerate(sentences):
        similar_count = 0
        for j, s2 in enumerate(sentences):
            if i == j:
                continue
            if SequenceMatcher(None, s1.lower(), s2.lower()).ratio() > similarity:
                similar_count += 1
                if similar_count >= threshold - 1:
                    return True
    return False


def has_ocr_artifacts(text):
    junk_count = len(OCR_JUNK.findall(text)) + len(MIXED_GARBAGE.findall(text))
    return junk_count > 5


def main():
    project_root = os.path.join(os.path.dirname(__file__), "..")
    input_path = os.path.join(project_root, INPUT_FILE)
    output_path = os.path.join(project_root, OUTPUT_FILE)

    with open(input_path) as f:
        pairs = [json.loads(line) for line in f if line.strip()]

    reasons = {"cjk": 0, "too_short": 0, "garbled": 0, "repeated": 0, "fuzzy_repeated": 0, "ocr_junk": 0}
    kept = []

    for pair in pairs:
        response = pair.get("response", "")

        if has_cjk(response):
            reasons["cjk"] += 1
            continue
        if len(response) < 200:
            reasons["too_short"] += 1
            continue
        if garbled_ratio(response) > 0.10:
            reasons["garbled"] += 1
            continue
        if has_repeated_sentences(response):
            reasons["repeated"] += 1
            continue
        if has_fuzzy_repeats(response):
            reasons["fuzzy_repeated"] += 1
            continue
        if has_ocr_artifacts(response):
            reasons["ocr_junk"] += 1
            continue

        kept.append(pair)

    with open(output_path, "w") as out:
        for pair in kept:
            out.write(json.dumps(pair, ensure_ascii=False) + "\n")

    total = len(pairs)
    removed = total - len(kept)
    print(f"Total: {total}")
    print(f"Kept: {len(kept)}")
    print(f"Removed: {removed}")
    print(f"  - CJK characters: {reasons['cjk']}")
    print(f"  - Too short (<200 chars): {reasons['too_short']}")
    print(f"  - Garbled (>10% garbled words): {reasons['garbled']}")
    print(f"  - Exact repeated sentences (3+): {reasons['repeated']}")
    print(f"  - Fuzzy repeated sentences (75%+ similar): {reasons['fuzzy_repeated']}")
    print(f"  - OCR artifacts: {reasons['ocr_junk']}")


if __name__ == "__main__":
    main()
