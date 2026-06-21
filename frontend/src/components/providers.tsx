"use client";

import { AuthProvider } from "@/contexts/AuthContext";
import { UserProfileProvider } from "@/contexts/UserProfileContext";
import { OfflineBanner } from "@/app/components/shared/OfflineBanner";

export function Providers({ children }: { children: React.ReactNode }) {
    return (
        <AuthProvider>
            <UserProfileProvider>
                <OfflineBanner />
                {children}
            </UserProfileProvider>
        </AuthProvider>
    );
}
