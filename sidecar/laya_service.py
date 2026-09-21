"""Optional local Laya adapter for ReflexDesk.

Run:
  python -m venv .venv
  .venv/bin/pip install laya
  .venv/bin/python sidecar/laya_service.py

Windows: use .venv\\Scripts\\python.exe.
"""
from __future__ import annotations

import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HOST = "127.0.0.1"
PORT = 8787
AGENT = None


def get_agent():
    global AGENT
    if AGENT is None:
        import laya
        AGENT = laya.load("convaiinnovations/laya")
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
        "type": "noul",
        "instructions": "Would executing this command create a destructive, sensitive, or irreversible effect?",
    },
}


class Handler(BaseHTTPRequestHandler):
    def _json(self, status, payload):
        data = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path == "/health":
            return self._json(200, {"ok": True, "provider": "laya"})
        self._json(404, {"error": "not found"})

    def do_POST(self):
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


if __name__ == "__main__":
    print(f"ReflexDesk Laya sidecar listening on http://{HOST}:{PORT}")
    ThreadingHTTPServer((HOST, PORT), Handler).serve_forever()
