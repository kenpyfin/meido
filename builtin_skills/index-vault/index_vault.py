#!/usr/bin/env python3
"""Built-in vault index script. Index ORIGIN vault markdown files into ChromaDB for semantic search."""
import json
import os
import re
import sys
from datetime import datetime, timezone
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
print(f"DEBUG: DB_PATH={DB_PATH}")
VAULT_PATH = _resolve_under_workspace(
    os.environ.get("VAULT_ORIGIN_VAULT_PATH"),
    os.path.join("shared", "ORIGIN"),
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


def chunk_md(content: str, path: str, chunk_size: int = 2080, overlap: int = 400) -> list[tuple[str, str]]:
    """Split markdown into overlapping chunks."""
    content = re.sub(r"\s+", " ", content.strip())
    chunks = []
    start = 0
    while start < len(content):
        end = start + chunk_size
        chunk = content[start:end]
        if chunk.strip():
            chunks.append((chunk, path))
        start = end - overlap
    return chunks


def chunk_id(relative_path: str, chunk_index: int) -> str:
    """Stable id for one chunk. Reindexing the same file keeps the same ids."""
    return f"{relative_path}#{chunk_index}"


def index_vault() -> int:
    vault = Path(VAULT_PATH)
    if not vault.exists():
        print(f"Vault path does not exist: {vault}", file=sys.stderr)
        return 1

    docs, metas, ids = [], [], []
    for md in vault.rglob("*.md"):
        try:
            text = md.read_text(encoding="utf-8", errors="replace")
            rel = str(md.relative_to(vault)).replace("\\", "/")
            for chunk_index, (chunk, _) in enumerate(chunk_md(text, rel)):
                docs.append(chunk)
                metas.append({"path": rel, "source": rel})
                ids.append(chunk_id(rel, chunk_index))
        except Exception as e:
            print(f"Skip {md}: {e}", file=sys.stderr)

    if not docs:
        print("No documents to index.")
        return 0

    print(f"Prepared {len(docs)} chunks. Starting upsert...")
    try:
        try:
            max_batch = int(client.get_max_batch_size())
        except Exception:
            max_batch = 5461
        # Stay under ChromaDB's hard limit (and leave headroom for embedding batches).
        batch_size = max(1, min(max_batch, 5000))
        total = len(docs)
        current_ids = set(ids)
        for start in range(0, total, batch_size):
            end = min(start + batch_size, total)
            print(f"Upserting batch {start + 1}-{end} of {total}...")
            collection.upsert(
                documents=docs[start:end],
                metadatas=metas[start:end],
                ids=ids[start:end],
            )
        try:
            existing = collection.get(include=[])
            existing_ids = existing.get("ids") or []
            stale = [item for item in existing_ids if item not in current_ids]
            for start in range(0, len(stale), batch_size):
                collection.delete(ids=stale[start : start + batch_size])
            if stale:
                print(f"Removed {len(stale)} stale chunks")
        except Exception as e:
            print(f"Stale-chunk cleanup failed: {type(e).__name__}: {e}", file=sys.stderr)
            return 1
        meta_dir = Path(DB_PATH)
        meta_dir.mkdir(parents=True, exist_ok=True)
        meta_path = meta_dir / "index_meta.json"
        meta_path.write_text(
            json.dumps(
                {
                    "generated_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
                    "chunk_count": total,
                    "collection": COLLECTION,
                }
            ),
            encoding="utf-8",
        )
        print(f"Successfully indexed {total} chunks from {vault}")
    except Exception as e:
        print(f"Upsert failed: {type(e).__name__}: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(index_vault())
