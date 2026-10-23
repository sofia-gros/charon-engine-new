#!/usr/bin/env python3
"""Record real X11 input and rendering to GIF on a private display."""

import argparse
import csv
import hashlib
import itertools
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

    def hold(self, symbol, down):
        code = self.x.XKeysymToKeycode(self.display, symbol)
        self.xt.XTestFakeKeyEvent(self.display, code, int(down), 0)
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


def check_debugger_rows(rows):
    # Segment by actual pause state, independent of driver/window startup latency.
    segments = [list(group) for paused, group in itertools.groupby(rows, key=lambda row: row["paused"]) if paused == "true"]
    assert len(segments) == 2, "expected two pause sessions"
    first, second = segments
    counts = list(dict.fromkeys(int(r["simulation_tick"]) for r in first))
    assert counts == list(range(counts[0], counts[0] + 4)), counts
    for segment in segments:
        for _, group in itertools.groupby(segment, key=lambda row: row["simulation_tick"]):
            block = list(group)
            assert len({(r["player_x"], r["player_y"]) for r in block}) == 1, "player moved between step commands"
            assert all(float(r["draw_fps"]) > 0 for r in block), "display stopped while paused"
    assert int(first[-1]["runtime_tick"]) > int(first[0]["runtime_tick"]), "input/display polling stopped"
    assert any(r["inspect_x"] == "250" and r["inspect_y"] == "60" and r["material"] == "1" and r["next_tile_awake"] == "true" for r in first), "paused edit/inspection was lost"
    reset_counts = list(dict.fromkeys(int(r["simulation_tick"]) for r in second))
    assert reset_counts[1:] == [0, 1], "reset/step lost pause or advanced more than once"
    assert rows[-1]["paused"] == "false" and int(rows[-1]["simulation_tick"]) > 1, "resume failed"
    return counts


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sample", choices=("sandbox", "dialogue", "platformer", "debugger"), default="sandbox")
    sample = parser.parse_args().sample
    package = "charon-sandbox" if sample != "dialogue" else "charon-dialogue-sample"
    prefix = f"{sample}-linux"
    local = LOCAL / sample
    duration = 12 if sample == "dialogue" else 14
    trace_path = local / f"inputs-{time.time_ns()}.trace"
    os.chdir(ROOT)
    local.mkdir(parents=True, exist_ok=True)
    MEDIA.mkdir(parents=True, exist_ok=True)
    xorg = "/usr/lib/xorg/Xorg"
    for tool in (xorg, "xauth", "import", "cargo"):
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
                client = subprocess.Popen([str(ROOT / "target/debug" / package), *(["--workers", "4"] if sample != "dialogue" else []), *(["--play", "--input-log", str(trace_path)] if sample in ("platformer", "debugger") else []), "--seconds", "25", "--record", str(local / "metrics.csv")], env=env, stdout=log, stderr=subprocess.STDOUT)
                processes.append(client)
            wait_for(lambda: "surface=" in (local / f"{sample}.log").read_text())
            def measured_frame_ready():
                path = local / "metrics.csv"
                if not path.exists(): return False
                return any(float(row.get("draw_fps") or 0) > 0 for row in csv.DictReader(path.open()))
            wait_for(measured_frame_ready)
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
                        path = temporary / f"frame-{index:04}.ppm"
                        before = time.monotonic()
                        # XGetImage works even when the virtual display has no
                        # shared-memory transport. This captures the actual window.
                        subprocess.run(["import", "-window", "root", "-filter", "point", "-resize", "960x540!", "-depth", "8", str(path)], env=env, check=True, capture_output=True, timeout=5)
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
                at(8)
                inputs.drag(3, [(960, y) for y in range(660, 741, 5)])
                at(11)
                inputs.key(ord("r"))
            elif sample == "dialogue":
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
            elif sample == "platformer":
                at(1)
                inputs.hold(ord("d"), True)
                at(2)
                inputs.hold(ord("d"), False)
                inputs.key(ord(" "))
                at(4)
                inputs.key(ord("b"))
                at(5.5)
                inputs.hold(ord("f"), True)
                at(8.5)
                inputs.hold(ord("f"), False)
                at(9)
                at(10)
                inputs.key(ord("r"))
            else:
                at(1)
                inputs.key(ord(" "))
                at(1.15)
                inputs.key(ord("p"))
                inputs.key(ord("i"))
                inputs.move(1000, 240)
                at(1.5)
                inputs.button(1, True)
                at(1.7)
                inputs.button(1, False)
                at(2.2)
                inputs.hold(ord("d"), True)
                at(2.4)
                inputs.key(ord("n"))
                at(3.4)
                inputs.key(ord("n"))
                at(4.4)
                inputs.key(ord("n"))
                at(5.4)
                inputs.hold(ord("d"), False)
                at(6)
                inputs.key(ord("p"))
                at(7)
                inputs.key(ord("p"))
                at(8)
                inputs.key(ord("r"))
                at(9)
                at(10)
                inputs.key(ord("n"))
                at(11)
                inputs.key(ord("p"))
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
            elif sample == "dialogue":
                pages = [page for page, _ in itertools.groupby(row["page"] for row in rows)]
                assert pages == ["Intro", "Choice", "River", "Intro", "Choice", "Hill"], pages
                checks = {"pages": pages, "checks": ["both choices", "keyboard restart", "Escape shutdown"]}
            elif sample == "platformer":
                xs = [float(row["player_x"]) for row in rows]
                ys = [float(row["player_y"]) for row in rows]
                assert max(xs) - min(xs) > 40, "movement input did not move player"
                assert min(ys) < 150, "jump did not leave the platform"
                assert max(ys) > 240, "dig did not destroy the support"
                assert any(row["sand_solid"] == "false" and 167 <= float(row["player_y"]) <= 169 and row["grounded"] == "true" for row in rows), "sand collision policy did not change"
                assert all(row["embedded"] == "false" for row in rows), "player became embedded"
                assert 119 <= xs[-1] <= 121 and 161 <= ys[-1] <= 163, "reset did not restore player"
                replay = subprocess.run([str(ROOT / "target/debug" / package), "--replay", str(trace_path), "--workers", "1"], env=env, text=True, capture_output=True, check=True, timeout=30)
                assert "replay_ticks=" in replay.stdout, "headless replay did not complete"
                shutil.copyfile(trace_path, MEDIA / f"{prefix}.trace")
                checks = {"replay": replay.stdout.strip(), "player_x_range": [min(xs), max(xs)], "player_y_range": [min(ys), max(ys)],
                          "checks": ["movement", "jump", "sand/wall collision policy", "support destruction", "reset", "Escape shutdown", "authoritative input replay with 1 worker"]}
            else:
                counts = check_debugger_rows(rows)
                replay = subprocess.run([str(ROOT / "target/debug" / package), "--replay", str(trace_path), "--workers", "1"], env=env, text=True, capture_output=True, check=True, timeout=30)
                shutil.copyfile(trace_path, MEDIA / f"{prefix}.trace")
                checks = {"paused_step_ticks": counts, "replay": replay.stdout.strip(),
                          "checks": ["frozen terrain/actor", "display while paused", "one tick per press", "paused edit and cell inspection", "reset preserves pause", "resume", "authoritative replay with 1 worker", "Escape shutdown"]}
            rates = [float(row["draw_fps"]) for row in rows if float(row.get("draw_fps") or 0) > 0]
            simulation_rates = [float(row["sim_hz"]) for row in rows if row.get("sim_hz")]
            assert rates and simulation_rates and max(simulation_rates) > 0, "missing rendering/simulation measurements"
            adapter_line = next(line for line in (local / f"{sample}.log").read_text().splitlines() if line.startswith("adapter="))
            assert "error:" not in (local / f"{sample}.log").read_text().lower()
            images = []
            for _, path in frames:
                with Image.open(path) as image:
                    images.append(image.convert("RGB"))
            durations = [max(10, round((b[0] - a[0]) * 100) * 10) for a, b in zip(frames, frames[1:])]
            durations.append(130)
            images[0].save(MEDIA / f"{prefix}.gif", save_all=True, append_images=images[1:], duration=durations, loop=0, optimize=True, disposal=2)
            with Image.open(MEDIA / f"{prefix}.gif") as gif:
                encoded_frames = gif.n_frames
            source_hash = hashlib.sha256()
            for path in sorted(path for folder in ("engine", "plugins", "samples") for path in (ROOT / folder).rglob("*")):
                if path.is_file() and path.parts[len(ROOT.parts)] in ("engine", "plugins", "samples") and path.suffix in (".rs", ".wgsl", ".toml"):
                    source_hash.update(str(path.relative_to(ROOT)).encode())
                    source_hash.update(path.read_bytes())
            report = {"source_sha256": source_hash.hexdigest(), "sample": sample, "base_revision": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
                      "platform": "Linux Xorg dummy", "adapter": adapter_line, "build_profile": "dev",
                      "target_capture_fps": 8,
                      "observed_capture_fps": round((len(frames) - 1) / (frames[-1][0] - frames[0][0]), 3),
                      "gif_encoded_frames": encoded_frames,
                      "draw_submission_fps_range": [min(rates), max(rates)],
                      "simulation_hz_range": [min(simulation_rates), max(simulation_rates)],
                      "measurement_scope": "On-screen DRAW counts completed present-submission calls per wall-clock interval; SIM counts simulation ticks. GIF rate is capture rate. GPU execution time and physical presentation are not measured.",
                      "captured_frames": len(frames), "gif_duration_ms": sum(durations),
                      "source_size": [1920, 1080], "gif_size": [960, 540],
                      **checks}
            (local / "result.json").write_text(json.dumps(report, indent=2) + "\n")
            (MEDIA / f"{prefix}.json").write_text(json.dumps(report, indent=2) + "\n")
            shutil.copyfile(local / "metrics.csv", MEDIA / f"{prefix}.csv")
            print(json.dumps(report, indent=2))
            print(f"Recorded docs/media/{prefix}.gif")
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
