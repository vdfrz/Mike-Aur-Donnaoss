"use client";

import { useRouter, useParams } from "next/navigation";
import { useEffect, useRef, useState } from "react";
import { ChatView } from "@/app/components/assistant/ChatView";
import { useAssistantChat } from "@/app/hooks/useAssistantChat";
import { useChatHistoryContext } from "@/app/contexts/ChatHistoryContext";
import { getChat } from "@/app/lib/mikeApi";
import type { MikeMessage } from "@/app/components/shared/types";

export default function AssistantChatPage() {
    const router = useRouter();
    const params = useParams();
    const id = params.id as string;

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
        useAssistantChat({ initialMessages, chatId: id });

    const hasAutoSent = useRef(false);
    const hasLoaded = useRef(false);

    useEffect(() => {
        setCurrentChatId(id);
    }, [id, setCurrentChatId]);

    useEffect(() => {
        if (hasLoaded.current) return;
        hasLoaded.current = true;
        getChat(id)
            .then(({ messages: loaded }) => {
                // A freshly created chat is legitimately empty until its first
                // message streams in — don't treat empty-but-OK as "missing"
                // and bounce away (that's what orphaned the blank chats). Only
                // a real fetch error redirects.
                if (loaded.length > 0) setMessages(loaded);
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
