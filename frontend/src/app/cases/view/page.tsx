"use client";

import { Suspense } from "react";
import { useSearchParams } from "next/navigation";
import ClientPage from "./page-client";

function KeyedClientPage() {
    // Static query-param routing reuses this route's component instance when only
    // ?id changes, so key by ?id to force a remount and reset mount-once load
    // state when a different case is selected.
    const id = useSearchParams().get("id");
    return <ClientPage key={id ?? "none"} />;
}

export default function Page() {
    return (
        <Suspense fallback={null}>
            <KeyedClientPage />
        </Suspense>
    );
}
