import DashboardLayout from "../_dashboard-layout";

export default function AssistantLayout({
    children,
}: {
    children: React.ReactNode;
}) {
    return <DashboardLayout>{children}</DashboardLayout>;
}
