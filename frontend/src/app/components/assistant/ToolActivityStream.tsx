"use client";

import { useEffect, useState, useRef } from "react";
import {
  ChevronDown,
  Search,
  Scale,
  FileText,
  CheckCircle2,
  Clock,
} from "lucide-react";
import type { AssistantEvent } from "../shared/types";
import { MOTION_TOKENS, getToolLabel, MOTION_KEYFRAMES } from "./MotionTokens";

interface ToolStep {
  name: string;
  label: string;
  timestamp: number;
  isStreaming: boolean;
  elapsedSecs?: number;
}

/**
 * Maps tool categories to icons for visual distinction.
 */
function getToolIcon(toolName: string) {
  if (toolName.includes("search")) return <Search size={14} className="shrink-0" />;
  if (toolName.includes("kanoon")) return <Scale size={14} className="shrink-0" />;
  if (toolName.includes("doc") || toolName.includes("document")) {
    return <FileText size={14} className="shrink-0" />;
  }
  return <Clock size={14} className="shrink-0" />;
}

interface Props {
  events: AssistantEvent[];
  isStreaming: boolean;
}

export function ToolActivityStream({ events, isStreaming }: Props) {
  const [isExpanded, setIsExpanded] = useState(true);
  const [completedSteps, setCompletedSteps] = useState<ToolStep[]>([]);
  const [currentStep, setCurrentStep] = useState<ToolStep | null>(null);

  // Extract tool events and build the stream state
  useEffect(() => {
    const toolEvents = events.filter((e) => e.type === "tool_call_start");

    // Rebuild completed and current steps
    const completed: ToolStep[] = [];
    let current: ToolStep | null = null;

    for (const event of toolEvents) {
      const label = getToolLabel(event.name);
      const step: ToolStep = {
        name: event.name,
        label,
        timestamp: Date.now(),
        isStreaming: !!event.isStreaming,
        elapsedSecs: event.elapsedSecs,
      };

      if (event.isStreaming) {
        // Most recent streaming event becomes current
        current = step;
      } else {
        completed.push(step);
      }
    }

    setCompletedSteps(completed);
    setCurrentStep(current);
  }, [events]);

  // Collapse to the compact "N steps completed" summary once the turn ends.
  useEffect(() => {
    if (!isStreaming) setIsExpanded(false);
  }, [isStreaming]);

  // Don't render if no tool activity
  if (!isStreaming && completedSteps.length === 0 && !currentStep) {
    return null;
  }

  const totalSteps = completedSteps.length + (currentStep ? 1 : 0);
  const stepWord = totalSteps === 1 ? "step" : "steps";

  return (
    <>
      <style>{MOTION_KEYFRAMES}</style>
      <div className="rounded-lg border border-gray-200 bg-white overflow-hidden">
        {/* Header with toggle and summary */}
        <button
          onClick={() => setIsExpanded(!isExpanded)}
          className="w-full flex items-center justify-between px-3 py-2.5 font-serif text-sm text-gray-600 hover:text-gray-800 hover:bg-gray-50 transition-colors"
        >
          <div className="flex items-center gap-2 min-w-0">
            <div className="w-1.5 h-1.5 rounded-full bg-blue-500 shrink-0" />
            <span className="truncate font-medium">
              {isStreaming
                ? `${totalSteps} ${stepWord} running…`
                : `${totalSteps} ${stepWord} completed`}
            </span>
          </div>
          <ChevronDown
            size={12}
            className={`shrink-0 ml-2 transition-transform duration-200 ${
              isExpanded ? "" : "-rotate-90"
            }`}
          />
        </button>

        {/* Content */}
        {isExpanded && (
          <div className="px-3 pb-2.5 space-y-2">
            {/* Current step with pulsing indicator */}
            {currentStep && (
              <div
                className="flex items-start gap-2.5 p-2 rounded-md bg-blue-50 border border-blue-100"
                style={{ animation: "fadeIn 180ms ease-out" }}
              >
                {/* Pulsing dot */}
                <div className="flex items-center justify-center shrink-0 mt-1">
                  <div className="relative w-2.5 h-2.5">
                    <div className="absolute inset-0 bg-blue-400 rounded-full animate-pulse" />
                    <div className="absolute inset-0 bg-blue-500 rounded-full" />
                  </div>
                </div>
                {/* Content */}
                <div className="flex-1 min-w-0">
                  <p className="text-xs font-medium text-blue-900">
                    {currentStep.label}
                  </p>
                  {currentStep.elapsedSecs && currentStep.elapsedSecs > 0 && (
                    <p className="text-xs text-blue-700 mt-0.5 tabular-nums">
                      {currentStep.elapsedSecs}s elapsed
                    </p>
                  )}
                </div>
                {/* Icon */}
                <div className="text-blue-400 shrink-0">
                  {getToolIcon(currentStep.name)}
                </div>
              </div>
            )}

            {/* Completed steps with stagger */}
            {completedSteps.length > 0 && (
              <div className="space-y-1.5">
                {completedSteps.map((step, idx) => (
                  <div
                    key={`${step.name}-${idx}`}
                    className="flex items-start gap-2.5 p-2 rounded-md bg-gray-50 hover:bg-gray-100 transition-colors"
                    style={{
                      animation: `slideInUp 180ms ease-out`,
                      animationDelay: `${idx * MOTION_TOKENS.STAGGER_FAST}ms`,
                    }}
                  >
                    {/* Check icon */}
                    <div className="text-green-500 shrink-0 mt-0.5">
                      <CheckCircle2 size={13} />
                    </div>
                    {/* Content */}
                    <div className="flex-1 min-w-0">
                      <p className="text-xs font-medium text-gray-700">
                        {step.label}
                      </p>
                    </div>
                    {/* Tool icon */}
                    <div className="text-gray-400 shrink-0">
                      {getToolIcon(step.name)}
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>
        )}
      </div>
    </>
  );
}
