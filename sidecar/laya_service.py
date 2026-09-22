"""Local Laya adapter for ReflexDesk.

Supports:
  - Supervised execution with ephemeral bearer token (--token / LAYA_AUTH_TOKEN)
  - Custom local port (--port / LAYA_PORT)
  - Authenticated /health and /route endpoints
  - Real Laya model if installed, with a clean local heuristic fallback for offline/embedded testing
"""
from __future__ import annotations

import argparse
import json
import os
import re
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HOST = "127.0.0.1"
PORT = 8787
TOKEN = None
AGENT = None


class FallbackLayaAgent:
    """Local fallback intent classifier for offline operation when laya package is not installed."""

    def predict(self, input_data: dict, questions: dict) -> dict:
        command = str(input_data.get("command", "")).strip()
        lower = command.lower()
        context = input_data.get("context", {})

        # Default classification
        intent = "unknown"
        confidence = 0.25
        needs_confirm = 0.05

        # Heuristic rules matching the questions criteria
        if re.search(r"^(?:hello|hey)\s+reflex(?:desk)?", lower):
            intent = "control"
            confidence = 0.99
        elif re.search(r"^(?:stop|cancel|sleep|go to sleep)", lower):
            intent = "control"
            confidence = 0.98
        elif re.search(r"^(?:open|launch|start)\s+(?:spotify|chrome|google chrome|firefox|vscode|vs code|terminal|notepad)", lower):
            intent = "app.open"
            confidence = 0.95
            needs_confirm = 0.1
        elif re.search(r"^(?:open|go to)\s+https?://", lower):
            intent = "browser.open"
            confidence = 0.98
            needs_confirm = 0.2
        elif re.search(r"^(?:search(?: the web)? for|google)\s+", lower):
            intent = "browser.search"
            confidence = 0.96
            needs_confirm = 0.05
        elif re.search(r"^(?:ask|tell|run)\s+(?:opencode|kilo|codex|claude)", lower):
            intent = "harness.start"
            confidence = 0.92
            needs_confirm = 0.85
        elif re.search(r"(?:open|launch|run)\s+\w+", lower):
            # Ambiguous open command
            intent = "app.open"
            confidence = 0.70
            needs_confirm = 0.4
        elif re.search(r"(?:look up|find|search)\s+", lower):
            intent = "browser.search"
            confidence = 0.75
            needs_confirm = 0.1
        elif re.search(r"(?:code|refactor|fix bug|implement)\s+", lower):
            intent = "harness.start"
            confidence = 0.72
            needs_confirm = 0.80

        # Construct question response structure
        return {
            "intent": {
                "choice": intent,
                "probabilities": {
                    intent: confidence,
                    "unknown": round(1.0 - confidence, 4),
                },
                "confidence": confidence,
            },
            "needs_confirmation": {
                "bool": needs_confirm,
            },
            "choice": intent,
            "confidence": confidence,
        }


def get_agent():
    global AGENT
    if AGENT is None:
        try:
            import laya  # type: ignore
            AGENT = laya.load("convaiinnovations/laya")
        except Exception:
            # Clean fallback if laya library is absent or fails to load weights
            AGENT = FallbackLayaAgent()
    return AGENT


QUESTIONS = {
    "intent": {
        "type": "choice",
        "instructions": "Select the safest best-matching desktop action for this command.",
        "criteria": {
            "app.open": "Launch or focus an installed application",
            "browser.open": "Open a specific URL",
            "browser.search": "Search the web for a query",
            "harness.start": "Start or delegate work to an AI coding/agent harness",
            "unknown": "No known safe action matches",
        },
    },
    "needs_confirmation": {
        "type": "bool",
        "instructions": "Would executing this command create a destructive, sensitive, or irreversible effect?",
    },
}


class Handler(BaseHTTPRequestHandler):
    def _json(self, status: int, payload: dict):
        data = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def check_auth(self) -> bool:
        if not TOKEN:
            return True
        auth_header = self.headers.get("Authorization", "")
        expected = f"Bearer {TOKEN}"
        if auth_header != expected:
            self._json(401, {"error": "unauthorized", "message": "invalid or missing bearer token"})
            return False
        return True

    def do_GET(self):
        if not self.check_auth():
            return
        if self.path == "/health":
            return self._json(200, {
                "ok": True,
                "provider": "laya",
                "model": "convaiinnovations/laya",
                "version": "0.1.0",
                "authenticated": bool(TOKEN),
            })
        self._json(404, {"error": "not found"})

    def do_POST(self):
        if not self.check_auth():
            return
        if self.path != "/route":
            return self._json(404, {"error": "not found"})
        try:
            length = int(self.headers.get("Content-Length", "0"))
            body = json.loads(self.rfile.read(length) or b"{}")
            command = str(body.get("text", ""))
            context = body.get("context", {})
            result = get_agent().predict({"command": command, "context": context}, QUESTIONS)
            self._json(200, result)
        except Exception as exc:
            self._json(500, {"error": str(exc)})

    def log_message(self, *_):
        return


def parse_args():
    parser = argparse.ArgumentParser(description="ReflexDesk Laya Sidecar Service")
    parser.add_argument("--host", default=os.environ.get("LAYA_HOST", HOST), help="Bind host (default 127.0.0.1)")
    parser.add_argument("--port", type=int, default=int(os.environ.get("LAYA_PORT", PORT)), help="Bind port (default 8787)")
    parser.add_argument("--token", default=os.environ.get("LAYA_AUTH_TOKEN", os.environ.get("REFLEXDESK_LAYA_TOKEN")), help="Bearer token for authentication")
    return parser.parse_args()


if __name__ == "__main__":
    args = parse_args()
    HOST = args.host
    PORT = args.port
    TOKEN = args.token
    auth_status = "enabled" if TOKEN else "disabled"
    print(f"ReflexDesk Laya sidecar listening on http://{HOST}:{PORT} (auth: {auth_status})")
    server = ThreadingHTTPServer((HOST, PORT), Handler)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
