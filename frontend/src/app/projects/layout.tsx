import DashboardLayout from "../_dashboard-layout";

export default function ProjectsLayout({
    children,
}: {
    children: React.ReactNode;
}) {
    return <DashboardLayout>{children}</DashboardLayout>;
}
