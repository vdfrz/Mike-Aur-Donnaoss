#!/usr/bin/env node
// Tauri's static export requires `dynamicParams = false` on every dynamic
// route, but web mode needs `true` so hard-loading a real URL doesn't 404.
// Next.js only accepts a literal boolean there, so flip the flag for the
// duration of the build and restore the sources afterwards.
const { execSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

const appDir = path.join(__dirname, "..", "src", "app");
const FLAG_TRUE = "export const dynamicParams = true;";
const FLAG_FALSE = "export const dynamicParams = false;";

const pages = [];
(function walk(dir) {
    for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
        const p = path.join(dir, entry.name);
        if (entry.isDirectory()) walk(p);
        else if (entry.name === "page.tsx") pages.push(p);
    }
})(appDir);

const touched = new Map();
for (const p of pages) {
    const src = fs.readFileSync(p, "utf8");
    if (src.includes(FLAG_TRUE)) {
        touched.set(p, src);
        fs.writeFileSync(p, src.replace(FLAG_TRUE, FLAG_FALSE));
    }
}
console.log(`[build:tauri] dynamicParams true -> false in ${touched.size} page(s)`);

try {
    execSync("npx next build", {
        stdio: "inherit",
        cwd: path.join(__dirname, ".."),
        env: { ...process.env, TAURI_BUILD: "1" },
    });
} finally {
    for (const [p, src] of touched) fs.writeFileSync(p, src);
    console.log("[build:tauri] restored dynamicParams flags");
}
