"use client";

/**
 * Deprecated. The clarification ("ask the user a question") UI is now rendered
 * INLINE in the chat thread — see `InlineClarification.tsx`, used by
 * `AssistantMessage`. The old centered modal was retired.
 *
 * This thin re-export remains only so any stale import keeps compiling.
 * It is safe to delete this file once you've confirmed nothing imports it.
 */
export { default } from "./InlineClarification";
