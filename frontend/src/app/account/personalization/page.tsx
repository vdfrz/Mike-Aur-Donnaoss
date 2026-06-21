"use client";

import { useEffect, useRef, useState } from "react";
import { useTranslations } from "next-intl";
import {
    getPersonalization,
    putPersonalization,
    deletePersonalization,
} from "@/app/lib/mikeApi";

const API_BASE = process.env.NEXT_PUBLIC_API_BASE_URL ?? "http://localhost:3001";

function getToken() {
    return typeof window !== "undefined"
        ? localStorage.getItem("mike_auth_token") ?? ""
        : "";
}

// ── Types ────────────────────────────────────────────────────────────────────

interface RevisedSection {
    heading: string; before: string; after: string;
    scoreBefore?: number; scoreAfter?: number; changed: boolean;
}

interface HarnessEdit {
    kind: 'intent' | 'rule-added' | 'rule-removed' | 'lesson-compiled' | 'lesson-retired';
    heading?: string; before?: string; after?: string; text?: string;
}
interface HarnessState {
    phase: 'rewriting' | 'done' | 'skipped';
    fromGeneration?: number;
    generation?: number;
    edits: HarnessEdit[];
    rationale?: string;
    reason?: string;
}

interface ChatTurn {
    role: 'you' | 'assistant';
    text: string;
    revised?: RevisedSection[];
    lessons?: { rule: string }[];
    featureRequestsQueued?: number;
    note?: string;
    harness?: HarnessState;
}

interface LessonRow { rule: string; kind: string; scope: Record<string, string>; effectiveness: number; }
interface GenInfo {
    active: { generation: number; planOverrides: string[]; extraRules: number; compiledLessons: number };
    generations: string[];
}
interface FeatureRow { id: string; request: string; createdAt: string; status: string; }

// ── MikeChat (ProposalChat port) ─────────────────────────────────────────────

function MikeChat({
    initialThread = [],
    suggestions = [],
    onRefreshLedger,
}: {
    initialThread?: ChatTurn[];
    suggestions?: string[];
    onRefreshLedger: () => void;
}) {
    const [thread, setThread] = useState<ChatTurn[]>(initialThread);
    const [input, setInput] = useState('');
    const [busy, setBusy] = useState(false);
    const [liveEvents, setLiveEvents] = useState<string[]>([]);
    const [attachments, setAttachments] = useState<File[]>([]);
    const attachRef = useRef<HTMLInputElement | null>(null);
    const turnRef = useRef(0);
    const threadEndRef = useRef<HTMLDivElement | null>(null);

    function scrollDown() {
        requestAnimationFrame(() => threadEndRef.current?.scrollIntoView({ behavior: 'smooth' }));
    }

    async function send(presetMessage?: string) {
        const message = (presetMessage ?? input).trim();
        const files = attachments;
        if ((!message && !files.length) || busy) return;
        turnRef.current += 1;
        const history = thread.slice(-10).map(t => ({ role: t.role, text: t.text }));
        const shown = message || 'Apply the feedback in the attached document(s).';
        setThread(t => [...t, { role: 'you', text: files.length ? `${shown}\n${files.map(f => `📎 ${f.name}`).join('  ')}` : shown }]);
        setInput('');
        setAttachments([]);
        setBusy(true);
        setLiveEvents(files.length ? ['Reading attachments…'] : []);
        scrollDown();

        try {
            const token = getToken();
            // Always multipart — the backend reads message/history/files uniformly.
            const fd = new FormData();
            fd.append('message', message);
            fd.append('chatTurn', String(turnRef.current));
            fd.append('history', JSON.stringify(history));
            files.forEach(f => fd.append('files', f));
            const res = await fetch(`${API_BASE}/mike-feedback/chat`, {
                method: 'POST', body: fd,
                headers: token ? { Authorization: `Bearer ${token}` } : {},
            });
            if (!res.ok || !res.body) {
                const e = await res.json().catch(() => ({ error: `HTTP ${res.status}` }));
                throw new Error(e.error || `HTTP ${res.status}`);
            }
            const reader = res.body.getReader();
            const dec = new TextDecoder();
            let buf = '';
            for (;;) {
                const { done, value } = await reader.read();
                if (done) break;
                buf += dec.decode(value, { stream: true });
                let nl: number;
                while ((nl = buf.indexOf('\n')) >= 0) {
                    const line = buf.slice(0, nl).trim(); buf = buf.slice(nl + 1);
                    // SSE framing: skip blanks and keep-alive comments; unwrap `data:`.
                    if (!line || line.startsWith(':')) continue;
                    const payload = line.startsWith('data:') ? line.slice(5).trim() : line;
                    if (!payload || payload === '[DONE]') continue;
                    // eslint-disable-next-line @typescript-eslint/no-explicit-any
                    let msg: any;
                    try { msg = JSON.parse(payload); } catch { continue; }
                    if (msg.type === 'reply') {
                        setThread(t => [...t, { role: 'assistant', text: msg.text }]);
                        scrollDown();
                    } else if (msg.type === 'harness') {
                        setThread(t => {
                            const next = [...t];
                            const last = [...next].reverse().find(x => x.role === 'assistant');
                            if (!last) return next;
                            if (msg.phase === 'start') {
                                last.harness = { phase: 'rewriting', fromGeneration: msg.generation, edits: [] };
                            } else if (msg.phase === 'edit' && last.harness) {
                                last.harness = { ...last.harness, edits: [...last.harness.edits, msg as HarnessEdit] };
                            } else if (msg.phase === 'done' && last.harness) {
                                last.harness = { ...last.harness, phase: 'done', generation: msg.generation, rationale: msg.rationale };
                            } else if (msg.phase === 'skipped') {
                                last.harness = last.harness
                                    ? { ...last.harness, phase: 'skipped', reason: msg.reason }
                                    : { phase: 'skipped', edits: [], reason: msg.reason };
                            }
                            return next;
                        });
                        scrollDown();
                    } else if (msg.type === 'event') {
                        setLiveEvents(ev => [...ev, `${msg.agent}: ${msg.text}`]);
                        scrollDown();
                    } else if (msg.type === 'complete') {
                        setThread(t => {
                            const next = [...t];
                            const lastAssistant = [...next].reverse().find(x => x.role === 'assistant');
                            if (lastAssistant) {
                                lastAssistant.revised = msg.revised;
                                lastAssistant.lessons = msg.lessons;
                                lastAssistant.featureRequestsQueued = msg.featureRequestsQueued;
                                lastAssistant.note = msg.note;
                            }
                            return next;
                        });
                        setLiveEvents([]);
                        onRefreshLedger();
                        scrollDown();
                    } else if (msg.type === 'error') {
                        setThread(t => [...t, { role: 'assistant', text: `Something went wrong: ${msg.error}` }]);
                    }
                }
            }
        } catch (e) {
            setThread(t => [...t, { role: 'assistant', text: e instanceof Error ? e.message : 'Feedback failed.' }]);
        } finally {
            setBusy(false);
            scrollDown();
        }
    }

    function HarnessCard({ h }: { h: HarnessState }) {
        const editLabel: Record<HarnessEdit['kind'], string> = {
            'intent': 'Section instruction rewritten',
            'rule-added': 'New drafting rule',
            'rule-removed': 'Rule removed',
            'lesson-compiled': 'Lesson learned',
            'lesson-retired': 'Lesson retired',
        };
        return (
            <div style={{
                marginTop: 10, border: '1px solid var(--color-border)', borderRadius: 10,
                background: 'rgba(0,0,0,0.025)', padding: '10px 12px', fontSize: 13,
            }}>
                <div style={{ display: 'flex', alignItems: 'center', gap: 8, fontWeight: 600 }}>
                    <span className={h.phase === 'rewriting' ? 'harness-pulse' : undefined}>🧬</span>
                    {h.phase === 'rewriting' && <span>Rewriting the drafting instructions from your feedback<span className="harness-dots" /></span>}
                    {h.phase === 'done' && <span>Drafting instructions upgraded — generation {h.generation}</span>}
                    {h.phase === 'skipped' && <span style={{ fontWeight: 400, opacity: 0.75 }}>{h.reason || 'Instructions unchanged this turn.'}</span>}
                </div>
                {h.edits.map((e, i) => (
                    <div key={i} className="harness-edit" style={{ marginTop: 8, paddingLeft: 4 }}>
                        <div style={{ fontWeight: 600, fontSize: 12 }}>{editLabel[e.kind]}{e.heading ? `: ${e.heading}` : ''}</div>
                        {e.kind === 'intent' ? (
                            <div style={{ marginTop: 4, display: 'grid', gap: 4 }}>
                                <div style={{ background: 'rgba(220,38,38,0.08)', borderRadius: 6, padding: '5px 8px', textDecoration: 'line-through', opacity: 0.7, fontSize: 12 }}>
                                    {(e.before ?? '').slice(0, 220)}{(e.before ?? '').length > 220 ? '…' : ''}
                                </div>
                                <div style={{ background: 'rgba(22,163,74,0.12)', borderRadius: 6, padding: '5px 8px', fontSize: 12 }}>
                                    {(e.after ?? '').slice(0, 320)}{(e.after ?? '').length > 320 ? '…' : ''}
                                </div>
                            </div>
                        ) : (
                            <div style={{
                                marginTop: 3, fontSize: 12, padding: '4px 8px', borderRadius: 6,
                                background: e.kind === 'rule-removed' || e.kind === 'lesson-retired' ? 'rgba(220,38,38,0.08)' : 'rgba(22,163,74,0.10)',
                                textDecoration: e.kind === 'rule-removed' ? 'line-through' : undefined,
                                opacity: e.kind === 'lesson-retired' ? 0.65 : 1,
                            }}>{e.text}</div>
                        )}
                    </div>
                ))}
                {h.phase === 'done' && (
                    <div style={{ marginTop: 8, fontSize: 11.5, color: 'var(--color-muted-foreground)' }}>
                        Every future draft uses these instructions automatically.
                    </div>
                )}
            </div>
        );
    }

    const bubble = (role: 'you' | 'assistant'): React.CSSProperties => ({
        alignSelf: role === 'you' ? 'flex-end' : 'flex-start',
        maxWidth: '85%',
        background: role === 'you' ? 'var(--color-blue)' : 'var(--card)',
        color: role === 'you' ? 'var(--primary-foreground)' : 'var(--color-foreground)',
        border: role === 'you' ? 'none' : '1px solid var(--color-border)',
        borderRadius: 12, padding: '10px 14px', fontSize: 14, lineHeight: 1.5, whiteSpace: 'pre-wrap',
    });

    return (
        <div style={{ marginTop: 4 }}>
            <style>{`
                .harness-edit { animation: harnessIn 420ms ease-out both; }
                @keyframes harnessIn { from { opacity: 0; transform: translateY(6px); } to { opacity: 1; transform: none; } }
                .harness-pulse { display: inline-block; animation: harnessPulse 1.1s ease-in-out infinite; }
                @keyframes harnessPulse { 0%,100% { transform: scale(1); opacity: 1; } 50% { transform: scale(1.25); opacity: 0.55; } }
                .harness-dots::after { content: ''; animation: harnessDots 1.4s steps(4, end) infinite; }
                @keyframes harnessDots { 0% { content: ''; } 25% { content: '.'; } 50% { content: '..'; } 75% { content: '...'; } }
            `}</style>

            <div style={{ display: 'flex', flexDirection: 'column', gap: 10, marginBottom: 14 }}>
                {thread.map((turn, i) => (
                    <div key={i} style={bubble(turn.role)}>
                        <div>{turn.text}</div>
                        {turn.harness && <HarnessCard h={turn.harness} />}
                        {turn.note && <div style={{ marginTop: 8, fontSize: 12.5, opacity: 0.85 }}>{turn.note}</div>}
                        {turn.revised && turn.revised.length > 0 && (
                            <div style={{ marginTop: 10 }}>
                                {turn.revised.map((r, j) => (
                                    <details key={j} style={{ marginBottom: 6, fontSize: 13 }}>
                                        <summary style={{ cursor: 'pointer' }}>
                                            {r.changed ? '✓' : '·'} {r.heading}
                                            {r.scoreBefore != null && r.scoreAfter != null ? ` — score ${r.scoreBefore}→${r.scoreAfter}` : ''}
                                        </summary>
                                        <div style={{ marginTop: 6, display: 'grid', gap: 8 }}>
                                            <div style={{ background: 'rgba(0,0,0,0.04)', padding: 8, borderRadius: 6 }}>
                                                <div style={{ fontSize: 11, textTransform: 'uppercase', opacity: 0.6, marginBottom: 3 }}>Before</div>
                                                {r.before}
                                            </div>
                                            <div style={{ background: 'rgba(22,163,74,0.10)', padding: 8, borderRadius: 6 }}>
                                                <div style={{ fontSize: 11, textTransform: 'uppercase', opacity: 0.6, marginBottom: 3 }}>After</div>
                                                {r.after}
                                            </div>
                                        </div>
                                    </details>
                                ))}
                            </div>
                        )}
                        {((turn.lessons && turn.lessons.length > 0) || (turn.featureRequestsQueued ?? 0) > 0) && (
                            <div style={{ marginTop: 10, display: 'flex', flexWrap: 'wrap', gap: 6 }}>
                                {turn.lessons?.slice(0, 4).map((l, k) => (
                                    <span key={k} title={l.rule} style={{ fontSize: 11, background: 'rgba(0,0,0,0.06)', borderRadius: 999, padding: '3px 10px' }}>
                                        learned: {l.rule.length > 48 ? l.rule.slice(0, 48) + '…' : l.rule}
                                    </span>
                                ))}
                                {(turn.featureRequestsQueued ?? 0) > 0 && (
                                    <span style={{ fontSize: 11, background: 'rgba(0,0,0,0.06)', borderRadius: 999, padding: '3px 10px' }}>
                                        {turn.featureRequestsQueued} queued for the dev team
                                    </span>
                                )}
                            </div>
                        )}
                    </div>
                ))}
                {busy && liveEvents.length > 0 && (
                    <div style={{ ...bubble('assistant'), fontFamily: 'monospace', fontSize: 12, opacity: 0.8 }}>
                        {liveEvents.slice(-6).map((e, i) => <div key={i}>{e}</div>)}
                    </div>
                )}
                <div ref={threadEndRef} />
            </div>

            {suggestions.length > 0 && !busy && (
                <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8, marginBottom: 10 }}>
                    {suggestions.map((s, i) => (
                        <button
                            key={i}
                            onClick={() => send(s)}
                            className="harness-edit"
                            style={{
                                fontFamily: 'var(--font-sans)', fontSize: 12.5, padding: '6px 12px', borderRadius: 999,
                                border: '1px solid var(--color-border)', background: 'var(--card)',
                                color: 'var(--color-foreground)', cursor: 'pointer',
                            }}
                        >
                            {s}
                        </button>
                    ))}
                </div>
            )}

            {attachments.length > 0 && (
                <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6, marginBottom: 8 }}>
                    {attachments.map((f, i) => (
                        <span key={i} style={{ fontSize: 12, background: 'rgba(0,0,0,0.06)', borderRadius: 999, padding: '4px 10px', display: 'inline-flex', alignItems: 'center', gap: 6 }}>
                            {f.name}
                            <button
                                onClick={() => setAttachments(a => a.filter((_, j) => j !== i))}
                                style={{ border: 'none', background: 'transparent', cursor: 'pointer', fontSize: 12, padding: 0, lineHeight: 1 }}
                                aria-label={`Remove ${f.name}`}
                            >✕</button>
                        </span>
                    ))}
                </div>
            )}

            <div style={{ display: 'flex', gap: 10, alignItems: 'flex-end' }}>
                <button
                    title="Attach a marked-up printout, scan, or corrected document (PDF, DOCX, images — OCR included)"
                    aria-label="Attach documents"
                    disabled={busy}
                    onClick={() => attachRef.current?.click()}
                    style={{
                        fontFamily: 'var(--font-sans)', fontSize: 15, fontWeight: 500, padding: '10px 13px', borderRadius: 8, cursor: busy ? 'default' : 'pointer',
                        background: 'transparent', color: 'var(--color-foreground)', border: '1px solid var(--color-border)', opacity: busy ? 0.5 : 1,
                    }}
                >📎</button>
                <input
                    ref={attachRef}
                    type="file"
                    multiple
                    accept=".pdf,.docx,.doc,.txt,.md,.rtf,.html,.png,.jpg,.jpeg,.webp,.tif,.tiff"
                    style={{ display: 'none' }}
                    onChange={e => { const fs = Array.from(e.target.files ?? []); if (fs.length) setAttachments(a => [...a, ...fs].slice(0, 4)); e.target.value = ''; }}
                />
                <textarea
                    value={input}
                    onChange={e => setInput(e.target.value)}
                    onKeyDown={e => { if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) { e.preventDefault(); send(); } }}
                    placeholder="What was wrong, what was right, or what you'd change — in your own words. Attach a marked-up printout and just hit Send."
                    rows={3}
                    disabled={busy}
                    style={{
                        flex: 1, fontFamily: 'var(--font-sans)', fontSize: 14, padding: '10px 12px', borderRadius: 8,
                        border: '1px solid var(--color-border)', background: 'var(--card)', color: 'var(--color-foreground)', resize: 'vertical',
                    }}
                />
                <button
                    disabled={busy || (!input.trim() && !attachments.length)}
                    onClick={() => send()}
                    style={{
                        fontFamily: 'var(--font-sans)', fontSize: 15, fontWeight: 500, padding: '12px 22px', borderRadius: 8,
                        cursor: (busy || (!input.trim() && !attachments.length)) ? 'default' : 'pointer',
                        background: 'var(--color-blue)', color: '#fff', border: 'none',
                        opacity: (busy || (!input.trim() && !attachments.length)) ? 0.5 : 1,
                    }}
                >
                    {busy ? 'Working…' : 'Send'}
                </button>
            </div>
            <div style={{ fontSize: 11.5, color: 'var(--color-muted-foreground)', marginTop: 6 }}>
                ⌘ Enter to send · Accepts PDF, DOCX, images (scans are OCR&apos;d)
            </div>
        </div>
    );
}

// ── Main page ─────────────────────────────────────────────────────────────────

export default function PersonalizationPage() {
    const t = useTranslations("Personalization");

    // Existing personalization text
    const [text, setText] = useState("");
    const [updatedAt, setUpdatedAt] = useState<string | null>(null);
    const [saving, setSaving] = useState(false);
    const [saveStatus, setSaveStatus] = useState<{ type: "ok" | "err"; msg: string } | null>(null);
    const [confirmDelete, setConfirmDelete] = useState(false);

    // Firm listens
    const [history, setHistory] = useState<ChatTurn[] | null>(null);
    const [lessons, setLessons] = useState<LessonRow[]>([]);
    const [lessonStats, setLessonStats] = useState<{ total: number; active: number; deprecated: number } | null>(null);
    const [gens, setGens] = useState<GenInfo | null>(null);

    // Feature requests
    const [features, setFeatures] = useState<FeatureRow[]>([]);
    const [featureInput, setFeatureInput] = useState('');
    const [featureSaving, setFeatureSaving] = useState(false);

    const token = getToken();

    async function refreshLedger() {
        try {
            const [l, g, f] = await Promise.all([
                fetch(`${API_BASE}/mike-feedback/lessons`, { headers: token ? { Authorization: `Bearer ${token}` } : {} }).then(r => r.json()),
                fetch(`${API_BASE}/mike-feedback/generations`, { headers: token ? { Authorization: `Bearer ${token}` } : {} }).then(r => r.json()),
                fetch(`${API_BASE}/mike-feedback/features`, { headers: token ? { Authorization: `Bearer ${token}` } : {} }).then(r => r.json()),
            ]);
            setLessons(l.selected ?? []);
            setLessonStats(l.stats ?? null);
            setGens(g);
            setFeatures(f.requests ?? []);
        } catch { /* ledger is best-effort */ }
    }

    useEffect(() => {
        getPersonalization().then((p) => {
            setText(p.profile_text);
            setUpdatedAt(p.updated_at);
        });
        fetch(`${API_BASE}/mike-feedback/history?limit=20`, { headers: token ? { Authorization: `Bearer ${token}` } : {} })
            .then(r => r.json())
            .then(d => setHistory((d.turns ?? []).map((t: { role: 'you' | 'assistant'; text: string; note?: string }) => ({ role: t.role, text: t.text, note: t.note }))))
            .catch(() => setHistory([]));
        refreshLedger();
    // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);

    async function handleSave() {
        setSaving(true);
        setSaveStatus(null);
        try {
            const res = await putPersonalization(text);
            setUpdatedAt(res.updated_at);
            setSaveStatus({ type: "ok", msg: t("saved") });
        } catch (e: unknown) {
            setSaveStatus({ type: "err", msg: e instanceof Error ? e.message : "Failed" });
        } finally {
            setSaving(false);
        }
    }

    async function handleDelete() {
        setSaving(true);
        setSaveStatus(null);
        try {
            await deletePersonalization();
            setText("");
            setUpdatedAt(null);
            setConfirmDelete(false);
            setSaveStatus({ type: "ok", msg: t("cleared") });
        } catch (e: unknown) {
            setSaveStatus({ type: "err", msg: e instanceof Error ? e.message : "Failed" });
        } finally {
            setSaving(false);
        }
    }

    async function handleSubmitFeature(e: React.FormEvent) {
        e.preventDefault();
        const req = featureInput.trim();
        if (!req) return;
        setFeatureSaving(true);
        try {
            const res = await fetch(`${API_BASE}/mike-feedback/features`, {
                method: 'POST',
                headers: { 'Content-Type': 'application/json', ...(token ? { Authorization: `Bearer ${token}` } : {}) },
                body: JSON.stringify({ request: req }),
            });
            if (res.ok) {
                setFeatureInput('');
                await refreshLedger();
            }
        } catch { /* best-effort */ } finally {
            setFeatureSaving(false);
        }
    }

    const card: React.CSSProperties = {
        border: '1px solid var(--color-border)', borderRadius: 12, padding: '18px 20px',
        background: 'var(--card)', marginBottom: 18,
    };

    return (
        <div style={{ fontFamily: 'var(--font-sans)', color: 'var(--color-foreground)' }}>

            {/* ── About your practice ── */}
            <section style={{ marginBottom: 40 }}>
                <h2 style={{ fontFamily: 'var(--font-serif)', fontSize: 26, fontWeight: 500, margin: '0 0 6px' }}>
                    {t("heading")}
                </h2>
                <p style={{ color: 'var(--color-muted-foreground)', fontSize: 14, lineHeight: 1.5, margin: '0 0 16px', maxWidth: 560 }}>
                    {t("description")}
                </p>
                <div style={{ maxWidth: 560 }}>
                    <textarea
                        value={text}
                        onChange={(e) => setText(e.target.value)}
                        placeholder={t("placeholder")}
                        rows={8}
                        maxLength={4000}
                        style={{
                            width: '100%', fontFamily: 'var(--font-sans)', fontSize: 14, padding: '10px 12px',
                            borderRadius: 8, border: '1px solid var(--color-border)', background: 'var(--card)',
                            color: 'var(--color-foreground)', resize: 'vertical', minHeight: 160, boxSizing: 'border-box',
                        }}
                    />
                    <div style={{ display: 'flex', justifyContent: 'space-between', marginTop: 4, fontSize: 12, color: 'var(--color-muted-foreground)' }}>
                        <span>{text.length}/4000</span>
                        {updatedAt && <span>{t("lastUpdated")}: {new Date(updatedAt).toLocaleDateString()}</span>}
                    </div>
                </div>
                <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginTop: 12 }}>
                    <button
                        onClick={handleSave}
                        disabled={saving}
                        style={{
                            fontFamily: 'var(--font-sans)', fontSize: 14, fontWeight: 500, padding: '9px 18px', borderRadius: 8,
                            background: 'var(--color-blue)', color: '#fff', border: 'none', cursor: saving ? 'default' : 'pointer', opacity: saving ? 0.6 : 1,
                        }}
                    >
                        {saving ? t("saving") : t("save")}
                    </button>
                    {text && !confirmDelete && (
                        <button onClick={() => setConfirmDelete(true)} style={{ fontSize: 14, color: '#dc2626', background: 'none', border: 'none', cursor: 'pointer' }}>
                            {t("reset")}
                        </button>
                    )}
                    {confirmDelete && (
                        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                            <span style={{ fontSize: 14, color: 'var(--color-muted-foreground)' }}>{t("confirmReset")}</span>
                            <button onClick={handleDelete} disabled={saving} style={{ fontSize: 13, color: '#dc2626', border: '1px solid #f3c4c4', borderRadius: 6, padding: '4px 10px', background: 'none', cursor: 'pointer' }}>{t("yes")}</button>
                            <button onClick={() => setConfirmDelete(false)} style={{ fontSize: 13, color: 'var(--color-muted-foreground)', border: '1px solid var(--color-border)', borderRadius: 6, padding: '4px 10px', background: 'none', cursor: 'pointer' }}>{t("no")}</button>
                        </div>
                    )}
                </div>
                {saveStatus && (
                    <p style={{ fontSize: 13, marginTop: 8, color: saveStatus.type === "ok" ? '#16a34a' : '#dc2626' }}>{saveStatus.msg}</p>
                )}
            </section>

            {/* ── Mike listens ── */}
            <h2 style={{ fontFamily: 'var(--font-serif)', fontSize: 34, fontWeight: 500, margin: '0 0 8px', letterSpacing: '-0.01em' }}>
                Mike listens.
            </h2>
            <p style={{ color: 'var(--color-muted-foreground)', fontSize: 16, lineHeight: 1.5, margin: '0 0 24px', maxWidth: 560 }}>
                Tell it what was wrong, what was right, or what you&apos;d change — in your own words. It rewrites its own drafting instructions in front of you, and never makes the same mistake twice.
            </p>

            <div style={card}>
                {history === null ? (
                    <div style={{ fontSize: 14, color: 'var(--color-muted-foreground)' }}>Loading your conversation…</div>
                ) : (
                    <MikeChat
                        initialThread={history}
                        suggestions={[
                            'What have you learned from my feedback so far?',
                            'Make the writing plainer and more direct',
                            'Always end pleadings with the verification clause',
                        ]}
                        onRefreshLedger={refreshLedger}
                    />
                )}
            </div>

            {/* ── What Mike has learned ── */}
            <h2 style={{ fontFamily: 'var(--font-serif)', fontSize: 26, fontWeight: 500, margin: '34px 0 14px' }}>What Mike has learned</h2>
            <div style={card}>
                {lessonStats && (
                    <div style={{ fontSize: 13, color: 'var(--color-muted-foreground)', marginBottom: 12 }}>
                        {lessonStats.active} active lessons · {lessonStats.deprecated} retired
                    </div>
                )}
                {lessons.length === 0 && <div style={{ fontSize: 14, color: 'var(--color-muted-foreground)' }}>Nothing yet — your first feedback starts the ledger.</div>}
                {lessons.map((l, i) => (
                    <div key={i} style={{ padding: '10px 0', borderTop: i ? '1px solid var(--color-border)' : 'none' }}>
                        <div style={{ fontSize: 14, lineHeight: 1.45 }}>{l.kind === 'dont' ? '✋' : '✓'} {l.rule}</div>
                        <div style={{ display: 'flex', alignItems: 'center', gap: 10, marginTop: 6 }}>
                            <div style={{ flex: '0 0 120px', height: 5, borderRadius: 999, background: 'rgba(0,0,0,0.08)', overflow: 'hidden' }}>
                                <div style={{ width: `${Math.round(l.effectiveness * 100)}%`, height: '100%', background: 'var(--color-blue)', transition: 'width 600ms ease' }} />
                            </div>
                            <span style={{ fontSize: 11.5, color: 'var(--color-muted-foreground)' }}>
                                {Math.round(l.effectiveness * 100)}% effective · applied to {Object.keys(l.scope).length ? Object.values(l.scope).join(', ') : 'all drafts'}
                            </span>
                        </div>
                    </div>
                ))}
            </div>

            {/* ── Drafting instructions — generation history ── */}
            <h2 style={{ fontFamily: 'var(--font-serif)', fontSize: 26, fontWeight: 500, margin: '34px 0 14px' }}>Drafting instructions — generation history</h2>
            <div style={card}>
                {gens ? (
                    <>
                        <div style={{ fontSize: 14 }}>
                            Active: <strong>generation {gens.active.generation}</strong>
                            {gens.active.planOverrides.length > 0 && <> · rewritten sections: {gens.active.planOverrides.join(', ')}</>}
                            {' '}· {gens.active.extraRules} extra rules · {gens.active.compiledLessons} compiled lessons
                        </div>
                        <div style={{ fontSize: 12.5, color: 'var(--color-muted-foreground)', marginTop: 8 }}>
                            {gens.generations.length
                                ? `Lineage: ${gens.generations.join(' → ')} — every generation was written from your feedback.`
                                : 'Built-in instructions (generation 0). The first feedback creates generation 1.'}
                        </div>
                    </>
                ) : <div style={{ fontSize: 14, color: 'var(--color-muted-foreground)' }}>Loading…</div>}
            </div>

            {/* ── Feature requests ── */}
            <h2 style={{ fontFamily: 'var(--font-serif)', fontSize: 26, fontWeight: 500, margin: '34px 0 6px' }}>Feature requests</h2>
            <p style={{ fontSize: 14, color: 'var(--color-muted-foreground)', margin: '0 0 14px', maxWidth: 500 }}>
                Features you want added to Mike — separate from the feedback above. The feedback above only improves how Mike drafts; new features go here for the dev team.
            </p>
            <div style={card}>
                <form onSubmit={handleSubmitFeature} style={{ display: 'flex', gap: 10, marginBottom: features.length ? 16 : 0 }}>
                    <input
                        type="text"
                        value={featureInput}
                        onChange={e => setFeatureInput(e.target.value)}
                        placeholder="e.g. Auto-number paragraphs in pleadings"
                        style={{
                            flex: 1, fontFamily: 'var(--font-sans)', fontSize: 14, padding: '9px 12px', borderRadius: 8,
                            border: '1px solid var(--color-border)', background: 'var(--card)', color: 'var(--color-foreground)',
                        }}
                    />
                    <button
                        type="submit"
                        disabled={featureSaving || !featureInput.trim()}
                        style={{
                            fontFamily: 'var(--font-sans)', fontSize: 14, fontWeight: 500, padding: '9px 18px', borderRadius: 8,
                            background: 'var(--color-blue)', color: '#fff', border: 'none',
                            cursor: (featureSaving || !featureInput.trim()) ? 'default' : 'pointer',
                            opacity: (featureSaving || !featureInput.trim()) ? 0.5 : 1,
                        }}
                    >
                        {featureSaving ? 'Adding…' : 'Add'}
                    </button>
                </form>
                {features.length === 0 && (
                    <div style={{ fontSize: 14, color: 'var(--color-muted-foreground)' }}>No feature requests yet.</div>
                )}
                {features.map(f => (
                    <div key={f.id} style={{ fontSize: 14, padding: '8px 0', borderTop: '1px solid var(--color-border)' }}>
                        {f.request}
                        <span style={{ marginLeft: 10, fontSize: 12, color: 'var(--color-muted-foreground)' }}>{f.status}</span>
                    </div>
                ))}
            </div>

        </div>
    );
}
