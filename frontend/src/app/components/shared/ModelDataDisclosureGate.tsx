"use client";

/**
 * ModelDataDisclosureGate - a BLOCKING acknowledgment gate for the on-device
 * fine-tuned model (mike-legal). The model's weights are open and were trained
 * on legal documents; structured identifiers were redacted, but some names may
 * have survived and the model also fabricates realistic-looking PII. Users must
 * read and explicitly acknowledge this before they can use the model - there is
 * no dismiss; the only ways out are "I understand" (after ticking the box) or
 * switching to a cloud model.
 *
 * Wiring (assistant shell): render when the on-device model is selected and not
 * yet acknowledged, e.g.
 *   {selectedModel === "local:mike-legal" && !disclosureAck && (
 *     <ModelDataDisclosureGate
 *       onAcknowledge={() => setDisclosureAck(true)}
 *       onUseCloud={() => setSelectedModel("local:deepseek-v4-flash")}
 *     />
 *   )}
 * Acknowledgment persists in localStorage (versioned) so it shows once per
 * disclosure revision. Bump ACK_KEY when the copy materially changes.
 */

import { useEffect, useState } from "react";
import { MikeIcon } from "@/components/chat/mike-icon";

const ACK_KEY = "mike_legal_disclosure_ack_v1";
// Takedown / data-rights contact.
const CONTACT = "vedantopensource@gmail.com";

export function hasAcknowledgedModelDisclosure(): boolean {
    if (typeof window === "undefined") return false;
    return window.localStorage.getItem(ACK_KEY) === "1";
}

export default function ModelDataDisclosureGate({
    onAcknowledge,
    onUseCloud,
}: {
    onAcknowledge: () => void;
    onUseCloud?: () => void;
}) {
    const [checked, setChecked] = useState(false);

    // Lock background scroll while the gate is open.
    useEffect(() => {
        const prev = document.body.style.overflow;
        document.body.style.overflow = "hidden";
        return () => {
            document.body.style.overflow = prev;
        };
    }, []);

    function accept() {
        if (!checked) return;
        try {
            window.localStorage.setItem(ACK_KEY, "1");
        } catch {
            /* storage may be unavailable; still let them proceed this session */
        }
        onAcknowledge();
    }

    return (
        <div
            role="dialog"
            aria-modal="true"
            aria-labelledby="model-disclosure-title"
            className="fixed inset-0 z-50 flex items-center justify-center bg-[rgba(15,23,42,0.45)] p-4 backdrop-blur-sm"
        >
            <div className="w-full max-w-lg animate-[pop_0.18s_ease-out] overflow-hidden rounded-[10px] border border-border bg-background text-foreground shadow-[0_10px_40px_-12px_rgba(15,23,42,0.35)]">
                {/* Header */}
                <div className="flex items-start gap-3 border-b border-border px-6 pt-5 pb-4">
                    <span className="mt-0.5 shrink-0">
                        <MikeIcon size={34} />
                    </span>
                    <div>
                        <h2
                            id="model-disclosure-title"
                            className="text-base font-semibold tracking-tight text-foreground"
                        >
                            Before you use{" "}
                            <span style={{ fontFamily: "var(--font-eb-garamond), 'EB Garamond', serif" }}>
                                mike-legal
                            </span>
                        </h2>
                        <p className="mt-1 text-xs text-muted-foreground">
                            An experimental, on-device fine-tuned model. Please read this notice
                            about its training data and your privacy.
                        </p>
                    </div>
                </div>

                {/* Body */}
                <div className="max-h-[52vh] overflow-y-auto px-6 py-4 text-[13px] leading-relaxed text-muted-foreground">
                    <ul className="space-y-3">
                        <li>
                            <span className="font-medium text-foreground">What it was trained on.</span>{" "}
                            mike-legal was fine-tuned on Indian legal material (court orders,
                            judgments and related filings drawn from publicly available and
                            firm sources) to help draft and explain legal documents.
                        </li>
                        <li>
                            <span className="font-medium text-foreground">What we removed.</span>{" "}
                            Before training we ran automated redaction over the source
                            material to strip structured personal identifiers such as Aadhaar
                            numbers, PAN, phone numbers, email addresses and bank-account
                            numbers. In our testing none of these were reproducible from the
                            model.
                        </li>
                        <li>
                            <span className="font-medium text-foreground">What may remain.</span>{" "}
                            Automated name detection is imperfect. Some personal{" "}
                            <span className="font-medium text-foreground">names</span> that
                            appeared in the source documents may have survived into the
                            training data and could, in rare cases, appear in the model&rsquo;s
                            output. Identifiers linked to those names (Aadhaar, PAN, address,
                            phone) were removed.
                        </li>
                        <li>
                            <span className="font-medium text-foreground">The model also invents data.</span>{" "}
                            mike-legal frequently fabricates realistic-looking names, numbers
                            and addresses that have no connection to any real person or to its
                            training data. Do not treat any name, number or address it outputs
                            as real, accurate, or belonging to an actual individual.
                        </li>
                        <li>
                            <span className="font-medium text-foreground">Not legal advice.</span>{" "}
                            Output is machine-generated text, not legal advice, and can be
                            wrong. Verify everything against primary sources before any use.
                        </li>
                        <li>
                            <span className="font-medium text-foreground">Open weights.</span>{" "}
                            These weights are openly distributed; once downloaded they run
                            outside this application and without its safeguards.
                        </li>
                    </ul>

                    {/* Removal / contact - placeholder-amber callout */}
                    <p
                        className="mt-4 rounded-[8px] px-3 py-2 text-[12.5px]"
                        style={{ background: "#fdeede", color: "#9a4a00" }}
                    >
                        If you believe the model has produced personal information relating to
                        you, contact the publishers at{" "}
                        <span className="font-medium">{CONTACT}</span> and we will review and
                        act on removal requests.
                    </p>
                </div>

                {/* Acknowledgment + actions */}
                <div className="border-t border-border px-6 py-4" style={{ background: "var(--blue-50)" }}>
                    <label className="flex cursor-pointer items-start gap-2.5 text-[13px] text-foreground">
                        <input
                            type="checkbox"
                            checked={checked}
                            onChange={(e) => setChecked(e.target.checked)}
                            className="mt-0.5 h-4 w-4 shrink-0 cursor-pointer rounded-[4px] border border-border accent-[var(--blue)]"
                        />
                        <span>
                            I have read and understand the above, including that the model may
                            output real names and fabricated information, and that its output is
                            not legal advice.
                        </span>
                    </label>

                    <div className="mt-4 flex items-center justify-end gap-3">
                        {onUseCloud && (
                            <button
                                type="button"
                                onClick={onUseCloud}
                                className="rounded-[8px] px-3 py-2 text-[13px] text-muted-foreground transition-colors hover:text-foreground"
                            >
                                Use a cloud model instead
                            </button>
                        )}
                        <button
                            type="button"
                            onClick={accept}
                            disabled={!checked}
                            aria-disabled={!checked}
                            className="rounded-[8px] px-4 py-2 text-[13px] font-medium text-white transition-colors disabled:cursor-not-allowed disabled:opacity-50"
                            style={{ background: "var(--blue)" }}
                            onMouseEnter={(e) => {
                                if (checked) e.currentTarget.style.background = "var(--blue-700)";
                            }}
                            onMouseLeave={(e) => {
                                e.currentTarget.style.background = "var(--blue)";
                            }}
                        >
                            I understand, continue
                        </button>
                    </div>
                </div>
            </div>
        </div>
    );
}
