---
name: weather
description: Current weather and short forecasts via wttr.in over curl (no API key).
when_to_use: |
  Use when the user asks for current conditions, a quick forecast, or weather by city or region and a lightweight curl-based answer is acceptable.
  Not a substitute for aviation/marine alerts or paid hyperlocal APIs unless the user accepts wttr.in limitations.
license: Proprietary. LICENSE.txt has complete terms
compatibility:
  deps:
    - curl
---

# Weather

Use this skill for quick weather lookups without API keys.

## Current weather

```bash
curl -s "wttr.in/San+Francisco?format=3"
```

## Compact format

```bash
curl -s "wttr.in/San+Francisco?format=%l:+%c+%t+%h+%w"
```

## Multi-day forecast

```bash
curl -s "wttr.in/San+Francisco?m"
```

## Usage guidance

- URL-encode spaces with `+`.
- Use `?m` for metric and `?u` for US units.
- For ambiguous place names, clarify state/country first.
