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
    return (
        <ProjectAssistantChatPage params={Promise.resolve({ id, chatId })} />
    );
}

export default function Page() {
    return (
        <Suspense fallback={null}>
            <ProjectAssistantChatGate />
        </Suspense>
    );
}
