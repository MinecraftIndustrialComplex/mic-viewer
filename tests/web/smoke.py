#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0
"""The viewer page's smoke test: builds and starts the demo world, drives headless Chromium over
the DevTools protocol (shot.py), checks the page, and writes the screenshots in docs/screenshots/.

    nix develop -c python3 tests/web/smoke.py [--no-build] [--url http://127.0.0.1:PORT/] [--out DIR]

Checks: no console errors, exceptions or CSP violations, and only GETs; the map drew tiles and
marks; the layer panel lists the demo's 14 layers; clicking a village opens its card; the theme
toggle switches; the timeline opens, scrubs into the past and back; the `demo:ring` frame draws
across its seam; at 390 x 844 there is no horizontal scroll, the panels open as sheets inside the
screen, and touch pan and pinch move the map; on a second, stepped demo (`--stepped`), stream
`frame` messages move the clock and refetch the marks that moved, and live event rows reach the frame
they belong to. Exits 0 only when every check passes. Stops only the demo servers and the browser it
started.
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import shot  # noqa: E402

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
MAX_PNG = 600 * 1024
LAYERS = 14

# Records CSP violations in the page (DevTools scripts run outside the page's policy).
RECORDER = """
window.__mv = { csp: [] };
document.addEventListener('securitypolicyviolation', e => {
  window.__mv.csp.push(e.violatedDirective + ' ' + (e.blockedURI || '') + ' ' + (e.sourceFile || '') + ':' + (e.lineNumber || 0));
});
"""

READY = "!!(window.MicViewer && MicViewer.app && MicViewer.app.ready && MicViewer.app.idle())"


class Smoke:
    def __init__(self, browser, base):
        self.b = browser
        self.base = base
        self.failures = []
        self.passed = 0
        self.csp = []

    def check(self, ok, what, detail=""):
        if ok:
            self.passed += 1
            print(f"  ok    {what}")
        else:
            self.failures.append(f"{what}{': ' + str(detail) if detail else ''}")
            print(f"  FAIL  {what}{': ' + str(detail) if detail else ''}")
        return ok

    def collect_csp(self):
        v = self.b.eval("window.__mv ? window.__mv.csp : []") or []
        self.csp.extend(v)

    def load(self, hash_=""):
        self.collect_csp_safe()
        # Through a blank page, so a link that differs only in its hash is a fresh load.
        self.b.navigate("about:blank")
        self.b.wait_for("location.href === 'about:blank'", timeout=5)
        self.b.navigate(self.base + hash_)
        time.sleep(0.3)
        ok = self.b.wait_for(READY, timeout=30)
        # Tiles decode and the stream says hello: let a few frames pass, then wait for idle again.
        time.sleep(0.6)
        self.b.wait_for(READY, timeout=20)
        return ok

    def collect_csp_safe(self):
        try:
            self.collect_csp()
        except Exception:  # noqa: BLE001
            pass

    def settle(self, timeout=15):
        time.sleep(0.25)
        return self.b.wait_for(READY, timeout=timeout)

    def shot(self, name, scale=None):
        path = os.path.join(self.out, name)
        self.b.screenshot(path, scale=scale)
        n = shrink(path)
        self.check(n <= MAX_PNG, f"{name} is under {MAX_PNG // 1024} KB", f"{n // 1024} KB")
        print(f"        {path}: {n // 1024} KB")
        return path

    def canvas_ink(self):
        """Share of the map canvas that is not its background colour, and per half."""
        return self.b.eval("""(() => {
          const c = document.getElementById('map');
          const g = c.getContext('2d');
          const d = g.getImageData(0, 0, c.width, c.height).data;
          const bg = getComputedStyle(document.documentElement).getPropertyValue('--map-paper').trim();
          const m = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(bg);
          const [r0, g0, b0] = m ? [1, 2, 3].map(i => parseInt(m[i], 16)) : [0, 0, 0];
          let ink = 0, left = 0, right = 0, n = 0;
          const W = c.width;
          for (let i = 0; i < d.length; i += 4 * 7) {
            n++;
            const px = (i / 4) % W;
            if (Math.abs(d[i] - r0) + Math.abs(d[i + 1] - g0) + Math.abs(d[i + 2] - b0) > 24) {
              ink++;
              if (px < W / 2) left++; else right++;
            }
          }
          return { ink: ink / n, left: left / n * 2, right: right / n * 2 };
        })()""")


def shrink(path):
    """Quantizes a screenshot to a palette (pngquant through nix; the map's terrain makes a full
    colour PNG of a desk view about 700 KB). Keeps the original when pngquant is not there."""
    tmp = path + ".q.png"
    try:
        cmd = ["pngquant", "--quality", "80-98", "--speed", "1", "--force", "-o", tmp, path]
        if not shutil.which("pngquant"):
            cmd = ["nix", "shell", "nixpkgs#pngquant", "-c"] + cmd
        r = subprocess.run(cmd, capture_output=True, timeout=300)
        if r.returncode == 0 and os.path.exists(tmp) and os.path.getsize(tmp) < os.path.getsize(path):
            os.replace(tmp, path)
    except Exception as e:  # noqa: BLE001
        print(f"        (not quantized: {e})")
    finally:
        if os.path.exists(tmp):
            os.remove(tmp)
    return os.path.getsize(path)


def get_json(base, path):
    with urllib.request.urlopen(base + path, timeout=10) as r:
        return json.load(r)


def start_demo(build, clock=("--still",)):
    if build:
        print("building the demo (cargo build --release -p mic-viewer-demo)")
        subprocess.run(["nice", "-n", "10", "cargo", "build", "--release", "-p", "mic-viewer-demo"], cwd=ROOT, check=True)
    port = shot.free_port()
    exe = os.path.join(ROOT, "target", "release", "mic-viewer-demo")
    proc = subprocess.Popen([exe, *clock, "--port", str(port)], cwd=ROOT,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    base = f"http://127.0.0.1:{port}/"
    for _ in range(100):
        try:
            get_json(base, "api/world")
            return proc, base
        except Exception:  # noqa: BLE001
            if proc.poll() is not None:
                raise SystemExit("smoke: the demo server exited")
            time.sleep(0.1)
    proc.terminate()
    raise SystemExit("smoke: the demo server did not answer")


def desktop(s, base):
    b = s.b
    print("desktop, 1440 x 900, dark")
    b.size(1440, 900)
    b.media(prefers_color_scheme="dark", prefers_reduced_motion="reduce")
    s.check(s.load(), "the page starts and settles")
    s.check(b.eval("document.documentElement.dataset.theme === undefined && getComputedStyle(document.body).backgroundColor === 'rgb(18, 20, 23)'"),
            "the dark theme follows prefers-color-scheme")
    n = b.eval("document.querySelectorAll('#side .lrow[data-layer]').length")
    s.check(n == LAYERS, f"the layer panel lists {LAYERS} layers", n)
    s.check(b.eval("!document.getElementById('side').hidden"), "the layer panel is open on a desk")
    st = b.eval("MicViewer.app.map.stats()")
    drawn = b.eval("MicViewer.app.lastDraw ? MicViewer.app.lastDraw.drawn : 0")
    s.check(st["tiles"] > 0, "the map drew tiles", st)
    s.check(drawn > 50, "the map drew marks", drawn)
    ink = s.canvas_ink()
    s.check(ink["ink"] > 0.3, "the canvas has more than background on it", ink)
    counts = b.eval("[...document.querySelectorAll('#side .lrow.on .lcount')].map(e => e.textContent).filter(Boolean).length")
    s.check(counts >= 5, "the panel shows counts for the layers on", counts)
    key = b.wait_for("(() => { const k = document.getElementById('key'); const n = k.hidden ? 0 : k.querySelectorAll('li').length; return n || null; })()", timeout=5)
    s.check((key or 0) >= 5, "the map key names the symbols on screen", key)
    s.shot("desktop-dark.png")

    # The theme toggle (a real click).
    b.click_selector("#btn-theme")
    time.sleep(0.2)
    s.check(b.eval("document.documentElement.dataset.theme === 'light' && getComputedStyle(document.body).backgroundColor !== 'rgb(18, 20, 23)'"),
            "the theme toggle switches to light")
    s.check(b.eval("localStorage.getItem('micviewer.theme') === '\"light\"'"), "the theme is kept for this viewer")
    s.settle()
    s.shot("desktop-light.png")
    b.click_selector("#btn-theme")
    time.sleep(0.2)
    s.check(b.eval("document.documentElement.dataset.theme === 'dark'"), "the theme toggle switches back to dark")
    s.settle()

    # A village: find one through /api/marks, click it on the canvas, read its card.
    target = b.eval("""(async () => {
      const app = MicViewer.app, map = app.map, v = map.view;
      const box = MicViewer.viewBox(v, -0.12);
      const url = MicViewer.marksURL(app.state.frame, box, v.bpp, ['settle.villages'], null);
      const ans = await (await fetch(url)).json();
      const r = document.getElementById('map').getBoundingClientRect();
      const card = document.getElementById('card').getBoundingClientRect();
      for (const m of ans.layers['settle.villages'].marks) {
        if (m.k !== 'point') continue;
        const [px, py] = map.toScreen(m.x, m.z);
        if (px < 60 || py < 60 || px > v.w - 420 || py > v.h - 80) continue;
        const hit = MicViewer.pick(app.lastDraw.targets, px, py);
        if (hit && hit.layer === 'settle.villages' && hit.id === String(m.id)) return { id: String(m.id), x: r.left + px, y: r.top + py };
      }
      return null;
    })()""")
    if s.check(bool(target), "a village from /api/marks is on the map and hit-tests", target):
        b.move(target["x"] - 30, target["y"] - 30)
        b.move(target["x"], target["y"])
        time.sleep(0.15)
        s.check(b.eval("!document.getElementById('tip').hidden"), "hovering the village shows a tooltip")
        b.click(target["x"], target["y"])
        card = get_json(base, f"api/thing/settle.villages/{target['id']}")
        title = b.wait_for(f"(() => {{ const t = document.querySelector('#card-body .card-title'); return !document.getElementById('card').hidden && t && t.textContent === {json.dumps(card['title'])} ? t.textContent : null; }})()", timeout=8)
        s.check(bool(title), "clicking the village opens its card", card["title"])
        s.check(b.eval("document.querySelectorAll('#card-body .kv dt').length") >= 2, "the card shows its fields")
        narrow = b.eval("[...document.querySelectorAll('#card-body .kv')].filter(d => [...d.querySelectorAll('dd')].some(v => v.clientWidth < 0.3 * d.clientWidth)).length")
        s.check(narrow == 0, "every card value keeps at least 30% of its row", f"{narrow} lists squeezed")
        s.check(b.eval("!!document.querySelector('#card-body .spark-svg path.spark-line')"), "the card draws its series as a sparkline")
        s.check(bool(b.wait_for("location.hash.includes('card=settle.villages/')", timeout=3)), "the card is in the URL hash")
        link = b.eval("!!document.querySelector('#card-body .linkbtn')")
        if s.check(link, "the card has a link"):
            b.click_selector("#card-body .linkbtn")
            ok = b.wait_for("(() => { const t = document.querySelector('#card-body .card-title'); return !document.getElementById('card-back').hidden && t && !t.closest('.card-state'); })()", timeout=8)
            s.check(bool(ok), "a link opens the linked card, with Back")
            b.click_selector("#card-back")
            ok = b.wait_for(f"(() => {{ const t = document.querySelector('#card-body .card-title'); return t && t.textContent === {json.dumps(card['title'])}; }})()", timeout=8)
            s.check(bool(ok), "Back returns to the village")

    # The timeline.
    b.click_selector("#btn-timeline")
    ok = b.wait_for("!document.getElementById('tl').hidden && document.querySelectorAll('#tl .tl-row').length > 0", timeout=10)
    s.check(bool(ok), "the timeline opens with events in view")
    s.settle()
    s.check(b.eval("(() => { const c = document.querySelector('#tl .tl-canvas'); const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data; let n = 0; for (let i = 3; i < d.length; i += 16) if (d[i] > 0) n++; return n > 500; })()"),
            "the histogram drew")
    n_kinds = b.eval("document.querySelectorAll('#tl .tl-kind').length")
    s.check(n_kinds == len(get_json(base, "api/world")["event_kinds"]), "the timeline offers every event kind", n_kinds)
    s.settle()
    s.shot("card-timeline.png")

    # Scrub into the past (a real click on the histogram) and back with Now.
    r = b.eval("(() => { const r = document.querySelector('#tl .tl-canvas').getBoundingClientRect(); return [r.left + r.width * 0.55, r.top + r.height / 2]; })()")
    b.click(r[0], r[1])
    ok = b.wait_for("!document.getElementById('past').hidden && MicViewer.app.state.t != null", timeout=6)
    s.check(bool(ok), "a click on the histogram shows the past")
    s.settle()
    s.check(bool(b.wait_for("MicViewer.app.state.marks.t === MicViewer.app.state.t && location.hash.includes('t=')", timeout=4)),
            "marks are asked at that time and the hash keeps it")
    b.click_selector("#past-now")
    ok = b.wait_for("document.getElementById('past').hidden && MicViewer.app.state.t == null", timeout=6)
    s.check(bool(ok), "Now returns to the present")
    s.settle()

    # An event row flies to the event.
    before = b.eval("[MicViewer.app.map.view.cx, MicViewer.app.map.view.cz]")
    b.click_selector("#tl .tl-row")
    time.sleep(0.3)
    s.settle()
    after = b.eval("[MicViewer.app.map.view.cx, MicViewer.app.map.view.cz, MicViewer.app.state.eventSel]")
    ev = after[2] or {}
    s.check(after[2] is not None and abs(after[0] - ev.get("x", 1e9)) < 2 and abs(after[1] - ev.get("z", 1e9)) < 2 and before[:2] != after[:2],
            "clicking an event flies to it", after)

    # Keyboard: the canvas takes focus and arrows pan.
    b.eval("document.getElementById('map').focus()")
    v0 = b.eval("MicViewer.app.map.view.cx")
    b.call("Input.dispatchKeyEvent", type="keyDown", key="ArrowRight", code="ArrowRight", windowsVirtualKeyCode=39)
    b.call("Input.dispatchKeyEvent", type="keyUp", key="ArrowRight", code="ArrowRight", windowsVirtualKeyCode=39)
    s.check(b.eval("MicViewer.app.map.view.cx") > v0, "the arrow keys pan the map")
    b.call("Input.dispatchKeyEvent", type="keyDown", key="Escape", code="Escape", windowsVirtualKeyCode=27)
    b.call("Input.dispatchKeyEvent", type="keyUp", key="Escape", code="Escape", windowsVirtualKeyCode=27)
    s.check(b.eval("document.getElementById('card').hidden"), "Escape closes the card")

    # A deep link restores the view.
    s.load("#frame=minecraft:overworld&x=1000&z=-500&bpp=2&layers=settle.villages,travel.roads")
    v = b.eval("[MicViewer.app.map.view.cx, MicViewer.app.map.view.cz, MicViewer.app.map.view.bpp, [...MicViewer.app.state.on].sort().join(',')]")
    s.check(abs(v[0] - 1000) < 1 and abs(v[1] + 500) < 1 and abs(v[2] - 2) < 1e-6 and v[3] == "settle.villages,travel.roads",
            "a deep link restores frame, view and layers", v)

    # The ring frame wraps east to west: draw across the seam.
    print("the demo:ring frame")
    s.load("#frame=demo:ring&x=2048&z=100&bpp=1.5")
    seam = b.eval("""(() => {
      const app = MicViewer.app, v = app.map.view, d = app.lastDraw;
      const tr = d.tracks.filter(t => t[1] === 'seam');
      return { cx: v.cx, w: v.w, left: tr.filter(t => Math.min(t[2], t[4]) < v.w / 2 - 20).length,
               right: tr.filter(t => Math.max(t[2], t[4]) > v.w / 2 + 20).length,
               pts: d.targets.filter(t => t.kind === 'pt' && t.layer === 'settle.villages').map(t => t.x) };
    })()""")
    s.check(seam["left"] > 0 and seam["right"] > 0, "the seam leg is drawn on both sides of the seam", seam)
    pts = seam["pts"]
    s.check(any(x < seam["w"] / 2 for x in pts) and any(x > seam["w"] / 2 for x in pts),
            "villages are drawn on both sides of the seam", len(pts))
    ink = s.canvas_ink()
    s.check(ink["left"] > 0.001 and ink["right"] > 0.001, "both halves of the canvas have ink", ink)
    s.check(b.eval("document.getElementById('frame').value === 'demo:ring'"), "the frame switcher shows the ring")


CARAVAN_VERSION = "(MicViewer.app.state.marks.layers['travel.caravans'] || {}).version"


def live(s):
    """On a stepped demo: the stream's `frame` messages and live event rows. A live world animates its
    caravans every frame, so the page is never idle here: wait for it to be loaded instead."""
    b = s.b
    print("live updates, a stepped demo (2000 ticks every 0.7 s)")
    b.size(1440, 900)
    loaded = "!!(window.MicViewer && MicViewer.app && MicViewer.app.ready && MicViewer.app.state.pending === 0)"

    def open_live(hash_):
        b.navigate("about:blank")
        b.wait_for("location.href === 'about:blank'", timeout=5)
        b.navigate(s.base + hash_)
        time.sleep(0.3)
        return bool(b.wait_for(loaded, timeout=30))

    s.check(open_live("#frame=minecraft:overworld&x=0&z=0&bpp=12"), "the page starts on a stepped demo")
    st0 = b.eval(f"({{ heard: MicViewer.app.state.heard, tick: MicViewer.app.state.time.tick, v: {CARAVAN_VERSION} }})")
    s.check(st0["v"] is not None, "the caravans are shown", st0)
    f0 = st0["heard"]["frames"]
    ok = b.wait_for(f"MicViewer.app.state.heard.frames >= {f0 + 3} && MicViewer.app.state.heard.rows > 0", timeout=20)
    st = b.eval(f"({{ heard: MicViewer.app.state.heard, tick: MicViewer.app.state.time.tick, v: {CARAVAN_VERSION} }})")
    s.check(bool(ok) and st["tick"] > st0["tick"], "stream frame messages move the clock and live rows arrive", [st0, st])
    ok = b.wait_for(f"{CARAVAN_VERSION} != null && {CARAVAN_VERSION} !== {json.dumps(st0['v'])}", timeout=10)
    s.check(bool(ok), "a frame message refetches the caravans that moved", [st0["v"], b.eval(CARAVAN_VERSION)])
    drawn = b.eval("MicViewer.app.lastDraw ? MicViewer.app.lastDraw.drawn : 0")
    s.check(drawn > 0 and b.wait_for(loaded, timeout=10), "the map keeps drawing and its requests finish between publishes", drawn)
    s.check(open_live("#frame=demo:ring&x=0&z=0&bpp=4"), "the ring frame opens on the stepped demo")
    ok = b.wait_for("MicViewer.app.state.heard.rows > 0", timeout=30)
    s.check(bool(ok), "the ring frame takes its own live rows", b.eval("MicViewer.app.state.heard"))


def phone(s):
    b = s.b
    print("phone, 390 x 844")
    b.size(390, 844, scale=2, mobile=True)
    b.media(prefers_color_scheme="dark", prefers_reduced_motion="reduce")
    s.check(s.load("#frame=minecraft:overworld"), "the page starts on a phone")
    over = b.eval("[document.documentElement.scrollWidth, document.body.scrollWidth, innerWidth]")
    s.check(over[0] <= over[2] and over[1] <= over[2], "no horizontal scroll", over)
    s.check(b.eval("document.getElementById('side').hidden && document.getElementById('tl').hidden"), "the panels start closed")
    outside = b.eval("""[...document.querySelectorAll('.top button, .top select, .zoom button, #source')]
      .filter(e => e.offsetParent !== null || e.id === 'source')
      .map(e => [e.id || e.className, e.getBoundingClientRect()])
      .filter(([, r]) => r.left < 0 || r.right > innerWidth || r.top < 0 || r.bottom > innerHeight)
      .map(([n]) => n)""")
    s.check(outside == [], "every control is on the screen", outside)
    gutter = b.eval("(() => { const t = document.querySelector('.top'); const cs = getComputedStyle(t); return [cs.paddingLeft, cs.paddingRight]; })()")
    s.check(gutter == ["16px", "16px"], "16 px side gutters", gutter)

    # Touch pan and pinch.
    v0 = b.eval("[MicViewer.app.map.view.cx, MicViewer.app.map.view.bpp]")
    b.drag_touch((200, 420), (120, 380))
    time.sleep(0.2)
    v1 = b.eval("[MicViewer.app.map.view.cx, MicViewer.app.map.view.bpp]")
    s.check(v1[0] > v0[0] + 10, "a touch drag pans the map", [v0, v1])
    b.pinch((195, 420), 80, 240)
    time.sleep(0.2)
    v2 = b.eval("MicViewer.app.map.view.bpp")
    s.check(v2 < v1[1] * 0.6, "a pinch zooms in", [v1[1], v2])
    b.eval("MicViewer.app.map.home()")
    s.settle()
    s.shot("phone.png", scale=1)

    # The layers sheet: inside the screen, its checkboxes reachable.
    b.click_selector("#btn-layers")
    time.sleep(0.3)
    r = b.eval("(() => { const r = document.getElementById('side').getBoundingClientRect(); return [r.left, r.top, r.right, r.bottom, innerWidth, innerHeight]; })()")
    s.check(not b.eval("document.getElementById('side').hidden") and r[0] >= 0 and r[2] <= r[4] and r[3] <= r[5] + 1 and r[1] > 0,
            "the layers open as a sheet inside the screen", r)
    hit = b.eval("""(() => { const cb = document.querySelector('#side .lrow[data-layer] input'); const r = cb.getBoundingClientRect();
      const e = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2); return !!e && (e === cb || e.closest('.lrow-main') === cb.closest('.lrow-main')); })()""")
    s.check(hit, "a layer checkbox can be reached")
    s.shot("phone-layers.png", scale=1)
    b.click_selector("#btn-timeline")
    time.sleep(0.4)
    s.check(b.eval("document.getElementById('side').hidden && !document.getElementById('tl').hidden"), "one sheet at a time: the timeline replaces the layers")
    r = b.eval("(() => { const r = document.getElementById('tl').getBoundingClientRect(); return [r.left, r.right, r.bottom, innerWidth, innerHeight]; })()")
    s.check(r[0] >= 0 and r[1] <= r[3] and r[2] <= r[4] + 1, "the timeline sheet is inside the screen", r)
    over = b.eval("[document.documentElement.scrollWidth, innerWidth]")
    s.check(over[0] <= over[1], "no horizontal scroll with a sheet open", over)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--no-build", action="store_true", help="use the demo binary already built")
    ap.add_argument("--url", help="test a viewer already running here instead of starting the demo")
    ap.add_argument("--out", default=os.path.join(ROOT, "docs", "screenshots"))
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    proc = live_proc = None
    if a.url:
        base = a.url if a.url.endswith("/") else a.url + "/"
    else:
        proc, base = start_demo(not a.no_build)
    try:
        with shot.Browser(1440, 900) as b:
            s = Smoke(b, base)
            s.out = a.out
            for d in ("Page", "Runtime", "Log", "Network"):
                b.call(f"{d}.enable")
            b.call("Page.addScriptToEvaluateOnNewDocument", source=RECORDER)
            desktop(s, base)
            phone(s)
            if not a.url:
                live_proc, s.base = start_demo(False, ("--stepped", "2000", "--every", "700"))
                live(s)
                s.base = base
            s.collect_csp_safe()
            b.eval("1")
            errors, methods = [], set()
            for e in b.ws.events:
                m, p = e["method"], e.get("params", {})
                if m == "Runtime.consoleAPICalled" and p.get("type") in ("error", "assert"):
                    errors.append("console: " + " ".join(str(x.get("value", x.get("description", ""))) for x in p.get("args", [])))
                elif m == "Runtime.exceptionThrown":
                    d = p.get("exceptionDetails", {})
                    errors.append("exception: " + ((d.get("exception") or {}).get("description") or d.get("text", "")))
                elif m == "Log.entryAdded" and p.get("entry", {}).get("level") == "error":
                    en = p["entry"]
                    errors.append(f"log ({en.get('source')}): {en.get('text')} {en.get('url', '')}")
                elif m == "Network.requestWillBeSent":
                    methods.add(p.get("request", {}).get("method"))
            s.check(not errors, "no console errors or exceptions", errors[:8])
            s.check(not s.csp, "no CSP violations", s.csp[:8])
            s.check(methods <= {"GET"}, "the page sends only GETs", sorted(m for m in methods if m))
    finally:
        for p in (proc, live_proc):
            if p:
                p.terminate()
                try:
                    p.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    p.kill()
    print(f"\n{s.passed} passed, {len(s.failures)} failed")
    for f in s.failures:
        print(f"  - {f}")
    sys.exit(0 if not s.failures else 1)


if __name__ == "__main__":
    main()
