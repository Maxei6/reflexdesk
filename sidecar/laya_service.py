"""Authenticated loopback adapter for an explicitly installed local Laya checkpoint.

No model downloads occur in this process. Run scripts/prepare_laya.py once while
online; voice routing itself remains entirely offline.
"""
from __future__ import annotations

import argparse
import json
import os
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

QUESTIONS = {
    "intent": {
        "type": "choice",
        "instructions": "Which desktop task does the user request? Choose unknown if no task is clear.",
        "criteria": {
            "app.open": "Launch or focus an application",
            "browser.open": "Open a URL in a browser",
            "browser.search": "Search the web for a topic",
            "harness.start": "Delegate a coding task to an AI harness",
            "unknown": "None of these actions is clearly requested",
        },
    },
}
AGENT = None
TOKEN = ""


class Handler(BaseHTTPRequestHandler):
    def respond(self, status: int, payload: dict):
        data = json.dumps(payload).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def authorized(self) -> bool:
        if self.headers.get("Authorization") != f"Bearer {TOKEN}":
            self.respond(401, {"error": "unauthorized"})
            return False
        return True

    def do_GET(self):
        if not self.authorized():
            return
        if self.path == "/health":
            self.respond(200, {"ok": AGENT is not None, "provider": "laya", "model": "multilingual"})
        else:
            self.respond(404, {"error": "not found"})

    def do_POST(self):
        if not self.authorized():
            return
        if self.path != "/route":
            self.respond(404, {"error": "not found"})
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            if length <= 0 or length > 65536:
                self.respond(413, {"error": "invalid request length"})
                return
            body = json.loads(self.rfile.read(length))
            text = body.get("text")
            if not isinstance(text, str) or not text.strip():
                self.respond(400, {"error": "text required"})
                return
            result = AGENT.predict(text.strip(), QUESTIONS)
            self.respond(200, result)
        except (ValueError, TypeError, KeyError) as exc:
            self.respond(400, {"error": str(exc)})
        except Exception:
            self.respond(500, {"error": "local inference failed"})

    def log_message(self, *_):
        return


def main():
    parser = argparse.ArgumentParser(description="Offline ReflexDesk Laya sidecar")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8787)
    parser.add_argument("--token", required=True)
    parser.add_argument("--model-dir", required=True)
    args = parser.parse_args()
    if args.host != "127.0.0.1" or not args.token:
        parser.error("loopback host and bearer token required")
    model_dir = Path(args.model_dir)
    if not (model_dir / "model.safetensors").is_file():
        parser.error("local Laya checkpoint missing; run scripts/prepare_laya.py")
    os.environ["HF_HUB_OFFLINE"] = "1"
    os.environ["TRANSFORMERS_OFFLINE"] = "1"
    import laya

    global AGENT, TOKEN
    AGENT = laya.load(str(model_dir.resolve()))
    TOKEN = args.token
    server = HTTPServer((args.host, args.port), Handler)
    print("ReflexDesk local Laya ready", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
