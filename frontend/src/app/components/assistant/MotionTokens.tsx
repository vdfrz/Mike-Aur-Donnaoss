"use client";

/**
 * Shared motion/animation tokens for the assistant chat UI.
 * All animations follow the Claude-desktop aesthetic:
 * soft springs, 150–220ms ease-out, subtle blur/elevation, gentle stagger.
 * Respects prefers-reduced-motion.
 */

export const MOTION_TOKENS = {
  // Duration (ms)
  FAST: 150,
  NORMAL: 180,
  SLOW: 220,

  // Easing
  EASE_OUT: "ease-out",
  EASE_IN_OUT: "ease-in-out",

  // Stagger (ms between child animations)
  STAGGER_FAST: 30,
  STAGGER_NORMAL: 50,
  STAGGER_SLOW: 80,
};

/**
 * Tool name → human-readable label mapping.
 */
export const TOOL_NAME_MAP: Record<string, string> = {
  statute_search: "Searching statutes",
  kanoon_search: "Researching case law",
  kanoon_get_fragment: "Reading judgments",
  kanoon_verify_case: "Verifying citations",
  generate_docx: "Drafting document",
  edit_document: "Revising document",
  read_document: "Reviewing draft",
  find_in_document: "Scanning document",
  ask_clarifying_questions: "Preparing a question",
  read_workflow: "Loading workflow",
  vanga_search: "Searching",
};

export function getToolLabel(toolName: string): string {
  return TOOL_NAME_MAP[toolName] || "Working";
}

/**
 * Global keyframe definitions (injected once, reused everywhere).
 */
export const MOTION_KEYFRAMES = `
@keyframes fadeIn {
  from { opacity: 0; }
  to { opacity: 1; }
}

@keyframes slideInUp {
  from { opacity: 0; transform: translateY(8px); }
  to { opacity: 1; transform: none; }
}

@keyframes popIn {
  from { opacity: 0; transform: translateY(6px) scale(.985); }
  to { opacity: 1; transform: none; }
}

@keyframes shimmer {
  0% { background-position: -1000px 0; }
  100% { background-position: 1000px 0; }
}

@keyframes pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.6; }
}

@media (prefers-reduced-motion: reduce) {
  @keyframes fadeIn,
  @keyframes slideInUp,
  @keyframes popIn,
  @keyframes shimmer,
  @keyframes pulse {
    from { animation-duration: 0s !important; }
    to { animation-duration: 0s !important; }
  }
}
`;

