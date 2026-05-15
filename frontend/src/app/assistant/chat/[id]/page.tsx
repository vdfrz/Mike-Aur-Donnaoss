import ClientPage from "./page-client";

export const dynamic = "force-static";
export const dynamicParams = true;
export function generateStaticParams() {
    return [{ id: "__ph__" }];
}
export default function Page() {
    return <ClientPage />;
}
