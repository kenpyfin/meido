---
name: search-vault
description: Semantic search over the ORIGIN vault via ChromaDB and the embedding server (query_vault.py).
when_to_use: |
  Use when the user wants meaning-based retrieval from their Obsidian/ORIGIN vault after indexing. Prefer the built-in search_vault tool path described in this skill; do not grep the vault for semantic questions. Requires python3 and VAULT_EMBEDDING_SERVER_URL.
license: MIT
compatibility:
  os:
    - darwin
    - linux
  deps:
    - python3
---

# Search Vault

Use this skill when the user asks to search their knowledge base, vault, notes, or ORIGIN documents for information by meaning (semantic search).

## How it works

The `search_vault` tool is registered automatically when vault config is present. It uses a local ChromaDB vector database and an embedding server to perform semantic similarity search over indexed markdown documents.

### Prerequisites

A Python venv with `chromadb` and `openai` must exist. Run the bundled setup script to create it:

```bash
# From repository root:
bash builtin_skills/search-vault/setup_vault_env.sh
```

This creates `shared/.venv-vault/` with the required packages.

`query_vault.py` loads `.env` from **this skill directory** (`search-vault/.env`). Set `VAULT_EMBEDDING_SERVER_URL` there (required; there is no default URL). When the script is spawned by the bot, variables already present in the process environment are left unchanged (dotenv does not override them).

An embedding server must be running at that URL. Any OpenAI-compatible embedding API works (e.g. llama.cpp with `--embedding`).

### Environment variables


| Variable                     | Description                      | Default / notes           |
| ---------------------------- | -------------------------------- | ------------------------- |
| `VAULT_EMBEDDING_SERVER_URL` | Embedding API base URL           | **Required** (no default) |
| `VAULT_VECTOR_DB_PATH`       | ChromaDB persistent storage path | `shared/vault_db`         |
| `VAULT_VECTOR_DB_COLLECTION` | ChromaDB collection name         | `origin_vault`            |


### Usage

The `search_vault` tool is the primary interface. Just call it with a natural language query:

```
search_vault(query="machine learning concepts", n_results=5)
```

If the tool is not available, you can run the bundled script directly:

```bash
shared/.venv-vault/bin/python builtin_skills/search-vault/query_vault.py "search terms" [n_results]
```

### Troubleshooting

- **"No relevant results"**: The vault may not be indexed yet. Run the index-vault skill first.
- **Embedding server errors**: Verify the embedding server is running at the configured URL.
- **ChromaDB import errors**: Re-run `setup_vault_env.sh` to recreate the venv.

