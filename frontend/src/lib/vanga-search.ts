import * as duckdb from "@duckdb/duckdb-wasm";

const S3_BUCKET = "indian-high-court-judgments";
const S3_REGION = "ap-south-1";
const S3_BASE = `https://${S3_BUCKET}.s3.${S3_REGION}.amazonaws.com`;

const IDB_NAME = "vanga-pdfs";
const IDB_STORE = "texts";
const IDB_VERSION = 1;
const MAX_TEXT_CHARS = 50_000;
const DEFAULT_CACHE_LIMIT = 500 * 1024 * 1024; // 500 MB
const CACHE_LIMIT_KEY = "vanga_cache_limit";

export interface VangaResult {
  case_id: string;
  title: string;
  court_name: string;
  court_code: string;
  judge: string;
  decision_date: string;
  snippet: string;
  pdf_url: string;
  year: string;
  full_text?: string;
  has_full_text: boolean;
}

export interface VangaSearchParams {
  court_code?: string;
  year_start?: number;
  year_end?: number;
  query: string;
}

interface CachedText {
  case_id: string;
  text: string;
  fetched_at: number;
  size_bytes: number;
}

// ---------------------------------------------------------------------------
// IndexedDB helpers
// ---------------------------------------------------------------------------

function openVangaDB(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const req = indexedDB.open(IDB_NAME, IDB_VERSION);
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains(IDB_STORE)) {
        db.createObjectStore(IDB_STORE, { keyPath: "case_id" });
      }
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

function idbGet(db: IDBDatabase, key: string): Promise<CachedText | undefined> {
  return new Promise((resolve, reject) => {
    const tx = db.transaction(IDB_STORE, "readonly");
    const req = tx.objectStore(IDB_STORE).get(key);
    req.onsuccess = () => resolve(req.result as CachedText | undefined);
    req.onerror = () => reject(req.error);
  });
}

function idbPut(db: IDBDatabase, entry: CachedText): Promise<void> {
  return new Promise((resolve, reject) => {
    const tx = db.transaction(IDB_STORE, "readwrite");
    tx.objectStore(IDB_STORE).put(entry);
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error);
  });
}

function idbGetAll(db: IDBDatabase): Promise<CachedText[]> {
  return new Promise((resolve, reject) => {
    const tx = db.transaction(IDB_STORE, "readonly");
    const req = tx.objectStore(IDB_STORE).getAll();
    req.onsuccess = () => resolve(req.result as CachedText[]);
    req.onerror = () => reject(req.error);
  });
}

function idbDelete(db: IDBDatabase, key: string): Promise<void> {
  return new Promise((resolve, reject) => {
    const tx = db.transaction(IDB_STORE, "readwrite");
    tx.objectStore(IDB_STORE).delete(key);
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error);
  });
}

function idbClear(db: IDBDatabase): Promise<void> {
  return new Promise((resolve, reject) => {
    const tx = db.transaction(IDB_STORE, "readwrite");
    tx.objectStore(IDB_STORE).clear();
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error);
  });
}

function getCacheLimit(): number {
  if (typeof window === "undefined") return DEFAULT_CACHE_LIMIT;
  const raw = localStorage.getItem(CACHE_LIMIT_KEY);
  if (!raw) return DEFAULT_CACHE_LIMIT;
  const n = parseInt(raw, 10);
  return isNaN(n) ? DEFAULT_CACHE_LIMIT : n;
}

export function setCacheLimit(bytes: number) {
  localStorage.setItem(CACHE_LIMIT_KEY, String(bytes));
}

export async function getCacheStats(): Promise<{ totalBytes: number; count: number }> {
  try {
    const db = await openVangaDB();
    const all = await idbGetAll(db);
    db.close();
    const totalBytes = all.reduce((sum, e) => sum + e.size_bytes, 0);
    return { totalBytes, count: all.length };
  } catch {
    return { totalBytes: 0, count: 0 };
  }
}

export async function clearCache(): Promise<void> {
  const db = await openVangaDB();
  await idbClear(db);
  db.close();
}

async function enforceCacheLimit(db: IDBDatabase, incomingBytes: number) {
  const limit = getCacheLimit();
  if (limit === 0) return;
  const all = await idbGetAll(db);
  let total = all.reduce((sum, e) => sum + e.size_bytes, 0) + incomingBytes;
  if (total <= limit) return;
  all.sort((a, b) => a.fetched_at - b.fetched_at);
  for (const entry of all) {
    if (total <= limit) break;
    await idbDelete(db, entry.case_id);
    total -= entry.size_bytes;
  }
}

// ---------------------------------------------------------------------------
// PDF text extraction (PDF.js text layer → Tesseract OCR fallback)
// ---------------------------------------------------------------------------

const OCR_MIN_CHARS = 100;
const OCR_MAX_PAGES = 15;
const OCR_SCALE = 1.5;

async function ocrPdfPages(
  doc: { numPages: number; getPage: (n: number) => Promise<unknown> },
  maxChars: number,
): Promise<string> {
  const { createWorker } = await import("tesseract.js");
  const worker = await createWorker("eng");
  let text = "";
  const pages = Math.min(doc.numPages, OCR_MAX_PAGES);
  try {
    for (let p = 1; p <= pages; p++) {
      const page = (await doc.getPage(p)) as {
        getViewport: (opts: { scale: number }) => { width: number; height: number };
        render: (ctx: { canvasContext: CanvasRenderingContext2D; viewport: unknown }) => { promise: Promise<void> };
      };
      const viewport = page.getViewport({ scale: OCR_SCALE });
      const canvas = document.createElement("canvas");
      canvas.width = viewport.width;
      canvas.height = viewport.height;
      const ctx = canvas.getContext("2d")!;
      await page.render({ canvasContext: ctx, viewport }).promise;
      const { data } = await worker.recognize(canvas);
      text += data.text + "\n";
      canvas.width = 0;
      canvas.height = 0;
      if (text.length >= maxChars) {
        text = text.slice(0, maxChars);
        break;
      }
    }
  } finally {
    await worker.terminate();
  }
  return text;
}

export async function fetchFullText(pdfUrl: string, caseId: string, shouldCache = true): Promise<string | null> {
  const db = await openVangaDB();
  const cached = await idbGet(db, caseId);
  if (cached) {
    db.close();
    return cached.text;
  }

  const { getPdfJs, STANDARD_FONT_DATA_URL } = await import(
    "@/app/components/shared/highlightQuote"
  );
  const pdfjsLib = await getPdfJs();

  // Use a lowercase-cors fetch and tolerate non-2xx responses without
  // throwing — stale pdf_link entries in the parquet are common and the
  // caller already handles "no full text" gracefully. Throwing here just
  // pollutes the Next.js dev overlay with red "Console Error" cards for
  // what's a normal degradation.
  let resp: Response;
  try {
    resp = await fetch(pdfUrl);
  } catch (e) {
    console.warn(`[vanga] ${caseId}: network error fetching PDF (${pdfUrl}):`, e);
    db.close();
    return null;
  }
  if (!resp.ok) {
    console.warn(`[vanga] ${caseId}: PDF fetch returned ${resp.status} (${pdfUrl}) — likely a stale S3 link; skipping full text for this result.`);
    db.close();
    return null;
  }
  const buf = await resp.arrayBuffer();
  const pdfData = new Uint8Array(buf);
  const doc = await pdfjsLib.getDocument({
    data: pdfData,
    standardFontDataUrl: STANDARD_FONT_DATA_URL,
  }).promise;

  let text = "";
  for (let p = 1; p <= doc.numPages; p++) {
    const page = await doc.getPage(p);
    const content = await page.getTextContent();
    const pageText = content.items
      .filter((item: Record<string, unknown>) => "str" in item)
      .map((item: Record<string, unknown>) => (item as { str: string }).str)
      .join(" ");
    text += pageText + "\n";
    if (text.length >= MAX_TEXT_CHARS) {
      text = text.slice(0, MAX_TEXT_CHARS);
      break;
    }
  }

  // Scanned PDF fallback: if text layer is too thin, OCR the pages
  if (text.trim().length < OCR_MIN_CHARS) {
    console.info(`[vanga] ${caseId}: only ${text.trim().length} chars from text layer, falling back to OCR`);
    try {
      text = await ocrPdfPages(doc, MAX_TEXT_CHARS);
    } catch (ocrErr) {
      console.error(`[vanga] OCR failed for ${caseId}:`, ocrErr);
    }
  }

  if (shouldCache && getCacheLimit() > 0) {
    const sizeBytes = new Blob([text]).size;
    await enforceCacheLimit(db, sizeBytes);
    await idbPut(db, { case_id: caseId, text, fetched_at: Date.now(), size_bytes: sizeBytes });
  }
  db.close();
  return text;
}

let dbInstance: duckdb.AsyncDuckDB | null = null;
let connInstance: duckdb.AsyncDuckDBConnection | null = null;
let initPromise: Promise<duckdb.AsyncDuckDBConnection> | null = null;

async function getConnection(): Promise<duckdb.AsyncDuckDBConnection> {
  if (connInstance) return connInstance;
  if (initPromise) return initPromise;

  initPromise = (async () => {
    const JSDELIVR_BUNDLES = duckdb.getJsDelivrBundles();
    const bundle = await duckdb.selectBundle(JSDELIVR_BUNDLES);

    const worker_url = URL.createObjectURL(
      new Blob([`importScripts("${bundle.mainWorker!}");`], {
        type: "text/javascript",
      }),
    );

    const worker = new Worker(worker_url);
    const logger = new duckdb.ConsoleLogger();
    const db = new duckdb.AsyncDuckDB(logger, worker);
    await db.instantiate(bundle.mainModule, bundle.pthreadWorker);

    const conn = await db.connect();
    await conn.query(`INSTALL httpfs; LOAD httpfs;`);
    await conn.query(`SET s3_region='${S3_REGION}';`);
    await conn.query(`SET s3_access_key_id='';`);
    await conn.query(`SET s3_secret_access_key='';`);

    dbInstance = db;
    connInstance = conn;
    URL.revokeObjectURL(worker_url);
    return conn;
  })();

  return initPromise;
}

function escapeSQL(s: string): string {
  return s.replace(/'/g, "''");
}

function buildSnippet(text: string, query: string): string {
  const lower = text.toLowerCase();
  const qLower = query.toLowerCase().split(/\s+/)[0] || query.toLowerCase();
  const idx = lower.indexOf(qLower);
  if (idx === -1) return text.slice(0, 200);
  const start = Math.max(0, idx - 100);
  const end = Math.min(text.length, idx + qLower.length + 100);
  return (start > 0 ? "..." : "") + text.slice(start, end) + (end < text.length ? "..." : "");
}

export async function vangaSearch(
  params: VangaSearchParams,
): Promise<VangaResult[]> {
  const conn = await getConnection();

  const yearStart = params.year_start ?? 2015;
  const yearEnd = params.year_end ?? new Date().getFullYear();
  const years: number[] = [];
  for (let y = yearStart; y <= yearEnd; y++) years.push(y);

  // Match court by friendly name in the data column (court_code partition values
  // are opaque codes like '1_8' which the model doesn't know — match against the
  // human-readable name in the `court` data column instead).
  const courtFilter = params.court_code
    ? `AND court ILIKE '%${escapeSQL(params.court_code)}%'`
    : "";

  const queryTerms = params.query
    .split(/\s+/)
    .filter((w) => w.length > 2)
    .slice(0, 8);
  if (queryTerms.length === 0) return [];

  // OR the terms together (any match counts) and score by how many terms hit.
  // Pure AND is too restrictive — legal queries have many keywords and the
  // parquet only indexes title + description (not full body text).
  const matchExprs = queryTerms.map(
    (t) =>
      `(title ILIKE '%${escapeSQL(t)}%' OR description ILIKE '%${escapeSQL(t)}%')`,
  );
  const whereClause = matchExprs.join(" OR ");
  const scoreExpr = matchExprs
    .map((e) => `CASE WHEN ${e} THEN 1 ELSE 0 END`)
    .join(" + ");

  const yearList = years.join(",");

  const sql = `
    SELECT
      cnr AS case_id,
      title,
      court AS court_name,
      court_code,
      judge,
      CAST(decision_date AS VARCHAR) AS decision_date,
      COALESCE(description, '') AS description,
      pdf_link,
      year,
      court AS court_partition
    FROM read_parquet(
      's3://${S3_BUCKET}/metadata/parquet/year=*/court=*/bench=*/metadata.parquet',
      hive_partitioning = true
    )
    WHERE CAST(year AS INTEGER) IN (${yearList})
      ${courtFilter}
      AND (${whereClause})
    ORDER BY (${scoreExpr}) DESC
    LIMIT 20
  `;

  const result = await conn.query(sql);
  const rows = result.toArray();

  return rows.map((row: Record<string, unknown>) => {
    const caseId = String(row.case_id ?? "");
    const year = String(row.year ?? "");
    const description = String(row.description ?? "");
    const pdfLink = String(row.pdf_link ?? "");

    // pdf_link from parquet has the correct path (e.g.
    // "court/cnrorders/patnahcucisdb94/orders/BRHC011164592023_1_2024-05-03.pdf")
    // Most are relative to bucket root — prefix with S3_BASE.
    const pdfUrl = pdfLink
      ? (pdfLink.startsWith("http") ? pdfLink : `${S3_BASE}/${pdfLink.replace(/^\//, "")}`)
      : "";

    return {
      case_id: caseId,
      title: String(row.title ?? "Unknown"),
      court_name: String(row.court_name ?? ""),
      court_code: String(row.court_code ?? ""),
      judge: String(row.judge ?? ""),
      decision_date: String(row.decision_date ?? ""),
      snippet: buildSnippet(description, params.query),
      pdf_url: pdfUrl,
      year,
      has_full_text: false,
    };
  });
}

// ---------------------------------------------------------------------------
// Enriched search: metadata + full judgment text for top candidates
// ---------------------------------------------------------------------------

export interface SearchWithFullTextProgress {
  phase: "searching" | "loading" | "done";
  loaded?: number;
  total?: number;
}

export async function searchWithFullText(
  params: VangaSearchParams,
  shouldCache = true,
  onProgress?: (p: SearchWithFullTextProgress) => void,
): Promise<VangaResult[]> {
  onProgress?.({ phase: "searching" });
  let results = await vangaSearch(params);

  // FALLBACK: if metadata search returns nothing, broaden to court+year only
  // and fetch top recent PDFs. The model then matches doctrine against
  // actual judgment text, not just titles. Prevents hallucinated citations
  // when keyword doesn't appear in title/description.
  if (results.length === 0 && (params.court_code || params.year_start)) {
    const broadParams: VangaSearchParams = {
      court_code: params.court_code,
      year_start: params.year_start,
      year_end: params.year_end,
      query: "a", // single common token — matches almost any case
    };
    results = await vangaSearch(broadParams);
  }

  if (results.length === 0) return results;

  const top5 = results.slice(0, 5);
  let loaded = 0;
  onProgress?.({ phase: "loading", loaded: 0, total: top5.length });

  const enriched = await Promise.all(
    top5.map(async (r) => {
      if (!r.pdf_url) {
        console.warn(`[vanga] skipping ${r.case_id}: no pdf_url`);
        loaded++;
        onProgress?.({ phase: "loading", loaded, total: top5.length });
        return r;
      }
      try {
        const text = await fetchFullText(r.pdf_url, r.case_id, shouldCache);
        loaded++;
        onProgress?.({ phase: "loading", loaded, total: top5.length });
        // text === null means the PDF couldn't be fetched (stale S3 link,
        // network error, etc.). We already logged a warning inside
        // fetchFullText; just return the metadata-only result.
        if (text === null) {
          return r;
        }
        const hasText = typeof text === "string" && text.trim().length > 100;
        if (!hasText) {
          console.warn(`[vanga] ${r.case_id}: PDF parsed but text too short (${text.trim().length} chars) — likely scanned/image PDF`);
        }
        return { ...r, full_text: hasText ? text : undefined, has_full_text: hasText };
      } catch (err) {
        // Unexpected error (parser crash, etc.) — log as warning so it
        // doesn't dominate the dev overlay, but keep the trace.
        loaded++;
        onProgress?.({ phase: "loading", loaded, total: top5.length });
        console.warn(`[vanga] unexpected error processing ${r.case_id} (${r.pdf_url}):`, err);
        return r;
      }
    }),
  );

  const remaining = results.slice(5);
  onProgress?.({ phase: "done" });
  return [...enriched, ...remaining];
}
