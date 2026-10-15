#!/home/ken/big_storage/projects/finally-a-value-bot/workspace/shared/.venv-vault/bin/python3
"""Built-in vault query script. Semantically search the ORIGIN vault via ChromaDB."""
import os
import sys
from pathlib import Path

from dotenv import load_dotenv

SCRIPT_DIR = Path(__file__).parent.resolve()
load_dotenv(SCRIPT_DIR / ".env")

def _default_vault_db_path() -> str:
    root = os.environ.get("WORKSPACE_DIR") or os.environ.get("FINALLY_A_VALUE_BOT_WORKSPACE_DIR") or os.getcwd()
    return os.path.abspath(os.path.join(root, "shared", "vault_db"))

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

DB_PATH = os.environ.get("VAULT_VECTOR_DB_PATH") or os.environ.get("VAULT_DB_PATH") or _default_vault_db_path()
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
