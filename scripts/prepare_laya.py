"""Explicit one-time Laya install. Requires Python 3.10+ and internet access.

The installed sidecar never downloads weights. Its runtime and checkpoint live in
~/.reflexdesk/laya, not in a developer checkout or the application bundle.
"""
from __future__ import annotations

import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import sys

REVISION = "5e7b2b1b8ca2ecdd3f2322d94069c9b6ce7e844b"
WEIGHTS_SHA256 = "9d628fd971b700382ac6f65920a86f149777b2e748e0c955fb3b19695aa8f204"
TOKENIZER_SHA256 = "609d8f4c067cd3950f88594c5a802616cea245823836ef5848ee4fc40aab5b6f"
HOME = Path.home() / ".reflexdesk" / "laya"
VENV = HOME / "venv"
MODEL = HOME / "model"
PATTERNS = ["multilingual/rl_agent_config.json", "multilingual/model.safetensors",
            "multilingual/encoder/*", "multilingual/tokenizer/*"]


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def run():
    if sys.version_info < (3, 10):
        raise SystemExit("Laya requires Python 3.10 or newer")
    HOME.mkdir(parents=True, exist_ok=True)
    if not VENV.exists():
        try:
            import venv
        except ModuleNotFoundError:
            subprocess.run([sys.executable, "-m", "pip", "install", "virtualenv"], check=True)
            subprocess.run([sys.executable, "-m", "virtualenv", str(VENV)], check=True)
        else:
            venv.create(VENV, with_pip=True)
    python = VENV / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
    subprocess.run([str(python), "-m", "pip", "install", "laya==0.3.7"], check=True)
    # Use the virtualenv's huggingface_hub, not a second system installation.
    download_dir = HOME / "download"
    download = (
        "from huggingface_hub import snapshot_download; "
        f"print(snapshot_download('convaiinnovations/laya', revision='{REVISION}', "
        f"allow_patterns={PATTERNS!r}, local_dir={str(download_dir)!r}))"
    )
    snapshot = Path(subprocess.check_output([str(python), "-c", download], text=True).strip().splitlines()[-1])
    source = snapshot / "multilingual"
    if sha256(source / "model.safetensors") != WEIGHTS_SHA256:
        raise SystemExit("Laya weights digest mismatch; refusing to install")
    if sha256(source / "tokenizer" / "tokenizer.json") != TOKENIZER_SHA256:
        raise SystemExit("Laya tokenizer digest mismatch; refusing to install")
    temporary = HOME / "model.tmp"
    if temporary.exists():
        shutil.rmtree(temporary)
    shutil.copytree(source, temporary)
    if MODEL.exists():
        shutil.rmtree(MODEL)
    temporary.rename(MODEL)
    print(f"Laya multilingual checkpoint verified at {MODEL}")
    print("Restart ReflexDesk to activate the local model.")


if __name__ == "__main__":
    run()
