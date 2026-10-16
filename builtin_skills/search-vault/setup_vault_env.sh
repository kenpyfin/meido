#!/bin/sh
# Create vault Python venv with ChromaDB and embedding support.
set -e
WORKSPACE="${WORKSPACE_DIR:-${FINALLY_A_VALUE_BOT_WORKSPACE_DIR:-$(pwd)}}"
VENV_DIR="$WORKSPACE/shared/.venv-vault"

echo "Creating vault venv at $VENV_DIR"
python3 -m venv "$VENV_DIR"
"$VENV_DIR/bin/pip" install --quiet chromadb openai python-dotenv
echo "Done. Vault venv ready at $VENV_DIR"
