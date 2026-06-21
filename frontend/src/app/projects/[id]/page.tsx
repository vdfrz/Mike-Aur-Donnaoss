import ClientPage from "./page-client";

export const dynamic = "force-static";
// true so web hard-loads work; scripts/build-tauri.js flips it to false
// during static export, which forbids dynamicParams.
export const dynamicParams = true;
export function generateStaticParams() {
    return [{ id: "__ph__" }];
}
export default async function Page(props: {
    params: Promise<{ id: string }>;
}) {
    const { id } = await props.params;
    return <ClientPage params={Promise.resolve({ id })} />;
}
