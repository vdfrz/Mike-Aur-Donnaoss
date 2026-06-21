"use client";

import { useEffect, useRef, useState } from "react";
import Link from "next/link";
import { useTranslations } from "next-intl";
import { Wifi, WifiOff, X, AlertTriangle } from "lucide-react";
import { useNetworkOnline } from "@/app/hooks/useNetworkOnline";
import { useOfflineMode } from "@/app/hooks/useOfflineMode";
import { useSelectedModel } from "@/app/hooks/useSelectedModel";

/**
 * Global, app-wide network indicator. When the browser loses connectivity
 * it surfaces a banner ("You're offline now") and, if a local model is
 * configured, automatically switches the chat to it so the user can keep
 * working — cloud models need the internet, local ones don't. When the
 * connection returns it restores the cloud model it switched away from
 * (but only if *it* made that switch, so a deliberate local choice is left
 * alone) and shows a brief "back online" note.
 *
 * Mounted once in <Providers> so it is available on every page.
 */
export function OfflineBanner() {
    const t = useTranslations("Network");
    const online = useNetworkOnline();
    const { canGoOffline, isOffline, goOffline, goOnline } = useOfflineMode();

    const [dismissed, setDismissed] = useState(false);
    const [reconnected, setReconnected] = useState(false);

    // mike-legal caveat: warn whenever the local fine-tune is the active model.
    const [model] = useSelectedModel();
    const isMikeLegal = model === "local:mike-legal";
    const [mlDismissed, setMlDismissed] = useState(false);
    // Re-show the caveat each time the user switches back to mike-legal.
    useEffect(() => {
        setMlDismissed(false);
    }, [model]);

    // The online/offline edge handlers run as window-event callbacks (not in
    // an effect body), so they read the latest offline-mode helpers through a
    // ref rather than capturing a stale closure.
    const helpers = useRef({ canGoOffline, isOffline, goOffline, goOnline });
    useEffect(() => {
        helpers.current = { canGoOffline, isOffline, goOffline, goOnline };
    });

    // Whether *we* forced the switch to a local model, so we only auto-restore
    // the cloud model on reconnect if the user didn't choose local themselves.
    const forcedOffline = useRef(false);

    useEffect(() => {
        const handleOffline = () => {
            setDismissed(false);
            setReconnected(false);
            const h = helpers.current;
            if (h.canGoOffline && !h.isOffline) {
                h.goOffline();
                forcedOffline.current = true;
            }
        };
        const handleOnline = () => {
            const h = helpers.current;
            if (forcedOffline.current) {
                h.goOnline();
                forcedOffline.current = false;
            }
            setReconnected(true);
        };
        window.addEventListener("offline", handleOffline);
        window.addEventListener("online", handleOnline);
        return () => {
            window.removeEventListener("offline", handleOffline);
            window.removeEventListener("online", handleOnline);
        };
    }, []);

    // Auto-dismiss the transient "back online" note.
    useEffect(() => {
        if (!reconnected) return;
        const id = setTimeout(() => setReconnected(false), 4000);
        return () => clearTimeout(id);
    }, [reconnected]);

    if (!online && !dismissed) {
        return (
            <div className="fixed inset-x-0 top-0 z-[100] flex justify-center px-3 pt-3 pointer-events-none">
                <div className="pointer-events-auto flex max-w-xl items-start gap-3 rounded-xl border border-amber-200 bg-amber-50 px-4 py-3 shadow-md">
                    <WifiOff className="mt-0.5 h-5 w-5 shrink-0 text-amber-600" />
                    <div className="flex-1 text-sm">
                        <p className="font-medium text-amber-900">
                            {t("offlineTitle")}
                        </p>
                        <p className="mt-0.5 text-amber-800">
                            {canGoOffline
                                ? t("offlineWithLocal")
                                : t("offlineNoLocal")}
                        </p>
                        {!canGoOffline && (
                            <Link
                                href="/account/models"
                                className="mt-2 inline-flex items-center text-sm font-medium text-amber-900 underline underline-offset-2 hover:text-amber-950"
                            >
                                {t("setupLocal")}
                            </Link>
                        )}
                    </div>
                    <button
                        type="button"
                        onClick={() => setDismissed(true)}
                        aria-label={t("dismiss")}
                        className="rounded-md p-1 text-amber-700 hover:bg-amber-100"
                    >
                        <X className="h-4 w-4" />
                    </button>
                </div>
            </div>
        );
    }

    if (online && reconnected) {
        return (
            <div className="fixed inset-x-0 top-0 z-[100] flex justify-center px-3 pt-3 pointer-events-none">
                <div className="pointer-events-auto flex items-center gap-2 rounded-xl border border-[var(--blue-200)] bg-[var(--blue-50)] px-4 py-2.5 text-sm text-[var(--blue-700)] shadow-md">
                    <Wifi className="h-4 w-4 shrink-0" />
                    <span>{t("backOnline")}</span>
                </div>
            </div>
        );
    }

    // mike-legal WIP caveat — same banner design as the offline notice above.
    if (isMikeLegal && !mlDismissed) {
        return (
            <div className="fixed inset-x-0 top-0 z-[100] flex justify-center px-3 pt-3 pointer-events-none">
                <div className="pointer-events-auto flex max-w-xl items-start gap-3 rounded-xl border border-amber-200 bg-amber-50 px-4 py-3 shadow-md">
                    <AlertTriangle className="mt-0.5 h-5 w-5 shrink-0 text-amber-600" />
                    <div className="flex-1 text-sm">
                        <p className="font-medium text-amber-900">
                            mike-legal is a fine-tuned model — very much a work in progress
                        </p>
                        <p className="mt-0.5 text-amber-800">
                            It can only handle about 4–5 messages and hallucinates hard. V2.0 will be much better — promise :3
                        </p>
                    </div>
                    <button
                        type="button"
                        onClick={() => setMlDismissed(true)}
                        aria-label={t("dismiss")}
                        className="rounded-md p-1 text-amber-700 hover:bg-amber-100"
                    >
                        <X className="h-4 w-4" />
                    </button>
                </div>
            </div>
        );
    }

    return null;
}
