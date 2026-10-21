#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0
"""Drive headless Chromium over the DevTools protocol (stdlib only), for the viewer's smoke test.

Adapted from Deep Time's tools/viewer/shot.py (NOTICE.md): the minimal WebSocket client, the free
port and the font fallback are that file's; this one keeps the protocol's events (console errors,
exceptions, log entries, requests) instead of dropping them, and adds clicks, touches and waits.

As a command it screenshots a page:

    nix develop -c python3 tests/web/shot.py URL OUT.png [--wait S] [--size 1440x900] [--eval JS]...
        [--scheme light|dark] [--phone]
"""

import argparse
import base64
import json
import os
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import time
import urllib.request


def ensure_fonts():
    """Chromium aborts when a page lays out text and fontconfig finds no font ("Could not find any
    font"). Point FONTCONFIG_FILE at DejaVu from nixpkgs when nothing else is configured."""
    if os.environ.get("FONTCONFIG_FILE"):
        return
    if shutil.which("fc-list"):
        if subprocess.run(["fc-list"], capture_output=True, text=True).stdout.strip():
            return
    try:
        out = subprocess.run(["nix", "build", "nixpkgs#dejavu_fonts", "--no-link", "--print-out-paths"],
                             capture_output=True, text=True, timeout=600).stdout.split()
        fonts = next(o for o in out if os.path.isdir(os.path.join(o, "share", "fonts")))
    except Exception as e:  # noqa: BLE001
        print(f"shot.ensure_fonts: no fonts available ({e}); Chromium may abort", file=sys.stderr)
        return
    d = tempfile.mkdtemp(prefix="mv-fontconfig-")
    conf = os.path.join(d, "fonts.conf")
    with open(conf, "w") as f:
        f.write('<?xml version="1.0"?><!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd"><fontconfig>'
                f'<dir>{fonts}/share/fonts</dir><cachedir>{d}/cache</cachedir></fontconfig>')
    os.environ["FONTCONFIG_FILE"] = conf


class WS:
    """Just enough of RFC 6455 for the DevTools protocol (text frames, client masking). Messages
    that are not the answer being waited for are kept in `events`."""

    def __init__(self, url):
        assert url.startswith("ws://")
        hostport, path = url[5:].split("/", 1)
        host, port = hostport.split(":")
        self.sock = socket.create_connection((host, int(port)), timeout=60)
        key = base64.b64encode(os.urandom(16)).decode()
        self.sock.sendall(
            (f"GET /{path} HTTP/1.1\r\nHost: {hostport}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
             f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").encode())
        head = b""
        while b"\r\n\r\n" not in head:
            head += self.sock.recv(1)
        if b" 101 " not in head.split(b"\r\n", 1)[0]:
            raise RuntimeError(head.decode(errors="replace"))
        self.next_id = 0
        self.events = []

    def _recv_exact(self, n):
        buf = b""
        while len(buf) < n:
            chunk = self.sock.recv(n - len(buf))
            if not chunk:
                raise EOFError("DevTools socket closed")
            buf += chunk
        return buf

    def send(self, obj):
        data = json.dumps(obj).encode()
        mask = os.urandom(4)
        n = len(data)
        head = bytes([0x81])
        if n < 126:
            head += bytes([0x80 | n])
        elif n < 65536:
            head += bytes([0x80 | 126]) + struct.pack(">H", n)
        else:
            head += bytes([0x80 | 127]) + struct.pack(">Q", n)
        self.sock.sendall(head + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))

    def recv(self):
        parts = b""
        while True:
            b0, b1 = self._recv_exact(2)
            n = b1 & 0x7F
            if n == 126:
                n = struct.unpack(">H", self._recv_exact(2))[0]
            elif n == 127:
                n = struct.unpack(">Q", self._recv_exact(8))[0]
            payload = self._recv_exact(n)
            parts += payload
            if b0 & 0x80:
                return json.loads(parts)

    def call(self, method, **params):
        self.next_id += 1
        mid = self.next_id
        self.send({"id": mid, "method": method, "params": params})
        while True:
            msg = self.recv()
            if msg.get("id") == mid:
                if "error" in msg:
                    raise RuntimeError(f"{method}: {msg['error']}")
                return msg.get("result", {})
            if "method" in msg:
                self.events.append(msg)


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


def find_chromium():
    return shutil.which("chromium") or shutil.which("chromium-browser") or shutil.which("google-chrome")


class Browser:
    """A headless Chromium with one page, driven over DevTools. Use as a context manager: the
    browser and its profile go away at the end (only the processes this started)."""

    def __init__(self, width=1440, height=900):
        chrome = find_chromium()
        if not chrome:
            raise SystemExit("shot.py: no chromium on PATH (run through nix develop)")
        ensure_fonts()
        self.port = free_port()
        self.profile = tempfile.mkdtemp(prefix="mv-shot-")
        self.proc = subprocess.Popen(
            [chrome, "--headless=new", "--disable-gpu", "--no-sandbox", "--hide-scrollbars",
             "--no-first-run", "--no-default-browser-check", "--disable-extensions",
             f"--remote-debugging-port={self.port}", f"--user-data-dir={self.profile}",
             f"--window-size={width},{height}", "about:blank"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        page = None
        for _ in range(150):
            try:
                targets = json.load(urllib.request.urlopen(f"http://127.0.0.1:{self.port}/json/list", timeout=2))
                page = next(t for t in targets if t.get("type") == "page")
                break
            except Exception:  # noqa: BLE001
                time.sleep(0.2)
        if page is None:
            self.close()
            raise SystemExit("shot.py: Chromium's DevTools endpoint did not come up")
        self.ws = WS(page["webSocketDebuggerUrl"])

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()

    def close(self):
        try:
            self.proc.terminate()
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        except Exception:  # noqa: BLE001
            pass
        shutil.rmtree(self.profile, ignore_errors=True)

    def call(self, method, **params):
        return self.ws.call(method, **params)

    def eval(self, js, await_promise=True):
        """The value of a JS expression (JSON-able), raising on an exception in the page."""
        r = self.call("Runtime.evaluate", expression=js, awaitPromise=await_promise, returnByValue=True)
        if r.get("exceptionDetails"):
            d = r["exceptionDetails"]
            text = (d.get("exception") or {}).get("description") or d.get("text")
            raise RuntimeError(f"page threw on {js[:80]!r}: {text}")
        return r.get("result", {}).get("value")

    def wait_for(self, js, timeout=20.0, every=0.1):
        """Polls `js` until it is truthy; returns its value or None at the timeout."""
        end = time.time() + timeout
        while time.time() < end:
            try:
                v = self.eval(js)
            except RuntimeError:
                v = None
            if v:
                return v
            time.sleep(every)
        return None

    def size(self, width, height, scale=1, mobile=False):
        self.call("Emulation.setDeviceMetricsOverride", width=width, height=height,
                  deviceScaleFactor=scale, mobile=mobile)
        self.call("Emulation.setTouchEmulationEnabled", enabled=mobile, maxTouchPoints=5 if mobile else 1)

    def media(self, **features):
        self.call("Emulation.setEmulatedMedia",
                  features=[{"name": k.replace("_", "-"), "value": v} for k, v in features.items()])

    def navigate(self, url):
        self.call("Page.navigate", url=url)

    def click(self, x, y):
        for t in ("mouseMoved", "mousePressed", "mouseReleased"):
            p = dict(type=t, x=x, y=y, button="left" if t != "mouseMoved" else "none", clickCount=1 if t != "mouseMoved" else 0)
            self.call("Input.dispatchMouseEvent", **p)

    def move(self, x, y):
        self.call("Input.dispatchMouseEvent", type="mouseMoved", x=x, y=y, button="none")

    def click_selector(self, sel):
        """Clicks the middle of the first element matching `sel`; False when it is not on screen."""
        r = self.eval("(() => { const e = document.querySelector(" + json.dumps(sel) + "); if (!e) return null;"
                      " const r = e.getBoundingClientRect(); if (!r.width || !r.height) return null;"
                      " return [r.left + r.width / 2, r.top + r.height / 2]; })()")
        if not r:
            return False
        self.click(r[0], r[1])
        return True

    def touch(self, kind, points):
        self.call("Input.dispatchTouchEvent", type=kind,
                  touchPoints=[{"x": x, "y": y, "id": i} for i, (x, y) in enumerate(points)])

    def drag_touch(self, start, end, steps=8):
        self.touch("touchStart", [start])
        for i in range(1, steps + 1):
            u = i / steps
            self.touch("touchMove", [(start[0] + (end[0] - start[0]) * u, start[1] + (end[1] - start[1]) * u)])
        self.touch("touchEnd", [])

    def pinch(self, centre, d0, d1, steps=8):
        cx, cy = centre
        self.touch("touchStart", [(cx - d0 / 2, cy), (cx + d0 / 2, cy)])
        for i in range(1, steps + 1):
            d = d0 + (d1 - d0) * i / steps
            self.touch("touchMove", [(cx - d / 2, cy), (cx + d / 2, cy)])
        self.touch("touchEnd", [])

    def screenshot(self, path, scale=None):
        params = {"format": "png"}
        if scale:
            m = self.eval("[innerWidth, innerHeight]")
            params["clip"] = {"x": 0, "y": 0, "width": m[0], "height": m[1], "scale": scale}
        shot = self.call("Page.captureScreenshot", **params)
        with open(path, "wb") as f:
            f.write(base64.b64decode(shot["data"]))
        return os.path.getsize(path)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("url")
    ap.add_argument("out")
    ap.add_argument("--wait", type=float, default=4.0, help="seconds after loading, before the first --eval")
    ap.add_argument("--size", default="1440x900")
    ap.add_argument("--eval", action="append", default=[], help="JS to run, then wait --after seconds")
    ap.add_argument("--after", type=float, default=2.0)
    ap.add_argument("--scheme", choices=("light", "dark"), help="the prefers-color-scheme to emulate")
    ap.add_argument("--phone", action="store_true", help="a touch screen at 2 device pixels a pixel, shot at 1")
    a = ap.parse_args()
    w, h = (int(v) for v in a.size.split("x"))
    with Browser(w, h) as b:
        b.size(w, h, scale=2 if a.phone else 1, mobile=a.phone)
        if a.scheme:
            b.media(prefers_color_scheme=a.scheme)
        b.call("Page.enable")
        b.navigate(a.url)
        time.sleep(a.wait)
        for js in a.eval:
            try:
                b.eval(js)
            except RuntimeError as e:
                print(f"shot.py: {e}", file=sys.stderr)
            time.sleep(a.after)
        n = b.screenshot(a.out, scale=1 if a.phone else None)
        print(f"{a.out}: {n} bytes")


if __name__ == "__main__":
    main()
