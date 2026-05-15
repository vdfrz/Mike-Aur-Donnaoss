import ClientPage from "./page-client";

export const dynamic = "force-static";
export const dynamicParams = false;
export function generateStaticParams() {
    return [{ id: "__ph__" }];
}
export default async function Page(props: {
    params: Promise<{ id: string }>;
}) {
    const { id } = await props.params;
    return <ClientPage params={Promise.resolve({ id })} />;
}
