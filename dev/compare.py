"""Render the same map, skin and times with Skin Studio's renderer (mapview) and with osu!lazer's own (the lazer-preview host),
and write the two frames side by side. A development tool: the host is the reference the Rust renderer has to match.

usage: python dev/compare.py --map file.osu --skin <folder|.osk|argon> --t 24500,26500 [--host <dir with lazer-preview.exe>] [--out <dir>]
Windows only (it reads the host's window with PrintWindow).
"""
import argparse, ctypes, json, os, subprocess, sys, time, zipfile
from PIL import Image

ap = argparse.ArgumentParser()
ap.add_argument("--map", required=True)
ap.add_argument("--skin", default="argon")
ap.add_argument("--t", required=True, help="comma separated times in ms")
ap.add_argument("--host", default=os.environ.get("LAZER_PREVIEW_DIR", os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "dev", "host")))
ap.add_argument("--out", default="dev/out")
ap.add_argument("--playing", action="store_true", help="real time: the frame is taken 300 ms after a seek and can be 30 ms off, but the cursor trail (a wall-clock effect in lazer) shows. The default plays the last 300 ms at quarter speed, so a timing slop of tens of ms is a few ms of game time and every fade and exit animation runs as it does in the game")
a = ap.parse_args()
os.makedirs(a.out, exist_ok=True)
times = [float(x) for x in a.t.split(",")]
root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# ---- the Rust renderer
subprocess.run(["cargo", "run", "--release", "-q", "-p", "mapview", "--", "--map", a.map, "--skin", a.skin, "--t", a.t, "--out", os.path.join(a.out, "rs")], cwd=root, check=True)

# ---- the reference
user32 = ctypes.windll.user32
gdi32 = ctypes.windll.gdi32


def grab(h, path):
    r = ctypes.wintypes.RECT()
    user32.GetClientRect(h, ctypes.byref(r))
    w, ht = r.right, r.bottom
    hdc = user32.GetDC(h)
    mdc = gdi32.CreateCompatibleDC(hdc)
    bmp = gdi32.CreateCompatibleBitmap(hdc, w, ht)
    gdi32.SelectObject(mdc, bmp)
    user32.PrintWindow(h, mdc, 3)  # client area only, with the window's own content
    class BI(ctypes.Structure):
        _fields_ = [("biSize", ctypes.c_uint32), ("biWidth", ctypes.c_int32), ("biHeight", ctypes.c_int32), ("biPlanes", ctypes.c_uint16), ("biBitCount", ctypes.c_uint16), ("biCompression", ctypes.c_uint32), ("biSizeImage", ctypes.c_uint32), ("x", ctypes.c_int32), ("y", ctypes.c_int32), ("a", ctypes.c_uint32), ("b", ctypes.c_uint32)]
    bi = BI(40, w, -ht, 1, 32, 0, 0, 0, 0, 0, 0)
    buf = ctypes.create_string_buffer(w * ht * 4)
    gdi32.GetDIBits(mdc, bmp, 0, ht, buf, ctypes.byref(bi), 0)
    Image.frombuffer("RGBA", (w, ht), buf, "raw", "BGRA", 0, 1).convert("RGB").save(path)
    gdi32.DeleteObject(bmp)
    gdi32.DeleteDC(mdc)
    user32.ReleaseDC(h, hdc)


import ctypes.wintypes  # noqa: E402

skin_path = "argonpro" if a.skin == "argon" else ""
if a.skin != "argon":
    # the studio hands lazer an .osk whose skin.ini carries a stamp, so that a variant is never taken for one imported before
    skin_path = os.path.join(a.out, "skin.osk")
    with zipfile.ZipFile(skin_path, "w", zipfile.ZIP_STORED) as z:
        if os.path.isdir(a.skin):
            for r, _, fs in os.walk(a.skin):
                for f in fs:
                    p = os.path.join(r, f)
                    n = os.path.relpath(p, a.skin).replace("\\", "/")
                    data = open(p, "rb").read()
                    if n.lower() == "skin.ini":
                        data += ("\n// compare %d\n" % time.time()).encode()
                    z.writestr(n, data)
        else:
            src = zipfile.ZipFile(a.skin)
            for n in src.namelist():
                data = src.read(n)
                if n.lower() == "skin.ini":
                    data += ("\n// compare %d\n" % time.time()).encode()
                z.writestr(n, data)

env = dict(os.environ, SKIN_STUDIO_HOST_DATA="-compare-%d" % time.time())
p = subprocess.Popen([os.path.join(a.host, "lazer-preview.exe")], stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, text=True, cwd=a.host, env=env)


def send(o):
    p.stdin.write(json.dumps(o) + "\n")
    p.stdin.flush()


try:
    time.sleep(3)
    send({"cmd": "skin", "path": skin_path, "id": 1})
    send({"cmd": "map", "path": a.map})
    time.sleep(10)
    h = user32.FindWindowW(None, "Skin Studio preview (osu!lazer)")
    for i, t in enumerate(times):
        if a.playing:
            send({"cmd": "seek", "ms": t - 300})
            send({"cmd": "play"})
            time.sleep(0.3)
            grab(h, os.path.join(a.out, "lz_%d.png" % i))
            send({"cmd": "pause"})
            time.sleep(0.3)
        else:
            send({"cmd": "pause"})
            send({"cmd": "speed", "value": 0.25})
            send({"cmd": "seek", "ms": t - 300})
            time.sleep(0.6)
            send({"cmd": "play"})
            time.sleep(1.2)
            grab(h, os.path.join(a.out, "lz_%d.png" % i))
            send({"cmd": "pause"})
            time.sleep(0.2)
finally:
    try:
        send({"cmd": "quit"})
    except Exception:
        pass
    time.sleep(1.5)
    if p.poll() is None:
        p.kill()

for i, t in enumerate(times):
    rs = Image.open(os.path.join(a.out, "rs_%d.png" % i)).convert("RGB")
    lz = Image.open(os.path.join(a.out, "lz_%d.png" % i)).convert("RGB")
    # the host window shows the playfield in its own proportions; both are shown at the same height
    h = 720
    rs = rs.resize((int(rs.width * h / rs.height), h))
    lz = lz.resize((int(lz.width * h / lz.height), h))
    sheet = Image.new("RGB", (rs.width + lz.width + 10, h), (40, 40, 40))
    sheet.paste(rs, (0, 0))
    sheet.paste(lz, (rs.width + 10, 0))
    sheet.save(os.path.join(a.out, "side_%d.png" % i))
    print("wrote", os.path.join(a.out, "side_%d.png" % i), "at", t, "ms")
