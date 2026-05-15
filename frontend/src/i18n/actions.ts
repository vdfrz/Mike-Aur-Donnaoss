"use server";

import { cookies } from "next/headers";
import { isLocale, LOCALE_COOKIE, type Locale } from "./config";

// Server action invoked by the language switcher in the account page. We
// store the locale in an HTTP cookie so the next render reads the right
// catalog server-side (next-intl resolves messages on the server before
// streaming the page). The same value is also persisted in the backend
// `user_settings.locale` column from the client (see LanguageSwitcher) so
// the user's choice follows their data folder, not just their browser
// origin.
export async function setLocaleCookie(locale: Locale): Promise<void> {
    if (!isLocale(locale)) return;
    const cookieStore = await cookies();
    cookieStore.set(LOCALE_COOKIE, locale, {
        path: "/",
        maxAge: 60 * 60 * 24 * 365,
        sameSite: "lax",
    });
}
