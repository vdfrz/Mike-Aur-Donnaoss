"use client";

/**
 * Settings → Telegram bot
 *
 * Interactive walkthrough for connecting Donna to Telegram:
 *  1-5  the BotFather conversation (animated phone preview)
 *  6    paste the HTTP API token into Mike (validated, stored locally)
 *  7    run the backend; Donna stays online while the laptop is open
 *
 * Plus a "What Donna can do" section (login / resurface chats / fetch docs).
 *
 * NOTE: the token save + live status here are SIMULATED for the walkthrough.
 * Wire `saveTelegramToken()` and the online/offline status to the real
 * backend (POST /telegram/token, GET /telegram/status) when ready — search
 * for "TODO(backend)" below.
 */

import { useEffect, useRef, useState } from "react";

const ACCENT = "#0f172a"; // brand ink (matches --accent-ink-strong)

const BOT_NAME = "Donna Legal Assistant";
const BOT_USER = "donna_legal_bot";
// A realistic-looking example token. Replace nothing — this is illustrative.
const SAMPLE_TOKEN = "8124453019:AAH9xQk3vN-pLm2RtY8wZ4bC6dE7fG1hIjk";
const BOTFATHER_PFP =
    "https://encrypted-tbn0.gstatic.com/images?q=tbn:ANd9GcT5iSC0CKDNXpY6hpwD5ejtNrtQCQiElz0lpw&s";

const TOKEN_RE = /^\d{6,}:[A-Za-z0-9_-]{30,}$/;

const TITLES = [
    "Open @BotFather",
    "Start the conversation",
    "Create a new bot",
    "Name your bot",
    "Pick a username",
    "Paste the token into Mike",
    "Keep your laptop open",
];
const DESCS = [
    "In Telegram, search for @BotFather. It is the official bot with the blue tick. Open the chat to begin.",
    "Send /start. BotFather replies with everything it can do.",
    "Send /newbot to begin. BotFather will ask what to call your bot.",
    "Send a display name. This is what people see in the chat, like Donna Legal Assistant.",
    "Send a unique username ending in bot. BotFather creates the bot and gives you an HTTP API token.",
    "Open this Telegram bot page in Mike and paste the token. Mike validates it and stores it locally on your machine.",
    "Run the command once to bring Donna online. She stays active as long as your laptop is open.",
];
const BTN = [
    "Send /start",
    "Send /newbot",
    "Choose a name",
    "Send the username",
    "I have copied the token",
    "Final step",
    null,
];

type TermLine = { text: string; cls: string };

const dot = (delay: string) => (
    <span
        className="inline-block h-1.5 w-1.5 rounded-full bg-slate-400"
        style={{ animation: `tg-dot 1.2s infinite ${delay}` }}
    />
);

function Typing() {
    return (
        <div className="self-start flex gap-1 rounded-xl bg-white px-3 py-2.5 shadow-sm">
            {dot("0s")}
            {dot(".15s")}
            {dot(".3s")}
        </div>
    );
}

const inBubble =
    "self-start max-w-[86%] rounded-[13px] rounded-bl-[5px] bg-white px-2.5 py-2 text-[13px] leading-[1.55] text-[#1a1a1a] shadow-sm whitespace-pre-wrap";
const outBubble =
    "self-end max-w-[80%] rounded-[13px] rounded-br-[5px] bg-[#eeffde] px-2.5 py-2 text-[13px] leading-[1.5] text-[#1a1a1a] shadow-sm";

export default function TelegramBotPage() {
    const [step, setStep] = useState(0);
    const [typing, setTyping] = useState(false);
    const [copied, setCopied] = useState(false);
    const [token, setToken] = useState("");
    const [connected, setConnected] = useState(false);

    const [termLines, setTermLines] = useState<TermLine[]>([]);
    const [running, setRunning] = useState(false);
    const [booted, setBooted] = useState(false);
    const [asleep, setAsleep] = useState(false);

    const msgRef = useRef<HTMLDivElement | null>(null);
    const termRef = useRef<HTMLDivElement | null>(null);
    const typingTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
    const seqTimers = useRef<ReturnType<typeof setTimeout>[]>([]);

    useEffect(() => {
        if (msgRef.current) msgRef.current.scrollTop = msgRef.current.scrollHeight;
        if (termRef.current) termRef.current.scrollTop = termRef.current.scrollHeight;
    });
    useEffect(
        () => () => {
            if (typingTimer.current) clearTimeout(typingTimer.current);
            seqTimers.current.forEach(clearTimeout);
        },
        [],
    );

    const valid = TOKEN_RE.test(token.trim());
    const online = booted && !asleep;

    const advance = () => {
        if (step >= 6) return;
        if (step === 5 && !connected) return;
        const next = step + 1;
        if (next >= 1 && next <= 4) {
            setStep(next);
            setTyping(true);
            if (typingTimer.current) clearTimeout(typingTimer.current);
            typingTimer.current = setTimeout(() => setTyping(false), 880);
        } else {
            setStep(next);
            setTyping(false);
        }
    };
    const goto = (i: number) => {
        if (typingTimer.current) clearTimeout(typingTimer.current);
        setStep(i);
        setTyping(false);
    };

    const copyToken = () => {
        try {
            navigator.clipboard?.writeText(SAMPLE_TOKEN);
        } catch {
            /* ignore */
        }
        setCopied(true);
    };

    // TODO(backend): replace the simulated success with a real call, e.g.
    //   await api("/telegram/token", { method: "POST", body: JSON.stringify({ token }) });
    const saveToken = () => {
        if (valid) setConnected(true);
    };

    const runBackend = () => {
        if (running || booted) return;
        const tok = SAMPLE_TOKEN.slice(0, 18) + "...";
        const seq: TermLine[] = [
            { text: "$ export TELEGRAM_BOT_TOKEN=" + tok, cls: "text-[#c9d1d9]" },
            { text: "$ cargo run --features rag", cls: "text-[#c9d1d9]" },
            { text: "   Compiling mike v0.4.0 (/Users/you/mike)", cls: "text-[#6e7681]" },
            { text: "    Finished release profile [optimized] in 11.8s", cls: "text-[#6e7681]" },
            { text: "     Running target/release/mike", cls: "text-[#6e7681]" },
            { text: "> axum listening on http://127.0.0.1:3001", cls: "text-[#58a6ff]" },
            { text: "> telegram: connected as @" + BOT_USER, cls: "text-[#58a6ff]" },
            { text: "OK long-polling started. Donna is online.", cls: "text-[#3fb950] font-medium" },
        ];
        setRunning(true);
        setAsleep(false);
        setTermLines([]);
        seqTimers.current = [];
        seq.forEach((line, i) => {
            const id = setTimeout(() => {
                setTermLines((prev) => [...prev, line]);
                if (i === seq.length - 1) {
                    setRunning(false);
                    setBooted(true);
                }
            }, 360 + i * 430);
            seqTimers.current.push(id);
        });
    };

    // Demonstrates the dependency: laptop open keeps Donna online; closing the
    // lid pauses her. Reopening resets so the command can be run again.
    const toggleLaptop = () => {
        if (booted && !asleep) {
            setAsleep(true);
            setBooted(false);
            setRunning(false);
            setTermLines((prev) => [
                ...prev,
                { text: " ", cls: "" },
                {
                    text: "[laptop asleep] process paused. Donna is now offline.",
                    cls: "text-[#f0883e]",
                },
            ]);
        } else {
            setAsleep(false);
            setBooted(false);
            setRunning(false);
            setTermLines([]);
        }
    };

    let composerText = "Message";
    let composerMono = false;
    let composerMuted = true;
    if (step === 0) { composerText = "/start"; composerMono = true; composerMuted = false; }
    else if (step === 1) { composerText = "/newbot"; composerMono = true; composerMuted = false; }
    else if (step === 2) { composerText = BOT_NAME; composerMuted = false; }
    else if (step === 3) { composerText = BOT_USER; composerMono = true; composerMuted = false; }
    const composerDisabled = composerMuted;

    return (
        <div className="space-y-7">
            <style>{`
                @keyframes tg-dot{0%,80%,100%{transform:translateY(0);opacity:.35}40%{transform:translateY(-4px);opacity:1}}
                @keyframes tg-msg{from{opacity:0;transform:translateY(9px) scale(.97)}to{opacity:1;transform:none}}
                @keyframes tg-stage{from{opacity:0;transform:translateY(10px)}to{opacity:1;transform:none}}
                @keyframes tg-pulse{0%{box-shadow:0 0 0 0 rgba(22,163,74,.45)}70%{box-shadow:0 0 0 7px rgba(22,163,74,0)}100%{box-shadow:0 0 0 0 rgba(22,163,74,0)}}
                @keyframes tg-blink{0%,49%{opacity:1}50%,100%{opacity:0}}
                .tg-msg{animation:tg-msg .35s ease both}
                .tg-stage{animation:tg-stage .4s ease both}
            `}</style>

            {/* Header */}
            <div className="flex items-start gap-3">
                <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-[10px] bg-[#229ED9]">
                    <TgPlane className="h-[22px] w-[22px]" />
                </div>
                <div>
                    <h2 className="font-serif text-[26px] font-semibold leading-tight text-gray-900">
                        Put Donna on Telegram
                    </h2>
                    <p className="mt-1 max-w-[600px] text-sm text-gray-500">
                        Create a Telegram bot with BotFather, paste its token here, and chat with
                        your case files from your phone. Follow the steps. The preview on the right
                        plays the exact conversation.
                    </p>
                </div>
            </div>

            <div className="grid grid-cols-1 items-start gap-11 lg:grid-cols-[minmax(320px,1fr)_352px]">
                {/* Stepper */}
                <div>
                    {TITLES.map((title, i) => {
                        const done = i < step;
                        const active = i === step;
                        const isLast = i === 6;
                        const numCls = done
                            ? "bg-[#111827] text-white border-[#111827]"
                            : active
                              ? "bg-white text-[#111827] border-[#111827]"
                              : "bg-white text-gray-400 border-gray-200";
                        const btnDisabled = i === 5 && !connected;
                        return (
                            <div key={i} className="flex gap-4">
                                <div className="flex shrink-0 flex-col items-center">
                                    <button
                                        onClick={() => goto(i)}
                                        className={`flex h-[30px] w-[30px] items-center justify-center rounded-full border-2 text-[13px] font-bold transition ${numCls}`}
                                    >
                                        {done ? "✓" : i + 1}
                                    </button>
                                    {!isLast && (
                                        <div
                                            className="w-0.5 flex-1 transition"
                                            style={{
                                                minHeight: active ? 20 : 14,
                                                marginTop: 5,
                                                background: i < step ? ACCENT : "#e5e7eb",
                                            }}
                                        />
                                    )}
                                </div>
                                <div className="min-w-0 flex-1 pb-5">
                                    <button
                                        onClick={() => goto(i)}
                                        className={`pt-1 text-[15px] font-semibold transition ${
                                            done || active ? "text-gray-900" : "text-gray-400"
                                        }`}
                                    >
                                        {title}
                                    </button>
                                    {active && (
                                        <>
                                            <p className="mt-1.5 text-[13.5px] leading-[1.55] text-gray-500">
                                                {DESCS[i]}
                                            </p>
                                            {!isLast && BTN[i] && (
                                                <button
                                                    onClick={advance}
                                                    disabled={btnDisabled}
                                                    className="mt-3.5 rounded-[9px] px-4 py-2 text-[13px] font-semibold text-white transition disabled:cursor-not-allowed disabled:bg-gray-200 disabled:text-gray-400"
                                                    style={
                                                        btnDisabled ? undefined : { background: ACCENT }
                                                    }
                                                >
                                                    {BTN[i]}
                                                </button>
                                            )}
                                            {isLast && (
                                                <div className="mt-3.5 inline-flex items-center gap-2 rounded-lg border border-green-200 bg-green-50 px-3 py-1.5 text-[13px] font-semibold text-green-700">
                                                    <span
                                                        className="h-1.5 w-1.5 rounded-full bg-green-600"
                                                        style={{ animation: "tg-pulse 1.8s infinite" }}
                                                    />
                                                    All set. Donna is connected.
                                                </div>
                                            )}
                                        </>
                                    )}
                                </div>
                            </div>
                        );
                    })}
                </div>

                {/* Stage */}
                <div className="lg:sticky lg:top-0">
                    {step <= 4 && (
                        <div className="tg-stage">
                            <div className="mx-auto w-[328px] rounded-[38px] bg-[#0b0d10] p-[11px] shadow-2xl">
                                <div className="relative flex h-[596px] flex-col overflow-hidden rounded-[28px] bg-[#cdd9e3]">
                                    <div className="flex h-[30px] shrink-0 items-center justify-between bg-[#527da3] px-[22px] text-[12px] font-semibold text-white">
                                        <span>9:41</span>
                                        <span className="text-[11px]">Telegram</span>
                                    </div>
                                    <div className="flex shrink-0 items-center gap-[11px] bg-[#527da3] px-3.5 py-2 text-white">
                                        <ChevronLeft className="h-5 w-5 shrink-0" />
                                        <img
                                            src={BOTFATHER_PFP}
                                            alt="BotFather"
                                            className="h-[38px] w-[38px] shrink-0 rounded-full object-cover"
                                        />
                                        <div className="min-w-0 flex-1">
                                            <div className="flex items-center gap-1.5 text-[14.5px] font-semibold">
                                                BotFather <VerifiedTick />
                                            </div>
                                            <div className="text-[11.5px] text-white/75">bot</div>
                                        </div>
                                    </div>
                                    <div
                                        ref={msgRef}
                                        className="flex flex-1 flex-col gap-2 overflow-y-auto bg-[#cdd9e3] px-3 py-3.5"
                                    >
                                        <div className="mb-0.5 self-center rounded-full bg-white/55 px-2.5 py-0.5 text-[11px] text-[#5c6b7a]">
                                            today
                                        </div>

                                        <div className={inBubble + " tg-msg"}>
                                            <b className="text-[#1f6fb2]">BotFather</b>
                                            {"\n"}I can help you create and manage Telegram bots.
                                        </div>

                                        {step >= 1 && (
                                            <div className={outBubble + " tg-msg font-mono"}>/start</div>
                                        )}
                                        {typing && step === 1 && <Typing />}
                                        {step >= 1 && !(typing && step === 1) && (
                                            <div className={inBubble + " tg-msg"}>
                                                You can control me by sending these commands:{"\n\n"}
                                                <span className="text-[#1f6fb2]">/newbot</span> - create a new bot{"\n"}
                                                <span className="text-[#1f6fb2]">/mybots</span> - edit your bots{"\n"}
                                                <span className="text-[#1f6fb2]">/setname</span> - change a bot&apos;s name
                                            </div>
                                        )}

                                        {step >= 2 && (
                                            <div className={outBubble + " tg-msg font-mono"}>/newbot</div>
                                        )}
                                        {typing && step === 2 && <Typing />}
                                        {step >= 2 && !(typing && step === 2) && (
                                            <div className={inBubble + " tg-msg"}>
                                                Alright, a new bot. How are we going to call it? Please choose a name for your bot.
                                            </div>
                                        )}

                                        {step >= 3 && <div className={outBubble + " tg-msg"}>{BOT_NAME}</div>}
                                        {typing && step === 3 && <Typing />}
                                        {step >= 3 && !(typing && step === 3) && (
                                            <div className={inBubble + " tg-msg"}>
                                                Good. Now let&apos;s choose a username for your bot. It must end in <b>bot</b>. Like this: TetrisBot or tetris_bot.
                                            </div>
                                        )}

                                        {step >= 4 && (
                                            <div className={outBubble + " tg-msg font-mono"}>{BOT_USER}</div>
                                        )}
                                        {typing && step === 4 && <Typing />}
                                        {step >= 4 && !(typing && step === 4) && (
                                            <div className={inBubble + " tg-msg"}>
                                                Done! Congratulations on your new bot. You will find it at t.me/{BOT_USER}.{"\n\n"}Use this token to access the HTTP API:
                                                <div className="mt-2 break-all rounded-lg border border-gray-200 bg-gray-100 px-2.5 py-2 font-mono text-[11.5px] leading-snug text-[#0f172a]">
                                                    {SAMPLE_TOKEN}
                                                </div>
                                                <button
                                                    onClick={copyToken}
                                                    className="mt-2 inline-flex items-center gap-1.5 rounded-md px-3 py-1.5 text-[12px] font-semibold text-white"
                                                    style={{ background: copied ? "#16a34a" : "#229ED9" }}
                                                >
                                                    {copied ? "✓ Copied" : "Copy token"}
                                                </button>
                                                <div className="mt-1.5 text-[11.5px] text-gray-500">
                                                    Keep your token secure and store it safely, it can be used by anyone to control your bot.
                                                </div>
                                            </div>
                                        )}
                                    </div>
                                    <div className="flex shrink-0 items-center gap-2.5 border-t border-gray-200 bg-white px-3 py-2.5">
                                        <Paperclip className="h-5 w-5 shrink-0 text-slate-400" />
                                        <div
                                            className={`min-w-0 flex-1 truncate text-[13.5px] ${
                                                composerMuted ? "text-slate-400" : "text-[#1a1a1a]"
                                            } ${composerMono ? "font-mono" : ""}`}
                                        >
                                            {composerText}
                                        </div>
                                        <button
                                            onClick={composerDisabled ? undefined : advance}
                                            disabled={composerDisabled}
                                            className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full disabled:bg-slate-300"
                                            style={composerDisabled ? undefined : { background: "#229ED9" }}
                                        >
                                            <TgPlane className="h-[17px] w-[17px]" />
                                        </button>
                                    </div>
                                </div>
                            </div>
                            <div className="mt-3 text-center text-[11.5px] font-medium text-gray-400">
                                Live preview. This is what happens in Telegram.
                            </div>
                        </div>
                    )}

                    {step === 5 && (
                        <div className="tg-stage">
                            <div className="overflow-hidden rounded-xl border border-gray-200 bg-white shadow-sm">
                                <div className="flex items-center gap-2.5 border-b border-gray-100 px-[18px] py-[15px]">
                                    <div className="flex h-[30px] w-[30px] shrink-0 items-center justify-center rounded-lg bg-[#229ED9]">
                                        <TgPlane className="h-[17px] w-[17px]" />
                                    </div>
                                    <div>
                                        <div className="font-serif text-[18px] font-semibold leading-tight text-gray-900">
                                            Telegram bot
                                        </div>
                                        <div className="text-[12px] text-gray-400">
                                            Stored locally on your machine
                                        </div>
                                    </div>
                                </div>
                                <div className="p-[18px]">
                                    {!connected ? (
                                        <>
                                            <label className="mb-1.5 block text-[12px] font-semibold text-gray-500">
                                                HTTP API token
                                            </label>
                                            <div className="flex gap-2">
                                                <input
                                                    value={token}
                                                    onChange={(e) => {
                                                        setToken(e.target.value);
                                                        setConnected(false);
                                                    }}
                                                    placeholder="123456789:AA..."
                                                    className="min-w-0 flex-1 rounded-[9px] border bg-white px-2.5 py-2.5 font-mono text-[12.5px] text-[#0f172a] outline-none"
                                                    style={{
                                                        borderColor:
                                                            token.length === 0
                                                                ? "#e5e7eb"
                                                                : valid
                                                                  ? "#bbf7d0"
                                                                  : "#fecaca",
                                                    }}
                                                />
                                                <button
                                                    onClick={() => {
                                                        setToken(SAMPLE_TOKEN);
                                                        setConnected(false);
                                                    }}
                                                    className="shrink-0 rounded-[9px] border border-gray-200 bg-white px-3.5 text-[12.5px] font-semibold text-gray-700"
                                                >
                                                    Paste
                                                </button>
                                            </div>
                                            <div className="mt-2.5 min-h-[18px]">
                                                {valid && (
                                                    <div className="flex items-center gap-1.5 text-[12px] text-green-700">
                                                        <Check className="h-3.5 w-3.5" /> Valid token format. Ready to connect.
                                                    </div>
                                                )}
                                                {token.length > 0 && !valid && (
                                                    <div className="flex items-center gap-1.5 text-[12px] text-red-600">
                                                        <AlertCircle className="h-3.5 w-3.5" /> That does not look like a BotFather token.
                                                    </div>
                                                )}
                                            </div>
                                            <button
                                                onClick={saveToken}
                                                disabled={!valid}
                                                className="mt-3.5 w-full rounded-[9px] py-2.5 text-[13.5px] font-semibold text-white transition disabled:cursor-not-allowed disabled:bg-gray-200 disabled:text-gray-400"
                                                style={valid ? { background: ACCENT } : undefined}
                                            >
                                                Save and connect
                                            </button>
                                        </>
                                    ) : (
                                        <>
                                            <div className="tg-msg flex items-center gap-3 rounded-[11px] border border-green-200 bg-green-50/60 p-3.5">
                                                <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-full bg-gradient-to-br from-[#3a9bdc] to-[#1c5d8f]">
                                                    <TgPlane className="h-[21px] w-[21px]" />
                                                </div>
                                                <div className="flex-1">
                                                    <div className="text-[14px] font-semibold text-gray-900">
                                                        @{BOT_USER}
                                                    </div>
                                                    <div className="mt-0.5 flex items-center gap-1.5 text-[12px] text-green-700">
                                                        <span className="h-1.5 w-1.5 rounded-full bg-green-600" />
                                                        Token saved, verified with Telegram
                                                    </div>
                                                </div>
                                                <button
                                                    onClick={() => setConnected(false)}
                                                    className="text-[12px] font-semibold text-red-600"
                                                >
                                                    Disconnect
                                                </button>
                                            </div>
                                            <div className="mt-3 text-[13px] leading-[1.55] text-gray-500">
                                                One thing left. Donna only answers while your laptop is open and Mike is running. Continue to start it.
                                            </div>
                                        </>
                                    )}
                                </div>
                            </div>
                            <div className="mt-3.5 flex items-start gap-2.5 rounded-[11px] border border-gray-200 bg-gray-50 px-3.5 py-3">
                                <Lock className="mt-0.5 h-4 w-4 shrink-0 text-gray-400" />
                                <div className="text-[12.5px] leading-snug text-gray-500">
                                    Your token never leaves this device. It is written to your local config and used only to talk to Telegram&apos;s API directly.
                                </div>
                            </div>
                        </div>
                    )}

                    {step === 6 && (
                        <div className="tg-stage">
                            <div className="overflow-hidden rounded-xl border border-[#1f2937] shadow-2xl">
                                <div className="flex items-center gap-2 bg-[#161b22] px-3.5 py-2.5">
                                    <span className="h-[11px] w-[11px] rounded-full bg-[#ff5f57]" />
                                    <span className="h-[11px] w-[11px] rounded-full bg-[#febc2e]" />
                                    <span className="h-[11px] w-[11px] rounded-full bg-[#28c840]" />
                                    <span className="-ml-10 flex-1 text-center font-mono text-[12px] text-[#8b949e]">
                                        mike: backend
                                    </span>
                                    <span
                                        className="inline-flex items-center gap-1.5 rounded-full px-2.5 py-0.5 text-[11px] font-semibold"
                                        style={{
                                            background: online ? "rgba(63,185,80,.15)" : "rgba(248,81,73,.15)",
                                            color: online ? "#3fb950" : "#f85149",
                                        }}
                                    >
                                        <span
                                            className="h-1.5 w-1.5 rounded-full"
                                            style={{ background: online ? "#3fb950" : "#f85149" }}
                                        />
                                        {online ? "Online" : "Offline"}
                                    </span>
                                </div>
                                <div
                                    ref={termRef}
                                    className="h-[260px] overflow-y-auto bg-[#0d1117] px-4 py-3.5 font-mono text-[12.5px] leading-[1.7] text-[#c9d1d9]"
                                >
                                    {termLines.map((ln, i) => (
                                        <div key={i} className={ln.cls}>
                                            {ln.text}
                                        </div>
                                    ))}
                                    {termLines.length === 0 && (
                                        <div className="text-[#6e7681]">
                                            ${" "}
                                            <span
                                                className="inline-block h-[15px] w-2 align-middle bg-[#3fb950]"
                                                style={{ animation: "tg-blink 1.1s step-end infinite" }}
                                            />
                                        </div>
                                    )}
                                </div>
                            </div>
                            <div className="mt-3.5 flex gap-2.5">
                                <button
                                    onClick={runBackend}
                                    disabled={running || booted}
                                    className="rounded-[9px] px-4 py-2.5 text-[13px] font-semibold text-white transition disabled:cursor-default disabled:bg-gray-200 disabled:text-gray-400"
                                    style={running || booted ? undefined : { background: ACCENT }}
                                >
                                    {running ? "Starting..." : booted ? "Running" : "Run the command"}
                                </button>
                                <button
                                    onClick={toggleLaptop}
                                    className="rounded-[9px] border border-gray-200 bg-white px-4 py-2.5 text-[13px] font-semibold text-gray-700"
                                >
                                    {online ? "Close the laptop lid" : "Reopen laptop"}
                                </button>
                            </div>
                            <div
                                className="mt-4 rounded-xl border p-4 transition"
                                style={{
                                    borderColor: online ? "#bbf7d0" : "#fecaca",
                                    background: online ? "rgba(22,163,74,.05)" : "rgba(220,38,38,.04)",
                                }}
                            >
                                <div className="flex items-center gap-2.5">
                                    <span
                                        className="h-2.5 w-2.5 rounded-full"
                                        style={{
                                            background: online ? "#3fb950" : "#f85149",
                                            animation: online ? "tg-pulse 1.8s infinite" : undefined,
                                        }}
                                    />
                                    <span
                                        className="text-[14px] font-semibold"
                                        style={{ color: online ? "#15803d" : "#b42318" }}
                                    >
                                        {online ? "Donna is online" : "Donna is offline"}
                                    </span>
                                </div>
                                <div className="mt-1.5 text-[13px] leading-[1.55] text-gray-500">
                                    {online
                                        ? "Donna is active as long as your laptop is open. She is answering messages on Telegram right now, running entirely on your machine. Try closing the laptop lid to see what happens."
                                        : "Donna runs on your own machine, so she is only active while your laptop is open and the command is running. Reopen your laptop and run the command again to bring her back online."}
                                </div>
                            </div>
                        </div>
                    )}
                </div>
            </div>

            {/* What Donna can do */}
            <div className="mt-12 border-t border-gray-100 pt-8">
                <h3 className="font-serif text-[22px] font-semibold text-gray-900">
                    What Donna can do
                </h3>
                <p className="mb-5 mt-0.5 text-[13.5px] text-gray-500">
                    Once connected, here is what you can do from any Telegram chat.
                </p>
                <div className="grid grid-cols-1 items-stretch gap-4 md:grid-cols-3">
                    <Capability
                        icon={<KeyRound className="h-4 w-4" />}
                        title="Log in each session"
                        body="Donna acts as you, so she asks you to sign in first. Send /login with your PIN and she syncs to your laptop, then deletes the PIN message for safety."
                    >
                        <MiniOut mono>/login ••••</MiniOut>
                        <MiniIn>Logged in. I am now acting as you and syncing with your laptop.</MiniIn>
                    </Capability>
                    <Capability
                        icon={<History className="h-4 w-4" />}
                        title="Resurface your chats"
                        body="Continue a conversation you started on your laptop. Send /chats, tap one from the list, and pick up right where you left off."
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
                            <span className="font-mono text-gray-500"># AFFIDAVIT ...</span>
                        </MiniIn>
                    </Capability>
                </div>
                <div className="mt-5 flex items-center gap-2 text-[14px] text-gray-700">
                    <TgPlane className="h-[17px] w-[17px]" filled="#229ED9" />
                    <span className="font-semibold text-gray-900">And much more.</span> Try it out :)
                </div>
            </div>
        </div>
    );
}

/* ── small presentational helpers ──────────────────────────────── */

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
        <div className="flex flex-col rounded-xl border border-gray-200 bg-white p-[17px]">
            <div className="mb-2.5 flex items-center gap-2.5">
                <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-gray-100 text-gray-900">
                    {icon}
                </div>
                <div className="text-[14.5px] font-semibold text-gray-900">{title}</div>
            </div>
            <p className="mb-3 flex-1 text-[12.5px] leading-snug text-gray-500">{body}</p>
            <div className="flex flex-col gap-1.5 rounded-[10px] bg-[#eef1f5] p-2.5">{children}</div>
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

/* ── inline icons (swap for lucide-react if preferred) ─────────── */

function TgPlane({ className, filled = "#fff" }: { className?: string; filled?: string }) {
    return (
        <svg viewBox="0 0 24 24" className={className} fill={filled} aria-hidden>
            <path d="M21.5 4.5 2.7 11.9c-1 .4-1 1.8 0 2.1l4.6 1.4 1.8 5.6c.3.9 1.4 1.1 2 .4l2.5-2.6 4.6 3.4c.7.5 1.7.1 1.9-.7L23.7 5.6c.2-1-.8-1.8-1.7-1.4Z" />
        </svg>
    );
}
function ChevronLeft({ className }: { className?: string }) {
    return (
        <svg viewBox="0 0 24 24" className={className} fill="none" stroke="#fff" strokeWidth={2.4} strokeLinecap="round" strokeLinejoin="round" aria-hidden>
            <polyline points="15 18 9 12 15 6" />
        </svg>
    );
}
function VerifiedTick() {
    return (
        <svg viewBox="0 0 24 24" className="h-3.5 w-3.5 shrink-0" aria-hidden>
            <circle cx="12" cy="12" r="11" fill="#3da5e8" />
            <path d="M7 12.4l3.2 3.1L17 8.7" fill="none" stroke="#fff" strokeWidth={2.2} strokeLinecap="round" strokeLinejoin="round" />
        </svg>
    );
}
function Paperclip({ className }: { className?: string }) {
    return (
        <svg viewBox="0 0 24 24" className={className} fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" aria-hidden>
            <path d="m21.4 11.1-9.2 9.2a5 5 0 0 1-7.1-7.1l9.2-9.2a3.3 3.3 0 0 1 4.7 4.7l-9.2 9.2a1.7 1.7 0 0 1-2.4-2.4l8.5-8.5" />
        </svg>
    );
}
function Check({ className }: { className?: string }) {
    return (
        <svg viewBox="0 0 24 24" className={className} fill="none" stroke="#16a34a" strokeWidth={2.4} strokeLinecap="round" strokeLinejoin="round" aria-hidden>
            <polyline points="20 6 9 17 4 12" />
        </svg>
    );
}
function AlertCircle({ className }: { className?: string }) {
    return (
        <svg viewBox="0 0 24 24" className={className} fill="none" stroke="#dc2626" strokeWidth={2.4} strokeLinecap="round" strokeLinejoin="round" aria-hidden>
            <circle cx="12" cy="12" r="9" />
            <line x1="12" y1="8" x2="12" y2="13" />
            <line x1="12" y1="16" x2="12" y2="16" />
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
