import DashboardLayout from "../_dashboard-layout";

export default function WorkflowsLayout({
    children,
}: {
    children: React.ReactNode;
}) {
    return <DashboardLayout>{children}</DashboardLayout>;
}
