#!/usr/bin/env python3
"""Extract text from PDF/DOCX files and output as JSONL for training."""

import argparse
import json
import subprocess
import sys
from pathlib import Path

import fitz  # pymupdf
import pytesseract
from docx import Document
from PIL import Image

OCR_THRESHOLD = 100  # characters; below this, fall back to OCR


def extract_pdf(path: Path) -> tuple[str, bool]:
    """Extract text from PDF. Returns (text, used_ocr)."""
    doc = fitz.open(str(path))
    text = "\n".join(page.get_text() for page in doc)

    if len(text.strip()) < OCR_THRESHOLD:
        pages = []
        for page in doc:
            pix = page.get_pixmap(dpi=150)
            img = Image.frombytes("RGB", [pix.width, pix.height], pix.samples)
            pages.append(pytesseract.image_to_string(img, lang="eng"))
        doc.close()
        return "\n".join(pages), True

    doc.close()
    return text, False


def extract_docx(path: Path) -> str:
    doc = Document(str(path))
    return "\n".join(p.text for p in doc.paragraphs)


def extract_textutil(path: Path) -> str:
    """Extract text from .doc/.odt using macOS textutil."""
    result = subprocess.run(
        ["textutil", "-convert", "txt", "-stdout", str(path)],
        capture_output=True, text=True,
    )
    if result.returncode != 0:
        raise RuntimeError(f"textutil failed: {result.stderr.strip()}")
    return result.stdout


def main():
    parser = argparse.ArgumentParser(description="Extract text from corpus files to JSONL.")
    parser.add_argument(
        "--corpus-path",
        type=Path,
        default=Path.home() / "Desktop" / "RAG TEST",
        help="Root directory to search for PDF/DOCX files (default: ~/Desktop/RAG TEST/)",
    )
    args = parser.parse_args()

    corpus = args.corpus_path
    if not corpus.is_dir():
        print(f"Error: corpus path does not exist: {corpus}", file=sys.stderr)
        sys.exit(1)

    output_path = Path(__file__).parent / "extracted_texts.jsonl"
    extensions = {".pdf", ".docx", ".doc", ".odt"}
    files = [f for f in corpus.rglob("*") if f.suffix.lower() in extensions]

    extracted = 0
    ocr_count = 0
    skipped = 0

    with open(output_path, "w", encoding="utf-8") as out:
        for f in files:
            suffix = f.suffix.lower()

            if suffix in (".doc", ".odt"):
                try:
                    text = extract_textutil(f)
                    fmt = suffix.lstrip(".")
                    record = {
                        "filepath": str(f),
                        "filename": f.name,
                        "text": text,
                        "format": fmt,
                    }
                    out.write(json.dumps(record, ensure_ascii=False) + "\n")
                    extracted += 1
                    continue
                except Exception as e:
                    print(f"SKIP ({suffix} extraction failed): {f} — {e}", file=sys.stderr)
                    skipped += 1
                    continue

            try:
                if suffix == ".pdf":
                    text, used_ocr = extract_pdf(f)
                    fmt = "pdf"
                    if used_ocr:
                        ocr_count += 1
                        print(f"OCR: {f.name}", file=sys.stderr)
                else:
                    text = extract_docx(f)
                    fmt = "docx"

                record = {
                    "filepath": str(f),
                    "filename": f.name,
                    "text": text,
                    "format": fmt,
                }
                out.write(json.dumps(record, ensure_ascii=False) + "\n")
                extracted += 1
            except Exception as e:
                print(f"SKIP (extraction failed): {f} — {e}", file=sys.stderr)
                skipped += 1

    direct = extracted - ocr_count
    print(f"\nSummary:")
    print(f"  Total files found: {len(files)}")
    print(f"  Extracted:         {extracted}")
    print(f"    Direct text:     {direct}")
    print(f"    OCR fallback:    {ocr_count}")
    print(f"  Skipped:           {skipped}")
    print(f"  Output:            {output_path}")


if __name__ == "__main__":
    main()
