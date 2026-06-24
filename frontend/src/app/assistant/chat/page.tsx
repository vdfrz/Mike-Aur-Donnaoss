"use client";

import { Suspense } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import { useEffect, useRef, useState } from "react";
import { ChatView } from "@/app/components/assistant/ChatView";
import { useAssistantChat } from "@/app/hooks/useAssistantChat";
import { useChatHistoryContext } from "@/app/contexts/ChatHistoryContext";
import { getChat } from "@/app/lib/mikeApi";
import type { MikeMessage } from "@/app/components/shared/types";

function AssistantChatClient() {
    const router = useRouter();
    // Static export can only prerender a single static page, so the chat id
    // travels in the query string (?id=<uuid>) instead of a dynamic path
    // segment. useSearchParams() requires the Suspense boundary below.
    const id = useSearchParams().get("id");

    const { setCurrentChatId, newChatMessages, setNewChatMessages } =
        useChatHistoryContext();

    // Freeze the landing-page handoff ONCE. Reading newChatMessages on every
    // render (and clearing it eagerly) let a re-render — or React StrictMode's
    // dev double-mount — observe an empty handoff, fall through to getChat() on
    // the just-created empty chat, and bounce home (router.replace) — orphaning
    // a blank "Untitled chat" and dropping the typed message. Capturing it here
    // and clearing only when we auto-send mirrors the working project-chat page.
    const [initialMessages] = useState<MikeMessage[]>(newChatMessages ?? []);
    const { messages, isResponseLoading, handleChat, setMessages, cancel } =
        useAssistantChat({ initialMessages, chatId: id ?? undefined });

    const hasAutoSent = useRef(false);
    const hasLoaded = useRef(false);

    useEffect(() => {
        if (!id) {
            router.replace("/assistant");
            return;
        }
        setCurrentChatId(id);
    }, [id, setCurrentChatId, router]);

    useEffect(() => {
        if (!id) return;
        if (hasLoaded.current) return;
        hasLoaded.current = true;
        getChat(id)
            .then(({ messages: loaded }) => {
                // A freshly created chat is legitimately empty until its first
                // message streams in — don't treat empty-but-OK as "missing"
                // and bounce away (that's what orphaned the blank chats). Only
                // a real fetch error redirects.
                // Don't clobber an in-progress streamed turn: if the loader
                // resolves after auto-send started, `loaded` is only the
                // persisted user msg and would wipe the streaming assistant
                // placeholder. Skip when a response is already streaming or an
                // assistant turn already exists in memory.
                setMessages((prev) => {
                    const streaming =
                        prev.some((m) => m.role === "assistant") ||
                        isResponseLoading;
                    if (streaming) return prev;
                    return loaded.length > 0 ? loaded : prev;
                });
            })
            .catch(() => router.replace("/assistant"));
    }, [id]); // eslint-disable-line react-hooks/exhaustive-deps

    useEffect(() => {
        if (
            newChatMessages &&
            newChatMessages.length === 1 &&
            newChatMessages[0].role === "user" &&
            !hasAutoSent.current &&
            !isResponseLoading &&
            messages.length === 1
        ) {
            hasAutoSent.current = true;
            setNewChatMessages(null);
            void handleChat(newChatMessages[0]);
        }
    }, [newChatMessages, messages.length, isResponseLoading]); // eslint-disable-line react-hooks/exhaustive-deps

    if (!id) return null;

    return (
        <ChatView
            messages={messages}
            isResponseLoading={isResponseLoading}
            handleChat={handleChat}
            cancel={cancel}
            showOfflineToggle
        />
    );
}

function KeyedAssistantChatClient() {
    // Static query-param routing reuses this route's component instance when only
    // ?id changes, so key by ?id to force a remount and reset mount-once load
    // state when a different chat is selected.
    const id = useSearchParams().get("id");
    return <AssistantChatClient key={id ?? "none"} />;
}

export default function Page() {
    return (
        <Suspense fallback={null}>
            <KeyedAssistantChatClient />
        </Suspense>
    );
}
