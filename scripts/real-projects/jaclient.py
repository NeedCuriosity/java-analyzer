"""Minimal LSP / DAP clients for driving `java-analyzer` from the real-project checks.

Both protocols use the same `Content-Length` framing over stdio. The server's stderr (its log,
see `JA_LOG`) goes to a file so the checks can grep it afterwards.
"""

import json
import os
import pathlib
import queue
import subprocess
import threading
import time


class Endpoint:
    """A child process speaking a Content-Length framed JSON protocol on stdio."""

    def __init__(self, argv, env=None, cwd=None, log_path=None):
        self.log = open(log_path, "wb") if log_path else subprocess.DEVNULL
        self.proc = subprocess.Popen(
            argv,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=self.log,
            env=env,
            cwd=cwd,
        )
        self.messages = queue.Queue()
        self.reader = threading.Thread(target=self._read_loop, daemon=True)
        self.reader.start()
        self.next_id = 0

    def _read_loop(self):
        out = self.proc.stdout
        while True:
            headers = {}
            while True:
                line = out.readline()
                if not line:
                    self.messages.put(None)
                    return
                if line in (b"\r\n", b"\n"):
                    break
                key, _, value = line.decode("ascii", "replace").partition(":")
                headers[key.strip().lower()] = value.strip()
            length = int(headers.get("content-length", "0"))
            body = out.read(length)
            self.messages.put(json.loads(body))

    def send(self, obj):
        data = json.dumps(obj).encode()
        self.proc.stdin.write(b"Content-Length: %d\r\n\r\n" % len(data) + data)
        self.proc.stdin.flush()

    def next_message(self, deadline):
        """The next message, or None when the process exited or the deadline passed."""
        left = deadline - time.monotonic()
        if left <= 0:
            return None
        try:
            return self.messages.get(timeout=left)
        except queue.Empty:
            return None

    def close(self, timeout=10):
        try:
            self.proc.stdin.close()
        except OSError:
            pass
        try:
            self.proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait()
        if self.log is not subprocess.DEVNULL:
            self.log.close()


def file_uri(path):
    return pathlib.Path(path).resolve().as_uri()


class LspClient(Endpoint):
    def __init__(self, binary, root, init_options=None, env=None, log_path=None):
        super().__init__([binary], env=env, cwd=root, log_path=log_path)
        self.diagnostics = {}
        self.request(
            "initialize",
            {
                "processId": os.getpid(),
                "rootUri": file_uri(root),
                "capabilities": {
                    "general": {"positionEncodings": ["utf-16"]},
                    "textDocument": {"diagnostic": {}},
                    "window": {"workDoneProgress": False},
                },
                "initializationOptions": init_options or {},
                "locale": "en",
            },
        )
        self.notify("initialized", {})

    def notify(self, method, params):
        self.send({"jsonrpc": "2.0", "method": method, "params": params})

    def request(self, method, params, timeout=900):
        """Send a request and wait for its response. Returns (result, error)."""
        self.next_id += 1
        rid = self.next_id
        self.send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
        deadline = time.monotonic() + timeout
        while True:
            msg = self.next_message(deadline)
            if msg is None:
                raise TimeoutError(f"no response to {method} within {timeout}s")
            if "method" in msg and "id" in msg:
                # server-to-client request (progress, registrations): accept it
                self.send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
            elif "method" in msg:
                if msg["method"] == "textDocument/publishDiagnostics":
                    p = msg["params"]
                    self.diagnostics[p["uri"]] = p["diagnostics"]
            elif msg.get("id") == rid:
                return msg.get("result"), msg.get("error")

    def did_open(self, path, text):
        self.notify(
            "textDocument/didOpen",
            {
                "textDocument": {
                    "uri": file_uri(path),
                    "languageId": "java",
                    "version": 1,
                    "text": text,
                }
            },
        )

    def project_classpath(self, path, scope="runtime"):
        result, error = self.request(
            "java-analyzer/projectClasspath", {"uri": file_uri(path), "scope": scope}
        )
        if error:
            raise RuntimeError(f"projectClasspath {path}: {error.get('message')}")
        return result

    def shutdown(self):
        try:
            self.request("shutdown", None, timeout=30)
            self.notify("exit", None)
        except (TimeoutError, OSError):
            pass
        self.close()


class DapClient(Endpoint):
    def __init__(self, binary, env=None, cwd=None, log_path=None):
        super().__init__([binary, "dap"], env=env, cwd=cwd, log_path=log_path)
        self.seq = 0
        self.output = []  # (category, text)
        self.events = []

    def _send_request(self, command, arguments):
        self.seq += 1
        self.send({"seq": self.seq, "type": "request", "command": command, "arguments": arguments})
        return self.seq

    def _absorb(self, msg):
        if msg.get("type") == "event":
            if msg["event"] == "output":
                body = msg.get("body", {})
                self.output.append((body.get("category", "console"), body.get("output", "")))
            self.events.append(msg["event"])

    def request(self, command, arguments, timeout=900):
        seq = self._send_request(command, arguments)
        deadline = time.monotonic() + timeout
        while True:
            msg = self.next_message(deadline)
            if msg is None:
                raise TimeoutError(f"no response to {command} within {timeout}s")
            if msg.get("type") == "response" and msg.get("request_seq") == seq:
                return msg
            self._absorb(msg)

    def pump(self, until, timeout):
        """Collect events until `until(self)` is true, the adapter exits or `timeout` passes."""
        deadline = time.monotonic() + timeout
        while not until(self):
            msg = self.next_message(deadline)
            if msg is None:
                return False
            self._absorb(msg)
        return True

    def text(self):
        return "".join(text for _, text in self.output)
