import DashboardLayout from "../_dashboard-layout";

export default function CaseSearchLayout({
    children,
}: {
    children: React.ReactNode;
}) {
    return <DashboardLayout>{children}</DashboardLayout>;
}
