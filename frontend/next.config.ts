import type { NextConfig } from "next";
import createNextIntlPlugin from "next-intl/plugin";

const withNextIntl = createNextIntlPlugin("./src/i18n/request.ts");

const isTauri = process.env.TAURI_BUILD === "1";

const nextConfig: NextConfig = {
    reactCompiler: true,
    // Static export for Tauri — no Node.js server needed
    ...(isTauri ? { output: "export", trailingSlash: true } : {}),
    // Use webpack instead of Turbopack (Turbopack has a bug detecting generateStaticParams
    // inside route groups with output: "export" in Next.js 16)
    // Force webpack (Turbopack has a bug detecting generateStaticParams in Next.js 16)
    webpack: (config) => config,
    // Rewrites only work in server mode (not static export)
    ...(!isTauri ? {
        async rewrites() {
            return [
                {
                    source: "/sitemap.xml",
                    destination: "/api/sitemap/sitemap.xml",
                },
                {
                    source: "/sitemap_:slug.xml",
                    destination: "/api/sitemap/sitemap_:slug.xml",
                },
            ];
        },
        skipTrailingSlashRedirect: true,
    } : {}),
};

export default withNextIntl(nextConfig);
