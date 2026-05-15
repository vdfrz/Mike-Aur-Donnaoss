import DashboardLayout from "../_dashboard-layout";

export default function TabularReviewsLayout({
    children,
}: {
    children: React.ReactNode;
}) {
    return <DashboardLayout>{children}</DashboardLayout>;
}
