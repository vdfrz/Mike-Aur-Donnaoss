#!/usr/bin/env python3
"""Augment training data by generating synthetic legal documents via DeepSeek API."""

import argparse
import json
import os
import sys
import time

from openai import OpenAI

DEEPSEEK_BASE_URL = "https://api.deepseek.com"
DEEPSEEK_MODEL = "deepseek-chat"
INPUT_FILE = "training/raw_pairs_clean.jsonl"
OUTPUT_FILE = "training/dataset.jsonl"

MIKE_SYSTEM_MSG = (
    "You are Mike, an expert Indian legal clerk. Draft formal legal documents "
    "based only on facts provided by the user. If specific names, dates, addresses, "
    "or numbers are not provided, leave them as ________ for the user to fill in. "
    "Do NOT invent facts, case details, or legal citations. Do NOT repeat any section "
    "of the document. Each section must appear exactly once."
)

GENERATOR_SYSTEM = (
    "You are a legal document training data generator for Indian law. "
    "You generate realistic, complete Indian legal documents for training purposes."
)

MAX_EXAMPLE_CHARS = 6000
DOCS_PER_CALL = 3
CALLS_PER_DOC = 4


def load_api_key():
    key = os.environ.get("DEEPSEEK_API_KEY")
    if key:
        return key
    env_path = os.path.join(os.path.dirname(__file__), "..", ".env")
    if os.path.exists(env_path):
        with open(env_path) as f:
            for line in f:
                line = line.strip()
                if line.startswith("DEEPSEEK_API_KEY="):
                    return line.split("=", 1)[1].strip().strip('"').strip("'")
    print("Error: DEEPSEEK_API_KEY not found in environment or .env file", file=sys.stderr)
    sys.exit(1)


def load_existing_instructions(output_path):
    existing = set()
    if os.path.exists(output_path):
        with open(output_path) as f:
            for line in f:
                line = line.strip()
                if line:
                    rec = json.loads(line)
                    existing.add(rec.get("instruction", ""))
    return existing


def generate_documents(client, doc_type, document_text, n=6):
    example = document_text[:MAX_EXAMPLE_CHARS]
    if len(document_text) > MAX_EXAMPLE_CHARS:
        example += "\n[... remainder omitted ...]"

    user_msg = (
        f"Here is a real Indian legal {doc_type}. Generate {n} completely new "
        f"{doc_type} documents with different parties, facts, dates, and court. "
        f"Maintain the exact same structure, formatting, legal tone, and section ordering. "
        f"Use realistic Indian names, courts, and legal references. "
        f"Leave names/dates as ________ only if they would normally be filled by the user.\n\n"
        f"Example document:\n---\n{example}\n---\n\n"
        f"Return ONLY a JSON array of objects, each with:\n"
        f'- "instruction": a brief user drafting request for this document\n'
        f'- "document": the complete legal document text\n'
    )

    resp = client.chat.completions.create(
        model=DEEPSEEK_MODEL,
        messages=[
            {"role": "system", "content": GENERATOR_SYSTEM},
            {"role": "user", "content": user_msg},
        ],
        temperature=0.9,
        max_tokens=8192,
    )
    text = resp.choices[0].message.content.strip()
    if text.startswith("```"):
        text = text.split("\n", 1)[1].rsplit("```", 1)[0].strip()
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        results = []
        for start in range(len(text)):
            if text[start] == '{':
                for end in range(len(text), start, -1):
                    try:
                        obj = json.loads(text[start:end])
                        if isinstance(obj, dict) and "document" in obj:
                            results.append(obj)
                            break
                    except json.JSONDecodeError:
                        continue
        if not results:
            raise
        return results


def main():
    parser = argparse.ArgumentParser(description="Augment training data with DeepSeek")
    parser.add_argument("--skip-existing", action="store_true",
                        help="Skip docs whose original instruction is already in output")
    parser.add_argument("--per-doc", type=int, default=CALLS_PER_DOC * DOCS_PER_CALL,
                        help=f"Target synthetic documents per original (default: {CALLS_PER_DOC * DOCS_PER_CALL})")
    parser.add_argument("--batch-size", type=int, default=DOCS_PER_CALL,
                        help=f"Documents to request per API call (default: {DOCS_PER_CALL})")
    args = parser.parse_args()

    project_root = os.path.join(os.path.dirname(__file__), "..")
    input_path = os.path.join(project_root, INPUT_FILE)
    output_path = os.path.join(project_root, OUTPUT_FILE)

    api_key = load_api_key()
    client = OpenAI(api_key=api_key, base_url=DEEPSEEK_BASE_URL)

    with open(input_path) as f:
        pairs = [json.loads(line) for line in f if line.strip()]

    existing = set()
    if args.skip_existing:
        existing = load_existing_instructions(output_path)
        print(f"Found {len(existing)} existing instructions in output, will skip those.")

    mode = "a" if args.skip_existing else "w"
    written = 0
    errors = 0
    calls_per_doc = max(1, args.per_doc // args.batch_size)

    print(f"Input: {len(pairs)} clean pairs")
    print(f"Target: ~{len(pairs) + len(pairs) * calls_per_doc * args.batch_size} total pairs")
    print(f"Strategy: {calls_per_doc} API calls/doc, {args.batch_size} docs/call\n")

    with open(output_path, mode) as out:
        for i, pair in enumerate(pairs):
            instruction = pair["instruction"]
            response = pair["response"]
            doc_type = pair.get("doc_type", "legal document")

            if args.skip_existing and instruction in existing:
                continue

            out.write(json.dumps({
                "system": MIKE_SYSTEM_MSG,
                "instruction": instruction,
                "input": "",
                "output": response,
            }, ensure_ascii=False) + "\n")
            written += 1

            for round_num in range(calls_per_doc):
                try:
                    docs = generate_documents(client, doc_type, response, n=args.batch_size)
                    for doc in docs:
                        if isinstance(doc, dict) and doc.get("document", "").strip():
                            out.write(json.dumps({
                                "system": MIKE_SYSTEM_MSG,
                                "instruction": doc.get("instruction", f"Draft a {doc_type}").strip(),
                                "input": "",
                                "output": doc["document"].strip(),
                            }, ensure_ascii=False) + "\n")
                            written += 1
                except Exception as e:
                    print(f"  Error on doc {i + 1} round {round_num + 1}: {e}", file=sys.stderr)
                    errors += 1
                time.sleep(1)

            if (i + 1) % 10 == 0:
                print(f"Progress: {i + 1}/{len(pairs)} docs, {written} total pairs")

    print(f"\nDone. {written} pairs written to {OUTPUT_FILE}. Errors: {errors}")


if __name__ == "__main__":
    main()
