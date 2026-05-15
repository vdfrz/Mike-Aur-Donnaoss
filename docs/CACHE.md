# Chat-attachment cache

Documents attached via the chat composer (the **+** button next to the message input) follow a different storage contract from project-library uploads. This doc explains the on-disk layout, the SQL schema, and the lifecycle.

## Why a separate cache

A user typing a question and dragging in 2-3 contracts has different expectations from a user uploading a doc to a project library:

- **Transient by default** — once the chat is deleted, the docs should go too. They were context for that conversation, not a long-lived library entry.
- **Survive across chats** — but if the user re-uses the same file in another chat, we shouldn't re-upload, re-hash, or re-extract it.
- **No filename collisions** — different users / different chats can both have a `contratto.pdf` with completely different content; the storage layout must not assume filenames are unique.

The cache solves all three by **keying both the binary and its extracted text by SHA-256 of the bytes**, then ref-counting cleanup at chat-deletion time.

## Storage layout

```
data/storage/
├── documents/<user_id>/<doc_id>     # legacy / project-library uploads (unchanged)
└── cache/
    ├── <sha256>.<ext>               # original binary
    └── <sha256>.txt                 # pre-extracted plain text
```

Examples:

```
data/storage/cache/
├── 9f3a…b21.pdf                     # original 200-page contract
├── 9f3a…b21.txt                     # pdfium-extracted text with [Page N] markers
├── e5c7…704.docx                    # NDA
└── e5c7…704.txt                     # docx text with [removed by author: …] markers
```

The `.txt` is generated once at upload time by `src/sync/scanner.rs::extract_text_dispatch` (the same dispatch the folder scanner uses, so the format treatment is identical).

## SQL schema

The `documents` table got two columns over migrations 0013 and 0014:

```sql
-- 0013_documents_chat_id.sql
ALTER TABLE documents ADD COLUMN chat_id TEXT
  REFERENCES chats(id) ON DELETE CASCADE;
CREATE INDEX idx_documents_chat_id ON documents(chat_id);

-- 0014_documents_cache_hash.sql
ALTER TABLE documents ADD COLUMN content_hash TEXT;
ALTER TABLE documents ADD COLUMN extracted_text_path TEXT;
CREATE INDEX idx_documents_content_hash ON documents(content_hash);
```

Two cache uploads of the same content produce two `documents` rows with **different** `id` and `chat_id` but the **same** `content_hash`, `storage_path`, and `extracted_text_path`. Multiple rows can co-own one set of physical files.

## Lifecycle

### Upload (`POST /single-documents`)

```
multipart/form-data: file=<bytes>, cache=true
                                   └── set by the chat composer's AddDocButton
                                       (frontend: uploadStandaloneDocument(f, { cache: true }))
```

Backend (`src/routes/documents.rs::upload_document`):

1. Compute `sha256_hex(bytes)`.
2. `bin_key  = "cache/{hash}.{ext}"`
   `text_key = "cache/{hash}.txt"`
3. If `bin_abs.exists()` → reuse; else `storage.put(bin_key, bytes)`.
4. If `text_abs.exists()` → reuse; else
   `extract_text_dispatch(bin_abs, &bytes) → storage.put(text_key, text.as_bytes())`.
5. `INSERT INTO documents (id, …, content_hash, extracted_text_path) VALUES (…)`. `chat_id` left NULL — the chat doesn't necessarily exist yet.

### First message send

`src/routes/chat.rs::stream_chat_root` collects `messages[].files[].document_id` from the request body. After resolving / creating the chat row:

```sql
UPDATE documents SET chat_id = ?
WHERE user_id = ?
  AND chat_id IS NULL
  AND content_hash IS NOT NULL    -- only cache uploads, not project docs
  AND id IN (?, ?, …)
```

The `content_hash IS NOT NULL` guard prevents a stray project-library doc from inheriting `chat_id` and being swept on chat deletion.

### Chat handler reads cached text

`src/routes/chat.rs::load_attached_docs` for each attachment:

1. `SELECT … extracted_text_path FROM documents WHERE id = ?`
2. If `extracted_text_path` is set and `storage.get(extracted_text_path)` returns non-empty → use it directly. Skip the per-format dispatch and the binary read.
3. Otherwise → fall through to the legacy path (read binary, dispatch per `file_type`). PDFs with empty cached text fall through too, so scanned-PDF page rendering for vision LLMs still works.

This saves the per-turn cost of re-running pdfium on a 200-page brief.

### Chat deletion

`src/routes/chat.rs::delete_chat`:

1. Snapshot `(id, storage_path, extracted_text_path, content_hash)` for all docs with `chat_id = <this>`.
2. `DELETE FROM chats WHERE id = ? AND user_id = ?` — FK cascade removes the linked document rows.
3. For each unique `content_hash` in the snapshot:
   - `SELECT 1 FROM documents WHERE content_hash = ? LIMIT 1`
   - If still referenced → keep on-disk files (another chat shares this hash).
   - If no reference → `storage.delete(storage_path)` + `storage.delete(extracted_text_path)`.

### Project-library uploads (`cache=false`, default)

Unchanged from before: `documents/<user_id>/<doc_id>` flat key, no hash, no pre-extraction. Project docs participate in the legacy on-demand extraction path in the chat handler.

## Operational notes

### Forcing re-extraction

A docx-extraction improvement (e.g. adding redline detection — see [DOCX.md](DOCX.md)) doesn't automatically refresh existing cache entries. To regenerate:

```bash
# Nuclear: wipe everything in cache. Will regenerate on next upload.
rm -r data/storage/cache/*

# Targeted: change one byte in the source doc → new hash → fresh extraction
```

### Inspecting cache footprint

```bash
du -sh data/storage/cache/
ls -l data/storage/cache/ | wc -l    # number of files (binaries + .txt = 2× docs)
```

### Migration safety

Existing rows from before migration 0013 / 0014 have `chat_id`, `content_hash`, `extracted_text_path` all NULL. They behave exactly as before:

- Stay in `documents/<user>/<id>`.
- Aren't touched by chat-delete sweeps (the snapshot SELECT yields no rows for them).
- Get re-extracted per-turn when attached.

There is no backfill — if you want existing chat-attached uploads to migrate to the cache layout, re-upload them via the composer's **+** button.
