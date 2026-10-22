#!/usr/bin/env python3
"""Record real X11 input and rendering to MP4/GIF/PNG on a private display."""

import argparse
import csv
import hashlib
import ctypes as C
import ctypes.util
import json
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import tempfile
import threading
import time

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
LOCAL = ROOT / "artifacts/local/capture"
MEDIA = ROOT / "docs/media"


def wait_for(predicate, timeout=10):
    deadline = time.monotonic() + timeout
    while not predicate():
        if time.monotonic() > deadline:
            raise RuntimeError("capture prerequisite timed out; inspect artifacts/local/capture")
        time.sleep(0.05)


class Input:
    def __init__(self):
        self.x = C.CDLL(ctypes.util.find_library("X11"))
        self.xt = C.CDLL(ctypes.util.find_library("Xtst"))
        signatures = {
            "XOpenDisplay": ([C.c_char_p], C.c_void_p),
            "XDefaultRootWindow": ([C.c_void_p], C.c_ulong),
            "XQueryTree": ([C.c_void_p, C.c_ulong, C.POINTER(C.c_ulong), C.POINTER(C.c_ulong), C.POINTER(C.POINTER(C.c_ulong)), C.POINTER(C.c_uint)], C.c_int),
            "XFetchName": ([C.c_void_p, C.c_ulong, C.POINTER(C.c_void_p)], C.c_int),
            "XFree": ([C.c_void_p], C.c_int),
            "XSetInputFocus": ([C.c_void_p, C.c_ulong, C.c_int, C.c_ulong], C.c_int),
            "XFlush": ([C.c_void_p], C.c_int),
            "XKeysymToKeycode": ([C.c_void_p, C.c_ulong], C.c_uint),
            "XCloseDisplay": ([C.c_void_p], C.c_int),
        }
        for name, (args, result) in signatures.items():
            function = getattr(self.x, name)
            function.argtypes, function.restype = args, result
        self.xt.XTestFakeMotionEvent.argtypes = [C.c_void_p, C.c_int, C.c_int, C.c_int, C.c_ulong]
        self.xt.XTestFakeButtonEvent.argtypes = [C.c_void_p, C.c_uint, C.c_int, C.c_ulong]
        self.xt.XTestFakeKeyEvent.argtypes = [C.c_void_p, C.c_uint, C.c_int, C.c_ulong]
        self.display = self.x.XOpenDisplay(None)
        if not self.display:
            raise RuntimeError("cannot open private X11 display")

    def focus(self):
        root = self.x.XDefaultRootWindow(self.display)
        parent, returned = C.c_ulong(), C.c_ulong()
        children, count = C.POINTER(C.c_ulong)(), C.c_uint()
        self.x.XQueryTree(self.display, root, C.byref(returned), C.byref(parent), C.byref(children), C.byref(count))
        found = None
        try:
            for index in range(count.value):
                name = C.c_void_p()
                if self.x.XFetchName(self.display, children[index], C.byref(name)) and name.value:
                    title = C.string_at(name).decode(errors="replace")
                    self.x.XFree(name)
                    if title.startswith("Charon"):
                        found = children[index]
                        break
        finally:
            if children:
                self.x.XFree(children)
        if found is None:
            raise RuntimeError("sandbox window not found")
        self.x.XSetInputFocus(self.display, found, 2, 0)
        self.x.XFlush(self.display)

    def move(self, x, y):
        self.xt.XTestFakeMotionEvent(self.display, 0, x, y, 0)
        self.x.XFlush(self.display)

    def button(self, button, down):
        self.xt.XTestFakeButtonEvent(self.display, button, int(down), 0)
        self.x.XFlush(self.display)

    def key(self, symbol):
        code = self.x.XKeysymToKeycode(self.display, symbol)
        for down in (1, 0):
            self.xt.XTestFakeKeyEvent(self.display, code, down, 0)
        self.x.XFlush(self.display)

    def drag(self, button, points, delay=0.04):
        self.move(*points[0])
        self.button(button, True)
        try:
            for point in points:
                self.move(*point)
                time.sleep(delay)
        finally:
            self.button(button, False)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sample", choices=("sandbox", "dialogue"), default="sandbox")
    sample = parser.parse_args().sample
    package = "charon-sandbox" if sample == "sandbox" else "charon-dialogue-sample"
    prefix = f"{sample}-linux"
    local = LOCAL / sample
    duration = 14 if sample == "sandbox" else 12
    os.chdir(ROOT)
    local.mkdir(parents=True, exist_ok=True)
    MEDIA.mkdir(parents=True, exist_ok=True)
    xorg = "/usr/lib/xorg/Xorg"
    for tool in (xorg, "xauth", "import", "ffmpeg", "cargo"):
        if not shutil.which(tool):
            raise SystemExit(f"missing recording prerequisite: {tool}")
    if not Path("/usr/lib/xorg/modules/drivers/dummy_drv.so").exists():
        raise SystemExit("Xorg dummy driver is required for this Linux capture helper")
    subprocess.run(["cargo", "build", "--locked", "-p", package], check=True)
    processes = []
    inputs = None
    capture_stop = threading.Event()
    capture_thread = None
    with tempfile.TemporaryDirectory(prefix="charon-capture-") as temporary:
        temporary = Path(temporary)
        number = next(n for n in range(91, 121) if not Path(f"/tmp/.X{n}-lock").exists() and not Path(f"/tmp/.X11-unix/X{n}").exists())
        env = os.environ.copy()
        env.update(DISPLAY=f":{number}", XAUTHORITY=str(temporary / "auth"),
                   XDG_RUNTIME_DIR=str(temporary), MESA_SHADER_CACHE_DIR=str(temporary / "mesa"),
                   LP_NUM_THREADS="2")
        os.environ.update({key: env[key] for key in ("DISPLAY", "XAUTHORITY")})
        subprocess.run(["xauth", "-f", env["XAUTHORITY"], "add", env["DISPLAY"], ".", secrets.token_hex(16)], check=True, capture_output=True)
        config = temporary / "xorg.conf"
        config.write_text('''Section "ServerFlags"
 Option "AutoAddDevices" "false"
 Option "AutoEnableDevices" "false"
EndSection
Section "Device"
 Identifier "dummy"
 Driver "dummy"
 VideoRam 192000
EndSection
Section "Monitor"
 Identifier "monitor"
 HorizSync 5.0-1000.0
 VertRefresh 5.0-200.0
 Modeline "1920x1080" 148.50 1920 2008 2052 2200 1080 1084 1089 1125
EndSection
Section "Screen"
 Identifier "screen"
 Device "dummy"
 Monitor "monitor"
 DefaultDepth 24
 SubSection "Display"
  Depth 24
  Modes "1920x1080"
 EndSubSection
EndSection
''')
        try:
            with open(local / "xorg.log", "w") as log:
                server = subprocess.Popen([xorg, env["DISPLAY"], "-config", str(config), "-auth", env["XAUTHORITY"], "-logfile", str(temporary / "xorg.log"), "-nolisten", "tcp", "-noreset", "-novtswitch", "-extension", "MIT-SHM"], env=env, stdout=log, stderr=subprocess.STDOUT)
                processes.append(server)
            wait_for(lambda: Path(f"/tmp/.X11-unix/X{number}").exists())
            with open(local / f"{sample}.log", "w") as log:
                client = subprocess.Popen([str(ROOT / "target/debug" / package), *(["--workers", "4"] if sample == "sandbox" else []), "--seconds", "25", "--record", str(local / "metrics.csv")], env=env, stdout=log, stderr=subprocess.STDOUT)
                processes.append(client)
            wait_for(lambda: "surface=" in (local / f"{sample}.log").read_text())
            inputs = Input()
            inputs.focus()
            start = time.monotonic()
            frames, capture_errors = [], []
            def capture():
                try:
                    for index in range(duration * 8):
                        if capture_stop.wait(max(0, start + index / 8 - time.monotonic())):
                            break
                        if time.monotonic() >= start + duration:
                            break
                        path = temporary / f"frame-{index:04}.png"
                        before = time.monotonic()
                        # XGetImage works even when the virtual display has no
                        # shared-memory transport. This captures the actual window.
                        subprocess.run(["import", "-window", "root", "-filter", "point", "-resize", "960x540!", str(path)], env=env, check=True, capture_output=True, timeout=5)
                        frames.append(((before + time.monotonic()) / 2, path))
                except Exception as error:
                    capture_errors.append(error)
            capture_thread = threading.Thread(target=capture, name="screen-capture")
            capture_thread.start()
            def at(seconds):
                time.sleep(max(0, start + seconds - time.monotonic()))
            if sample == "sandbox":
                at(0.5)
                inputs.drag(2, [(x, 700) for x in range(700, 1221, 8)])
                at(3.4)
                inputs.drag(1, [(880 + (i % 20) * 8, 180) for i in range(65)])
                at(7)
                subprocess.run(["import", "-window", "root", str(MEDIA / f"{prefix}.png")], env=env, check=True, timeout=10)
                at(8)
                inputs.drag(3, [(960, y) for y in range(660, 741, 5)])
                at(11)
                inputs.key(ord("r"))
            else:
                at(1)
                inputs.key(ord(" "))
                at(3)
                inputs.key(ord("1"))
                at(5)
                inputs.key(ord("r"))
                at(7)
                inputs.key(ord(" "))
                at(9)
                inputs.key(ord("2"))
                at(10)
                subprocess.run(["import", "-window", "root", str(MEDIA / f"{prefix}.png")], env=env, check=True, timeout=10)
            capture_thread.join(timeout=10)
            if capture_thread.is_alive() or capture_errors:
                raise RuntimeError(f"screen capture failed: {capture_errors}")
            assert len(frames) >= 20, "insufficient captured frames"
            inputs.key(0xff1b)  # Escape verifies normal application shutdown.
            if client.wait(timeout=5) != 0:
                raise RuntimeError(f"{sample} exited with an error")
            rows = list(csv.DictReader((local / "metrics.csv").open()))
            if sample == "sandbox":
                peak = max(int(row["sand"]) for row in rows)
                assert peak > 147, "mouse painting did not add sand"
                assert int(rows[-1]["sand"]) == 0, "keyboard reset did not clear sand"
                assert int(rows[-1]["active_tiles"]) == 0, "reset world did not sleep"
                assert int(rows[-1]["converted_cells"]) == 0, "idle image was regenerated"
                assert int(rows[-1]["upload_bytes"]) == 0, "idle image was uploaded"
                checks = {"peak_sand": peak, "final_sand": int(rows[-1]["sand"]),
                          "checks": ["mouse painting", "keyboard reset", "sleep after reset", "zero idle pixel conversion/upload", "Escape shutdown"]}
            else:
                pages = [row["page"] for row in rows]
                assert pages == ["Intro", "Choice", "River", "Intro", "Choice", "Hill"], pages
                checks = {"pages": pages, "checks": ["both choices", "keyboard restart", "Escape shutdown"]}
            assert "error:" not in (local / f"{sample}.log").read_text().lower()
            images = []
            for _, path in frames:
                with Image.open(path) as image:
                    images.append(image.convert("RGB"))
            durations = [max(10, round((b[0] - a[0]) * 100) * 10) for a, b in zip(frames, frames[1:])]
            durations.append(130)
            images[0].save(MEDIA / f"{prefix}.gif", save_all=True, append_images=images[1:], duration=durations, loop=0, optimize=True, disposal=2)
            # The timed image sequence is also available as an ordinary H.264
            # video. GIF input preserves the measured, quantized frame delays.
            subprocess.run(["ffmpeg", "-y", "-hide_banner", "-loglevel", "error", "-i", str(MEDIA / f"{prefix}.gif"), "-fps_mode", "vfr", "-c:v", "libx264", "-threads", "2", "-preset", "fast", "-crf", "18", "-pix_fmt", "yuv420p", "-movflags", "+faststart", str(MEDIA / f"{prefix}.mp4")], check=True, timeout=30)
            source_hash = hashlib.sha256()
            for path in sorted(path for folder in ("engine", "plugins", "samples") for path in (ROOT / folder).rglob("*")):
                if path.is_file() and path.parts[len(ROOT.parts)] in ("engine", "plugins", "samples") and path.suffix in (".rs", ".wgsl", ".toml"):
                    source_hash.update(str(path.relative_to(ROOT)).encode())
                    source_hash.update(path.read_bytes())
            report = {"source_sha256": source_hash.hexdigest(), "sample": sample, "base_revision": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
                      "platform": "Linux Xorg dummy / software GPU", "target_capture_fps": 8,
                      "captured_frames": len(frames), "gif_duration_ms": sum(durations),
                      "source_size": [1920, 1080], "gif_size": [960, 540],
                      **checks}
            (local / "result.json").write_text(json.dumps(report, indent=2) + "\n")
            (MEDIA / f"{prefix}.json").write_text(json.dumps(report, indent=2) + "\n")
            shutil.copyfile(local / "metrics.csv", MEDIA / f"{prefix}.csv")
            print(json.dumps(report, indent=2))
            print(f"Recorded docs/media/{prefix}.mp4, .gif and .png")
        finally:
            capture_stop.set()
            if capture_thread:
                capture_thread.join(timeout=6)
            if inputs:
                inputs.x.XCloseDisplay(inputs.display)
            for process in reversed(processes):
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()


if __name__ == "__main__":
    main()
