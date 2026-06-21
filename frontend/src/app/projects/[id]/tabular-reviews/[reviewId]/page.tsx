import ClientPage from "./page-client";

export const dynamic = "force-static";
export const dynamicParams = true;
export function generateStaticParams() {
    return [{ id: "__ph__", reviewId: "__ph__" }];
}
export default async function Page(props: {
    params: Promise<{ id: string; reviewId: string }>;
}) {
    const { id, reviewId } = await props.params;
    return <ClientPage params={Promise.resolve({ id, reviewId })} />;
}
