---
name: comfy-video-run
description: Submit, recover, and review ComfyUI video jobs with immutable input snapshots and verified per-job outputs. Use for video execution and delivery; does not draft creative briefs or install models.
---

# Comfy video runs

Updated 2026-09-13 UTC. Use this runner for new video jobs instead of copying a previous take's waiter. Scripts resolve relative to this skill folder; bot `run_skill_script` can invoke `scripts/video_run.py`.

## Run contract

- Keep each attempt in a new directory. `spec.json` declares the API graph, local image assets by LoadImage node, expected video output nodes, dimensions, frame count, and fps. The runner snapshots inputs and the actual submitted graph before queueing.
- A server success is **rendered**, a downloaded/probed video is **review_pending**, and a visually reviewed candidate is **reviewed**. None means user acceptance. Never call a candidate accepted until the user accepts that exact run.
- A missing history entry means unknown/pending, not failure. Submission transport errors are ambiguous; do not automatically resubmit. Inspect the queue/history for the recorded `client_id` first.
- Collect only the declared output nodes from the exact completed prompt ID. Never use a glob, latest file, old master, or last item in an arbitrary outputs list.
- The runner saves frame probes for inspection, not as proof of quality. Inspect the full sequence for pacing, jumps, framing, and identity; compare first/middle/end and detailed crops against the actual references. Review reports identify observed defects and separate them from causal hypotheses.
- Save Stage-1 and refined outputs separately when investigating refinement. Do not infer Stage-1 quality from a refined master. Failed tests remain recorded; retain the user's preferred comparison without calling it universally proven.
- Change one experimental factor per comparison. Fix seed, input hashes, prompt, duration, and graph unless they are the declared factor. Never silently shorten or resize after OOM.
- Respect current authorization: routine read-only checks and fixes already approved do not need repeated creative approval. Show a changed creative brief before generating unapproved content. Never interrupt another user's GPU jobs.

## Commands

```bash
python3 scripts/video_run.py --url http://10.0.1.217:8188 submit --spec /absolute/spec.json --run-dir /absolute/new-run
python3 scripts/video_run.py collect --run-dir /absolute/new-run
python3 scripts/video_run.py status --run-dir /absolute/new-run
python3 scripts/video_run.py --url http://10.0.1.217:8188 recover --prompt-id UUID --node 24 --run-dir /absolute/new-recovery --width 1536 --height 1024 --frames 241 --fps 24
python3 scripts/video_run.py review --run-dir /absolute/new-run --report /absolute/review.json
```

`collect` performs one bounded check, so the agent can communicate while rendering. Poll according to observed runtime (usually 30–60 seconds); keep pending jobs visible and retrieve completed output without waiting for the user to ask. When run through `spawn_background_command`, the runner writes verified outputs to the gateway-provided `$ARTIFACT_MANIFEST`; the gateway publishes those exact files before its completion message. For foreground runs, call `publish_artifact` on each verified output and return the server-generated URL with the actual review and limitations. Retrieve earlier takes with `find_artifacts` / `get_artifact`; never reconstruct upload URLs.

`spec.json` example:

```json
{"graph":{"...":"ComfyUI API graph"},"assets":{"8":"/absolute/reference.png"},"outputs":{"24":{"width":768,"height":448,"frames":121,"fps":24}},"hypothesis":"Reference appearance persists during a simple motion","changed_factor":"reference conditioning","baseline":null}
```

Read `references/ingredients.md` before constructing an Ingredients graph. The neutral test builder is `scripts/neutral_ingredients.py`; its installed-stack adaptations are explicit. It is not a general claim that this graph is validated.

For an authorized cloud-assisted sequence review, `scripts/review_video.py --video /absolute/video.mp4 --reference /absolute/sheet.png --brief /absolute/brief.txt --env-file /absolute/existing-skill/.env --out /absolute/new-critique.json` uses the configured Gemini API key without copying or displaying it. It sends the selected video and reference for an 8fps timestamped critique. Inspect and corroborate the critique; it does not set run acceptance or substitute for checking the actual images. This is an external API call and must stay within the user's authorized media scope. Local media probing uses system ffmpeg/ffprobe or the existing `imageio_ffmpeg` package.
