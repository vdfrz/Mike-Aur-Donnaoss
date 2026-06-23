#!/usr/bin/env bash
#
# Mike aur Donna — one-command installer (macOS / Linux).
#
# Installs prerequisites, downloads the pdfium library, builds the desktop
# app, and installs it so it shows up in your applications / launcher.
# After this runs once, just search "Mike" to open it — no terminals needed.
#
#   ./scripts/setup.sh
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
OS="$(uname -s)"
ARCH="$(uname -m)"
echo "==> Mike aur Donna installer"
echo "    repo: $ROOT"
echo "    os:   $OS ($ARCH)"

note() { printf '\n==> %s\n' "$1"; }
fail() { printf '\n!! %s\n' "$1" >&2; exit 1; }

# 1. Rust -------------------------------------------------------------------
if ! command -v cargo >/dev/null 2>&1; then
  note "Installing Rust (rustup)…"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  # shellcheck disable=SC1091
  source "$HOME/.cargo/env"
fi

# 2. Node.js (needed to build the frontend) ---------------------------------
command -v npm >/dev/null 2>&1 || \
  fail "Node.js (v18+) is required. Install it from https://nodejs.org/ and re-run."

# 3. System libraries for OCR / PDF / DOCX ----------------------------------
# pandoc is the optional, redline-path-only docx→markdown converter (see
# docs/DOCX.md). The desktop app bundles its own copy under src-tauri/pandoc/,
# but installing it here puts a `pandoc` on PATH so the server/dev surface
# (cargo run, the bot backend) also gets the higher-fidelity reader. Absent
# pandoc, the pure-Rust extractor still handles every redline — just flatter.
if [ "$OS" = "Darwin" ]; then
  if command -v brew >/dev/null 2>&1; then
    note "Installing system libraries (tesseract, leptonica, pkg-config, pandoc)…"
    brew install tesseract leptonica pkg-config pandoc >/dev/null || true
  else
    echo "!! Homebrew not found — the OCR build step may fail."
    echo "   Install Homebrew from https://brew.sh then run:"
    echo "   brew install tesseract leptonica pkg-config pandoc"
  fi
elif [ "$OS" = "Linux" ] && command -v apt-get >/dev/null 2>&1; then
  note "Installing system libraries (tesseract, leptonica, pkg-config, pandoc)…"
  sudo apt-get update -y && \
    sudo apt-get install -y libtesseract-dev libleptonica-dev pkg-config pandoc || true
fi

# 4. Tauri CLI --------------------------------------------------------------
if ! cargo tauri --version >/dev/null 2>&1; then
  note "Installing Tauri CLI…"
  cargo install tauri-cli --version '^2' --locked
fi

# 5. pdfium (bundled into the app) ------------------------------------------
PDFIUM_DIR="$ROOT/src-tauri/pdfium"
mkdir -p "$PDFIUM_DIR"
case "$OS" in
  Darwin) [ "$ARCH" = "arm64" ] && ASSET="pdfium-mac-arm64" || ASSET="pdfium-mac-x64"
          LIB="libpdfium.dylib"; INNER="lib/$LIB" ;;
  Linux)  ASSET="pdfium-linux-x64"; LIB="libpdfium.so"; INNER="lib/$LIB" ;;
  *)      fail "Unsupported OS: $OS (use scripts/setup.ps1 on Windows)" ;;
esac
if [ ! -f "$PDFIUM_DIR/$LIB" ]; then
  note "Downloading pdfium ($ASSET)…"
  TMP="$(mktemp -d)"
  curl -L "https://github.com/bblanchon/pdfium-binaries/releases/latest/download/${ASSET}.tgz" \
    -o "$TMP/pdfium.tgz"
  tar -xzf "$TMP/pdfium.tgz" -C "$TMP"
  cp "$TMP/$INNER" "$PDFIUM_DIR/$LIB"
  rm -rf "$TMP"
fi

# 6. Frontend dependencies --------------------------------------------------
note "Installing frontend dependencies…"
npm --prefix ./frontend install

# 7. Build the desktop app --------------------------------------------------
note "Building the desktop app (first build takes a few minutes)…"
cargo tauri build

# 8. Install it -------------------------------------------------------------
if [ "$OS" = "Darwin" ]; then
  # Tauri v2 writes the bundle to the workspace-root target/ when the crate is a
  # workspace member, but to src-tauri/target/ otherwise — search both.
  APP="$(/usr/bin/find target/release/bundle/macos src-tauri/target/release/bundle/macos -maxdepth 1 -name '*.app' 2>/dev/null | head -n1)"
  [ -n "$APP" ] || fail "Build finished but no .app was found under target/release/bundle/macos/ or src-tauri/target/release/bundle/macos/"
  note "Installing $(basename "$APP") to /Applications…"
  rm -rf "/Applications/$(basename "$APP")"
  cp -R "$APP" /Applications/
  open "/Applications/$(basename "$APP")"
  echo ""
  echo "==> Done. Search \"Mike\" in Spotlight to open it any time."
else
  echo ""
  echo "==> Build complete. Your installer is under target/release/bundle/ (or src-tauri/target/release/bundle/)."
  echo "    Run it to install, then launch Mike from your applications menu."
fi
