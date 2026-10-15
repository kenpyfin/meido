---
name: tavily-full
description: Tavily APIs beyond built-in web_search — extract, crawl, map, research (async), and usage reporting via the bundled Python CLI.
when_to_use: |
  Use after activating when the user needs deep page extraction, site mapping, multi-page crawls, or long-form research beyond a simple web_search call. Requires TAVILY_API_KEY in the environment (same as web_search).
  Skip when web_search + web_fetch are enough for shallow discovery.
compatibility:
  os: [linux, darwin, windows]
  deps: []
---

# Tavily (full API)

The bot’s built-in **`web_search`** tool already calls Tavily **`POST /search`** when `TAVILY_API_KEY` is set. This skill covers the **other** Tavily endpoints so you can use the platform end-to-end.

Official reference: [Tavily documentation](https://docs.tavily.com/) and [OpenAPI](https://docs.tavily.com/documentation/api-reference/openapi.json).

## Credentials

Set **`TAVILY_API_KEY`** in the **main bot `.env`** (recommended — same variable as `web_search`).

Optional: copy `.env.example` to `.env` in this skill folder **only** if you need a key that is not in the process environment.

## CLI (bash)

From the repository root, use either path (they are the same skill):

```bash
python3 workspace/skills/tavily-full/tavily_tool.py usage
# or
python3 builtin_skills/tavily-full/tavily_tool.py usage
```

If `builtin_skills/` is not next to your `workspace/` data root, set **`FINALLY_A_VALUE_BOT_BUILTIN_SKILLS`** to the absolute directory that contains `tavily-full/`, then point `python3` at `…/tavily-full/tavily_tool.py` there.

### Commands

| Command | Tavily endpoint | When to use |
|--------|-------------------|-------------|
| `usage` | `GET /usage` | Check credits / plan limits before heavy crawl or research. |
| `search` | `POST /search` | Parity with `web_search` or scripted searches from shell. Prefer **`web_search`** in chat. |
| `extract` | `POST /extract` | Clean **markdown/text** from known URLs (batch up to API limits). Optional `query` reranks chunks. |
| `crawl` | `POST /crawl` | **Graph crawl** from a base URL with optional natural-language `instructions`, depth/breadth/limit. |
| `map` | `POST /map` | **Discover URLs** on a site (sitemap-style) without full extraction. |
| `research` | `POST /research` | Start an **async research** job; returns `request_id`. |
| `research-wait` | `GET /research/{request_id}` | Poll until **completed** / **failed** or timeout. |

#### Examples

```bash
TV=workspace/skills/tavily-full/tavily_tool.py

# Account / credits
python3 "$TV" usage

# Extract two pages as markdown
python3 "$TV" extract \
  --urls "https://example.com/a,https://example.com/b" --format markdown

# Map a docs site (discover URLs)
python3 "$TV" map --url "https://docs.tavily.com" --limit 80

# Crawl with instructions (higher credit use — see Tavily pricing)
python3 "$TV" crawl \
  --url "https://docs.tavily.com" \
  --instructions "Find pages about the Python SDK" \
  --limit 30

# Deep research: start, then poll
python3 "$TV" research \
  --input "Compare NAFS vs NFRC for commercial aluminum windows in California" \
  --model mini

python3 "$TV" research-wait \
  --request-id "<paste request_id from previous command>" \
  --interval 5 --max-wait 600
```

## Agent workflow (recommended)

1. **Quick facts / links:** use **`web_search`** (Tavily search is already wired there).
2. **You already have URLs** and need clean text/tables: **`extract`**.
3. **Explore everything under a domain** (docs portal, vendor site): **`map`** first to list URLs, then **`extract`** on the best URLs (cheaper than blind crawl).
4. **Multi-page narrative report:** **`research`** + **`research-wait`** (or poll manually with `research-wait`).
5. **Before large jobs:** run **`usage`** once to avoid surprise limit errors.

## Output

The CLI prints **JSON** to stdout (or a small JSON error object on failure). Parse `results`, `content`, `sources`, `status`, and `detail.error` fields per Tavily’s responses.

## Notes

- **Credits** differ by endpoint and options (`search_depth`, `instructions` on crawl/map, `advanced` extract, research model). See [API credits](https://docs.tavily.com/documentation/api-credits).
- **`research --stream`**: requests streaming mode; this script does not consume SSE — use non-stream + `research-wait`, or integrate streaming separately.
- For generic **fetch a single URL** without Tavily, the bot’s **`web_fetch`** tool may still be enough; use **`extract`** when you need Tavily’s cleaned markdown / chunking / batch behavior.
