import { getRequestConfig } from "next-intl/server";
import { defaultLocale, isLocale, LOCALE_COOKIE } from "./config";

// next-intl entry point. For the Tauri static build we skip the cookie
// read (it's unavailable with output: "export") and always use the default
// locale. In server mode the user's cookie preference is honoured.
export default getRequestConfig(async () => {
    if (process.env.TAURI_BUILD === "1") {
        return {
            locale: defaultLocale,
            messages: (await import(`../../messages/${defaultLocale}.json`))
                .default,
        };
    }

    const { cookies } = await import("next/headers");
    const cookieStore = await cookies();
    const cookieLocale = cookieStore.get(LOCALE_COOKIE)?.value;
    const locale = isLocale(cookieLocale) ? cookieLocale : defaultLocale;

    const messages = (await import(`../../messages/${locale}.json`)).default;

    return {
        locale,
        messages,
    };
});
