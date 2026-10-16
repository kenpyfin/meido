#!/bin/sh
# Create Steel Python venv with steel-sdk and playwright (CDP client only).
set -e
WORKSPACE="${WORKSPACE_DIR:-${FINALLY_A_VALUE_BOT_WORKSPACE_DIR:-$(pwd)}}"
VENV_DIR="$WORKSPACE/shared/.venv-steel"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

echo "Creating Steel venv at $VENV_DIR"
python3 -m venv "$VENV_DIR"
"$VENV_DIR/bin/pip" install --quiet -r "$SCRIPT_DIR/requirements.txt"
echo "Done. Steel venv ready at $VENV_DIR"
echo "Note: playwright install chromium is NOT required — Steel hosts the browser."
