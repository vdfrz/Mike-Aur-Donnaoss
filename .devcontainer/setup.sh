#!/bin/bash
set -e

# Download pdfium for Linux x86_64
PDFIUM_DIR="libs/pdfium"
if [ ! -f "$PDFIUM_DIR/libpdfium.so" ]; then
  mkdir -p "$PDFIUM_DIR"
  echo "Downloading pdfium..."
  curl -L -o /tmp/pdfium.tgz \
    "https://github.com/nicholasgasior/pdfium-binaries/releases/download/chromium%2F6721/pdfium-linux-x64.tgz"
  tar -xzf /tmp/pdfium.tgz -C /tmp/pdfium-extract 2>/dev/null || true
  # The archive structure varies — find the .so and copy it
  mkdir -p /tmp/pdfium-extract
  tar -xzf /tmp/pdfium.tgz -C /tmp/pdfium-extract
  find /tmp/pdfium-extract -name "libpdfium.so" -exec cp {} "$PDFIUM_DIR/" \; 2>/dev/null || \
    find /tmp/pdfium-extract -name "lib*pdfium*" -exec cp {} "$PDFIUM_DIR/libpdfium.so" \; 2>/dev/null || true
  rm -rf /tmp/pdfium.tgz /tmp/pdfium-extract
  echo "pdfium installed to $PDFIUM_DIR"
fi

# Install pandoc — optional higher-fidelity docx→markdown reader for the
# redline path (see docs/DOCX.md). This is the non-Tauri backend surface, so
# there's no bundled binary here; a `pandoc` on PATH lets it use the structured
# reader. Non-fatal (|| true): the pure-Rust extractor covers every path
# without it, so a missing package never aborts the container build.
if ! command -v pandoc >/dev/null 2>&1; then
  echo "Installing pandoc..."
  sudo apt-get update -y && sudo apt-get install -y pandoc || true
fi

# Install frontend dependencies
cd frontend && npm install && cd ..

# Copy .env if not present
if [ ! -f .env ]; then
  cp .env.example .env
  echo ""
  echo "================================================"
  echo "  .env created from .env.example"
  echo "  You MUST edit .env and add:"
  echo "    - JWT_SECRET"
  echo "    - DEEPSEEK_API_KEY"
  echo "    - IK_API_KEY"
  echo "  See SETUP.md for details."
  echo "================================================"
  echo ""
fi
