#!/usr/bin/env bash
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export LD_LIBRARY_PATH="$DIR/lib:$DIR:${LD_LIBRARY_PATH:-}"
exec "$DIR/pdfview" "$@"
