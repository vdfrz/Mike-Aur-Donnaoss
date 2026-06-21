"use client";

import { useState } from "react";
import { useTranslations } from "next-intl";
import { MikeIcon } from "@/components/chat/mike-icon";
import type { AssistantEvent } from "../shared/types";

/**
 * Inline clarification ("ask the user a question") — renders in the chat thread
 * as one of Mike's turns. No modal, no backdrop.
 *
 * Design: Mike aur Donna system (.claude/skills/mike-design). The accent is the
 * app's `--blue` token (now monochrome ink, mode-aware), so this matches light
 * and dark automatically. Structure = Concept B "answer card": accent rail, a
 * faint turning aperture motif, options with descriptions, quick-reply chips
 * (the `chips` field), an "Other" free-text fallback, and a Send / Skip footer.
 * Collapses to a compact recap once answered.
 */
export default function InlineClarification({
  event,
}: {
  event: Extract<AssistantEvent, { type: "clarification" }>;
}) {
  const [selectedByQuestion, setSelectedByQuestion] = useState<
    Record<number, string[]>
  >({});
  const [otherByQuestion, setOtherByQuestion] = useState<Record<number, string>>(
    {},
  );
  const [otherOpenByQuestion, setOtherOpenByQuestion] = useState<
    Record<number, boolean>
  >({});
  const [submitted, setSubmitted] = useState(false);
  const [error, setError] = useState(false);
  const tA = useTranslations("Assistant");

  const toggleValue = (qi: number, value: string, multiSelect?: boolean) => {
    setSelectedByQuestion((prev) => {
      const current = prev[qi] ?? [];
      if (multiSelect) {
        const next = current.includes(value)
          ? current.filter((x) => x !== value)
          : [...current, value];
        return { ...prev, [qi]: next };
      }
      return { ...prev, [qi]: [value] };
    });
  };

  const openOther = (qi: number) =>
    setOtherOpenByQuestion((prev) => ({ ...prev, [qi]: !prev[qi] }));

  const picksFor = (qi: number) => {
    const picks = [...(selectedByQuestion[qi] ?? [])];
    const other = (otherByQuestion[qi] ?? "").trim();
    if (other) picks.push(other);
    return picks;
  };

  const submit = async (proceed: boolean) => {
    setError(false);
    const apiBase =
      process.env.NEXT_PUBLIC_API_BASE_URL ?? "http://localhost:3001";
    const token =
      typeof window !== "undefined"
        ? localStorage.getItem("mike_auth_token")
        : null;

    const answers = proceed
      ? []
      : event.questions.map((q, qi) => ({
          question: q.text,
          selected: picksFor(qi),
        }));

    try {
      await fetch(`${apiBase}/chat/client-tool-result`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          ...(token ? { Authorization: `Bearer ${token}` } : {}),
        },
        body: JSON.stringify({
          request_id: event.request_id,
          result: JSON.stringify({ answers, proceed }),
        }),
      });
      setSubmitted(true);
    } catch (err) {
      console.error("[clarification] submit error:", err);
      setError(true);
    }
  };

  const isDisabled = submitted;

  // Answered → compact recap so it reads like a sent reply.
  if (submitted) {
    const anySelected = event.questions.some((_, qi) => picksFor(qi).length > 0);
    return (
      <div className="mt-3 flex items-center gap-2.5 rounded-xl border border-border bg-muted/40 px-3.5 py-2.5">
        <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-blue text-white">
          <svg className="h-3 w-3" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={3}>
            <polyline points="20 6 9 17 4 12" />
          </svg>
        </span>
        <div className="min-w-0 text-sm">
          {anySelected ? (
            <span className="text-muted-foreground">
              {event.questions
                .map((q, qi) => {
                  const sel = picksFor(qi);
                  if (sel.length === 0) return null;
                  return `${q.header ? q.header + ": " : ""}${sel.join(", ")}`;
                })
                .filter(Boolean)
                .join("  ·  ")}
            </span>
          ) : (
            <span className="text-muted-foreground">
              {tA("proceedingWithPlaceholders") ||
                "Proceeding with placeholders for any unknown details."}
            </span>
          )}
        </div>
      </div>
    );
  }

  return (
    <div className="relative mt-3 overflow-hidden rounded-2xl border border-blue-200 bg-blue-50/60 shadow-sm animate-in fade-in slide-in-from-bottom-1 duration-300">
      <style>{`@keyframes mikeAperture{to{transform:rotate(360deg)}}`}</style>
      {/* accent rail */}
      <span className="absolute inset-y-0 left-0 w-[3px] bg-blue" />
      {/* faint turning aperture motif */}
      <div
        aria-hidden
        className="pointer-events-none absolute -right-6 -top-5 opacity-[0.05]"
        style={{ animation: "mikeAperture 90s linear infinite", transformOrigin: "50% 50%" }}
      >
        <MikeIcon size={128} />
      </div>

      <div className="relative p-4 pl-5">
        {/* header */}
        <div className="mb-3.5 flex items-center gap-2.5">
          <div className="flex h-6 w-6 shrink-0 items-center justify-center rounded-lg bg-[#0f172a]">
            <MikeIcon size={15} />
          </div>
          <span className="text-[13.5px] font-semibold text-foreground">
            {tA("clarifyingQuestions") || "A quick check before I draft"}
          </span>
          <span className="ml-auto rounded-full border border-blue-200 bg-blue-100 px-2.5 py-0.5 text-[11px] font-semibold text-blue-700">
            {event.questions.length === 1
              ? "1 question"
              : `${event.questions.length} questions`}
          </span>
        </div>

        {/* questions */}
        {event.questions.map((q, qi) => {
          const isMulti = !!q.multiSelect;
          const selected = selectedByQuestion[qi] ?? [];
          const otherOpen = !!otherOpenByQuestion[qi];
          return (
            <div key={qi} className="mb-4 last:mb-1">
              <div className="mb-2.5 flex items-center gap-2">
                {q.header && (
                  <span className="rounded-md bg-blue-100 px-2 py-0.5 text-[10px] font-bold uppercase tracking-wide text-blue-700">
                    {q.header}
                  </span>
                )}
                <span className="text-[11px] text-muted-foreground">
                  {isMulti
                    ? tA("selectAllThatApply") || "select all that apply"
                    : tA("pickOne") || "pick one"}
                </span>
              </div>

              <p className="mb-2.5 text-sm font-semibold text-foreground">
                {q.text}
              </p>

              {q.options && q.options.length > 0 && (
                <div className="flex flex-col gap-2">
                  {q.options.map((opt) => {
                    const isSelected = selected.includes(opt.label);
                    return (
                      <button
                        key={opt.label}
                        type="button"
                        disabled={isDisabled}
                        onClick={() => toggleValue(qi, opt.label, isMulti)}
                        className={`group flex items-start gap-3 rounded-xl border px-3 py-2.5 text-left transition-all ${
                          isSelected
                            ? "border-blue bg-card shadow-[0_0_0_3px_var(--color-blue-100)]"
                            : "border-border bg-card hover:-translate-y-px hover:border-blue-200 hover:shadow-sm"
                        } ${isDisabled ? "cursor-not-allowed opacity-50" : "cursor-pointer"}`}
                      >
                        <Control selected={isSelected} multi={isMulti} />
                        <span className="min-w-0">
                          <span className="block text-[13.5px] font-semibold text-foreground">
                            {opt.label}
                          </span>
                          {opt.description && (
                            <span className="mt-0.5 block text-xs leading-snug text-muted-foreground">
                              {opt.description}
                            </span>
                          )}
                        </span>
                      </button>
                    );
                  })}
                </div>
              )}

              {/* quick-reply chips */}
              {q.chips && q.chips.length > 0 && (
                <div className="mt-2.5 flex flex-wrap gap-2">
                  {q.chips.map((chip) => {
                    const isSelected = selected.includes(chip);
                    return (
                      <button
                        key={chip}
                        type="button"
                        disabled={isDisabled}
                        onClick={() => toggleValue(qi, chip, isMulti)}
                        className={`rounded-full border px-3 py-1.5 text-[12.5px] font-medium transition-all ${
                          isSelected
                            ? "border-blue bg-blue-100 text-blue-700"
                            : "border-border bg-card text-foreground hover:-translate-y-px hover:border-blue-200 hover:text-blue-700"
                        } ${isDisabled ? "cursor-not-allowed opacity-50" : "cursor-pointer"}`}
                      >
                        {chip}
                      </button>
                    );
                  })}
                </div>
              )}

              {/* Other free-text fallback */}
              <div className="mt-2.5">
                <button
                  type="button"
                  disabled={isDisabled}
                  onClick={() => openOther(qi)}
                  className={`flex items-center gap-2 rounded-xl border border-dashed px-3 py-1.5 text-[12.5px] font-semibold transition-colors ${
                    otherOpen
                      ? "border-blue bg-blue-50 text-blue-700"
                      : "border-blue-200 text-blue-700 hover:bg-blue-50"
                  } ${isDisabled ? "cursor-not-allowed opacity-50" : "cursor-pointer"}`}
                >
                  <svg className="h-3.5 w-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2.2} strokeLinecap="round">
                    <path d="M12 5v14M5 12h14" />
                  </svg>
                  {tA("other") || "Other"}
                </button>
                {otherOpen && (
                  <input
                    autoFocus
                    disabled={isDisabled}
                    value={otherByQuestion[qi] ?? ""}
                    onChange={(e) =>
                      setOtherByQuestion((prev) => ({
                        ...prev,
                        [qi]: e.target.value,
                      }))
                    }
                    placeholder={tA("typeYourAnswer") || "Type the specific answer…"}
                    className="mt-2 w-full rounded-[10px] border border-blue-200 bg-background px-3 py-2 text-[13px] text-foreground shadow-[0_0_0_3px_var(--color-blue-100)] outline-none placeholder:text-muted-foreground"
                  />
                )}
              </div>
            </div>
          );
        })}

        {/* footer */}
        <div className="mt-4 flex items-center gap-3 border-t border-blue-200 pt-3.5">
          <button
            type="button"
            disabled={isDisabled}
            onClick={() => submit(false)}
            className="inline-flex items-center gap-2 rounded-[10px] bg-blue px-4 py-2 text-[13px] font-semibold text-white shadow-sm transition-all hover:-translate-y-px hover:bg-blue-700 disabled:cursor-not-allowed disabled:opacity-50"
          >
            {tA("submit") || "Send answers"}
            <svg className="h-3.5 w-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2.2} strokeLinecap="round" strokeLinejoin="round">
              <line x1="5" y1="12" x2="19" y2="12" />
              <polyline points="12 5 19 12 12 19" />
            </svg>
          </button>
          <span className="flex-1" />
          <button
            type="button"
            disabled={isDisabled}
            onClick={() => submit(true)}
            className="rounded-[10px] px-3 py-2 text-[13px] font-semibold text-muted-foreground transition-colors hover:text-blue-700 disabled:cursor-not-allowed disabled:opacity-50"
          >
            {tA("draftNow") || "Skip — draft now"}
          </button>
        </div>

        {error && (
          <p className="mt-2 text-xs text-destructive">
            {tA("couldntSubmit") || "Couldn't submit — please try again."}
          </p>
        )}
      </div>
    </div>
  );
}

/** Monochrome radio (single) / checkbox (multi) control. */
function Control({ selected, multi }: { selected: boolean; multi: boolean }) {
  return (
    <span
      className={`mt-px flex h-[19px] w-[19px] shrink-0 items-center justify-center border transition-all ${
        multi ? "rounded-md" : "rounded-full"
      } ${selected ? "border-blue bg-blue" : "border-input bg-transparent"}`}
    >
      {selected &&
        (multi ? (
          <svg className="h-3 w-3 text-white" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={3.5} strokeLinecap="round" strokeLinejoin="round">
            <polyline points="20 6 9 17 4 12" />
          </svg>
        ) : (
          <span className="h-[7px] w-[7px] rounded-full bg-white" />
        ))}
    </span>
  );
}
