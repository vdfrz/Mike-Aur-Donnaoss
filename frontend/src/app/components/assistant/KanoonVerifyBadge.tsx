"use client";

/**
 * KanoonVerifyBadge
 *
 * Renders inline next to a Kanoon citation in chat. Shows the current
 * eCourts verification status (if any) and lets the user run a manual
 * verification: open eCourts, solve the CAPTCHA, find the case, paste
 * the canonical case number back here.
 *
 * Persists to the backend `/ecourts-verify` route so verifications
 * survive across sessions and across chats referencing the same Kanoon
 * tid.
 *
 * Why "manual paste" instead of automated DOM scraping: see the design
 * note in src/routes/ecourts.rs. eCourts ToS, CAPTCHA arms race, and
 * page-structure stability all point at human-in-the-loop as the only
 * defensible flow. The user types the case number; Mike records it.
 */

import { useEffect, useState } from "react";
import { CheckCircle2, ExternalLink, X } from "lucide-react";

const ECOURTS_URL = "https://judgments.ecourts.gov.in/pdfsearch/index.php";
const SCR_URL = "https://scr.sci.gov.in";

function isSupremeCourt(title: string, court?: string): boolean {
  if (court) {
    const c = court.toLowerCase();
    if (/\bsupreme court\b/.test(c)) return true;
  }
  const t = title.toLowerCase();
  return (
    /\bsupreme court\b/.test(t) ||
    /\bapex court\b/.test(t) ||
    /\binsc\b/.test(t) ||
    /\bscc\s+(online|onLine)?\s*sc\b/i.test(title) ||
    /\(sc\)/.test(t) ||
    /\bair\s+\d{4}\s+sc\b/.test(t) ||
    /\b\d{4}\s+\d+\s+scc\b/.test(t) ||  // "1973 4 SCC 225"
    /\bscc\s+\d+\b/.test(t) ||           // "SCC 225"
    /\bscr\s+\d+\b/.test(t) ||           // "SCR 123"
    /\b\d{4}\s+\d+\s+scr\b/.test(t) ||   // "1973 1 SCR 1"
    /\bscale\s+\d+\b/.test(t) ||          // SCALE reports (SC only)
    /\bjt\s+\d{4}\s*\(\s*\d+\s*\)\s+sc\b/.test(t) // "JT 2024 (1) SC"
  );
}

/**
 * Fetch the court (docsource) for a Kanoon tid from the backend's
 * lightweight metadata endpoint. Returns the court string or null.
 */
async function fetchKanoonCourt(tid: number): Promise<string | null> {
  try {
    const token = typeof window !== "undefined" ? localStorage.getItem("mike_auth_token") : null;
    const r = await fetch(`${API_BASE}/indian-kanoon/meta/${tid}`, {
      headers: token ? { Authorization: `Bearer ${token}` } : {},
    });
    if (!r.ok) return null;
    const data = await r.json();
    return data.docsource ?? null;
  } catch {
    return null;
  }
}

// Base URL for Mike's Rust backend — same env var the rest of the
// frontend reads. Without this, relative fetches go to the Next.js dev
// server (port 3000) and 404 since the eCourts routes live on the Rust
// API (typically port 3001).
const API_BASE =
  process.env.NEXT_PUBLIC_API_BASE_URL ?? "http://localhost:3001";

type Status = "loading" | "none" | "verified" | "not_found";

interface Verification {
  id: string;
  kanoon_tid: number;
  status: "verified" | "not_found" | "pending";
  ecourts_case_number: string | null;
  ecourts_pdf_url: string | null;
  verified_at: string;
}

async function openExternalUrl(url: string) {
  // Tauri's invoke is preferred (uses the system's default browser); fall
  // back to window.open when running outside the Tauri shell (browser dev).
  try {
    const tauri = await import("@tauri-apps/api/core");
    await tauri.invoke("open_external_url", { url });
  } catch {
    window.open(url, "_blank", "noopener,noreferrer");
  }
}

/**
 * Parse SCR / SCC / AIR citation components from a Kanoon title so we can
 * pre-fill the detailed eCourts SCR search form that appears *after* the
 * CAPTCHA is solved.
 */
function parseCitationDetails(title: string): {
  citYear?: number;
  citVolume?: number;
  citPage?: number;
  neuYear?: number;
  neuNumber?: number;
} {
  const out: {
    citYear?: number;
    citVolume?: number;
    citPage?: number;
    neuYear?: number;
    neuNumber?: number;
  } = {};
  let m: RegExpMatchArray | null;

  // "1973 1 SCR 1" — year volume SCR page
  m = title.match(/(\d{4})\s+(\d+)\s+SCR\s+(\d+)/i);
  if (m) {
    out.citYear = parseInt(m[1]);
    out.citVolume = parseInt(m[2]);
    out.citPage = parseInt(m[3]);
    return out;
  }
  // "SCR 1973 (1) 1" variant
  m = title.match(/SCR\s+(\d{4})\s*\(\s*(\d+)\s*\)\s+(\d+)/i);
  if (m) {
    out.citYear = parseInt(m[1]);
    out.citVolume = parseInt(m[2]);
    out.citPage = parseInt(m[3]);
    return out;
  }
  // "1973 4 SCC 225"
  m = title.match(/(\d{4})\s+(\d+)\s+SCC\s+(\d+)/i);
  if (m) {
    out.citYear = parseInt(m[1]);
    out.citVolume = parseInt(m[2]);
    out.citPage = parseInt(m[3]);
    return out;
  }
  // "AIR 1973 SC 1461"
  m = title.match(/AIR\s+(\d{4})\s+SC\s+(\d+)/i);
  if (m) {
    out.citYear = parseInt(m[1]);
    out.citPage = parseInt(m[2]);
    return out;
  }
  // NEUTRAL citation: "2024 INSC 123"
  m = title.match(/(\d{4})\s+INSC\s+(\d+)/i);
  if (m) {
    out.neuYear = parseInt(m[1]);
    out.neuNumber = parseInt(m[2]);
  }
  return out;
}

/**
 * Build a self-contained JS snippet that pre-fills the eCourts pdfsearch
 * form. Runs as a Tauri `initialization_script` — fires on EVERY page
 * load in the webview, including navigations after the CAPTCHA.
 *
 *   Phase 1 (initial search form): fill court type, keyword/party,
 *     year, and the #escr_details section if SCR.
 *   Phase 2 (post-CAPTCHA results / detail page): detect that the
 *     initial search form is gone, then fill SCR citation fields
 *     (year, volume, page, neutral citation) that appear on the
 *     deeper search/detail form.
 *
 * Defensive design:
 *  - Tries multiple common selectors per field — eCourts varies.
 *  - Never throws — if no fields match, just no-ops.
 *  - Shows a green banner confirming what was pre-filled.
 *  - Uses MutationObserver to catch late-rendered fields.
 */
function buildECourtsPrefillScript(
  title: string,
  year?: number,
  isSC?: boolean,
  citation?: ReturnType<typeof parseCitationDetails>,
): string {
  const escapedTitle = JSON.stringify(title);
  const escapedYear = year ? String(year) : "null";
  const courtValue = isSC ? "3" : "2";
  const citJSON = JSON.stringify(citation ?? {});
  return `
    (function() {
      try {
        var MIKE_TITLE = ${escapedTitle};
        var MIKE_YEAR = ${escapedYear};
        var MIKE_COURT = "${courtValue}";
        var MIKE_CIT = ${citJSON};

        function setValue(selector, value) {
          var el = document.querySelector(selector);
          if (el && 'value' in el) {
            el.value = value;
            el.dispatchEvent(new Event('input', { bubbles: true }));
            el.dispatchEvent(new Event('change', { bubbles: true }));
            return true;
          }
          return false;
        }

        function setSelect(selector, value) {
          var el = document.querySelector(selector);
          if (!el) return false;
          el.value = value;
          if (typeof $ !== 'undefined') {
            try { $(selector).val(value).trigger('chosen:updated').trigger('change'); } catch(_){}
          } else {
            el.dispatchEvent(new Event('change', { bubbles: true }));
          }
          return true;
        }

        // ----- SCR citation field filler (works for both phases) -----
        // Targets both the eCourts #escr_details panel AND the standalone
        // scr.sci.gov.in form (which uses placeholder-based inputs:
        // "Year", "Vol", "Supl", "Page", "Enter Year", "Enter Number").
        function fillSCRFields(filled) {
          var citYear = (MIKE_CIT.citYear || MIKE_YEAR || null);
          var citVol  = (MIKE_CIT.citVolume || null);
          var citPage = (MIKE_CIT.citPage || null);
          var neuYear = (MIKE_CIT.neuYear || null);
          var neuNum  = (MIKE_CIT.neuNumber || null);

          // SCR Year
          if (citYear) {
            var yearSels = [
              'input[placeholder="Year"]',
              '#citation_year', '#escr_citation_year',
              'select[name="citation_year"]', 'input[name="citation_year"]',
              '#rpt_citation_year'];
            for (var i = 0; i < yearSels.length; i++) {
              if (setSelect(yearSels[i], String(citYear)) || setValue(yearSels[i], String(citYear))) {
                filled.push(yearSels[i]); break;
              }
            }
          }
          // SCR Volume
          if (citVol) {
            var volSels = [
              'input[placeholder="Vol"]',
              '#citation_volume', '#escr_citation_volume',
              'select[name="citation_volume"]', 'input[name="citation_volume"]',
              '#volume', 'input[name="volume"]', '#rpt_citation_volume'];
            for (var i = 0; i < volSels.length; i++) {
              if (setSelect(volSels[i], String(citVol)) || setValue(volSels[i], String(citVol))) {
                filled.push(volSels[i]); break;
              }
            }
          }
          // SCR Page
          if (citPage) {
            var pgSels = [
              'input[placeholder="Page"]',
              '#citation_page', '#escr_citation_page',
              'input[name="citation_page"]', 'input[name="start_page"]',
              '#start_page', '#page_no', 'input[name="page_no"]',
              '#rpt_citation_page'];
            for (var i = 0; i < pgSels.length; i++) {
              if (setValue(pgSels[i], String(citPage))) { filled.push(pgSels[i]); break; }
            }
          }
          // Neutral Citation Year
          if (neuYear) {
            var neuYearSels = [
              'input[placeholder="Enter Year"]',
              '#neu_citation_year', '#escr_neu_citation_year',
              'select[name="neu_citation_year"]', 'input[name="neu_citation_year"]'];
            for (var i = 0; i < neuYearSels.length; i++) {
              if (setSelect(neuYearSels[i], String(neuYear)) || setValue(neuYearSels[i], String(neuYear))) {
                filled.push(neuYearSels[i]); break;
              }
            }
          }
          // Neutral Citation Number
          if (neuNum) {
            var neuNumSels = [
              'input[placeholder="Enter Number"]',
              '#neu_citation_no', '#escr_neu_citation_no',
              'input[name="neu_citation_no"]', '#neu_citation_number',
              'input[name="neu_citation_number"]'];
            for (var i = 0; i < neuNumSels.length; i++) {
              if (setValue(neuNumSels[i], String(neuNum))) { filled.push(neuNumSels[i]); break; }
            }
          }
          // Keyword on the SCR form (uses same "Enter Keyword" placeholder)
          var scrNameSels = [
            'input[placeholder="Enter Keyword"]',
            'input[placeholder="Enter keyword"]',
            '#pet_res_name', '#party_name_escr',
            '#petitioner_respondent', 'input[name="pet_res_name"]',
            'input[name="party_name_escr"]'];
          for (var i = 0; i < scrNameSels.length; i++) {
            if (setValue(scrNameSels[i], MIKE_TITLE)) { filled.push(scrNameSels[i]); break; }
          }
        }

        // ----- Phase 1: initial search form -----
        function runPhase1() {
          var filled = [];
          var courtEl = document.querySelector('#fcourt_type');
          if (courtEl) {
            setSelect('#fcourt_type', MIKE_COURT);
            filled.push('#fcourt_type');
          }
          var titleSelectors = [
            '#search_text',
            'input[placeholder="Enter Keyword"]',
            'input[placeholder="Enter keyword"]',
            'input[name="keyword"]',
            'input[name="free_text"]',
            'input[name="search_keyword"]',
            'input[name="party_name"]',
            'input[name="petitioner_name"]',
            'input[name="petitioner"]',
            'input[name="respondent"]',
            '#party_name', '#petitioner_name', '#petitioner',
            '#keyword', '#free_text',
          ];
          for (var i = 0; i < titleSelectors.length; i++) {
            if (setValue(titleSelectors[i], MIKE_TITLE)) { filled.push(titleSelectors[i]); break; }
          }
          if (MIKE_YEAR) {
            var yearSelectors = [
              'select[name="year"]', 'input[name="year"]',
              'select[name="dec_year"]', 'input[name="dec_year"]',
              '#year', '#dec_year',
            ];
            for (var j = 0; j < yearSelectors.length; j++) {
              if (setValue(yearSelectors[j], String(MIKE_YEAR))) { filled.push(yearSelectors[j]); break; }
            }
          }
          if (MIKE_COURT === '3') { fillSCRFields(filled); }
          return filled;
        }

        // ----- Phase 2: post-CAPTCHA / SCR portal page -----
        // The SCR search lives at scr.sci.gov.in — a separate site from
        // the initial eCourts form at judgments.ecourts.gov.in. When the
        // webview navigates there (redirect or user click), the
        // initialization_script fires again and this detects it.
        function isPostCaptchaPage() {
          var url = window.location.href.toLowerCase();
          // scr.sci.gov.in is the dedicated SCR portal
          if (url.indexOf('scr.sci.gov.in') !== -1) return true;
          if (url.indexOf('escr_flag') !== -1) return true;
          if (url.indexOf('search_result') !== -1) return true;
          // DOM: SCR-specific placeholder-based fields
          var hasSCRField = !!(
            document.querySelector('input[placeholder="Year"]') ||
            document.querySelector('input[placeholder="Vol"]') ||
            document.querySelector('input[placeholder="Page"]') ||
            document.querySelector('#citation_volume') ||
            document.querySelector('#citation_page') ||
            document.querySelector('[name="citation_volume"]')
          );
          // On scr.sci.gov.in the page has its own CAPTCHA, but the
          // #fcourt_type select (from the initial eCourts form) is absent.
          var hasInitialForm = !!document.querySelector('#fcourt_type');
          if (hasSCRField && !hasInitialForm) return true;
          if (document.querySelector('.result_table, .case_result, #result_table, table.display')) return true;
          return false;
        }

        function runPhase2() {
          if (MIKE_COURT !== '3') return [];
          var filled = [];
          fillSCRFields(filled);
          return filled;
        }

        function showBanner(filled, phase) {
          var existing = document.getElementById('mike-ecourts-banner');
          if (existing) existing.remove();
          var banner = document.createElement('div');
          banner.id = 'mike-ecourts-banner';
          banner.style.cssText = 'position:fixed;top:0;left:0;right:0;z-index:2147483647;background:#d1fae5;color:#065f46;padding:10px 16px;font-family:-apple-system,BlinkMacSystemFont,sans-serif;font-size:13px;border-bottom:1px solid #10b981;box-shadow:0 2px 6px rgba(0,0,0,0.08);';
          var courtLabel = MIKE_COURT === '3' ? 'Supreme Court (SCR)' : 'High Court';
          if (phase === 2 && filled.length > 0) {
            banner.textContent = '✓ Mike pre-filled SCR citation details (' + filled.length + ' fields). Review and click Search.';
          } else if (filled.length > 0) {
            banner.textContent = '✓ Mike pre-filled the form (' + courtLabel + '). Solve the CAPTCHA and click Search.';
          } else {
            banner.textContent = '⚠ Could not auto-fill. Search for: ' + MIKE_TITLE;
          }
          document.body.appendChild(banner);
          setTimeout(function () { banner.remove(); }, 20000);
        }

        function run() {
          var filled;
          if (isPostCaptchaPage()) {
            filled = runPhase2();
            showBanner(filled, 2);
          } else {
            filled = runPhase1();
            showBanner(filled, 1);
          }
        }

        if (document.readyState === 'loading') {
          document.addEventListener('DOMContentLoaded', run);
        } else {
          run();
        }
        setTimeout(run, 2000);
        setTimeout(run, 4000);

        // MutationObserver: catch late-rendered SCR fields.
        if (MIKE_COURT === '3') {
          var observed = false;
          var mo = new MutationObserver(function() {
            if (observed) return;
            var hasNew = !!(
              document.querySelector('input[placeholder="Vol"]') ||
              document.querySelector('input[placeholder="Page"]') ||
              document.querySelector('#citation_volume') ||
              document.querySelector('#citation_page') ||
              document.querySelector('[name="citation_volume"]') ||
              document.querySelector('#pet_res_name')
            );
            if (hasNew) {
              observed = true;
              mo.disconnect();
              var filled = [];
              fillSCRFields(filled);
              if (filled.length > 0) showBanner(filled, 2);
            }
          });
          mo.observe(document.documentElement, { childList: true, subtree: true });
          setTimeout(function() { mo.disconnect(); }, 30000);
        }
      } catch (e) {
        console.warn('[Mike] eCourts pre-fill error:', e);
      }
    })();
  `;
}

/**
 * Open eCourts for verification with best-available pre-fill UX:
 *   1. Copy case title to clipboard immediately — always works, gives
 *      the user a one-paste fallback even if everything else fails.
 *   2. Try opening eCourts in a Tauri-controlled WebviewWindow so we
 *      can run the pre-fill script. If that fails (Tauri webview API
 *      unavailable, window-creation blocked, etc.), fall through.
 *   3. Fall back to the system default browser via the existing
 *      open_external_url Tauri command, where the user pastes the
 *      clipboard contents manually.
 */
async function openECourtsForVerification(title: string, year?: number, court?: string, tid?: number) {
  // 1. Clipboard pre-fill — always.
  try {
    await navigator.clipboard.writeText(title);
  } catch {
    // Non-fatal.
  }

  // 1b. If court is unknown, try fetching it from Kanoon metadata.
  let resolvedCourt = court;
  if (!resolvedCourt && tid) {
    const fetched = await fetchKanoonCourt(tid);
    if (fetched) resolvedCourt = fetched;
  }

  // 2. Try the Rust-side Tauri command for full webview + JS injection.
  try {
    const tauri = await import("@tauri-apps/api/core");
    const isSC = isSupremeCourt(title, resolvedCourt);
    const citation = parseCitationDetails(title);
    const script = buildECourtsPrefillScript(title, year, isSC, citation);
    // For SC cases, go directly to the SCR portal — skips the eCourts
    // intermediate step and lands on the form with Year/Vol/Page fields.
    const targetUrl = isSC ? SCR_URL : ECOURTS_URL;
    const windowLabel = isSC ? "Verify on SCR" : "Verify on eCourts";
    await tauri.invoke("open_ecourts_verify_window", {
      url: targetUrl,
      windowTitle: `${windowLabel} — ${title.slice(0, 60)}`,
      initScript: script,
    });
    return;
  } catch (e) {
    console.warn(
      "[Mike] open_ecourts_verify_window invoke failed, falling back to default browser:",
      e,
    );
  }
  // 3. Default browser fallback. User pastes the clipboard contents.
  const isSCFallback = isSupremeCourt(title, resolvedCourt);
  await openExternalUrl(isSCFallback ? SCR_URL : ECOURTS_URL);
}

export default function KanoonVerifyBadge({
  tid,
  title,
  court,
  decisionDate,
}: {
  tid: number;
  title: string;
  court?: string;
  decisionDate?: string;
}) {
  const [status, setStatus] = useState<Status>("loading");
  const [verification, setVerification] = useState<Verification | null>(null);
  const [showForm, setShowForm] = useState(false);
  const [caseNumber, setCaseNumber] = useState("");
  const [pdfUrl, setPdfUrl] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Fetch existing verification on mount.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const r = await fetch(`${API_BASE}/ecourts-verify/${tid}`, { credentials: "include" });
        if (cancelled) return;
        if (!r.ok) {
          setStatus("none");
          return;
        }
        const data = await r.json();
        if (data?.verification) {
          setVerification(data.verification);
          setStatus(
            data.verification.status === "verified" ? "verified" : "not_found",
          );
        } else {
          setStatus("none");
        }
      } catch {
        if (!cancelled) setStatus("none");
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [tid]);

  async function record(outcome: "verified" | "not_found") {
    setSaving(true);
    setError(null);
    try {
      const r = await fetch(`${API_BASE}/ecourts-verify`, {
        method: "POST",
        credentials: "include",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          kanoon_tid: tid,
          kanoon_title: title,
          kanoon_court: court ?? null,
          kanoon_decision_date: decisionDate ?? null,
          status: outcome,
          ecourts_case_number: outcome === "verified" ? caseNumber.trim() : null,
          ecourts_pdf_url:
            outcome === "verified" && pdfUrl.trim() ? pdfUrl.trim() : null,
        }),
      });
      if (!r.ok) {
        const body = await r.json().catch(() => ({}));
        throw new Error(body?.detail ?? `HTTP ${r.status}`);
      }
      const data = await r.json();
      setVerification({
        id: data.id,
        kanoon_tid: tid,
        status: outcome,
        ecourts_case_number: data.ecourts_case_number ?? null,
        ecourts_pdf_url: pdfUrl.trim() || null,
        verified_at: new Date().toISOString(),
      });
      setStatus(outcome);
      setShowForm(false);
      setCaseNumber("");
      setPdfUrl("");
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to record verification");
    } finally {
      setSaving(false);
    }
  }

  async function reset() {
    if (saving) return;
    setSaving(true);
    try {
      await fetch(`${API_BASE}/ecourts-verify/${tid}`, {
        method: "DELETE",
        credentials: "include",
      });
      setVerification(null);
      setStatus("none");
    } finally {
      setSaving(false);
    }
  }

  // ─── Rendered states ─────────────────────────────────────────────────────

  if (status === "loading") {
    return (
      <span className="inline-flex items-center ml-1.5 text-xs text-gray-400">
        …
      </span>
    );
  }

  if (status === "verified" && verification) {
    return (
      <span
        className="inline-flex items-center gap-1 ml-1.5 px-1.5 py-0.5 rounded text-xs bg-green-50 text-green-700 border border-green-200"
        title={`Verified on eCourts as ${verification.ecourts_case_number}`}
      >
        <CheckCircle2 className="h-3 w-3" />
        <span className="font-medium">
          {verification.ecourts_case_number ?? "Verified"}
        </span>
        {verification.ecourts_pdf_url && (
          <a
            href={verification.ecourts_pdf_url}
            onClick={(e) => {
              e.preventDefault();
              openExternalUrl(verification.ecourts_pdf_url!);
            }}
            className="hover:underline"
            title="Open canonical eCourts PDF"
          >
            <ExternalLink className="h-3 w-3" />
          </a>
        )}
        <button
          onClick={reset}
          disabled={saving}
          className="ml-0.5 text-green-600 hover:text-green-800 opacity-60 hover:opacity-100"
          title="Clear verification"
        >
          <X className="h-3 w-3" />
        </button>
      </span>
    );
  }

  if (status === "not_found") {
    return (
      <span
        className="inline-flex items-center gap-1 ml-1.5 px-1.5 py-0.5 rounded text-xs bg-amber-50 text-amber-800 border border-amber-200"
        title="Searched eCourts; case not found there. Citation kept as Kanoon-only."
      >
        Not on eCourts
        <button
          onClick={reset}
          disabled={saving}
          className="ml-0.5 text-amber-700 hover:text-amber-900 opacity-60 hover:opacity-100"
          title="Clear and re-verify"
        >
          <X className="h-3 w-3" />
        </button>
      </span>
    );
  }

  // status === "none" — show the Verify button + (optionally) an inline form.
  // Year is derived from decisionDate if it looks like a 4-digit year.
  const yearFromDate = (() => {
    if (!decisionDate) return undefined;
    const m = decisionDate.match(/(19|20)\d{2}/);
    return m ? parseInt(m[0], 10) : undefined;
  })();
  return (
    <span className="inline-flex flex-col items-start ml-1.5 align-baseline">
      <span className="inline-flex items-center gap-1">
        <button
          onClick={() => {
            // Best-effort: opens eCourts (Tauri webview if available, else
            // default browser), copies the case title to the clipboard,
            // and tries to JS-inject form pre-fill so the user only has
            // to solve the CAPTCHA + click Search.
            openECourtsForVerification(title, yearFromDate, court, tid);
            setShowForm(true);
          }}
          className="px-1.5 py-0.5 rounded text-xs bg-gray-50 text-gray-700 border border-gray-200 hover:bg-gray-100"
          title="Opens eCourts portal (case title copied to clipboard). Solve the CAPTCHA, find the case, then paste the case number here."
        >
          🔍 Verify on eCourts
        </button>
      </span>
      {showForm && (
        <span className="mt-1.5 mb-1 flex flex-col gap-1.5 rounded border border-gray-200 bg-gray-50 p-2 text-xs">
          <span className="text-gray-600">
            Paste the eCourts case number you found:
          </span>
          <input
            type="text"
            value={caseNumber}
            onChange={(e) => setCaseNumber(e.target.value)}
            placeholder="e.g. CRL.A. 1124/2020"
            className="px-2 py-1 text-xs border border-gray-300 rounded focus:outline-none focus:ring-1 focus:ring-blue-400"
            autoFocus
          />
          <input
            type="text"
            value={pdfUrl}
            onChange={(e) => setPdfUrl(e.target.value)}
            placeholder="(optional) eCourts PDF URL"
            className="px-2 py-1 text-xs border border-gray-300 rounded focus:outline-none focus:ring-1 focus:ring-blue-400"
          />
          <span className="flex items-center gap-1">
            <button
              onClick={() => record("verified")}
              disabled={saving || !caseNumber.trim()}
              className="px-2 py-1 rounded text-xs bg-green-600 text-white hover:bg-green-700 disabled:opacity-50"
            >
              {saving ? "Saving…" : "Mark verified"}
            </button>
            <button
              onClick={() => record("not_found")}
              disabled={saving}
              className="px-2 py-1 rounded text-xs bg-amber-100 text-amber-800 hover:bg-amber-200 disabled:opacity-50"
            >
              Not found
            </button>
            <button
              onClick={() => {
                setShowForm(false);
                setCaseNumber("");
                setPdfUrl("");
                setError(null);
              }}
              disabled={saving}
              className="px-2 py-1 rounded text-xs text-gray-600 hover:text-gray-800"
            >
              Cancel
            </button>
          </span>
          {error && <span className="text-red-600">{error}</span>}
        </span>
      )}
    </span>
  );
}

/**
 * Extract the Kanoon document id (tid) from a kanoon URL.
 * URL shape: https://indiankanoon.org/doc/{tid}/
 * Returns null on any URL that doesn't match the expected pattern.
 */
export function extractKanoonTid(href: string | undefined): number | null {
  if (!href) return null;
  const m = href.match(/indiankanoon\.org\/doc\/(\d+)/);
  if (!m) return null;
  const n = parseInt(m[1], 10);
  return Number.isFinite(n) && n > 0 ? n : null;
}
