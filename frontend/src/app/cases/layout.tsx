import DashboardLayout from "../_dashboard-layout";

export default function CasesLayout({
    children,
}: {
    children: React.ReactNode;
}) {
    return <DashboardLayout>{children}</DashboardLayout>;
}
