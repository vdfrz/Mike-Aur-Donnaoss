import { Suspense } from "react";
import ClientPage from "./page-client";

export default function Page() {
    return (
        <Suspense fallback={null}>
            <ClientPage />
        </Suspense>
    );
}
