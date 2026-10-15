#!/usr/bin/env python3
"""Cursor-agent host runner. Listens for POST /spawn from the FinallyAValueBot bot (in Docker).
Requires: cursor-agent CLI and tmux on the host.
API: POST /spawn
  Body: {"prompt": "...", "workdir": "...", "model": "...", "detach": bool}
  Response: {"success": true, "session_name": "..."} for detach, {"success": true, "output": "..."} for inline.
"""

import json
import os
import subprocess
import sys
from http.server import HTTPServer, BaseHTTPRequestHandler

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 3847
CURSOR_AGENT = "cursor-agent"
SESSION_PREFIX = "finally-a-value-bot-cursor"

# Derive the project root from this script's location (scripts/cursor-agent-runner.py → project root)
PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# Container path prefix that maps to <PROJECT_ROOT>/workspace on the host
CONTAINER_WORKSPACE_PREFIX = "/app/workspace"
# Host workspace root (git ceiling so agent shells do not bind the bot checkout)
HOST_WORKSPACE = os.path.join(PROJECT_ROOT, "workspace")


def translate_path(container_path: str) -> str:
    """Translate a Docker container path to the equivalent host path."""
    if container_path.startswith(CONTAINER_WORKSPACE_PREFIX):
        suffix = container_path[len(CONTAINER_WORKSPACE_PREFIX):]
        return HOST_WORKSPACE + suffix
    return container_path


def spawn_env():
    """Env for cursor-agent: inherit host env + git ceiling at workspace."""
    env = os.environ.copy()
    ceiling = os.path.realpath(HOST_WORKSPACE)
    existing = env.get("GIT_CEILING_DIRECTORIES", "").strip()
    if existing:
        parts = existing.split(os.pathsep)
        if ceiling not in parts:
            env["GIT_CEILING_DIRECTORIES"] = existing + os.pathsep + ceiling
    else:
        env["GIT_CEILING_DIRECTORIES"] = ceiling
    return env


class SpawnHandler(BaseHTTPRequestHandler):
    def do_POST(self):
        if self.path != "/spawn":
            self.send_response(404)
            self.end_headers()
            return
        try:
            content_length = int(self.headers.get("Content-Length", 0))
            body = self.rfile.read(content_length)
            data = json.loads(body)
        except (ValueError, json.JSONDecodeError) as e:
            self.send_json(400, {"success": False, "error": str(e)})
            return

        prompt = data.get("prompt") or ""
        workdir = translate_path(data.get("workdir") or ".")
        model = (data.get("model") or "").strip()
        detach = data.get("detach", False)

        if not prompt:
            self.send_json(400, {"success": False, "error": "prompt required"})
            return

        try:
            if detach:
                import time
                session_name = f"{SESSION_PREFIX}-{int(time.time() * 1000)}"
                ca_args = [CURSOR_AGENT, "-p", prompt, "--trust"]
                if model:
                    ca_args.extend(["--model", model])
                ca_args.extend(["--output-format", "text"])
                # Use env so GIT_CEILING applies even when tmux server is already running.
                ceiling = os.path.realpath(HOST_WORKSPACE)
                cmd = [
                    "tmux",
                    "new-session",
                    "-d",
                    "-s",
                    session_name,
                    "-c",
                    workdir,
                    "--",
                    "env",
                    f"GIT_CEILING_DIRECTORIES={ceiling}",
                ] + ca_args
                subprocess.run(cmd, check=True, capture_output=True, timeout=10, env=spawn_env())
                self.send_json(200, {"success": True, "session_name": session_name})
            else:
                cmd = [CURSOR_AGENT, "-p", prompt, "--trust", "--output-format", "text"]
                if model:
                    cmd.extend(["--model", model])
                result = subprocess.run(
                    cmd,
                    cwd=workdir,
                    capture_output=True,
                    text=True,
                    timeout=600,
                    env=spawn_env(),
                )
                output = result.stdout or ""
                if result.stderr:
                    output += "\nSTDERR:\n" + result.stderr
                self.send_json(200, {"success": result.returncode == 0, "output": output})
        except subprocess.TimeoutExpired:
            self.send_json(500, {"success": False, "error": "cursor-agent timed out"})
        except subprocess.CalledProcessError as e:
            self.send_json(500, {"success": False, "error": f"tmux/cursor-agent failed: {e}"})
        except FileNotFoundError as e:
            self.send_json(500, {"success": False, "error": f"cursor-agent or tmux not found: {e}"})
        except Exception as e:
            self.send_json(500, {"success": False, "error": str(e)})

    def send_json(self, code, obj):
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps(obj).encode())

    def log_message(self, format, *args):
        sys.stderr.write(f"[cursor-agent-runner] {format % args}\n")


if __name__ == "__main__":
    server = HTTPServer(("0.0.0.0", PORT), SpawnHandler)
    print(f"Cursor-agent runner listening on 0.0.0.0:{PORT} (POST /spawn)", file=sys.stderr)
    server.serve_forever()
