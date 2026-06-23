"use client";

import { Suspense, useEffect } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import ClientPage from "./page-client";

function WorkflowViewGate() {
    // Static export prerenders a single page; the workflow id rides in the
    // query string (?id=<uuid>). useSearchParams() needs the Suspense below.
    const router = useRouter();
    const id = useSearchParams().get("id");

    useEffect(() => {
        if (!id) router.replace("/workflows");
    }, [id, router]);

    if (!id) return null;
    return <ClientPage params={Promise.resolve({ id })} />;
}

export default function Page() {
    return (
        <Suspense fallback={null}>
            <WorkflowViewGate />
        </Suspense>
    );
}
