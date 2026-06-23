"use client";

/**
 * Settings → Telegram bot
 *
 * Three steps to put Donna on Telegram:
 *   1  paste the BotFather token  → validated live with Telegram's getMe,
 *      shows the real @handle on success or a clear error on a bad token
 *   2  copy and run one command   → the validated token is injected into the
 *      exact start command (the bot reads TELEGRAM_BOT_TOKEN from that env),
 *      with the laptop-lifecycle reality spelled out, not in fine print
 *   3  message your bot           → /login, resurface chats, fetch docs
 *
 * Plus "Donna's persona": the exact blurb the bot introduces itself with,
 * mirrored from telegram-bot/src/main.rs so it stays accurate.
 *
 * No backend round-trip: getMe is called straight from the browser
 * (api.telegram.org sends Access-Control-Allow-Origin: *), and the token is
 * carried by the copyable command rather than written to a file. The bot
 * picks it up from the environment that command sets.
 */

import { useEffect, useRef, useState } from "react";

// Mirrors telegram-bot/src/main.rs (set_my_short_description /
// set_my_description, called at startup). Keep in sync if the bot's blurb
// changes — this is the single display copy of that persona text.
// ponytail: hand-mirrored; Rust↔TS const sharing isn't worth codegen for one string.
const DONNA_SHORT = "Donna, your Mike aur Donna legal assistant on Telegram.";
const DONNA_DESCRIPTION =
    "Donna is the Telegram half of Mike aur Donna, your local legal assistant. " +
    "Send /login <PIN> to connect to your account, then ask a question, use /chats " +
    "to continue a conversation from your laptop, or /docs to download a draft.";

// A BotFather token looks like  <digits>:<35+ url-safe chars>.
const TOKEN_RE = /^\d{6,}:[A-Za-z0-9_-]{30,}$/;

const PLACEHOLDER_TOKEN = "<your token>";
const PLACEHOLDER_ROOT = "<your project root>";

type Status = "idle" | "checking" | "ok" | "error";
type OS = "mac" | "win";

// Only inject the real token once it is validated (connected), so the command
// the user copies always matches the one shown in the terminal. PowerShell and
// the Mac/Linux shell set an env var differently, so build the command per-OS.
function startCommand(token: string, connected: boolean, os: OS) {
    const t = connected ? token.trim() : PLACEHOLDER_TOKEN;
    if (os === "win") {
        return `cd "${PLACEHOLDER_ROOT}\\telegram-bot"; $env:TELEGRAM_BOT_TOKEN="${t}"; cargo run`;
    }
    return `cd "${PLACEHOLDER_ROOT}/telegram-bot" && TELEGRAM_BOT_TOKEN=${t} cargo run`;
}

export default function TelegramBotPage() {
    const [token, setToken] = useState("");
    const [status, setStatus] = useState<Status>("idle");
    const [handle, setHandle] = useState(""); // real @username from getMe
    const [error, setError] = useState("");
    const [copied, setCopied] = useState<"cmd" | "persona" | null>(null);
    const [os, setOs] = useState<OS>("mac");
    const copyTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

    useEffect(() => () => {
        if (copyTimer.current) clearTimeout(copyTimer.current);
    }, []);

    const trimmed = token.trim();
    const wellFormed = TOKEN_RE.test(trimmed);
    const connected = status === "ok";

    async function validate() {
        if (!wellFormed || status === "checking") return;
        setStatus("checking");
        setError("");
        try {
            const res = await fetch(`https://api.telegram.org/bot${trimmed}/getMe`);
            const data = await res.json();
            // Telegram usernames are [A-Za-z0-9_]{5,32}; gate the API-returned
            // value before it reaches an href / link text (defense in depth).
            const username = data.result?.username;
            if (data.ok && typeof username === "string" && /^[A-Za-z0-9_]{5,32}$/.test(username)) {
                setHandle(username);
                setStatus("ok");
            } else {
                setStatus("error");
                setError(
                    data.description
                        ? `Telegram rejected this token (${data.description}). Check you copied it whole from BotFather.`
                        : "Telegram did not recognise this token. Check you copied it whole from BotFather.",
                );
            }
        } catch {
            setStatus("error");
            setError("Could not reach Telegram. Check your internet connection, then try again.");
        }
    }

    async function copy(text: string, which: "cmd" | "persona") {
        try {
            // await so a rejected write (permission denied / non-secure context)
            // is caught here rather than left as a floating promise.
            await navigator.clipboard?.writeText(text);
        } catch {
            /* clipboard unavailable; the text is still on screen to copy by hand */
        }
        setCopied(which);
        if (copyTimer.current) clearTimeout(copyTimer.current);
        copyTimer.current = setTimeout(() => setCopied((c) => (c === which ? null : c)), 1600);
    }

    const command = startCommand(token, connected, os);

    return (
        <div className="space-y-8">
            <style>{`
                @keyframes tg-msg{from{opacity:0;transform:translateY(8px)}to{opacity:1;transform:none}}
                .tg-msg{animation:tg-msg .4s ease both}
            `}</style>

            {/* Header */}
            <div className="flex items-start gap-3">
                <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-[10px] bg-[#229ED9]">
                    <TgPlane className="h-[22px] w-[22px]" />
                </div>
                <div>
                    <h2 className="font-eb-garamond text-[26px] font-semibold leading-tight text-foreground">
                        Put Donna on Telegram
                    </h2>
                    <p className="mt-1 max-w-[620px] text-sm leading-relaxed text-muted-foreground">
                        Create a bot with BotFather, paste its token, and run one command. Donna then
                        answers from your phone, using your case files on this machine. Three steps.
                    </p>
                </div>
            </div>

            {/* Step 1 — paste the token */}
            <StepCard n={1} title="Paste your bot token">
                <div className="grid grid-cols-1 gap-7 lg:grid-cols-[1fr_300px]">
                    <div className="min-w-0">
                        <p className="text-[13.5px] leading-relaxed text-muted-foreground">
                            Open Telegram and message{" "}
                            <a
                                href="https://t.me/BotFather"
                                target="_blank"
                                rel="noreferrer"
                                className="font-medium text-blue-700 underline-offset-2 hover:underline"
                            >
                                @BotFather
                            </a>
                            . Send <Mono>/newbot</Mono>, choose a name and a username, and it hands you
                            an HTTP API token. Paste that token here.
                        </p>

                        <label
                            htmlFor="tg-token"
                            className="mb-1.5 mt-5 block text-[12px] font-semibold text-muted-foreground"
                        >
                            HTTP API token
                        </label>
                        <div className="flex gap-2">
                            <input
                                id="tg-token"
                                value={token}
                                onChange={(e) => {
                                    setToken(e.target.value);
                                    setStatus("idle");
                                    setError("");
                                }}
                                onKeyDown={(e) => e.key === "Enter" && validate()}
                                placeholder="123456789:AA..."
                                spellCheck={false}
                                autoComplete="off"
                                className="min-w-0 flex-1 rounded-[9px] border bg-background px-2.5 py-2.5 font-mono text-[12.5px] text-foreground outline-none transition focus:ring-[3px] focus:ring-blue-100"
                                style={{
                                    borderColor:
                                        token.length === 0
                                            ? "var(--color-border)"
                                            : status === "error"
                                              ? "rgba(220,38,38,.5)"
                                              : wellFormed
                                                ? "var(--color-blue-200)"
                                                : "rgba(220,38,38,.5)",
                                }}
                            />
                            <button
                                onClick={validate}
                                disabled={!wellFormed || status === "checking" || connected}
                                aria-busy={status === "checking"}
                                className="shrink-0 rounded-[9px] bg-blue-600 px-4 text-[13px] font-semibold text-background transition hover:bg-blue-700 disabled:cursor-not-allowed disabled:bg-muted disabled:text-muted-foreground"
                            >
                                {status === "checking" ? "Checking…" : connected ? "Connected" : "Connect"}
                            </button>
                        </div>

                        {/* status line */}
                        <div className="mt-3 min-h-[20px] text-[12.5px]" aria-live="polite">
                            {status === "ok" && (
                                <div className="tg-msg flex items-center gap-2 rounded-[9px] border border-green-600/30 bg-green-600/10 px-3 py-2 text-green-600">
                                    <Check className="h-4 w-4 shrink-0" />
                                    <span>
                                        Connected to{" "}
                                        <a
                                            href={`https://t.me/${handle}`}
                                            target="_blank"
                                            rel="noreferrer"
                                            className="font-semibold underline-offset-2 hover:underline"
                                        >
                                            @{handle}
                                        </a>
                                        . Telegram confirmed this token is live.
                                    </span>
                                </div>
                            )}
                            {status === "error" && (
                                <div className="tg-msg flex items-start gap-2 rounded-[9px] border border-red-600/30 bg-red-600/10 px-3 py-2 text-red-600">
                                    <AlertCircle className="mt-px h-4 w-4 shrink-0" />
                                    <span>{error}</span>
                                </div>
                            )}
                            {status === "idle" && token.length > 0 && !wellFormed && (
                                <div className="flex items-center gap-2 text-muted-foreground">
                                    <AlertCircle className="h-4 w-4 shrink-0" />
                                    <span>That does not look like a BotFather token yet.</span>
                                </div>
                            )}
                            {status === "idle" && wellFormed && (
                                <div className="flex items-center gap-2 text-muted-foreground">
                                    <span>Press Connect to verify it with Telegram.</span>
                                </div>
                            )}
                        </div>

                        <div className="mt-3 flex items-start gap-2 text-[12px] leading-snug text-muted-foreground">
                            <Lock className="mt-px h-3.5 w-3.5 shrink-0" />
                            <span>
                                Your token is checked directly against Telegram from this device and is
                                never sent to our servers. Keep it private; anyone with it can control
                                your bot.
                            </span>
                        </div>
                    </div>

                    {/* BotFather preview */}
                    <BotFatherPreview />
                </div>
            </StepCard>

            {/* Step 2 — run the command */}
            <StepCard n={2} title="Open a terminal and run one command">
                <p className="text-[13.5px] leading-relaxed text-muted-foreground">
                    Open a terminal (PowerShell on Windows, or Terminal on the Mac) and paste this exact
                    command. You paste it every time you want to use the bot, because Donna runs only
                    while that window stays open.
                </p>

                {/* OS picker — the command differs between PowerShell and a Mac/Linux shell */}
                <div className="mt-3.5 inline-flex rounded-[9px] border border-border bg-muted p-0.5 text-[12.5px] font-semibold">
                    {([["mac", "Mac / Linux"], ["win", "Windows"]] as const).map(([key, label]) => (
                        <button
                            key={key}
                            onClick={() => setOs(key)}
                            aria-pressed={os === key}
                            className={`rounded-[7px] px-3 py-1.5 transition ${
                                os === key
                                    ? "bg-card text-foreground shadow-sm"
                                    : "text-muted-foreground hover:text-foreground"
                            }`}
                        >
                            {label}
                        </button>
                    ))}
                </div>

                <div className="mt-2.5 overflow-hidden rounded-xl border border-[#1f2937] shadow-sm">
                    <div className="flex items-center gap-2 bg-[#161b22] px-3.5 py-2.5">
                        <span className="h-[11px] w-[11px] rounded-full bg-[#ff5f57]" />
                        <span className="h-[11px] w-[11px] rounded-full bg-[#febc2e]" />
                        <span className="h-[11px] w-[11px] rounded-full bg-[#28c840]" />
                        <span className="-ml-12 flex-1 text-center font-mono text-[12px] text-[#8b949e]">
                            {os === "win" ? "PowerShell" : "Terminal"}
                        </span>
                        <button
                            onClick={() => copy(command, "cmd")}
                            className="inline-flex items-center gap-1.5 rounded-md bg-white/10 px-2.5 py-1 text-[11.5px] font-semibold text-[#c9d1d9] transition hover:bg-white/20"
                        >
                            {copied === "cmd" ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
                            {copied === "cmd" ? "Copied" : "Copy"}
                        </button>
                    </div>
                    <div className="bg-[#0d1117] px-4 py-3.5 font-mono text-[12.5px] leading-[1.7]">
                        <span className="select-none text-[#6e7681]">{os === "win" ? "PS> " : "$ "}</span>
                        <span className="whitespace-pre-wrap break-all text-[#c9d1d9]">{command}</span>
                    </div>
                </div>
                {!connected && (
                    <p className="mt-2 text-[12px] text-muted-foreground">
                        Connect your token above and it drops into the command automatically.
                    </p>
                )}

                {/* What is the project root — plain language + how to copy the path */}
                <div className="mt-4 rounded-xl border border-border bg-muted px-4 py-3.5">
                    <div className="text-[13px] font-semibold text-foreground">
                        What is &lt;your project root&gt;?
                    </div>
                    <p className="mt-1 text-[13px] leading-relaxed text-muted-foreground">
                        It is the folder where Mike lives, the one that contains the{" "}
                        <Mono>telegram-bot</Mono> folder. Open it in Finder or File Explorer, copy its
                        full path, and paste it in place of <Mono>&lt;your project root&gt;</Mono>.
                    </p>
                    <ul className="mt-2 space-y-1 text-[13px] leading-relaxed text-muted-foreground">
                        <li>
                            <span className="font-semibold text-foreground">Mac:</span> right-click the
                            folder, hold Option, and choose &quot;Copy as Pathname&quot;.
                        </li>
                        <li>
                            <span className="font-semibold text-foreground">Windows:</span> open the
                            folder, click the address bar, and copy the path shown there.
                        </li>
                    </ul>
                </div>

                {/* Lifecycle reality — prominent, not fine print */}
                <div className="mt-4 flex items-start gap-3 rounded-xl border border-[#f0883e]/40 bg-[#f0883e]/10 px-4 py-3.5">
                    <Power className="mt-0.5 h-5 w-5 shrink-0 text-[#c2620f]" />
                    <div>
                        <div className="text-[13.5px] font-semibold text-foreground">
                            Paste this command every time you want the bot
                        </div>
                        <p className="mt-1 text-[13px] leading-relaxed text-muted-foreground">
                            Donna runs on your own machine, so she is live only while this terminal
                            window stays open. Close the window or your laptop and she goes offline. To
                            bring her back, open a terminal and paste the command again.
                        </p>
                    </div>
                </div>
            </StepCard>

            {/* Step 3 — message your bot */}
            <StepCard n={3} title="Message your bot">
                <p className="text-[13.5px] leading-relaxed text-muted-foreground">
                    Open your bot in Telegram and send <Mono>/login &lt;your PIN&gt;</Mono> so Donna can
                    act as you. She syncs to this machine and deletes the PIN message for safety. Then
                    ask anything.
                </p>
                {connected && (
                    <a
                        href={`https://t.me/${handle}`}
                        target="_blank"
                        rel="noreferrer"
                        className="mt-3.5 inline-flex items-center gap-2 rounded-[9px] bg-blue-600 px-4 py-2.5 text-[13px] font-semibold text-background transition hover:bg-blue-700"
                    >
                        <TgPlane className="h-[15px] w-[15px]" />
                        Open @{handle} in Telegram
                    </a>
                )}

                <div className="mt-6 grid grid-cols-1 items-stretch gap-4 md:grid-cols-3">
                    <Capability
                        icon={<KeyRound className="h-4 w-4" />}
                        title="Log in each session"
                        body="Donna acts as you, so she asks you to sign in first. Send /login with your PIN; she syncs to your laptop, then deletes the PIN message."
                    >
                        <MiniOut mono>/login ••••</MiniOut>
                        <MiniIn>Logged in. I am now acting as you and syncing with your laptop.</MiniIn>
                    </Capability>
                    <Capability
                        icon={<History className="h-4 w-4" />}
                        title="Resurface your chats"
                        body="Continue a conversation you started on your laptop. Send /chats, tap one from the list, and pick up where you left off."
                    >
                        <MiniOut mono>/chats</MiniOut>
                        <MiniIn>Pick a conversation to continue here:</MiniIn>
                        <MiniRow>Starting with hello</MiniRow>
                        <MiniRow>[TEST] Markdown draft</MiniRow>
                    </Capability>
                    <Capability
                        icon={<FileText className="h-4 w-4" />}
                        title="Fetch your documents"
                        body="Ask Donna to pull a draft straight from the Mike desktop app and she sends the full text to your phone."
                    >
                        <MiniOut>Send me the draft here</MiniOut>
                        <MiniIn>
                            Sure, here is the full text of the draft:
                            <br />
                            <span className="font-mono text-muted-foreground"># AFFIDAVIT ...</span>
                        </MiniIn>
                    </Capability>
                </div>
            </StepCard>

            {/* Donna's persona */}
            <div className="rounded-xl border border-border bg-card p-5 shadow-sm">
                <h3 className="font-eb-garamond text-[20px] font-semibold text-foreground">
                    Donna&apos;s persona
                </h3>
                <p className="mt-1 text-[13px] leading-relaxed text-muted-foreground">
                    This is the bio Donna introduces herself with on Telegram, pulled straight from the
                    bot so it stays accurate. The bot sets it automatically on launch; copy it into
                    BotFather&apos;s <Mono>/setdescription</Mono> if you would rather brand her by hand.
                </p>
                <div className="mt-3.5 rounded-[10px] border border-border bg-muted px-4 py-3.5">
                    <div className="text-[13px] font-semibold text-foreground">{DONNA_SHORT}</div>
                    <p className="mt-2 text-[13px] leading-relaxed text-muted-foreground">
                        {DONNA_DESCRIPTION}
                    </p>
                </div>
                <button
                    onClick={() => copy(`${DONNA_SHORT}\n\n${DONNA_DESCRIPTION}`, "persona")}
                    className="mt-3 inline-flex items-center gap-1.5 rounded-[9px] border border-border bg-background px-3.5 py-2 text-[12.5px] font-semibold text-foreground transition hover:bg-muted"
                >
                    {copied === "persona" ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
                    {copied === "persona" ? "Copied" : "Copy persona"}
                </button>
            </div>
        </div>
    );
}

/* ── step + presentational helpers ─────────────────────────────── */

function StepCard({ n, title, children }: { n: number; title: string; children: React.ReactNode }) {
    return (
        <section className="rounded-xl border border-border bg-card p-5 shadow-sm">
            <div className="mb-4 flex items-center gap-3">
                <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-full bg-blue-600 text-[13px] font-bold text-background">
                    {n}
                </span>
                <h3 className="text-[16px] font-semibold text-foreground">{title}</h3>
            </div>
            {children}
        </section>
    );
}

function Mono({ children }: { children: React.ReactNode }) {
    return (
        <code className="rounded bg-muted px-1.5 py-0.5 font-mono text-[12px] text-foreground">
            {children}
        </code>
    );
}

function BotFatherPreview() {
    const inBubble =
        "self-start max-w-[88%] rounded-[12px] rounded-bl-[4px] bg-white px-2.5 py-1.5 text-[11.5px] leading-[1.5] text-[#1a1a1a] shadow-sm";
    const outBubble =
        "self-end max-w-[82%] rounded-[12px] rounded-br-[4px] bg-[#eeffde] px-2.5 py-1.5 text-[11.5px] leading-[1.45] text-[#1a1a1a] shadow-sm";
    // staggered fade so the convo reads top-to-bottom without a JS state machine
    const d = (i: number) => ({ animationDelay: `${i * 0.18}s` });
    return (
        <div className="mx-auto w-full max-w-[300px] overflow-hidden rounded-[20px] border border-border bg-[#cdd9e3] shadow-sm">
            <div className="flex items-center gap-2.5 bg-[#527da3] px-3 py-2 text-white">
                <span className="flex h-[30px] w-[30px] shrink-0 items-center justify-center rounded-full bg-white/20 text-[13px] font-bold">
                    BF
                </span>
                <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-1 text-[13px] font-semibold">
                        BotFather <VerifiedTick />
                    </div>
                    <div className="text-[10.5px] text-white/75">bot</div>
                </div>
            </div>
            <div className="flex flex-col gap-1.5 px-3 py-3">
                <div className={`${outBubble} tg-msg font-mono`} style={d(0)}>
                    /newbot
                </div>
                <div className={`${inBubble} tg-msg`} style={d(1)}>
                    Alright, a new bot. How are we going to call it?
                </div>
                <div className={`${outBubble} tg-msg`} style={d(2)}>
                    Donna Legal Assistant
                </div>
                <div className={`${inBubble} tg-msg`} style={d(3)}>
                    Good. Now choose a username. It must end in <b>bot</b>.
                </div>
                <div className={`${outBubble} tg-msg font-mono`} style={d(4)}>
                    donna_legal_bot
                </div>
                <div className={`${inBubble} tg-msg`} style={d(5)}>
                    Done. Use this token to access the HTTP API:
                    <div className="mt-1.5 break-all rounded-md border border-gray-200 bg-gray-100 px-2 py-1.5 font-mono text-[10.5px] text-[#0f172a]">
                        8124453019:AAH9xQk3vN-pLm2RtY8wZ4bC6dE7fG1hIjk
                    </div>
                </div>
            </div>
        </div>
    );
}

function Capability({
    icon,
    title,
    body,
    children,
}: {
    icon: React.ReactNode;
    title: string;
    body: string;
    children: React.ReactNode;
}) {
    return (
        <div className="flex flex-col rounded-xl border border-border bg-background p-[17px]">
            <div className="mb-2.5 flex items-center gap-2.5">
                <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-muted text-foreground">
                    {icon}
                </div>
                <div className="text-[14px] font-semibold text-foreground">{title}</div>
            </div>
            <p className="mb-3 flex-1 text-[12.5px] leading-snug text-muted-foreground">{body}</p>
            <div className="flex flex-col gap-1.5 rounded-[10px] bg-muted p-2.5">{children}</div>
        </div>
    );
}

function MiniOut({ children, mono }: { children: React.ReactNode; mono?: boolean }) {
    return (
        <div
            className={`self-end max-w-[88%] rounded-[10px] rounded-br-[4px] bg-[#dcf3c5] px-2.5 py-1.5 text-[11.5px] text-[#1a1a1a] ${
                mono ? "font-mono" : ""
            }`}
        >
            {children}
        </div>
    );
}
function MiniIn({ children }: { children: React.ReactNode }) {
    return (
        <div className="self-start max-w-[92%] rounded-[10px] rounded-bl-[4px] bg-white px-2.5 py-1.5 text-[11.5px] leading-snug text-[#1a1a1a] shadow-sm">
            {children}
        </div>
    );
}
function MiniRow({ children }: { children: React.ReactNode }) {
    return (
        <div className="rounded-lg border border-gray-200 bg-white px-2.5 py-1 text-center text-[11px] text-gray-700">
            {children}
        </div>
    );
}

/* ── inline icons ──────────────────────────────────────────────── */

function TgPlane({ className }: { className?: string }) {
    return (
        <svg viewBox="0 0 24 24" className={className} fill="#fff" aria-hidden>
            <path d="M21.5 4.5 2.7 11.9c-1 .4-1 1.8 0 2.1l4.6 1.4 1.8 5.6c.3.9 1.4 1.1 2 .4l2.5-2.6 4.6 3.4c.7.5 1.7.1 1.9-.7L23.7 5.6c.2-1-.8-1.8-1.7-1.4Z" />
        </svg>
    );
}
function VerifiedTick() {
    return (
        <svg viewBox="0 0 24 24" className="h-3 w-3 shrink-0" aria-hidden>
            <circle cx="12" cy="12" r="11" fill="#3da5e8" />
            <path d="M7 12.4l3.2 3.1L17 8.7" fill="none" stroke="#fff" strokeWidth={2.2} strokeLinecap="round" strokeLinejoin="round" />
        </svg>
    );
}
function Check({ className }: { className?: string }) {
    return (
        <svg viewBox="0 0 24 24" className={className} fill="none" stroke="currentColor" strokeWidth={2.4} strokeLinecap="round" strokeLinejoin="round" aria-hidden>
            <polyline points="20 6 9 17 4 12" />
        </svg>
    );
}
function AlertCircle({ className }: { className?: string }) {
    return (
        <svg viewBox="0 0 24 24" className={className} fill="none" stroke="currentColor" strokeWidth={2.4} strokeLinecap="round" strokeLinejoin="round" aria-hidden>
            <circle cx="12" cy="12" r="9" />
            <line x1="12" y1="8" x2="12" y2="13" />
            <line x1="12" y1="16" x2="12" y2="16" />
        </svg>
    );
}
function Copy({ className }: { className?: string }) {
    return (
        <svg viewBox="0 0 24 24" className={className} fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" aria-hidden>
            <rect x="9" y="9" width="13" height="13" rx="2" />
            <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
        </svg>
    );
}
function Power({ className }: { className?: string }) {
    return (
        <svg viewBox="0 0 24 24" className={className} fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" aria-hidden>
            <path d="M18.36 6.64a9 9 0 1 1-12.73 0" />
            <line x1="12" y1="2" x2="12" y2="12" />
        </svg>
    );
}
function Lock({ className }: { className?: string }) {
    return (
        <svg viewBox="0 0 24 24" className={className} fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" aria-hidden>
            <rect x="3" y="11" width="18" height="11" rx="2" />
            <path d="M7 11V7a5 5 0 0 1 10 0v4" />
        </svg>
    );
}
function KeyRound({ className }: { className?: string }) {
    return (
        <svg viewBox="0 0 24 24" className={className} fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" aria-hidden>
            <circle cx="8" cy="15" r="4" />
            <path d="m10.85 12.15 6.15-6.15m-2 0 2 2m-4 0 2 2" />
        </svg>
    );
}
function History({ className }: { className?: string }) {
    return (
        <svg viewBox="0 0 24 24" className={className} fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" aria-hidden>
            <path d="M3 12a9 9 0 1 0 3-6.7L3 8" />
            <path d="M3 3v5h5" />
            <path d="M12 7v5l3 2" />
        </svg>
    );
}
function FileText({ className }: { className?: string }) {
    return (
        <svg viewBox="0 0 24 24" className={className} fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" aria-hidden>
            <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
            <polyline points="14 2 14 8 20 8" />
            <line x1="16" y1="13" x2="8" y2="13" />
            <line x1="16" y1="17" x2="8" y2="17" />
        </svg>
    );
}
