import DashboardLayout from "../_dashboard-layout";

export default function MessyDocxLayout({
    children,
}: {
    children: React.ReactNode;
}) {
    return <DashboardLayout>{children}</DashboardLayout>;
}
