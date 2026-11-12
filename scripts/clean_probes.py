#!/usr/bin/env python3
"""Archive the Codex threads a live probe left in the user's own history.

A live probe drives real turns through the installed `codex`, which writes
every thread it starts to `$CODEX_HOME/sessions`. Codex lists every thread
there whatever folder it ran in, so a probe's "sleep 8" and "is 2027 prime"
turns sit in the user's Codex sidebar until something archives them.

A probe thread is one FlowX started (`originator` FlowX) in a temporary
folder. FlowX's own threads run in a project, so they are never matched.
Archiving goes through `codex app-server`'s `thread/archive`, the same call
Codex's own Archive makes, so it can be undone from Codex's archived list.

    ./fx clean-probes            archive them
    ./fx clean-probes --dry-run  only list them
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile


def temp_roots():
    roots = {"/tmp", "/private/tmp", tempfile.gettempdir()}
    if os.environ.get("TMPDIR"):
        roots.add(os.environ["TMPDIR"])
    resolved = set()
    for root in roots:
        resolved.add(os.path.realpath(root))
        resolved.add(os.path.normpath(root))
    return sorted(resolved)


def is_temporary(path, roots):
    if not path:
        return False
    for candidate in {os.path.normpath(path), os.path.realpath(path)}:
        for root in roots:
            if candidate == root or candidate.startswith(root.rstrip("/") + "/"):
                return True
    return False


def first_prompt(path):
    """The first thing the probe asked, so the listing says which turn it was."""
    try:
        with open(path, encoding="utf-8") as rollout:
            for line in rollout:
                try:
                    record = json.loads(line)
                except ValueError:
                    continue
                payload = record.get("payload") or {}
                if payload.get("type") == "user_message":
                    text = str(payload.get("message", ""))
                elif record.get("type") == "response_item" and payload.get("role") == "user":
                    # codex-cli 0.167 records the prompt only as the model's
                    # input, after the injected context blocks.
                    text = " ".join(
                        str(part.get("text", ""))
                        for part in payload.get("content") or []
                        if isinstance(part, dict)
                    )
                    if text.lstrip().startswith(("<", "# AGENTS.md")):
                        continue
                else:
                    continue
                return " ".join(text.split())[:70]
    except OSError:
        pass
    return ""


def probe_threads(codex_home):
    roots = temp_roots()
    sessions = os.path.join(codex_home, "sessions")
    found = []
    for folder, _, files in os.walk(sessions):
        for name in sorted(files):
            if not (name.startswith("rollout-") and name.endswith(".jsonl")):
                continue
            path = os.path.join(folder, name)
            try:
                with open(path, encoding="utf-8") as rollout:
                    meta = json.loads(rollout.readline())
            except (OSError, ValueError):
                continue
            payload = meta.get("payload") or {}
            if meta.get("type") != "session_meta" or payload.get("originator") != "FlowX":
                continue
            if not is_temporary(payload.get("cwd"), roots):
                continue
            found.append((payload.get("id"), payload.get("cwd"), first_prompt(path)))
    return [thread for thread in found if thread[0]]


class AppServer:
    def __init__(self, codex):
        self.process = subprocess.Popen(
            [codex, "app-server"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            bufsize=1,
        )
        self.next_id = 10
        self.request(0, "initialize", {"clientInfo": {"name": "flowx-clean-probes", "version": "1"}})
        self.send({"jsonrpc": "2.0", "method": "initialized"})

    def send(self, message):
        self.process.stdin.write(json.dumps(message) + "\n")
        self.process.stdin.flush()

    def request(self, request_id, method, params):
        self.send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        for line in self.process.stdout:
            try:
                message = json.loads(line)
            except ValueError:
                continue
            if message.get("id") == request_id and "method" not in message:
                return message
        raise RuntimeError("codex app-server exited before answering " + method)

    def archive(self, thread_id):
        self.next_id += 1
        return self.request(self.next_id, "thread/archive", {"threadId": thread_id}).get("error")

    def close(self):
        self.process.stdin.close()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.terminate()


def main(argv):
    dry_run = "--dry-run" in argv[1:]
    codex_home = os.environ.get("CODEX_HOME") or os.path.expanduser("~/.codex")
    threads = probe_threads(codex_home)
    if not threads:
        print("No probe threads in " + codex_home)
        return 0
    if dry_run:
        for thread_id, cwd, prompt in threads:
            print(thread_id + "  " + prompt + "  (" + cwd + ")")
        return 0

    codex = os.environ.get("CODEX") or shutil.which("codex")
    if not codex:
        print("codex is not on PATH; set CODEX to its path", file=sys.stderr)
        return 1
    server = AppServer(codex)
    kept = 0
    try:
        for thread_id, _, prompt in threads:
            error = server.archive(thread_id)
            if error:
                kept += 1
                print("kept      " + thread_id + "  " + prompt + "  " + str(error.get("message", error)))
            else:
                print("archived  " + thread_id + "  " + prompt)
    finally:
        server.close()
    if kept:
        # A thread open in the Codex app has an active writer and is refused;
        # close it there, or archive it from its own menu.
        print(str(kept) + " kept: close them in Codex and run this again.", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
