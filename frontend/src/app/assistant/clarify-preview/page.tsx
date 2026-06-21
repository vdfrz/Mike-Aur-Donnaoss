"use client";

// TEMPORARY preview — open http://localhost:4000/assistant/clarify-preview to
// see the redesigned inline clarification card with mock data, without waiting
// for the agent to call ask_clarifying_questions. Safe to delete this folder.

import InlineClarification from "../../components/assistant/InlineClarification";
import { MikeIcon } from "@/components/chat/mike-icon";
import type { AssistantEvent } from "../../components/shared/types";

const mockEvent = {
  type: "clarification",
  request_id: "preview-only",
  questions: [
    {
      header: "Jurisdiction",
      text: "Which forum is this petition being filed in?",
      multiSelect: false,
      options: [
        { label: "Sessions Court", description: "Regular bail u/s 439 CrPC — trial court." },
        { label: "High Court", description: "Writ or appellate jurisdiction — Art. 226 / 227." },
        { label: "Tribunal", description: "AFT, ITAT, consumer forum, MACT." },
      ],
    },
    {
      header: "Relief sought",
      text: "What should the draft request?",
      multiSelect: true,
      options: [
        { label: "Release on bail", description: "Primary prayer — enlarge the applicant on bail." },
        { label: "Interim relief", description: "Interim bail pending the final hearing." },
      ],
      chips: ["Own surety", "Personal bond", "Passport surrender"],
    },
  ],
} as Extract<AssistantEvent, { type: "clarification" }>;

export default function ClarifyPreviewPage() {
  return (
    <div className="mx-auto max-w-3xl px-6 py-12">
      <p className="mb-6 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
        Preview — redesigned inline clarification (mock data)
      </p>
      <div className="flex items-start gap-3">
        <div className="flex h-[30px] w-[30px] shrink-0 items-center justify-center rounded-lg bg-[#0f172a]">
          <MikeIcon size={19} />
        </div>
        <div className="min-w-0 flex-1">
          <p className="text-xs font-semibold text-muted-foreground mb-1">Mike</p>
          <p className="font-serif text-[17px] leading-relaxed text-foreground">
            Happy to. Two quick things first, so the petition is filed in the right
            place and asks for the right relief.
          </p>
          <InlineClarification event={mockEvent} />
        </div>
      </div>
    </div>
  );
}
