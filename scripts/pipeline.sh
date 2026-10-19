#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "=========================================================="
echo "          Verso Reader — Quality & CI Pipeline            "
echo "=========================================================="

cd "$REPO_DIR"

# 1. Format check
echo ""
echo "▶ [1/5] Checking code formatting (cargo fmt)..."
cargo fmt --check
echo "✓ Formatting clean."

# 2. Clippy linter
echo ""
echo "▶ [2/5] Running clippy linter (cargo clippy --all-targets -- -D warnings)..."
cargo clippy --all-targets -- -D warnings
echo "✓ Zero clippy warnings."

# 3. Unit and stress test suite
echo ""
echo "▶ [3/5] Running automated test suite (cargo test)..."
cargo test
echo "✓ All automated tests passed."

# 4. Optimized release build
echo ""
echo "▶ [4/5] Building optimized release binary (cargo build --release)..."
cargo build --release
echo "✓ Release binary built."

# 5. Distribution packaging
echo ""
echo "▶ [5/5] Packaging standalone distribution bundle..."
"$REPO_DIR/scripts/package.sh"
echo "✓ Distribution bundle packaged."

echo ""
echo "=========================================================="
echo "             🎉 PIPELINE PASSED SUCCESSFULLY!             "
echo "=========================================================="
echo "To test run locally:    ./target/release/verso-reader fixtures/MLE.pdf"
echo "To install to desktop:  bash scripts/install.sh"
echo "To share with friends:  dist/verso-reader-linux-x86_64.tar.gz"
echo "=========================================================="

