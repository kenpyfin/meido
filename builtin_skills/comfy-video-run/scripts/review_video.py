#!/usr/bin/env python3
"""Request a timestamped video critique; never auto-promotes a quality verdict."""
import argparse
import base64
import json
import mimetypes
import os
from pathlib import Path
import urllib.request

from video_run import file_hash, now, save


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--video", required=True, type=Path)
    p.add_argument("--reference", required=True, type=Path)
    p.add_argument("--brief", required=True, type=Path)
    p.add_argument("--env-file", type=Path)
    p.add_argument("--out", required=True, type=Path)
    p.add_argument("--model", default="gemini-2.5-flash")
    a = p.parse_args()
    key = os.environ.get("GEMINI_API_KEY")
    if not key and a.env_file:
        for line in a.env_file.read_text().splitlines():
            if line.startswith("GEMINI_API_KEY="):
                key = line.split("=", 1)[1].strip().strip('"').strip("'")
    if not key:
        raise SystemExit("GEMINI_API_KEY is not configured")
    if a.out.exists():
        raise SystemExit("Choose a new output path; preserve previous critiques")
    if a.video.stat().st_size + a.reference.stat().st_size > 14 * 1024 * 1024:
        raise SystemExit("Inline review limited to 14MiB of source media")
    prompt = (
        "Review this generated, fully clothed fashion video against the supplied visual reference and brief. "
        "The first image is a reference inventory, not a desired collage composition. The second media item is "
        "the actual video. Watch the complete sequence at the supplied 8fps sampling. Do not infer successful "
        "motion merely from start/end poses. Look for identity drift, reference-board leakage, wrong accessory "
        "shape/color/strap/clasp, cropping of head or feet, zooms, abrupt movements, anatomical distortion, "
        "texture instability and missing action beats. Distinguish observed defects from guesses about cause. "
        "Return JSON with verdict (pass/fail/inconclusive), identity, reference_fidelity, framing, motion, detail, "
        "limitations (all strings), and observations (list of timestamped observations). Mention limitations of "
        "sampling and do not claim pixel-perfect garment identity. Be candid and specific. Brief:\n" + a.brief.read_text()
    )
    def media(path):
        return {"inlineData": {"mimeType": mimetypes.guess_type(path.name)[0],
                               "data": base64.b64encode(path.read_bytes()).decode()}}
    video = media(a.video)
    video["videoMetadata"] = {"fps": 8}
    payload = {"contents": [{"role": "user", "parts": [{"text": prompt}, media(a.reference), video]}],
               "generationConfig": {"temperature": 0.1, "responseMimeType": "application/json", "maxOutputTokens": 6000}}
    req = urllib.request.Request("https://generativelanguage.googleapis.com/v1beta/models/" + a.model + ":generateContent",
                                 data=json.dumps(payload).encode(),
                                 headers={"Content-Type": "application/json", "x-goog-api-key": key})
    with urllib.request.urlopen(req, timeout=180) as r:
        result = json.load(r)
    save(a.out.with_suffix(".response.json"), result)
    text = "".join(part.get("text", "") for part in result["candidates"][0]["content"]["parts"] if not part.get("thought"))
    report = json.loads(text)
    report.update(model=a.model, video_sha256=file_hash(a.video), reference_sha256=file_hash(a.reference),
                  requested_sampling_fps=8, at=now(), source="automated critique; agent review still required")
    save(a.out, report)
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
