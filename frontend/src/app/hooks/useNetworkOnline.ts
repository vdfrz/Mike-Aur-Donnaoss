"use client";

import { useSyncExternalStore } from "react";

/**
 * Tracks real network connectivity via the browser's `navigator.onLine`
 * flag and the `online` / `offline` window events. These flip the moment
 * the OS network interface changes — i.e. when the user toggles Wi-Fi,
 * pulls the ethernet cable, or enables airplane mode — which is exactly
 * the "turned off the internet" case we want to catch.
 *
 * Implemented with `useSyncExternalStore` so it subscribes to the browser
 * store without a setState-in-effect, and renders `true` on the server
 * (navigator is unavailable during SSR) while reflecting the real value as
 * soon as it mounts on the client.
 */
function subscribe(callback: () => void): () => void {
    window.addEventListener("online", callback);
    window.addEventListener("offline", callback);
    return () => {
        window.removeEventListener("online", callback);
        window.removeEventListener("offline", callback);
    };
}

export function useNetworkOnline(): boolean {
    return useSyncExternalStore(
        subscribe,
        () => navigator.onLine,
        () => true,
    );
}
