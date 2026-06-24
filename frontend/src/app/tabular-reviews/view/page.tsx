"use client";

import { Suspense, useEffect } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import ClientPage from "./page-client";

function TabularReviewViewGate() {
    // Static export prerenders a single page; the review id rides in the
    // query string (?id=<uuid>). useSearchParams() needs the Suspense below.
    const router = useRouter();
    const id = useSearchParams().get("id");

    useEffect(() => {
        if (!id) router.replace("/tabular-reviews");
    }, [id, router]);

    if (!id) return null;
    // Static query-param routing reuses this component instance when only ?id
    // changes; key by ?id to force a remount and reset mount-once load state.
    return <ClientPage key={id} params={Promise.resolve({ id })} />;
}

export default function Page() {
    return (
        <Suspense fallback={null}>
            <TabularReviewViewGate />
        </Suspense>
    );
}
