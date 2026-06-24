"use client";

import { Suspense, useEffect } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import ClientPage from "./page-client";

function ProjectTabularReviewGate() {
    // Static export prerenders a single page; both ids ride in the query
    // string (?id=<projectId>&reviewId=<reviewId>). useSearchParams() needs
    // the Suspense boundary below.
    const router = useRouter();
    const params = useSearchParams();
    const id = params.get("id");
    const reviewId = params.get("reviewId");

    useEffect(() => {
        if (!id) {
            router.replace("/projects");
        } else if (!reviewId) {
            router.replace(`/projects/view?id=${id}&tab=reviews`);
        }
    }, [id, reviewId, router]);

    if (!id || !reviewId) return null;
    // Static query-param routing reuses this component instance when only the
    // query changes; key by both ids to force a remount and reset mount-once
    // load state when a different review is selected.
    return (
        <ClientPage
            key={`${id}:${reviewId}`}
            params={Promise.resolve({ id, reviewId })}
        />
    );
}

export default function Page() {
    return (
        <Suspense fallback={null}>
            <ProjectTabularReviewGate />
        </Suspense>
    );
}
