---
name: index-vault
description: Index ORIGIN vault markdown into ChromaDB for semantic search (run before search_vault or after large vault edits).
when_to_use: |
  Use when the vault must be (re)indexed for search_vault, after bulk vault imports, or when embeddings/collection are stale. Requires python3 and the skill venv per setup_vault_env.sh; VAULT_EMBEDDING_SERVER_URL must be set (skill .env or process env).
license: MIT
compatibility:
  os:
    - darwin
    - linux
  deps:
    - python3
---

# Index Vault

Use this skill to index (or re-index) the ORIGIN vault into the ChromaDB vector database. This must be run before `search_vault` will return results, and should be re-run whenever vault content changes.

## How it works

The bundled `index_vault.py` script walks the ORIGIN vault directory for `*.md` files, chunks them into overlapping segments, generates embeddings via the embedding server, and upserts them into a local ChromaDB collection.

### Prerequisites

Same as the search-vault skill — a Python venv with `chromadb` and `openai`:

```bash
bash builtin_skills/index-vault/setup_vault_env.sh
```

`index_vault.py` loads `.env` from **this skill directory** (`index-vault/.env`). Set `VAULT_EMBEDDING_SERVER_URL` there (**required**; no default). When spawned by the bot, existing process environment variables are not overridden by dotenv.

### Environment variables


| Variable                     | Description                                      | Default / notes        |
| ---------------------------- | ------------------------------------------------ | ---------------------- |
| `VAULT_ORIGIN_VAULT_PATH`    | Path to the ORIGIN vault (relative to workspace) | `shared/ORIGIN`        |
| `VAULT_EMBEDDING_SERVER_URL` | Embedding API base URL                           | **Required** (no default) |
| `VAULT_VECTOR_DB_PATH`       | ChromaDB persistent storage path                 | `shared/vault_db`      |
| `VAULT_VECTOR_DB_COLLECTION` | ChromaDB collection name                         | `origin_vault`         |


### Usage

Run the indexing script directly:

```bash
shared/.venv-vault/bin/python builtin_skills/index-vault/index_vault.py
```

Or if the system Python has chromadb installed:

```bash
python3 builtin_skills/index-vault/index_vault.py
```

### Scheduling

Indexing is automatically scheduled to run every 6 hours. The agent receives a prompt to run this script and report the result. You can also trigger it manually at any time by asking the agent to re-index the vault.

### Chunking strategy

- Chunk size: 1000 characters
- Overlap: 200 characters
- Whitespace is normalized before chunking
- Each chunk is stored with its source file path as metadata

