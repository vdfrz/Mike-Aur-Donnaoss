import ProjectAssistantChatPage from "./page-client";

export const dynamic = "force-static";
export const dynamicParams = true;
export function generateStaticParams() {
    return [{ id: "__ph__", chatId: "__ph__" }];
}
export default async function Page(props: {
    params: Promise<{ id: string; chatId: string }>;
}) {
    const { id, chatId } = await props.params;
    return (
        <ProjectAssistantChatPage
            params={Promise.resolve({ id, chatId })}
        />
    );
}
