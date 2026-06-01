"use client";

// "Powered by IKanoon" attribution badge.
//
// Required by the Indian Kanoon API agreement (https://api.indiankanoon.org/agreement/):
//   "When our information is displayed directly to a user, such as in search
//    results, the graphic must be shown on top of the results"
//   "Make sure the logo is full and clearly visible, never altered, resized
//    or partially covered."
//
// We render the official PNGs at their native pixel sizes (desktop 150x61,
// mobile 28x42). No CSS scaling. The badge links back to indiankanoon.org.

import Image from "next/image";

export default function PoweredByIKanoon({
    className = "",
    variant = "auto",
}: {
    className?: string;
    /** "auto" picks desktop on >=sm screens, mobile below. */
    variant?: "auto" | "desktop" | "mobile";
}) {
    const Desktop = (
        <Image
            src="/ikanoon/powered-desktop.png"
            alt="Powered by Indian Kanoon"
            width={150}
            height={61}
            unoptimized
            priority={false}
        />
    );
    const Mobile = (
        <Image
            src="/ikanoon/powered-mobile.png"
            alt="Powered by Indian Kanoon"
            width={28}
            height={42}
            unoptimized
            priority={false}
        />
    );
    return (
        <a
            href="https://indiankanoon.org/"
            target="_blank"
            rel="noopener noreferrer"
            aria-label="Powered by Indian Kanoon"
            className={`inline-block ${className}`}
        >
            {variant === "desktop" ? (
                Desktop
            ) : variant === "mobile" ? (
                Mobile
            ) : (
                <>
                    <span className="hidden sm:inline-block">{Desktop}</span>
                    <span className="inline-block sm:hidden">{Mobile}</span>
                </>
            )}
        </a>
    );
}
