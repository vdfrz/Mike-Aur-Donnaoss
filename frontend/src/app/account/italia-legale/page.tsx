"use client";

import { useEffect, useMemo, useState } from "react";
import {
    Search,
    AlertCircle,
    Loader2,
    CheckCircle2,
    Trash2,
    Download,
    Info,
    RefreshCw,
    CheckSquare,
    Square,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

const API_BASE =
    process.env.NEXT_PUBLIC_API_BASE_URL ?? "http://localhost:3001";

function getToken() {
    return typeof window !== "undefined"
        ? localStorage.getItem("mike_auth_token") ?? ""
        : "";
}

async function api<T>(path: string, init: RequestInit = {}): Promise<T> {
    const res = await fetch(`${API_BASE}${path}`, {
        ...init,
        headers: {
            "Content-Type": "application/json",
            Authorization: `Bearer ${getToken()}`,
            ...(init.headers ?? {}),
        },
    });
    if (!res.ok) {
        const text = await res.text().catch(() => "");
        throw new Error(text || `HTTP ${res.status}`);
    }
    if (res.status === 204) return undefined as T;
    return (await res.json()) as T;
}

interface ConfigOut {
    enabled: boolean;
    sources: string[];
}

interface ImportStatus {
    job_state: "idle" | "downloading" | "importing" | "ready" | "failed";
    current_shard: number;
    total_shards: number;
    rows_imported: number;
    percent: number;
    row_count: number;
    last_import_at: string | null;
    dataset_revision: string | null;
    job_error: string | null;
}

interface SearchHit {
    hf_id: string;
    row_offset: number;
    source: string;
    doc_type: string | null;
    title: string | null;
    authority: string | null;
    number: string | null;
    year: number | null;
    date: string | null;
    text_length: number;
}

interface IndexedDoc {
    id: string;
    filename: string;
    corpus_identifier: string | null;
    size_bytes: number;
    created_at: string;
    status: string;
}

const SOURCE_OPTIONS = [
    { value: "normattiva", label: "Normattiva (leggi, decreti, codici)" },
    { value: "corte_costituzionale", label: "Corte Costituzionale (sentenze)" },
];

/**
 * Authoritative Italian legal corpora that exist as data sources but
 * aren't yet wired into MikeRust. Surfaced as disabled checkboxes so
 * the user can see what's coming and we can track the roadmap inline.
 *
 * Each entry's `note` ends up as the row's tooltip + caption — keep it
 * short and concrete (what's needed to make this row real).
 */
const PLANNED_SOURCES: {
    value: string;
    label: string;
    approxRows: string;
    note: string;
}[] = [
    {
        value: "openga",
        label: "OpenGA — TAR + Consiglio di Stato",
        approxRows: "~125K",
        note: "Già nel dataset HF. Manca solo l'opt-in nell'importer (filtro source). Volumi grandi: utile a chi fa diritto amministrativo / appalti.",
    },
    {
        value: "cassazione",
        label: "Cassazione (civile, penale, sezioni unite)",
        approxRows: "—",
        note: "Non presente nel dataset HF. Richiede un fetcher dedicato per italgiure.giustizia.it (snapshot annuale via CSV/XML) o un dataset terzo. Roadmap: V2.",
    },
    {
        value: "live_normattiva",
        label: "Aggiornamenti live Normattiva (post-2026-03)",
        approxRows: "incrementale",
        note: "Il dataset è uno snapshot al 2026-03-01. Per atti più recenti serve un client Normattiva via URN (fetch on-demand singolo atto), per restare nei limiti dei ToS.",
    },
    {
        value: "leggi_regionali",
        label: "Leggi regionali (BUR + CRIN)",
        approxRows: "—",
        note: "20 sistemi diversi (uno per regione, alcuni con BUR + sito Consiglio). Approccio: adapter generico che accetta URL Akoma Ntoso e lascia la mappatura regione-per-regione al config utente.",
    },
    {
        value: "gazzetta_ufficiale",
        label: "Gazzetta Ufficiale (atti recenti)",
        approxRows: "incrementale",
        note: "Sumario XML quotidiano. Sovrapposizione parziale con Normattiva live. Utile come sorgente primaria per atti molto recenti che Normattiva consolida con qualche giorno di ritardo.",
    },
];

const DOC_TYPE_OPTIONS = [
    { value: "law", label: "Legge" },
    { value: "legislative_decree", label: "Decreto legislativo" },
    { value: "decree_law", label: "Decreto-legge" },
    { value: "presidential_decree", label: "Decreto del Presidente" },
    { value: "royal_decree", label: "Regio decreto" },
    { value: "constitutional_law", label: "Legge costituzionale" },
    { value: "code", label: "Codice" },
    { value: "decision", label: "Sentenza" },
    { value: "order", label: "Ordinanza" },
    { value: "regulation", label: "Regolamento" },
    { value: "opinion", label: "Parere" },
];

export default function ItaliaLegalePage() {
    // Config
    const [enabled, setEnabled] = useState(false);
    const [enabledSources, setEnabledSources] = useState<string[]>([
        "normattiva",
        "corte_costituzionale",
    ]);
    const [configLoading, setConfigLoading] = useState(true);

    // Import state
    const [importStatus, setImportStatus] = useState<ImportStatus | null>(null);
    const [importStarting, setImportStarting] = useState(false);

    // Search filters
    const [query, setQuery] = useState("");
    const [filterSources, setFilterSources] = useState<string[]>([
        "normattiva",
        "corte_costituzionale",
    ]);
    const [filterDocTypes, setFilterDocTypes] = useState<string[]>([]);
    const [yearMin, setYearMin] = useState<string>("");
    const [yearMax, setYearMax] = useState<string>("");
    const [searching, setSearching] = useState(false);
    const [hits, setHits] = useState<SearchHit[] | null>(null);
    const [searchError, setSearchError] = useState<string | null>(null);

    // Selection + indexing
    const [selected, setSelected] = useState<Set<string>>(new Set());
    const [indexingId, setIndexingId] = useState<string | null>(null);
    const [bulkIndexing, setBulkIndexing] = useState(false);

    // Indexed docs list
    const [indexedDocs, setIndexedDocs] = useState<IndexedDoc[]>([]);
    const [deletingId, setDeletingId] = useState<string | null>(null);
    const [resyncingId, setResyncingId] = useState<string | null>(null);
    const [error, setError] = useState<string | null>(null);

    // Live progress for the doc currently being embedded. Reuses
    // /eurlex/embed-progress because the underlying state on
    // EmbeddingService is process-wide, not corpus-specific.
    const [embedProgress, setEmbedProgress] = useState<{
        document_id: string;
        current: number;
        total: number;
        percent: number;
    } | null>(null);

    // ---------------- Config bootstrap ----------------
    useEffect(() => {
        let cancelled = false;
        api<ConfigOut>("/italian-legal/config")
            .then((cfg) => {
                if (cancelled) return;
                setEnabled(cfg.enabled);
                setEnabledSources(cfg.sources);
            })
            .catch((e) => {
                if (!cancelled) setError(String(e));
            })
            .finally(() => {
                if (!cancelled) setConfigLoading(false);
            });
        return () => {
            cancelled = true;
        };
    }, []);

    // Persist config on change.
    useEffect(() => {
        if (configLoading) return;
        const handle = setTimeout(() => {
            api("/italian-legal/config", {
                method: "PUT",
                body: JSON.stringify({ enabled, sources: enabledSources }),
            }).catch((e) => setError(String(e)));
        }, 300);
        return () => clearTimeout(handle);
    }, [enabled, enabledSources, configLoading]);

    // ---------------- Import status polling ----------------
    const refreshStatus = async () => {
        try {
            const s = await api<ImportStatus>("/italian-legal/import-status");
            setImportStatus(s);
        } catch (e) {
            console.warn("[italian-legal] status poll failed:", e);
        }
    };
    useEffect(() => {
        refreshStatus();
    }, []);
    useEffect(() => {
        if (
            !importStatus ||
            (importStatus.job_state !== "downloading" &&
                importStatus.job_state !== "importing")
        ) {
            return;
        }
        const handle = setInterval(refreshStatus, 1500);
        return () => clearInterval(handle);
    }, [importStatus?.job_state]);

    // ---------------- Indexed docs list ----------------
    const refreshIndexed = async () => {
        try {
            const r = await api<{ documents: IndexedDoc[] }>(
                "/italian-legal/documents",
            );
            setIndexedDocs(r.documents);
        } catch (e) {
            console.warn("[italian-legal] refresh failed:", e);
        }
    };
    useEffect(() => {
        refreshIndexed();
    }, []);

    // Poll the global embed-progress endpoint while any indexed doc is
    // in the syncing state. Stops as soon as no row is syncing.
    useEffect(() => {
        const anySyncing = indexedDocs.some((d) => d.status === "syncing");
        if (!anySyncing) {
            setEmbedProgress(null);
            return;
        }
        let cancelled = false;
        const tick = async () => {
            try {
                const p = await api<
                    | {
                          document_id: string;
                          current: number;
                          total: number;
                          percent: number;
                      }
                    | null
                >("/eurlex/embed-progress");
                if (!cancelled) setEmbedProgress(p);
                if (!cancelled && (p === null || p.percent >= 100)) {
                    await refreshIndexed();
                }
            } catch (e) {
                console.warn("[italian-legal] progress poll failed:", e);
            }
        };
        tick();
        const handle = setInterval(tick, 500);
        return () => {
            cancelled = true;
            clearInterval(handle);
        };
    }, [indexedDocs]);

    // ---------------- Actions ----------------
    const startImport = async () => {
        setImportStarting(true);
        try {
            await api("/italian-legal/import", { method: "POST" });
            await refreshStatus();
        } catch (e) {
            setError(String(e));
        } finally {
            setImportStarting(false);
        }
    };

    const runSearch = async () => {
        setSearchError(null);
        setHits(null);
        setSelected(new Set());
        setSearching(true);
        try {
            const body: Record<string, unknown> = {
                query: query.trim() || undefined,
                sources: filterSources.length ? filterSources : undefined,
                doc_types: filterDocTypes.length ? filterDocTypes : undefined,
                limit: 50,
            };
            const yMin = parseInt(yearMin, 10);
            const yMax = parseInt(yearMax, 10);
            if (Number.isFinite(yMin)) body.year_min = yMin;
            if (Number.isFinite(yMax)) body.year_max = yMax;
            const r = await api<{ hits: SearchHit[] }>(
                "/italian-legal/search",
                { method: "POST", body: JSON.stringify(body) },
            );
            setHits(r.hits);
        } catch (e) {
            setSearchError(String(e));
        } finally {
            setSearching(false);
        }
    };

    const indexHit = async (hit: SearchHit) => {
        setIndexingId(hit.hf_id);
        setError(null);
        try {
            await api("/italian-legal/fetch", {
                method: "POST",
                body: JSON.stringify({ hf_id: hit.hf_id }),
            });
            await refreshIndexed();
        } catch (e) {
            setError(String(e));
        } finally {
            setIndexingId(null);
        }
    };

    const indexSelected = async () => {
        if (!hits || selected.size === 0) return;
        setBulkIndexing(true);
        const ids = Array.from(selected);
        for (const id of ids) {
            const hit = hits.find((h) => h.hf_id === id);
            if (!hit) continue;
            setIndexingId(id);
            try {
                await api("/italian-legal/fetch", {
                    method: "POST",
                    body: JSON.stringify({ hf_id: id }),
                });
                // Refresh AFTER EACH item so the indexed list grows
                // progressively and the search row flips to
                // "Indicizzato" the moment it lands — instead of all
                // at once at the end of the loop.
                await refreshIndexed();
                // Drop this id from selection so the user sees the
                // remaining count tick down.
                setSelected((prev) => {
                    const c = new Set(prev);
                    c.delete(id);
                    return c;
                });
            } catch (e) {
                setError(String(e));
            }
        }
        setIndexingId(null);
        setBulkIndexing(false);
    };

    const resyncDoc = async (doc: IndexedDoc) => {
        setError(null);
        setResyncingId(doc.id);
        setIndexedDocs((prev) =>
            prev.map((d) =>
                d.id === doc.id ? { ...d, status: "syncing" } : d,
            ),
        );
        try {
            const resp = await api<{
                status: string;
                indexing_error?: string | null;
            }>(`/italian-legal/documents/${doc.id}/resync`, { method: "POST" });
            setIndexedDocs((prev) =>
                prev.map((d) =>
                    d.id === doc.id ? { ...d, status: resp.status } : d,
                ),
            );
            if (resp.indexing_error) {
                setError(`Resync fallito: ${resp.indexing_error}`);
            }
        } catch (e) {
            setError(String(e));
            await refreshIndexed();
        } finally {
            setResyncingId(null);
        }
    };

    const deleteDoc = async (doc: IndexedDoc) => {
        if (!confirm(`Rimuovere "${doc.filename}" dall'indice?`)) return;
        setDeletingId(doc.id);
        try {
            await api(`/italian-legal/documents/${doc.id}`, {
                method: "DELETE",
            });
            setIndexedDocs((prev) => prev.filter((d) => d.id !== doc.id));
        } catch (e) {
            setError(String(e));
        } finally {
            setDeletingId(null);
        }
    };

    const toggleSelected = (id: string) => {
        setSelected((prev) => {
            const c = new Set(prev);
            if (c.has(id)) c.delete(id);
            else c.add(id);
            return c;
        });
    };
    const selectAll = () => {
        if (!hits) return;
        if (selected.size === hits.length) setSelected(new Set());
        else setSelected(new Set(hits.map((h) => h.hf_id)));
    };

    const importReady = importStatus?.job_state === "ready";
    const importInProgress =
        importStatus?.job_state === "downloading" ||
        importStatus?.job_state === "importing";

    const indexedSet = useMemo(
        () => new Set(indexedDocs.map((d) => d.corpus_identifier)),
        [indexedDocs],
    );

    return (
        <div className="space-y-6 max-w-4xl">
            <div>
                <h2 className="text-2xl font-medium font-serif mb-2">
                    Italia legale
                </h2>
                <p className="text-sm text-gray-500 leading-relaxed">
                    Indice di norme italiane (Normattiva) e sentenze (Corte
                    Costituzionale) dal dataset CC-BY-4.0{" "}
                    <code>dossier-legal/italian-legal-corpus</code> su
                    HuggingFace. Snapshot del 2026-03-01. Solo i metadati
                    vengono scaricati localmente; il testo completo arriva
                    on-demand quando indicizzi un documento.
                </p>
            </div>

            {error && (
                <div className="text-sm text-red-600 bg-red-50 border border-red-200 rounded-md px-3 py-2 flex items-start gap-2">
                    <AlertCircle className="h-4 w-4 shrink-0 mt-0.5" />
                    <span>{error}</span>
                </div>
            )}

            {/* Sync toggle + sources */}
            <section className="border border-gray-200 rounded-lg p-4 space-y-3">
                <label className="flex items-start gap-3 cursor-pointer">
                    <input
                        type="checkbox"
                        checked={enabled}
                        onChange={(e) => setEnabled(e.target.checked)}
                        className="mt-1"
                    />
                    <span className="flex-1">
                        <span className="text-sm font-medium block">
                            Connettore attivo
                        </span>
                        <span className="text-xs text-gray-500 block mt-0.5">
                            Quando attivo, i risultati cercati possono essere
                            aggiunti al tuo indice locale.
                        </span>
                    </span>
                </label>
                <div className="space-y-3">
                    <div>
                        <div className="text-[11px] font-semibold uppercase tracking-wider text-gray-500 mb-1.5">
                            Disponibili ora
                        </div>
                        <div className="text-xs text-gray-700 space-y-1">
                            {SOURCE_OPTIONS.map((s) => (
                                <label
                                    key={s.value}
                                    className="flex items-center gap-2 cursor-pointer"
                                >
                                    <input
                                        type="checkbox"
                                        checked={enabledSources.includes(s.value)}
                                        onChange={(e) => {
                                            setEnabledSources((prev) =>
                                                e.target.checked
                                                    ? Array.from(
                                                          new Set([
                                                              ...prev,
                                                              s.value,
                                                          ]),
                                                      )
                                                    : prev.filter(
                                                          (x) => x !== s.value,
                                                      ),
                                            );
                                        }}
                                        disabled={!enabled}
                                    />
                                    {s.label}
                                </label>
                            ))}
                        </div>
                    </div>
                    <div>
                        <div className="text-[11px] font-semibold uppercase tracking-wider text-gray-500 mb-1.5">
                            In preparazione
                        </div>
                        <div className="space-y-1.5">
                            {PLANNED_SOURCES.map((s) => (
                                <div
                                    key={s.value}
                                    className="text-xs text-gray-500"
                                >
                                    <label
                                        className="flex items-center gap-2 cursor-not-allowed opacity-70"
                                        title={s.note}
                                    >
                                        <input
                                            type="checkbox"
                                            disabled
                                            className="cursor-not-allowed"
                                        />
                                        <span>
                                            {s.label}{" "}
                                            <span className="text-gray-400">
                                                ({s.approxRows})
                                            </span>
                                        </span>
                                        <span className="ml-auto inline-flex items-center px-1.5 py-0.5 rounded bg-amber-50 text-amber-800 border border-amber-200 text-[10px] font-normal">
                                            in arrivo
                                        </span>
                                    </label>
                                    <div className="text-[11px] text-gray-400 ml-6 mt-0.5 leading-snug">
                                        {s.note}
                                    </div>
                                </div>
                            ))}
                        </div>
                        <p className="text-[11px] text-gray-400 mt-3">
                            Vedi <code>docs/CORPORA.md</code> per la roadmap
                            di ogni fonte e i passi concreti per abilitarla.
                        </p>
                    </div>
                </div>
            </section>

            {/* Import section */}
            <section className="border border-gray-200 rounded-lg p-4 space-y-3">
                <div className="flex items-start justify-between gap-3">
                    <div>
                        <h3 className="text-sm font-medium">
                            Indice locale
                        </h3>
                        <p className="text-xs text-gray-500 mt-1">
                            Scarica una volta sola i metadati del dataset (~80
                            MB, ~91k righe). Da quel momento le ricerche sono
                            istantanee, locali. Il testo dei singoli documenti
                            è scaricato solo quando lo indicizzi.
                        </p>
                    </div>
                    <Button
                        onClick={startImport}
                        disabled={
                            !enabled || importStarting || importInProgress
                        }
                        className="bg-black text-white hover:bg-gray-900"
                    >
                        {importStarting || importInProgress ? (
                            <Loader2 className="h-3.5 w-3.5 mr-1 animate-spin" />
                        ) : (
                            <Download className="h-3.5 w-3.5 mr-1" />
                        )}
                        {importInProgress
                            ? "In corso…"
                            : importReady
                              ? "Re-importa"
                              : "Scarica indice"}
                    </Button>
                </div>

                {importStatus && (
                    <div className="text-xs text-gray-600 space-y-1">
                        <div className="flex items-center gap-2">
                            Stato:{" "}
                            <span
                                className={`font-mono ${
                                    importStatus.job_state === "failed"
                                        ? "text-red-600"
                                        : importReady
                                          ? "text-green-700"
                                          : ""
                                }`}
                            >
                                {importStatus.job_state}
                            </span>
                            {importInProgress && (
                                <span className="tabular-nums">
                                    · shard {importStatus.current_shard}/
                                    {importStatus.total_shards} ·{" "}
                                    {importStatus.rows_imported} righe
                                </span>
                            )}
                            {importReady && (
                                <span>
                                    · {importStatus.row_count} righe
                                    indicizzate
                                    {importStatus.last_import_at && (
                                        <>
                                            {" "}
                                            · ultimo import:{" "}
                                            {new Date(
                                                importStatus.last_import_at +
                                                    "Z",
                                            ).toLocaleString()}
                                        </>
                                    )}
                                </span>
                            )}
                        </div>
                        {importInProgress && (
                            <div className="h-1.5 bg-gray-100 rounded overflow-hidden">
                                <div
                                    className="h-full bg-blue-500 transition-all"
                                    style={{
                                        width: `${importStatus.percent}%`,
                                    }}
                                />
                            </div>
                        )}
                        {importStatus.job_state === "failed" && (
                            <div className="text-red-600">
                                Errore: {importStatus.job_error ?? "sconosciuto"}
                            </div>
                        )}
                    </div>
                )}
            </section>

            {/* Search */}
            <section className="border border-gray-200 rounded-lg p-4 space-y-3">
                <h3 className="text-sm font-medium">Cerca nell'indice</h3>
                <Input
                    value={query}
                    onChange={(e) => setQuery(e.target.value)}
                    placeholder="Parole chiave nel titolo (es. 'codice civile', 'sentenza 242', 'D.Lgs. 196/2003')"
                    disabled={!enabled || !importReady}
                    onKeyDown={(e) => {
                        if (e.key === "Enter") runSearch();
                    }}
                />
                <div className="grid grid-cols-2 gap-3 text-xs">
                    <div>
                        <label className="block text-gray-500 mb-1">
                            Fonte
                        </label>
                        <div className="space-y-1">
                            {SOURCE_OPTIONS.map((s) => (
                                <label
                                    key={s.value}
                                    className="flex items-center gap-2"
                                >
                                    <input
                                        type="checkbox"
                                        checked={filterSources.includes(s.value)}
                                        onChange={(e) => {
                                            setFilterSources((prev) =>
                                                e.target.checked
                                                    ? Array.from(
                                                          new Set([
                                                              ...prev,
                                                              s.value,
                                                          ]),
                                                      )
                                                    : prev.filter(
                                                          (x) => x !== s.value,
                                                      ),
                                            );
                                        }}
                                    />
                                    {s.label}
                                </label>
                            ))}
                        </div>
                    </div>
                    <div>
                        <label className="block text-gray-500 mb-1">
                            Tipo documento (opzionale)
                        </label>
                        <select
                            multiple
                            size={4}
                            value={filterDocTypes}
                            onChange={(e) => {
                                const opts = Array.from(
                                    e.target.selectedOptions,
                                ).map((o) => o.value);
                                setFilterDocTypes(opts);
                            }}
                            className="w-full rounded-md border border-gray-200 bg-white px-2 py-1 text-xs"
                        >
                            {DOC_TYPE_OPTIONS.map((o) => (
                                <option key={o.value} value={o.value}>
                                    {o.label}
                                </option>
                            ))}
                        </select>
                    </div>
                    <div>
                        <label className="block text-gray-500 mb-1">
                            Anno minimo
                        </label>
                        <Input
                            value={yearMin}
                            onChange={(e) => setYearMin(e.target.value)}
                            placeholder="es. 2000"
                        />
                    </div>
                    <div>
                        <label className="block text-gray-500 mb-1">
                            Anno massimo
                        </label>
                        <Input
                            value={yearMax}
                            onChange={(e) => setYearMax(e.target.value)}
                            placeholder="es. 2024"
                        />
                    </div>
                </div>
                <div className="flex justify-end">
                    <Button
                        onClick={runSearch}
                        disabled={!enabled || !importReady || searching}
                        className="bg-black text-white hover:bg-gray-900"
                    >
                        {searching ? (
                            <Loader2 className="h-3.5 w-3.5 mr-1 animate-spin" />
                        ) : (
                            <Search className="h-3.5 w-3.5 mr-1" />
                        )}
                        {searching ? "Ricerca…" : "Cerca"}
                    </Button>
                </div>
                {!importReady && (
                    <div className="text-xs text-amber-700 bg-amber-50 border border-amber-200 rounded px-2 py-1.5 flex items-start gap-1.5">
                        <Info className="h-3.5 w-3.5 mt-0.5 shrink-0" />
                        Scarica prima l'indice qui sopra per abilitare la
                        ricerca.
                    </div>
                )}
            </section>

            {searchError && (
                <div className="text-sm text-red-600 bg-red-50 border border-red-200 rounded-md px-3 py-2 flex items-start gap-2">
                    <AlertCircle className="h-4 w-4 shrink-0 mt-0.5" />
                    <span>{searchError}</span>
                </div>
            )}

            {/* Search results */}
            {hits !== null && (
                <section>
                    <div className="flex items-center justify-between mb-2">
                        <h3 className="text-sm font-medium">
                            Risultati ({hits.length})
                        </h3>
                        {hits.length > 0 && (
                            <div className="flex items-center gap-2">
                                <button
                                    type="button"
                                    onClick={selectAll}
                                    className="inline-flex items-center gap-1 rounded-md border border-gray-200 px-2.5 py-1 text-xs text-gray-700 hover:bg-gray-50 transition-colors"
                                    aria-label={
                                        selected.size === hits.length
                                            ? "Deseleziona tutti"
                                            : "Seleziona tutti"
                                    }
                                >
                                    {selected.size === hits.length ? (
                                        <CheckSquare className="h-3.5 w-3.5" />
                                    ) : (
                                        <Square className="h-3.5 w-3.5" />
                                    )}
                                    {selected.size === hits.length
                                        ? "Deseleziona tutti"
                                        : "Seleziona tutti"}
                                </button>
                                <Button
                                    onClick={indexSelected}
                                    disabled={
                                        bulkIndexing || selected.size === 0
                                    }
                                    className="bg-black text-white hover:bg-gray-900 h-8 text-xs"
                                >
                                    {bulkIndexing && (
                                        <Loader2 className="h-3 w-3 mr-1 animate-spin" />
                                    )}
                                    Indicizza {selected.size} selezionati
                                </Button>
                            </div>
                        )}
                    </div>
                    {hits.length === 0 ? (
                        <p className="text-sm text-gray-400">Nessun risultato.</p>
                    ) : (
                        <ul className="space-y-2">
                            {hits.map((hit) => {
                                const isSelected = selected.has(hit.hf_id);
                                const isThisIndexing =
                                    indexingId === hit.hf_id;
                                const alreadyIndexed = indexedSet.has(
                                    hit.hf_id,
                                );
                                return (
                                    <li
                                        key={hit.hf_id}
                                        className="border border-gray-200 rounded-lg p-3 flex items-start gap-3"
                                    >
                                        <input
                                            type="checkbox"
                                            checked={isSelected}
                                            onChange={() =>
                                                toggleSelected(hit.hf_id)
                                            }
                                            disabled={alreadyIndexed}
                                            className="mt-1"
                                        />
                                        <div className="min-w-0 flex-1">
                                            <div className="text-sm font-medium truncate">
                                                {hit.title ?? hit.hf_id}
                                            </div>
                                            <div className="text-xs text-gray-500 mt-0.5">
                                                {hit.source}
                                                {hit.doc_type && (
                                                    <> · {hit.doc_type}</>
                                                )}
                                                {hit.year && (
                                                    <> · {hit.year}</>
                                                )}
                                                {hit.number && (
                                                    <> · n. {hit.number}</>
                                                )}
                                                {hit.authority && (
                                                    <> · {hit.authority}</>
                                                )}
                                                <span className="text-gray-400 ml-2">
                                                    {(
                                                        hit.text_length / 1024
                                                    ).toFixed(0)}{" "}
                                                    KB
                                                </span>
                                            </div>
                                        </div>
                                        <button
                                            type="button"
                                            onClick={() => indexHit(hit)}
                                            disabled={
                                                isThisIndexing ||
                                                alreadyIndexed
                                            }
                                            className={`shrink-0 inline-flex items-center gap-1 rounded-md px-2.5 py-1.5 text-xs ${
                                                alreadyIndexed
                                                    ? "bg-green-50 text-green-700 border border-green-200"
                                                    : "bg-black text-white hover:bg-gray-900 disabled:opacity-50"
                                            }`}
                                        >
                                            {isThisIndexing ? (
                                                <Loader2 className="h-3.5 w-3.5 animate-spin" />
                                            ) : alreadyIndexed ? (
                                                <CheckCircle2 className="h-3.5 w-3.5" />
                                            ) : null}
                                            {isThisIndexing
                                                ? "Sync…"
                                                : alreadyIndexed
                                                  ? "Indicizzato"
                                                  : "Indicizza"}
                                        </button>
                                    </li>
                                );
                            })}
                        </ul>
                    )}
                </section>
            )}

            {/* Indexed list */}
            <section>
                <h3 className="text-sm font-medium mb-2">
                    Documenti indicizzati ({indexedDocs.length})
                </h3>
                {indexedDocs.length === 0 ? (
                    <p className="text-sm text-gray-400">
                        Nessun documento ancora indicizzato.
                    </p>
                ) : (
                    <ul className="space-y-2">
                        {indexedDocs.map((doc) => {
                            const isDeleting = deletingId === doc.id;
                            const isResyncing = resyncingId === doc.id;
                            const isSyncing =
                                doc.status === "syncing" || isResyncing;
                            const isInterrupted = doc.status === "interrupted";
                            const isReady = doc.status === "ready";
                            const isActiveEmbed =
                                isSyncing &&
                                embedProgress?.document_id === doc.id;
                            const isQueued = isSyncing && !isActiveEmbed;
                            return (
                                <li
                                    key={doc.id}
                                    className="border border-gray-200 rounded-lg p-3 flex items-start justify-between gap-3"
                                >
                                    <div className="min-w-0 flex-1">
                                        <div className="text-sm font-medium truncate flex items-center gap-2">
                                            {doc.filename}
                                            {isReady && (
                                                <span className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded bg-green-50 text-green-700 border border-green-200 text-[10px] font-normal">
                                                    <CheckCircle2 className="h-3 w-3" />
                                                    indicizzato
                                                </span>
                                            )}
                                            {isActiveEmbed && (
                                                <span className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded bg-blue-50 text-blue-700 border border-blue-200 text-[10px] font-normal">
                                                    <Loader2 className="h-3 w-3 animate-spin" />
                                                    in sync
                                                    {embedProgress && (
                                                        <span className="ml-1 tabular-nums">
                                                            {embedProgress.percent}%
                                                        </span>
                                                    )}
                                                </span>
                                            )}
                                            {isQueued && (
                                                <span
                                                    className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded bg-gray-50 text-gray-600 border border-gray-200 text-[10px] font-normal"
                                                    title="Le sync sono sequenziali — questa partirà al termine di quella in corso."
                                                >
                                                    <Loader2 className="h-3 w-3" />
                                                    in attesa
                                                </span>
                                            )}
                                            {isInterrupted && (
                                                <span className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded bg-red-50 text-red-700 border border-red-200 text-[10px] font-normal">
                                                    <AlertCircle className="h-3 w-3" />
                                                    interrotto
                                                </span>
                                            )}
                                        </div>
                                        <div className="text-xs text-gray-500 mt-0.5">
                                            hf_id {doc.corpus_identifier}
                                            <span className="ml-2 text-gray-400">
                                                {(doc.size_bytes / 1024).toFixed(0)} KB
                                            </span>
                                        </div>
                                        {isActiveEmbed && embedProgress && (
                                            <div className="mt-2">
                                                <div className="h-1.5 bg-gray-100 rounded overflow-hidden">
                                                    <div
                                                        className="h-full bg-blue-500 transition-all"
                                                        style={{
                                                            width: `${Math.min(100, embedProgress.percent)}%`,
                                                        }}
                                                    />
                                                </div>
                                                <div className="text-[10px] text-gray-500 mt-0.5 tabular-nums">
                                                    {embedProgress.current}/
                                                    {embedProgress.total} chunk embed
                                                </div>
                                            </div>
                                        )}
                                        {isQueued && (
                                            <div className="mt-1 text-[11px] text-gray-500">
                                                In coda — partirà al termine della sync attiva.
                                            </div>
                                        )}
                                    </div>
                                    <div className="flex items-center gap-1 shrink-0">
                                        {isInterrupted && (
                                            <button
                                                type="button"
                                                onClick={() => resyncDoc(doc)}
                                                disabled={isResyncing}
                                                className="rounded-md px-2 py-1 text-xs text-blue-700 hover:bg-blue-50 transition-colors disabled:opacity-40 inline-flex items-center gap-1"
                                                aria-label="Riavvia sync"
                                                title="Riavvia sincronizzazione"
                                            >
                                                <RefreshCw
                                                    className={`h-3.5 w-3.5 ${isResyncing ? "animate-spin" : ""}`}
                                                />
                                                Riavvia
                                            </button>
                                        )}
                                        <button
                                            type="button"
                                            onClick={() => deleteDoc(doc)}
                                            disabled={isDeleting}
                                            className="rounded-md p-1.5 text-gray-400 hover:text-red-600 hover:bg-red-50 transition-colors disabled:opacity-40"
                                            aria-label="Rimuovi"
                                            title={
                                                isQueued
                                                    ? "Annulla questa sync in coda"
                                                    : isActiveEmbed
                                                      ? "Interrompi e rimuovi"
                                                      : "Rimuovi dall'indice"
                                            }
                                        >
                                            {isDeleting ? (
                                                <Loader2 className="h-3.5 w-3.5 animate-spin" />
                                            ) : (
                                                <Trash2 className="h-3.5 w-3.5" />
                                            )}
                                        </button>
                                    </div>
                                </li>
                            );
                        })}
                    </ul>
                )}
            </section>
        </div>
    );
}
