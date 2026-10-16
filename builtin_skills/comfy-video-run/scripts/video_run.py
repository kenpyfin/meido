#!/usr/bin/env python3
"""Per-attempt ComfyUI video execution; no implicit retries or shared master."""
from __future__ import annotations

import argparse
from contextlib import contextmanager
from datetime import datetime, timezone
from fractions import Fraction
import hashlib
import json
import mimetypes
import os
from pathlib import Path
import re
import shutil
import subprocess
import urllib.parse
import urllib.request
import uuid


def now():
    return datetime.now(timezone.utc).isoformat()


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def file_hash(path):
    h = hashlib.sha256()
    with Path(path).open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def save(path, value):
    path = Path(path)
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(json.dumps(value, indent=2) + "\n")
    tmp.replace(path)


@contextmanager
def locked(root):
    lock = root / ".operation.lock"
    with lock.open("x"):
        try:
            yield
        finally:
            lock.unlink()


def event(root, state, **fields):
    record = json.loads((root / "run.json").read_text())
    record.update(state=state, updated_at=now(), **fields)
    save(root / "run.json", record)
    # The gateway consumes this explicit contract before announcing completion.
    manifest = os.environ.get("ARTIFACT_MANIFEST")
    if manifest and record.get("artifacts"):
        save(manifest, {"artifacts": record["artifacts"], "state": state})
    with (root / "events.jsonl").open("a") as f:
        f.write(json.dumps({"at": record["updated_at"], "state": state, **fields}) + "\n")
    return record


class Client:
    def __init__(self, url):
        parts = urllib.parse.urlsplit(url)
        if parts.scheme not in ("http", "https") or not parts.netloc or parts.username:
            raise ValueError("Expected an HTTP(S) ComfyUI URL without embedded credentials")
        self.url = url.rstrip("/")

    def json(self, path, payload=None):
        data = None if payload is None else json.dumps(payload).encode()
        req = urllib.request.Request(self.url + path, data=data,
                                     headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=30) as r:
            return json.load(r)

    def upload(self, path, name):
        boundary = "comfy-video-" + uuid.uuid4().hex
        mime = mimetypes.guess_type(path.name)[0] or "application/octet-stream"
        data = (f'--{boundary}\r\nContent-Disposition: form-data; name="image"; filename="{name}"\r\n'
                f'Content-Type: {mime}\r\n\r\n').encode() + path.read_bytes()
        data += (f'\r\n--{boundary}\r\nContent-Disposition: form-data; name="overwrite"\r\n\r\nfalse'
                 f'\r\n--{boundary}--\r\n').encode()
        req = urllib.request.Request(self.url + "/upload/image", data=data,
                                     headers={"Content-Type": f"multipart/form-data; boundary={boundary}"})
        with urllib.request.urlopen(req, timeout=60) as r:
            result = json.load(r)
        return "/".join(x for x in [result.get("subfolder", ""), result["name"]] if x)

    def download(self, descriptor, path):
        query = urllib.parse.urlencode({k: descriptor[k] for k in ("filename", "subfolder", "type")})
        with urllib.request.urlopen(self.url + "/view?" + query, timeout=120) as r, path.open("xb") as f:
            shutil.copyfileobj(r, f)


def canonical_graph(graph):
    return {str(k): {"class_type": v["class_type"], "inputs": v.get("inputs", {})}
            for k, v in graph.items()}


def validate_spec(spec):
    graph = spec["graph"]
    if not isinstance(graph, dict) or not graph or not spec.get("outputs"):
        raise ValueError("A nonempty API graph and explicit video output nodes are required")
    for nid, expected in spec["outputs"].items():
        if not re.fullmatch(r"[A-Za-z0-9_-]+", nid):
            raise ValueError("Output node ID must be a safe filename component")
        if nid not in graph or graph[nid]["class_type"] not in ("SaveVideo", "VHS_VideoCombine"):
            raise ValueError(f"{nid}: expected a declared video save node")
        if any(float(expected.get(k, 0)) <= 0 for k in ("width", "height", "frames", "fps")):
            raise ValueError(f"{nid}: positive width, height, frames and fps required")
    for nid, path in spec.get("assets", {}).items():
        if not re.fullmatch(r"[A-Za-z0-9_-]+", nid):
            raise ValueError("Asset node ID must be a safe filename component")
        if graph[nid]["class_type"] != "LoadImage" or not Path(path).is_file():
            raise ValueError(f"{nid}: asset must name an existing image and a LoadImage node")
    missing = {nid for nid, node in graph.items() if node["class_type"] == "LoadImage"} - set(spec.get("assets", {}))
    if missing:
        raise ValueError(f"All LoadImage nodes need local asset snapshots: {sorted(missing)}")


def initialize(root, url, spec):
    root.mkdir(parents=True, exist_ok=False)
    (root / "assets").mkdir()
    (root / "outputs").mkdir()
    save(root / "spec.json", spec)
    save(root / "run.json", {"version": 1, "client_id": "video-" + uuid.uuid4().hex,
                              "url": url, "state": "prepared", "created_at": now(),
                              "outputs": spec["outputs"], "artifacts": [],
                              "user_acceptance": "pending"})


def submit(root, spec, client):
    validate_spec(spec)
    initialize(root, client.url, spec)
    with locked(root):
        graph = json.loads(json.dumps(spec["graph"]))
        try:
            snapshots = {}
            rid = json.loads((root / "run.json").read_text())["client_id"]
            for nid, source in spec.get("assets", {}).items():
                source = Path(source)
                dest = root / "assets" / (nid + source.suffix.lower())
                shutil.copyfile(source, dest)
                name = client.upload(dest, f"{rid}_{nid}{source.suffix.lower()}")
                graph[nid]["inputs"]["image"] = name
                snapshots[nid] = {"source": str(source.resolve()), "local": str(dest.resolve()),
                                  "sha256": file_hash(dest), "server_name": name}
            for nid in spec["outputs"]:
                graph[nid]["inputs"]["filename_prefix"] = f"verified-video/{rid}/node-{nid}"
            save(root / "graph.json", graph)
            event(root, "prepared", inputs=snapshots, graph_sha256=digest(canonical_graph(graph)))
            # Persist before the side effect. A lost response is NOT permission to repeat POST.
            event(root, "submitting")
            result = client.json("/prompt", {"prompt": graph, "client_id": rid})
        except Exception as exc:
            old = json.loads((root / "run.json").read_text())
            event(root, "submission_unknown" if old["state"] == "submitting" else "preparation_failed",
                  error=str(exc))
            raise
        save(root / "submission.json", result)
        if result.get("node_errors") or not result.get("prompt_id"):
            event(root, "submission_rejected", error=result)
            raise ValueError("ComfyUI rejected the graph; see submission.json")
        return event(root, "queued", prompt_id=result["prompt_id"])


def history_entry(history, record):
    pid = record["prompt_id"]
    entry = history.get(pid)
    if entry is None:
        return None
    prompt = entry.get("prompt", [])
    if len(prompt) < 3 or prompt[1] != pid:
        raise ValueError("History prompt identity mismatch")
    if digest(canonical_graph(prompt[2])) != record["graph_sha256"]:
        raise ValueError("Server graph differs from the snapshotted job graph")
    status = entry.get("status", {})
    if status.get("status_str") == "error" or any(
            m[0] in ("execution_error", "execution_interrupted") for m in status.get("messages", []) if m):
        raise ValueError("Server reports failed/interrupted execution")
    if status.get("status_str") != "success" or status.get("completed") is not True:
        return None
    return entry


def choose_output(entry, nid):
    candidates = {}
    for values in entry.get("outputs", {}).get(nid, {}).values():
        if not isinstance(values, list):
            continue
        for item in values:
            if not isinstance(item, dict) or not item.get("filename"):
                continue
            name = item["filename"]
            if Path(name).suffix.lower() not in (".mp4", ".webm", ".mov", ".mkv"):
                continue
            folder = item.get("subfolder", "").replace("\\", "/")
            if "/" in name or "\\" in name or name in (".", "..") or ":" in name:
                raise ValueError("Unsafe server output filename")
            if folder.startswith("/") or ":" in folder or ".." in folder.split("/"):
                raise ValueError("Unsafe server output subfolder")
            if item.get("type") != "output":
                continue
            desc = {"filename": name, "subfolder": folder, "type": "output"}
            candidates[json.dumps(desc, sort_keys=True)] = desc
    if len(candidates) != 1:
        raise ValueError(f"Node {nid}: expected exactly one completed output video, got {len(candidates)}")
    return next(iter(candidates.values()))


def probe(path):
    if not shutil.which("ffprobe"):
        # Existing bot environments bundle ffmpeg through imageio rather than PATH.
        # Decode the entire stream to count real frames instead of estimating duration*fps.
        import imageio_ffmpeg
        reader = imageio_ffmpeg.read_frames(str(path), pix_fmt="rgb24")
        try:
            meta = next(reader)
            count = sum(1 for _ in reader)
        finally:
            reader.close()
        return {"width": meta["size"][0], "height": meta["size"][1],
                "frames": count, "fps": meta["fps"]}
    cmd = ["ffprobe", "-v", "error", "-select_streams", "v:0", "-count_frames", "-show_entries",
           "stream=width,height,nb_read_frames,avg_frame_rate,duration", "-of", "json", str(path)]
    result = subprocess.run(cmd, capture_output=True, text=True, check=True, timeout=120)
    stream = json.loads(result.stdout)["streams"][0]
    return {"width": int(stream["width"]), "height": int(stream["height"]),
            "frames": int(stream["nb_read_frames"]), "fps": float(Fraction(stream["avg_frame_rate"]))}


def check_probe(actual, expected):
    for key in ("width", "height", "frames", "fps"):
        if abs(float(actual[key]) - float(expected[key])) > (0.001 if key == "fps" else 0):
            raise ValueError(f"Video {key}: expected {expected[key]}, got {actual[key]}")


def probes(path, frames, root):
    root.mkdir(exist_ok=True)
    indices = sorted({round(i * (frames - 1) / 12) for i in range(13)})
    result = []
    ffmpeg = shutil.which("ffmpeg")
    if not ffmpeg:
        import imageio_ffmpeg
        ffmpeg = imageio_ffmpeg.get_ffmpeg_exe()
    for index in indices:
        target = root / f"frame-{index:05d}.png"
        subprocess.run([ffmpeg, "-v", "error", "-y", "-i", str(path), "-vf",
                        f"select=eq(n\\,{index})", "-frames:v", "1", str(target)],
                       check=True, capture_output=True, timeout=120)
        if not target.is_file() or target.stat().st_size == 0:
            raise ValueError(f"Frame probe missing: {index}")
        result.append(str(target.resolve()))
    return result


def collect(root, client=None, do_probes=True):
    with locked(root):
        record = json.loads((root / "run.json").read_text())
        if record["state"] in ("review_pending", "reviewed"):
            for artifact in record["artifacts"]:
                if file_hash(artifact["path"]) != artifact["sha256"]:
                    raise ValueError("Previously verified output changed on disk")
            manifest = os.environ.get("ARTIFACT_MANIFEST")
            if manifest:
                save(manifest, {"artifacts": record["artifacts"], "state": record["state"]})
            return record
        if not record.get("prompt_id"):
            raise ValueError("No known prompt_id; reconcile submission before collecting")
        client = client or Client(record["url"])
        try:
            history = client.json("/history/" + urllib.parse.quote(record["prompt_id"], safe=""))
        except Exception as exc:
            return event(root, "connection_unavailable", error=str(exc))
        save(root / "history.json", history)
        try:
            entry = history_entry(history, record)
            if entry is None:
                return event(root, "pending_or_unknown")
            event(root, "rendered")
            artifacts = []
            for nid, expected in record["outputs"].items():
                desc = choose_output(entry, nid)
                path = root / "outputs" / ("node-" + nid + Path(desc["filename"]).suffix)
                partial = path.with_name(path.stem + ".partial" + path.suffix)
                if partial.exists():
                    partial.unlink()
                client.download(desc, partial)
                actual = probe(partial)
                check_probe(actual, expected)
                partial.replace(path)
                artifact = {"node": nid, "prompt_id": record["prompt_id"], "path": str(path.resolve()),
                            "server_output": desc, "sha256": file_hash(path), "probe": actual}
                if do_probes:
                    artifact["probes"] = probes(path, actual["frames"], root / ("qc-" + nid))
                artifacts.append(artifact)
            return event(root, "review_pending", artifacts=artifacts, error=None)
        except Exception as exc:
            event(root, "verification_failed", error=str(exc))
            raise


def recover(root, client, pid, nid, expected):
    if not re.fullmatch(r"[A-Za-z0-9_-]+", nid):
        raise ValueError("Output node ID must be a safe filename component")
    history = client.json("/history/" + urllib.parse.quote(pid, safe=""))
    entry = history.get(pid)
    if not entry:
        raise ValueError("No server history for this ID; no recovery created")
    graph = entry["prompt"][2]
    spec = {"graph": graph, "outputs": {nid: expected}, "source": "server-history-recovery"}
    initialize(root, client.url, spec)
    save(root / "graph.json", graph)
    event(root, "queued", prompt_id=pid, graph_sha256=digest(canonical_graph(graph)),
          provenance="Recovered server graph; original local input bytes not attested")
    return collect(root, client)


def review(root, report):
    with locked(root):
        record = json.loads((root / "run.json").read_text())
        if record["state"] not in ("review_pending", "reviewed"):
            raise ValueError("Only verified videos can be reviewed")
        expected = {a["sha256"] for a in record["artifacts"]}
        if set(report.get("artifact_sha256", [])) != expected:
            raise ValueError("Review must identify the exact verified artifact hashes")
        if report.get("verdict") not in ("pass", "fail", "inconclusive"):
            raise ValueError("Review verdict must be pass, fail or inconclusive")
        for k in ("identity", "reference_fidelity", "framing", "motion", "detail", "limitations"):
            if not isinstance(report.get(k), str) or not report[k].strip():
                raise ValueError(f"Missing review evidence: {k}")
        for artifact in record["artifacts"]:
            if file_hash(artifact["path"]) != artifact["sha256"]:
                raise ValueError("Video changed before review")
        report["reviewed_at"] = now()
        save(root / "review.json", report)
        return event(root, "reviewed", visual_verdict=report["verdict"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default="http://10.0.1.217:8188")
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("submit", "collect", "status", "recover", "review"):
        p = commands.add_parser(name)
        p.add_argument("--run-dir", type=Path, required=True)
        if name == "submit": p.add_argument("--spec", type=Path, required=True)
        if name == "review": p.add_argument("--report", type=Path, required=True)
        if name == "recover":
            p.add_argument("--prompt-id", required=True)
            p.add_argument("--node", required=True)
            for k in ("width", "height", "frames", "fps"):
                p.add_argument("--" + k, type=int, required=True)
    a = parser.parse_args()
    root = a.run_dir.resolve()
    if a.command == "submit": result = submit(root, json.loads(a.spec.read_text()), Client(a.url))
    elif a.command == "collect": result = collect(root)
    elif a.command == "recover":
        result = recover(root, Client(a.url), a.prompt_id, a.node,
                         {k: getattr(a, k) for k in ("width", "height", "frames", "fps")})
    elif a.command == "review": result = review(root, json.loads(a.report.read_text()))
    else: result = json.loads((root / "run.json").read_text())
    print(json.dumps({k: result[k] for k in ("state", "prompt_id", "artifacts", "visual_verdict", "error") if k in result}, indent=2))


if __name__ == "__main__":
    main()
