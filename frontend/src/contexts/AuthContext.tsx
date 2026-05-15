"use client";

import React, {
    createContext,
    useContext,
    useEffect,
    useState,
    ReactNode,
} from "react";

const TOKEN_KEY = "mike_auth_token";
const USER_KEY = "mike_auth_user";

interface User {
    id: string;
    email?: string;
    username?: string;
    display_name?: string | null;
}

interface AuthContextType {
    user: User | null;
    isAuthenticated: boolean;
    authLoading: boolean;
    setSession: (token: string, user: User) => void;
    signOut: () => Promise<void>;
}

const AuthContext = createContext<AuthContextType | undefined>(undefined);

export function AuthProvider({ children }: { children: ReactNode }) {
    const [user, setUser] = useState<User | null>(null);
    const [authLoading, setAuthLoading] = useState(true);

    useEffect(() => {
        try {
            const storedUser = localStorage.getItem(USER_KEY);
            const token = localStorage.getItem(TOKEN_KEY);
            const apiBase =
                process.env.NEXT_PUBLIC_API_BASE_URL ?? "http://localhost:3001";
            console.log(
                `[mike] boot · api=${apiBase} · ` +
                    (storedUser && token
                        ? `session=present user=${JSON.parse(storedUser).email ?? "?"}`
                        : "session=none"),
            );
            if (storedUser && token) {
                setUser(JSON.parse(storedUser) as User);
            }
        } catch {
            // localStorage not available (SSR) — ignore
        }
        setAuthLoading(false);
    }, []);

    const setSession = (token: string, newUser: User) => {
        localStorage.setItem(TOKEN_KEY, token);
        localStorage.setItem(USER_KEY, JSON.stringify(newUser));
        setUser(newUser);
    };

    const signOut = async () => {
        localStorage.removeItem(TOKEN_KEY);
        localStorage.removeItem(USER_KEY);
        setUser(null);
    };

    return (
        <AuthContext.Provider
            value={{
                user,
                isAuthenticated: !!user,
                authLoading,
                setSession,
                signOut,
            }}
        >
            {children}
        </AuthContext.Provider>
    );
}

export function useAuth() {
    const context = useContext(AuthContext);
    if (context === undefined) {
        throw new Error("useAuth must be used within an AuthProvider");
    }
    return context;
}
