#!/usr/bin/env python3
"""Built-in vault query script. Semantically search the ORIGIN vault via ChromaDB."""
import os
import sys
from pathlib import Path

from dotenv import load_dotenv

SCRIPT_DIR = Path(__file__).parent.resolve()
load_dotenv(SCRIPT_DIR / ".env")

def _checkout_root() -> Path | None:
    for parent in (SCRIPT_DIR, *SCRIPT_DIR.parents):
        if (parent / "Cargo.toml").is_file() and (parent / "builtin_skills").is_dir():
            return parent
    return None

def _workspace_root() -> Path:
    """Workspace directory. A relative WORKSPACE_DIR is anchored at the checkout, not cwd."""
    raw = (
        os.environ.get("MEIDO_WORKSPACE_DIR")
        or os.environ.get("WORKSPACE_DIR")
        or os.environ.get("FINALLY_A_VALUE_BOT_WORKSPACE_DIR")
        or ""
    ).strip()
    if raw:
        candidate = Path(raw).expanduser()
        if candidate.is_absolute():
            return candidate
        checkout = _checkout_root()
        if checkout is not None:
            return (checkout / candidate).resolve()
        for parent in (SCRIPT_DIR, *SCRIPT_DIR.parents):
            resolved = (parent / candidate).resolve()
            if resolved.is_dir():
                return resolved
        return (Path.cwd() / candidate).resolve()
    for parent in (SCRIPT_DIR, *SCRIPT_DIR.parents):
        if parent.name == "workspace" and (parent / "shared").is_dir():
            return parent
        nested = parent / "workspace"
        if (nested / "shared").is_dir():
            return nested
    return Path.cwd()

def _resolve_under_workspace(configured: str | None, default_rel: str) -> str:
    """Absolute path. Relative values are joined onto the workspace root."""
    raw = (configured or "").strip() or default_rel
    path = Path(raw).expanduser()
    if path.is_absolute():
        return str(path)
    return str((_workspace_root() / path).resolve())

def _require_embed_openai_base() -> str:
    url = (os.environ.get("VAULT_EMBEDDING_SERVER_URL") or os.environ.get("VAULT_EMBED_URL") or "").strip()
    if not url:
        print(
            "error: VAULT_EMBEDDING_SERVER_URL or VAULT_EMBED_URL must be set (no default). "
            "Add `.env` beside this script or export the variable before running.",
            file=sys.stderr,
        )
        sys.exit(1)
    return url.rstrip("/") + "/v1" if not url.endswith("/v1") else url

DB_PATH = _resolve_under_workspace(
    os.environ.get("VAULT_VECTOR_DB_PATH") or os.environ.get("VAULT_DB_PATH"),
    os.path.join("shared", "vault_db"),
)
EMBED_URL = _require_embed_openai_base()
COLLECTION = os.environ.get("VAULT_VECTOR_DB_COLLECTION", "origin_vault")

import chromadb
from chromadb.utils import embedding_functions

class CustomLlamaEF(chromadb.EmbeddingFunction):
    def __init__(self, embed_url: str):
        self.embed_url = embed_url.rstrip("/") + "/embeddings"

    def __call__(self, input: chromadb.Documents) -> chromadb.Embeddings:
        import requests
        payload = {
            "model": "ignored",
            "input": input
        }
        response = requests.post(self.embed_url, json=payload)
        response.raise_for_status()
        data = response.json()
        return [d["embedding"] for d in data["data"]]

llama_ef = CustomLlamaEF(embed_url=EMBED_URL)

client = chromadb.PersistentClient(path=DB_PATH)
collection = client.get_or_create_collection(name=COLLECTION, embedding_function=llama_ef)


def query(text: str, n: int = 5) -> None:
    try:
        results = collection.query(query_texts=[text], n_results=n)
        if not results["documents"] or not results["documents"][0]:
            print("No relevant results found.")
            return
        for i, doc in enumerate(results["documents"][0]):
            meta = results["metadatas"][0][i] if results["metadatas"] else {}
            path = meta.get("path", meta.get("source", "unknown"))
            excerpt = doc[:2000] + "..." if len(doc) > 2000 else doc
            print(f"--- Result {i + 1} (Source: {path}) ---")
            print(excerpt)
            print()
    except Exception as e:
        print(f"Query error: {e}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    if len(sys.argv) > 1:
        n = int(sys.argv[2]) if len(sys.argv) > 2 else 5
        query(sys.argv[1], n=n)
    else:
        print("Usage: python3 query_vault.py \"search terms\" [n_results]")
