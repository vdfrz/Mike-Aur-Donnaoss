"use client";

import { AuthProvider } from "@/contexts/AuthContext";
import { UserProfileProvider } from "@/contexts/UserProfileContext";
import { OfflineBanner } from "@/app/components/shared/OfflineBanner";
import ModelDataDisclosureGate from "@/app/components/shared/ModelDataDisclosureGate";

export function Providers({ children }: { children: React.ReactNode }) {
    return (
        <AuthProvider>
            <UserProfileProvider>
                <OfflineBanner />
                <ModelDataDisclosureGate />
                {children}
            </UserProfileProvider>
        </AuthProvider>
    );
}
