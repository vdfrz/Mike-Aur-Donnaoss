"use client";

import { Suspense, useEffect } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import ProjectAssistantChatPage from "./page-client";

function ProjectAssistantChatGate() {
    // Static export prerenders a single page; both ids ride in the query
    // string (?id=<projectId>&chatId=<chatId>). useSearchParams() needs the
    // Suspense boundary below.
    const router = useRouter();
    const params = useSearchParams();
    const id = params.get("id");
    const chatId = params.get("chatId");

    useEffect(() => {
        if (!id) {
            router.replace("/projects");
        } else if (!chatId) {
            router.replace(`/projects/view?id=${id}&tab=assistant`);
        }
    }, [id, chatId, router]);

    if (!id || !chatId) return null;
    // Static query-param routing reuses this component instance when only the
    // query changes; key by both ids to force a remount and reset mount-once
    // load state when a different chat is selected.
    return (
        <ProjectAssistantChatPage
            key={`${id}:${chatId}`}
            params={Promise.resolve({ id, chatId })}
        />
    );
}

export default function Page() {
    return (
        <Suspense fallback={null}>
            <ProjectAssistantChatGate />
        </Suspense>
    );
}
