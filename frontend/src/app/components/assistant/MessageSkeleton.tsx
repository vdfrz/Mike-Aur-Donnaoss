"use client";

import { MOTION_KEYFRAMES } from "./MotionTokens";

/**
 * Shimmer skeleton placeholder for message bubbles while awaiting first token.
 * Matches the motion language of the chat UI.
 */
export function MessageSkeleton() {
  return (
    <>
      <style>{MOTION_KEYFRAMES}</style>
      <div
        className="rounded-lg bg-gradient-to-r from-gray-200 via-gray-100 to-gray-200"
        style={{
          animation: "shimmer 2s linear infinite",
          backgroundSize: "200% 100%",
          backgroundPosition: "0 0",
          height: "64px",
        }}
      />
    </>
  );
}
