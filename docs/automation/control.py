#!/usr/bin/env python3
"""Drives a running HybridSliceCraft window through its control server (craft-automation).

As a module:

    from control import HybridSliceCraft
    with HybridSliceCraft("target/release/hybridslicecraft", config_dir) as app:
        app.wait_ready()
        app.call("ui.menu.invoke", {"path": ["View", "Theme", "Light"]})
        app.call("ui.screenshot", {"path": "light.png"})

From the command line, a steps file runs one request after the other and prints each reply:

    python3 docs/automation/control.py target/release/hybridslicecraft steps.json [model.stl ...]

    steps.json: [{"method": "ui.menu.invoke", "params": {"path": ["Help", "Keyboard shortcuts"]}},
                 {"method": "ui.screenshot", "params": {"path": "shortcuts.png"}}]

Pictures with a `path` land in `--out` (default target/automation). The app runs with a settings
folder of its own (`--config`, default target/automation/config), so it never touches yours.
The methods are listed in src/app/control.rs.
"""

import argparse
import json
import os
import re
import secrets
import socket
import subprocess
import sys
import threading
import time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


class HybridSliceCraft:
    """One HybridSliceCraft window started with a control port, and a connection to it."""

    def __init__(self, binary, config, models=(), out=None, env=None):
        self.token = secrets.token_hex(32)
        os.makedirs(config, exist_ok=True)
        full = dict(os.environ)
        full.update(env or {})
        full.update({
            "HYBRIDSLICECRAFT_CONFIG": config,
            "HYBRIDSLICECRAFT_NO_WINDOW_RESTORE": "1",
            "HYBRIDSLICECRAFT_CONTROL_PORT": "0",
            "HYBRIDSLICECRAFT_CONTROL_TOKEN": self.token,
        })
        if out:
            full["HYBRIDSLICECRAFT_CONTROL_DIR"] = out
        self.process = subprocess.Popen([binary, *models], env=full, stderr=subprocess.PIPE, text=True)
        port = None
        for line in self.process.stderr:
            found = re.search(r"control server listening on [^:]+:(\d+)", line)
            if found:
                port = int(found.group(1))
                break
        if port is None:
            raise RuntimeError("HybridSliceCraft didn't start its control server")
        # Keep reading what the program prints, so it never waits on a full pipe.
        threading.Thread(target=lambda: [sys.stderr.write(l) for l in self.process.stderr], daemon=True).start()
        self.socket = socket.create_connection(("127.0.0.1", port))
        self.stream = self.socket.makefile("rw", encoding="utf-8")
        self.next_id = 0
        self.call("auth", {"token": self.token})

    def call(self, method, params=None):
        """Sends one request and returns its result; a failed request raises."""
        self.next_id += 1
        request = {"id": self.next_id, "method": method, "params": params or {}}
        self.stream.write(json.dumps(request) + "\n")
        self.stream.flush()
        reply = json.loads(self.stream.readline())
        if not reply.get("ok"):
            raise RuntimeError(f"{method}: {reply.get('error')}")
        return reply.get("result")

    def wait_ready(self, timeout=60):
        """Waits until nothing is loading or slicing, and returns the window's state."""
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            state = self.call("app.state")
            if not state["busy"]:
                return state
            time.sleep(0.1)
        raise TimeoutError("the window stayed busy")

    def close(self):
        try:
            self.call("app.quit")
        except (OSError, RuntimeError, ValueError):
            pass
        try:
            self.process.wait(timeout=20)
        except subprocess.TimeoutExpired:
            self.process.kill()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("binary", help="the hybridslicecraft program")
    parser.add_argument("steps", help="a JSON list of {method, params}")
    parser.add_argument("models", nargs="*", help="model files to open")
    parser.add_argument("--out", default=os.path.join(ROOT, "target", "automation"))
    parser.add_argument("--config", default=os.path.join(ROOT, "target", "automation", "config"))
    args = parser.parse_args()
    with open(args.steps, encoding="utf-8") as file:
        steps = json.load(file)
    with HybridSliceCraft(args.binary, args.config, args.models, out=args.out) as app:
        app.wait_ready()
        for step in steps:
            try:
                result = app.call(step["method"], step.get("params"))
            except RuntimeError as error:
                sys.exit(str(error))
            print(json.dumps({"method": step["method"], "result": result}))


if __name__ == "__main__":
    main()
