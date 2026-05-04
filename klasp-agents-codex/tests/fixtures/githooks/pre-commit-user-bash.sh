#!/bin/bash
# Pre-commit hook hand-written by a contributor — no foreign tool
# fingerprint, no klasp marker. The W2 install path is expected to
# preserve every byte of this file and append klasp's managed block
# below it with a blank-line separator.

set -euo pipefail

echo "running project lint…"
make lint

# Reject commits with WIP markers in tracked source.
if git diff --cached --name-only -z | xargs -0 grep -l "WIP-do-not-merge" 2>/dev/null; then
    echo "refusing to commit: WIP marker detected" >&2
    exit 1
fi
