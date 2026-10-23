#!/usr/bin/env python3
"""Offline native-worker, CLI and MCP integration checks. Never signs in."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import queue
import threading
import time


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--bin-dir", default="target/debug")
    args = parser.parse_args()
    folder = Path(args.bin_dir).resolve()
    executable = folder / ("hrctl.exe" if os.name == "nt" else "hrctl")
    with tempfile.TemporaryDirectory(prefix="hr-synthetic-") as data:
        env = {**os.environ, "HUMBLE_REDEEMER_DATA_DIR": data}
        def cli(*arguments):
            print("Checking CLI " + arguments[0], flush=True)
            return json.loads(subprocess.check_output([str(executable), "--demo", "--json", *arguments], env=env, text=True, timeout=60))
        mcp = None
        try:
            state = cli("status")
            assert state["demo"] and state["humble"]["connected"]
            plan = cli("plan", "--games", "demo-0")
            denied = subprocess.run([str(executable), "--demo", "batch", "start", plan["id"]], env=env, capture_output=True, timeout=30)
            assert denied.returncode != 0
            cli("batch", "approve", plan["id"], "--digest", plan["digest"])
            cli("batch", "start", plan["id"])
            for _ in range(12):
                state = cli("status")
                if next(b for b in state["batches"] if b["id"] == plan["id"])["state"] == "complete":
                    break
                time.sleep(1)
            assert next(b for b in state["batches"] if b["id"] == plan["id"])["state"] == "complete"
            report = Path(data) / "report.csv"
            cli("export", str(report))
            assert "product_key" not in report.read_text()
            mcp = subprocess.Popen([str(executable), "--demo", "mcp"], env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            incoming = queue.Queue()
            def read_protocol():
                for line in mcp.stdout:
                    incoming.put(line)
                incoming.put(None)
            threading.Thread(target=read_protocol, daemon=True).start()
            def rpc(message):
                print("Checking MCP " + message["method"], flush=True)
                mcp.stdin.write(json.dumps(message) + "\n")
                mcp.stdin.flush()
                if "id" in message:
                    try:
                        line = incoming.get(timeout=25)
                    except queue.Empty:
                        raise RuntimeError("MCP response timed out for " + message["method"])
                    if line is None: raise RuntimeError("MCP process exited before a response")
                    return json.loads(line)
            init = rpc({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "synthetic-smoke", "version": "1"}}})
            assert "result" in init
            rpc({"jsonrpc": "2.0", "method": "notifications/initialized"})
            tools = rpc({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})
            names = {t["name"] for t in tools["result"]["tools"]}
            assert {"status", "plan_batch", "approve_batch", "control_batch"} <= names
            assert not any("password" in name or "reveal_key" in name for name in names)
            status = rpc({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "status", "arguments": {}}})
            payload = json.loads(status["result"]["content"][0]["text"])
            assert payload["demo"] and len(payload["games"]) == len(state["games"])
            mcp.stdin.close()
            mcp.wait(timeout=10)
            print("PASS: native IPC, approval gate, queue completion, redacted export, official SDK MCP handshake/tools/status")
        finally:
            if mcp is not None and mcp.poll() is None:
                mcp.terminate()
                try: mcp.wait(timeout=10)
                except subprocess.TimeoutExpired: mcp.kill(); mcp.wait(timeout=10)
            subprocess.run([str(executable), "--demo", "quit", "--pause"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=30)
            time.sleep(0.5)


if __name__ == "__main__":
    main()
