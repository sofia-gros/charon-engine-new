#!/usr/bin/env python3
"""Record real X11 input and rendering to GIF on a private display."""

import argparse
import csv
import hashlib
import itertools
import ctypes as C
import ctypes.util
import json
import math
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
            "XResizeWindow": ([C.c_void_p, C.c_ulong, C.c_uint, C.c_uint], C.c_int),
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
        self.window = found
        self.x.XSetInputFocus(self.display, found, 2, 0)
        self.x.XFlush(self.display)

    def resize(self, width, height):
        self.x.XResizeWindow(self.display, self.window, width, height)
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


def source_fingerprint():
    source_hash=hashlib.sha256()
    for path in sorted(path for folder in ("engine","plugins","samples") for path in (ROOT/folder).rglob("*")):
        if path.is_file() and path.suffix in (".rs",".wgsl",".toml",".png",".jpg",".json",".ttf",".otf"):
            source_hash.update(str(path.relative_to(ROOT)).encode());source_hash.update(path.read_bytes())
    for name in ("Cargo.toml","Cargo.lock"):
        source_hash.update(name.encode());source_hash.update((ROOT/name).read_bytes())
    return source_hash.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sample", choices=("mining", "world-combat", "world", "combat", "exploration", "sandbox", "dialogue", "platformer", "debugger", "projectiles", "camera", "placement", "sea", "cup", "fluid-wave", "fluid-dam"), default="sandbox")
    parser.add_argument("--fluid-backend", choices=("cpu", "gpu"), default="cpu")
    parser.add_argument("--grid", default="480x270", help="simulation grid; client remains 1920x1080")
    args = parser.parse_args()
    sample = args.sample
    if sample in ("world", "world-combat"): args.grid="1920x1080"
    package = "charon-sandbox" if sample != "dialogue" else "charon-dialogue-sample"
    prefix = f"{sample}{'-gpu' if args.fluid_backend == 'gpu' else ''}-linux"
    local = LOCAL / sample
    duration = 22 if sample == "mining" else 50 if sample == "world-combat" else 20 if sample in ("world", "combat") else 23 if sample == "dialogue" else (40 if sample == "sea" else (36 if sample == "cup" else (20 if sample == "fluid-wave" else 14)))
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
    built_source=source_fingerprint()
    processes = []
    inputs = None
    capture_stop = threading.Event()
    capture_thread = None
    audio_probe = None
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
            if sample == "dialogue":
                from audio_probe import AudioProbe
                audio_probe = AudioProbe(temporary)
                env["ALSA_CONFIG_PATH"] = str(audio_probe.config)
            with open(local / "xorg.log", "w") as log:
                server = subprocess.Popen([xorg, env["DISPLAY"], "-config", str(config), "-auth", env["XAUTHORITY"], "-logfile", str(temporary / "xorg.log"), "-nolisten", "tcp", "-noreset", "-novtswitch", "-extension", "MIT-SHM"], env=env, stdout=log, stderr=subprocess.STDOUT)
                processes.append(server)
            wait_for(lambda: Path(f"/tmp/.X11-unix/X{number}").exists())
            with open(local / f"{sample}.log", "w") as log:
                client = subprocess.Popen([str(ROOT / "target/debug" / package), *(["--workers", "4", "--fluid-backend", args.fluid_backend, "--grid", args.grid] if sample != "dialogue" else ["--save-file", str(local / "slot.json")]), *(["--play"] if sample in ("platformer", "debugger", "projectiles", "camera") else []), *(["--explore", "42"] if sample in ("exploration", "mining") else []), *(["--game", "42"] if sample == "combat" else []), *(["--world-game" if sample == "world-combat" else "--world", "42", "--world-dir", str(temporary / "world")] if sample in ("world", "world-combat") else []), *(["--input-log", str(trace_path)] if sample != "dialogue" else []), "--seconds", str(duration + 20), "--record", str(local / "metrics.csv")], env=env, stdout=log, stderr=subprocess.STDOUT)
                processes.append(client)
            wait_for(lambda: "surface=" in (local / f"{sample}.log").read_text())
            def measured_frame_ready():
                path = local / "metrics.csv"
                if not path.exists(): return False
                return any(float(row.get("draw_fps") or 0) > 0 and (sample not in ("world", "world-combat") or row.get("world_waiting") == "false") for row in csv.DictReader(path.open()))
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
                        # shared-memory transport. Capture only the client window,
                        # so a resize never includes the surrounding desktop.
                        subprocess.run(["import", "-window", str(inputs.window), "-filter", "point", "-resize", "960x540!", "-depth", "8", str(path)], env=env, check=True, capture_output=True, timeout=5)
                        frames.append(((before + time.monotonic()) / 2, path))
                except Exception as error:
                    capture_errors.append(error)
            capture_thread = threading.Thread(target=capture, name="screen-capture")
            capture_thread.start()
            def at(seconds):
                time.sleep(max(0, start + seconds - time.monotonic()))
            if sample == "mining":
                stages = []
                def latest():
                    return list(csv.DictReader((local / "metrics.csv").open()))[-1]
                def stage(predicate):
                    wait_for(lambda: predicate(latest()), timeout=4)
                    row = latest()
                    stages.append({key: row[key] for key in ("simulation_tick", "block_pixels", "block_drops", "rubble_pixels", "block_items", "inventory_blocks")})
                def click_cell(x, y):
                    inputs.move(x * 4 + 1, y * 4 + 1)
                    inputs.button(1, True); time.sleep(.12); inputs.button(1, False)
                def six_ticks():
                    for _ in range(6):
                        inputs.key(0x6e); time.sleep(.1)
                at(.3); inputs.key(0x70)
                stage(lambda r: r["paused"] == "true")
                baseline = int(latest()["block_pixels"])
                inputs.key(0x33); click_cell(85, 157)
                stage(lambda r: int(r["block_pixels"]) == baseline + 16)
                inputs.key(0x74); inputs.key(0x5b); click_cell(84, 156)
                stage(lambda r: int(r["block_pixels"]) == baseline + 15 and r["block_drops"] == "0")
                six_ticks(); inputs.key(0x5d); click_cell(85, 157)
                stage(lambda r: int(r["block_pixels"]) == baseline + 12 and r["block_drops"] == "0")
                six_ticks(); click_cell(87, 157)
                stage(lambda r: r["block_drops"] == "1" and r["rubble_pixels"] == "8" and r["block_items"] == "1")
                inputs.key(0x70); time.sleep(1); inputs.key(0x70)
                stage(lambda r: r["paused"] == "true" and r["rubble_pixels"] == "8")
                inputs.key(0x70); inputs.hold(0x64, True); time.sleep(.3); inputs.hold(0x64, False)
                time.sleep(.2); inputs.key(0x70)
                stage(lambda r: r["inventory_blocks"] == "33" and r["block_items"] == "0")
                inputs.key(0x31)
                for _ in range(14): inputs.key(0x5d); time.sleep(.03)
                click_cell(128, 156)
                at(17); inputs.key(0x70)
                at(20); inputs.key(0x70)
            elif sample == "sandbox":
                at(0.5)
                inputs.drag(2, [(x, 700) for x in range(700, 1221, 8)])
                at(3.4)
                inputs.drag(1, [(880 + (i % 20) * 8, 180) for i in range(65)])
                at(7)
                inputs.key(ord("2"))
                inputs.key(ord("p"))
                inputs.key(ord("i"))
                inputs.move(1000, 240)
                inputs.button(1, True)
                at(7.5)
                inputs.button(1, False)
                at(8)
                inputs.key(ord("p"))
                inputs.drag(3, [(960, y) for y in range(660, 741, 5)])
                at(11)
                inputs.key(ord("r"))
            elif sample in ("cup", "fluid-wave"):
                at(.5)
                inputs.key(ord("k"))
                at(1)
                if sample == "cup":
                    inputs.key(ord("="))
                    inputs.key(ord("="))
                # The wave driver holds motion keys in wall time. Faster
                # simulation can move the cup anywhere in the world, so retain
                # a full-world view and keep the whole-cup assertion below.
                at(2)
                inputs.hold(ord("d"),True)
                at(4)
                inputs.hold(ord("d"),False)
                at(4.5)
                inputs.key(ord("o"))
                at(6)
                inputs.hold(ord("a"),True)
                at(8)
                inputs.hold(ord("a"),False)
                at(9)
                inputs.hold(ord("d"),True)
                at(11)
                inputs.hold(ord("d"),False)
                at(12)
                inputs.hold(ord("a"),True)
                at(14)
                inputs.hold(ord("a"),False)
                at(16)
                inputs.key(ord("p"))
                inputs.hold(ord("d"),True)
                at(17)
                inputs.hold(ord("d"),False)
                at(17.5)
                inputs.key(ord("p"))
            elif sample=="fluid-dam":
                at(.5); inputs.key(ord("h"))
                at(1); inputs.key(ord("=")); inputs.key(ord("="))
                at(7); inputs.key(ord("p"))
                at(8); inputs.key(ord("p"))
            elif sample == "sea":
                at(.5)
                inputs.key(ord("l"))
                at(2)
                inputs.key(ord("1"))
                # Rain many grains above the sea, away from the water surface.
                # Painting air adds sand without replacing any existing water.
                inputs.move(180,100)
                inputs.button(1,True)
                for cycle in range(8):
                    at(2+cycle*.8)
                    inputs.move(180+(cycle%2)*500,100)
                at(8.5)
                inputs.button(1,False)
                at(16)
                inputs.key(ord("p"))
                at(17)
                inputs.key(ord("p"))
            elif sample == "dialogue":
                at(1)
                inputs.move(1470,918)
                inputs.button(1,True)
                inputs.button(1,False)
                at(2)
                inputs.move(420,918)
                inputs.button(1,True)
                at(2.2)
                inputs.move(50,400)
                inputs.button(1,False)
                at(3)
                inputs.key(ord("1"))
                at(5)
                inputs.move(1470,918)
                inputs.button(1,True)
                inputs.button(1,False)
                at(6.3)
                inputs.resize(1280,720)
                at(7)
                inputs.move(980,612)
                inputs.button(1,True)
                inputs.button(1,False)
                at(8)
                inputs.key(0xff09)  # Tab selects the second choice.
                at(9)
                inputs.move(920,612)
                inputs.button(1,True)
                inputs.button(1,False)
                at(10)
                inputs.resize(1920,1080)
                at(11)
                inputs.key(ord("="))
                inputs.key(ord("="))
                at(12)
                inputs.key(0xffc2)  # F5 saves Hill and volume 70.
                at(13)
                inputs.key(ord("r"))
                at(14)
                for _ in range(3):
                    inputs.key(ord("-"))
                at(15)
                inputs.key(ord("m"))
                at(16)
                inputs.key(0xffc6)  # F9 restores Hill/70/unmuted.
                at(18)
                inputs.key(ord("p"))
                at(19.5)
                inputs.key(ord("p"))
                at(21)
                inputs.key(ord("m"))
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
            elif sample == "placement":
                at(0.4)
                inputs.key(ord("p"))
                inputs.key(ord("i"))
                inputs.key(ord("3"))
                at(1)
                inputs.move(405,279)
                inputs.button(1, True)
                at(1.2)
                inputs.button(1, False)
                at(1.7)
                inputs.move(441,279)
                inputs.button(1, True)
                at(1.9)
                inputs.button(1, False)
                at(3)
                inputs.key(ord("4"))
                inputs.move(493,335)
                inputs.button(1, True)
                at(3.2)
                inputs.button(1, False)
                at(4)
                inputs.move(555,357)
                inputs.button(1, True)
                at(4.2)
                inputs.button(1, False)
                at(5)
                inputs.key(ord("c"))
                at(6)
                inputs.move(900,400)
                inputs.button(1, True)
                at(6.2)
                inputs.button(1, False)
                at(8)
                inputs.button(3, True)
                at(8.2)
                inputs.button(3, False)
                at(10)
                inputs.key(ord("c"))
                at(11)
                inputs.key(ord("r"))
            elif sample == "camera":
                at(0.5)
                inputs.key(ord("c"))
                at(1)
                inputs.hold(ord("d"), True)
                at(2.8)
                inputs.hold(ord("d"), False)
                at(3)
                inputs.key(ord("p"))
                inputs.key(ord("i"))
                at(3.4)
                inputs.move(1440,540)
                inputs.button(2, True)
                at(3.7)
                inputs.button(2, False)
                at(4.5)
                inputs.key(ord("="))
                at(5.5)
                inputs.key(ord("-"))
                at(6)
                inputs.key(ord("p"))
                inputs.hold(ord("a"), True)
                at(6.5)
                inputs.resize(1280,720)
                inputs.move(960,360)
                at(7.2)
                inputs.resize(1920,1080)
                inputs.move(1440,540)
                at(8)
                inputs.hold(ord("a"), False)
                at(8.5)
                inputs.key(ord("p"))
                at(9)
                inputs.key(ord("n"))
                at(10)
                inputs.key(ord("r"))
                at(11)
                inputs.key(ord("c"))
                at(11.5)
                inputs.key(ord("p"))
            elif sample == "world-combat":
                def observe():
                    return list(csv.DictReader((local / "metrics.csv").open()))[-1]
                previous_enemy = None
                def aim_enemy(row):
                    nonlocal previous_enemy
                    # Lead the moving target using measured global coordinates,
                    # rather than aiming at a CSV sample already behind it.
                    point = [float(row["enemy_x"]), float(row["enemy_y"])]
                    now = float(row["elapsed_s"])
                    global_point = [point[0] + int(row["world_origin_x"]), point[1] + int(row["world_origin_y"])]
                    if previous_enemy and previous_enemy[0] == row["enemy_id"] and now > previous_enemy[1]:
                        dt = now - previous_enemy[1]
                        velocity = [(global_point[i] - previous_enemy[2][i]) / dt for i in range(2)]
                        flight = min(.25, math.hypot(point[0] - float(row["player_x"]), point[1] - float(row["player_y"])) / 810 + .06)
                        point[0] += max(-90, min(90, velocity[0])) * flight
                        point[1] += max(-140, min(240, velocity[1])) * flight
                        if abs(velocity[1]) > 10: point[1] += 200 * flight * flight
                    previous_enemy = (row["enemy_id"], now, global_point)
                    x = (point[0] - float(row["camera_x"])) / float(row["camera_width"]) * float(row["window_width"])
                    y = (point[1] - float(row["camera_y"])) / float(row["camera_height"]) * float(row["window_height"])
                    if 0 <= x < float(row["window_width"]) and 0 <= y < float(row["window_height"]):
                        inputs.move(round(x), round(y))
                        inputs.button(1, True); time.sleep(.06); inputs.button(1, False)
                def mine_ahead(row, direction):
                    x = (float(row["player_x"]) + direction * 6 - float(row["camera_x"])) / float(row["camera_width"]) * float(row["window_width"])
                    y = (float(row["player_y"]) + 6 - float(row["camera_y"])) / float(row["camera_height"]) * float(row["window_height"])
                    inputs.move(round(x), round(y))
                    inputs.button(1, True); time.sleep(.1); inputs.button(1, False)
                at(.5); inputs.key(0xff0d)
                for _ in range(14): inputs.key(0x5d); time.sleep(.03)
                inputs.hold(ord("d"), True)
                progress = []
                for index in range(116):
                    at(1.2 + index * .2)
                    row = observe()
                    assert row["run_phase"] != "Defeated", "player died before the world roundtrip; inspect the private preview"
                    if int(row["kills"] or 0) == 1 and int(row["world_x"] or 0) > 820: break
                    if index % 2 == 0: inputs.key(0x20)
                    if row["world_waiting"] != "false": continue
                    progress.append(int(row["world_x"]))
                    if row.get("enemy_id") == "2":
                        inputs.key(ord("q")); aim_enemy(row)
                    elif len(progress) > 4 and progress[-1] - progress[-4] < 5:
                        inputs.key(ord("t")); mine_ahead(row, 1)
                assert int(observe()["world_x"]) > 820 and int(observe()["kills"] or 0) == 1, "forward exploration/combat did not complete"
                inputs.hold(ord("d"), False)
                inputs.key(ord("t")); inputs.hold(ord("a"), True)
                reverse_at = time.monotonic()
                for index in range(110):
                    time.sleep(max(0, reverse_at + index * .2 - time.monotonic()))
                    row = observe()
                    if row["world_x"] and float(row["world_x"]) <= 80: break
                    if row.get("player_x") and row["world_waiting"] == "false": mine_ahead(row, -1)
                    if index % 2 == 0: inputs.key(0x20)
                inputs.button(1, False); inputs.hold(ord("a"), False)
                inputs.key(ord("p"))
            elif sample == "world":
                at(0.5)
                inputs.key(ord("3"))
                inputs.move(1200, 540)
                inputs.button(1, True)
                at(0.8)
                inputs.button(1, False)
                at(1.1)
                inputs.button(3, True)
                at(1.4)
                inputs.button(3, False)
                inputs.hold(ord("d"), True)
                for index in range(12):
                    at(1.8+index*0.4)
                    inputs.key(0x20)
                at(6.7)
                inputs.hold(ord("d"), False)
                inputs.hold(ord("a"), True)
                for index in range(14):
                    at(7+index*0.4)
                    inputs.key(0x20)
                at(12.7)
                inputs.hold(ord("a"), False)
                at(13.3)
                inputs.hold(ord("f"), True)
                at(13.8)
                inputs.hold(ord("f"), False)
                at(15)
                inputs.key(ord("p"))
                at(16)
                inputs.key(ord("q"))
                inputs.move(1100, 650)
                inputs.button(1, True)
                at(16.2)
                inputs.button(1, False)
                at(17)
                inputs.key(ord("n"))
                at(18)
                inputs.key(ord("p"))
            elif sample == "combat":
                at(0.8)
                inputs.key(0xff0d)
                at(1.2)
                inputs.key(ord("3"))
                inputs.key(ord("i"))
                inputs.move(480, 672)
                inputs.button(1, True)
                at(1.5)
                inputs.button(1, False)
                at(1.8)
                inputs.button(3, True)
                at(2.1)
                inputs.button(3, False)
                at(2.3)
                inputs.key(ord("q"))
                # Aim through the real mouse input at the most recently observed
                # enemy, rather than assuming it stays at its spawn position.
                for moment in (2.4, 2.65, 2.9, 3.15, 3.4, 3.65, 3.9, 4.15):
                    at(moment)
                    observed = list(csv.DictReader((local / "metrics.csv").open()))[-1]
                    if observed.get("enemy_x"):
                        x = (float(observed["enemy_x"]) - float(observed["camera_x"])) / float(observed["camera_width"]) * float(observed["window_width"])
                        y = (float(observed["enemy_y"]) - float(observed["camera_y"])) / float(observed["camera_height"]) * float(observed["window_height"])
                        inputs.move(round(x), round(y))
                        inputs.button(1, True)
                        at(moment + 0.1)
                        inputs.button(1, False)
                at(4.8)
                inputs.hold(ord("d"), True)
                at(5.3)
                inputs.hold(ord("d"), False)
                at(5.4)
                inputs.hold(ord("f"), True)
                at(5.9)
                inputs.hold(ord("f"), False)
                at(6.5)
                inputs.key(ord("r"))
                at(14)
                inputs.key(ord("r"))
                at(14.6)
                inputs.hold(ord("d"), True)
                at(15.2)
                inputs.hold(ord("d"), False)
                at(15.4)
                inputs.hold(ord("f"), True)
                at(16)
                inputs.hold(ord("f"), False)
                at(16.2)
                inputs.key(ord("p"))
                at(17)
                inputs.key(ord("r"))
                at(17.1)
                inputs.key(ord("e"))
                inputs.move(640, 640)
                inputs.button(1, True)
                at(17.25)
                inputs.button(1, False)
                for moment in (17.3, 17.4, 17.5, 17.6, 17.7, 17.8, 17.9):
                    at(moment)
                    inputs.key(ord("n"))
                at(18)
                inputs.key(ord("p"))
            elif sample == "exploration":
                at(0.4)
                inputs.key(ord("p"))
                inputs.key(ord("i"))
                inputs.key(ord("3"))
                inputs.move(480, 640)
                at(0.8)
                inputs.button(1, True)
                at(1.1)
                inputs.button(1, False)
                at(1.5)
                inputs.button(3, True)
                at(1.8)
                inputs.button(3, False)
                at(2)
                inputs.key(ord("p"))
                at(2.3)
                inputs.hold(ord("d"), True)
                at(3.4)
                inputs.hold(ord("d"), False)
                at(3.6)
                inputs.hold(ord("f"), True)
                at(4.3)
                inputs.hold(ord("f"), False)
                at(5)
                inputs.key(ord("r"))
                at(6)
                inputs.key(ord("p"))
                at(6.5)
                inputs.key(ord("g"))
                at(7.5)
                inputs.key(ord("r"))
                at(8.5)
                inputs.key(ord("p"))
                at(9)
                inputs.hold(ord("d"), True)
                at(9.4)
                inputs.key(0x20)
                at(10.2)
                inputs.hold(ord("d"), False)
                at(11)
                inputs.key(ord("j"))
                at(12)
                inputs.key(ord("p"))
            elif sample == "projectiles":
                at(0.5)
                inputs.key(ord("p"))
                inputs.key(ord("i"))
                inputs.move(960, 672)
                inputs.button(2, True)
                at(0.8)
                inputs.button(2, False)
                at(1.5)
                inputs.key(ord("j"))
                at(2)
                inputs.key(ord("n"))
                at(2.8)
                inputs.key(ord("p"))
                for index in range(10):
                    at(3.5 + index * 0.65)
                    inputs.key(ord("j"))
                at(11)
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
            capture_thread.join(timeout=max(10, start + duration - time.monotonic() + 6))
            if capture_thread.is_alive() or capture_errors:
                raise RuntimeError(f"screen capture failed: {capture_errors}")
            assert len(frames) >= 20, "insufficient captured frames"
            inputs.key(0xff1b)  # Escape verifies normal application shutdown.
            if client.wait(timeout=5) != 0:
                raise RuntimeError(f"{sample} exited with an error")
            resume_evidence = None
            if sample == "world-combat":
                before = list(csv.DictReader((local / "metrics.csv").open()))[-1]
                resume_csv = local / "resume.csv"
                resume_trace = local / f"resume-{time.time_ns()}.trace"
                with open(local / "resume.log", "w") as log:
                    client = subprocess.Popen([str(ROOT / "target/debug" / package), "--world-game", "42", "--world-dir", str(temporary / "world"), "--workers", "4", "--fluid-backend", "cpu", "--input-log", str(resume_trace), "--record", str(resume_csv), "--seconds", "30"], env=env, stdout=log, stderr=subprocess.STDOUT)
                    processes.append(client)
                def restored_ready():
                    if not resume_csv.exists(): return False
                    observed=list(csv.DictReader(resume_csv.open()))
                    return any(r["world_waiting"]=="false" and r["simulation_tick"]==before["simulation_tick"] for r in observed)
                wait_for(restored_ready)
                inputs.focus()
                first = list(csv.DictReader(resume_csv.open()))[-1]
                assert float(first["sim_hz"]) == 0, "restored history was counted as live simulation work"
                for key in ("simulation_tick", "world_x", "world_y", "player_x", "player_y", "player_health", "enemy_id", "enemy_health", "kills", "paused"):
                    assert first[key] == before[key], f"restart did not restore {key}: {before[key]} -> {first[key]}"
                # Both sides of the process restart are real client captures.
                # The timestamp gap includes shutdown/startup; it is not extra simulation.
                resumed_at = time.monotonic()
                for index in range(32):
                    time.sleep(max(0, resumed_at + index / 8 - time.monotonic()))
                    if index == 8:
                        inputs.key(ord("p"))
                        inputs.hold(ord("d"), True)
                    if index == 20:
                        inputs.hold(ord("d"), False)
                        inputs.key(ord("p"))
                    path = temporary / f"resume-frame-{index:04}.ppm"
                    timestamp = time.monotonic()
                    subprocess.run(["import", "-window", str(inputs.window), "-filter", "point", "-resize", "960x540!", "-depth", "8", str(path)], env=env, check=True, capture_output=True, timeout=5)
                    frames.append((timestamp, path))
                head_path = temporary / "world" / "42" / "checkpoint.json"
                head_before_retry = json.loads(head_path.read_text())["generation"]
                inputs.key(ord("r"))
                def retry_ready():
                    observed=list(csv.DictReader(resume_csv.open()))
                    return any(r["run_phase"]=="Ready" and r["simulation_tick"]=="0" and r["world_waiting"]=="false" for r in observed)
                wait_for(retry_ready)
                ready = list(csv.DictReader(resume_csv.open()))[-1]
                assert (ready["player_health"], ready["enemy_health"], ready["kills"], ready["paused"]) == ("3", "3", "0", "true"), "retry did not reset combat and retain debug pause"
                assert json.loads(head_path.read_text())["generation"] != head_before_retry, "old run was not saved before retry"
                # Show the restored title, then start and operate the fresh world.
                retry_at=time.monotonic()
                for index in range(24):
                    time.sleep(max(0,retry_at+index/8-time.monotonic()))
                    if index==8:
                        inputs.key(0xff0d)
                        inputs.key(ord("p"))
                        inputs.hold(ord("d"),True)
                    if index==18:
                        inputs.hold(ord("d"),False)
                        inputs.key(ord("p"))
                    path=temporary/f"retry-frame-{index:04}.ppm"
                    timestamp=time.monotonic()
                    subprocess.run(["import","-window",str(inputs.window),"-filter","point","-resize","960x540!","-depth","8",str(path)],env=env,check=True,capture_output=True,timeout=5)
                    frames.append((timestamp,path))
                inputs.key(0xff1b)
                assert client.wait(timeout=5)==0, "resumed process failed"
                resumed = list(csv.DictReader(resume_csv.open()))
                assert max(float(r["world_x"]) for r in resumed if r["world_x"]) > float(before["world_x"]) + 30, "resumed controls did not move the player"
                assert max(int(r["simulation_tick"]) for r in resumed) > int(before["simulation_tick"]), "resumed simulation never advanced"
                assert all(r["embedded"] == "false" for r in resumed)
                replay = subprocess.run([str(ROOT / "target/debug" / package), "--replay", str(resume_trace), "--workers", "1"], env=env, text=True, capture_output=True, check=True, timeout=60)
                shutil.copyfile(resume_trace, MEDIA / "world-combat-resume-linux.trace")
                shutil.copyfile(resume_csv, MEDIA / "world-combat-resume-linux.csv")
                resume_evidence = {"restored_tick":int(before["simulation_tick"]), "enemy_id":first["enemy_id"], "enemy_health":first["enemy_health"], "replay":replay.stdout.strip(), "checks":["normal shutdown publishes checkpoint", "new process restores player/enemy/tick/pause", "real keyboard resumes movement", "restored snapshot replay with 1 worker", "R saves previous run and resets same-seed world", "retry retains pause and restores health/enemies", "new run accepts start and movement"]}
            # Retain a private GIF even when acceptance fails, so visual bugs can
            # be inspected immediately. Published media still requires all checks.
            preview=[]
            for _,path in frames:
                with Image.open(path) as im: preview.append(im.convert("RGB"))
            preview[0].save(local / "preview.gif",save_all=True,append_images=preview[1:],duration=125,loop=0,disposal=2)
            del preview
            rows = list(csv.DictReader((local / "metrics.csv").open()))
            if sample == "mining":
                assert stages[-1]["inventory_blocks"] == "33"
                assert all(r["block_drops"] in ("0", "1") for r in rows), "debris yielded another item"
                assert all(r["embedded"] == "false" for r in rows)
                replay = subprocess.run([str(ROOT / "target/debug" / package), "--replay", str(trace_path), "--workers", "1"], env=env, text=True, capture_output=True, check=True, timeout=30)
                records = trace_path.read_text()
                assert "mine:" in records and ":16:1" in records, "circular tool sizes were not recorded"
                shutil.copyfile(trace_path, MEDIA / f"{prefix}.trace")
                checks = {"stages": stages, "replay": replay.stdout.strip(), "checks": ["4x4 placed block", "1px circular corner cut without drop", "2px circular cuts to half", "exactly eight falling debris pixels and one item", "item pickup into inventory", "16px circular sand brush", "authoritative replay with one worker"]}
            elif sample == "sandbox":
                peak = max(int(row["sand"]) for row in rows)
                assert peak > 147, "mouse painting did not add sand"
                assert int(rows[-1]["sand"]) == 0, "keyboard reset did not clear sand"
                assert int(rows[-1]["active_tiles"]) == 0, "reset world did not sleep"
                assert int(rows[-1]["converted_cells"]) == 0, "idle image was regenerated"
                assert int(rows[-1]["upload_bytes"]) == 0, "idle image was uploaded"
                assert any(r["material"] == "3" and r["paused"] == "true" for r in rows), "game-defined ash not painted/inspected"
                replay = subprocess.run([str(ROOT / "target/debug" / package), "--replay", str(trace_path), "--workers", "1"], env=env, text=True, capture_output=True, check=True, timeout=30)
                shutil.copyfile(trace_path, MEDIA / f"{prefix}.trace")
                checks = {"replay": replay.stdout.strip(), "custom_material": "demo.ash", "peak_sand": peak, "final_sand": int(rows[-1]["sand"]),
                          "checks": ["mouse painting", "game-defined ash selection/inspection", "authoritative replay with 1 worker", "keyboard reset", "sleep after reset", "zero idle pixel conversion/upload", "Escape shutdown"]}
            elif sample == "fluid-wave":
                fluid_rows=[r for r in rows if r["cup_x"]]
                assert fluid_rows, "wave scene did not open"
                assert "Fluid step rejected" not in (local / f"{sample}.log").read_text(), "CFD wave solve failed"
                water=float(fluid_rows[0]["fluid_water_volume"])
                assert water>0 and all(abs(float(r["fluid_water_volume"])-water)<1e-6 for r in fluid_rows), "wave scene lost water"
                oil=[float(r["fluid_oil_volume"]) for r in fluid_rows if float(r["fluid_oil_volume"])>0]
                assert oil and max(oil)-min(oil)<1e-6, "wave scene lost oil"
                xs=[int(r["cup_x"]) for r in fluid_rows]
                assert max(xs)-min(xs)>float(fluid_rows[0]["cup_width"])/5, "wave cup moved less than one fifth of its width"
                assert all(float(r["camera_x"]) <= float(r["cup_x"]) and float(r["cup_x"])+float(r["cup_width"]) <= float(r["camera_x"])+float(r["camera_width"]) for r in fluid_rows), "wave cup left the recorded view"
                assert any(b>a for a,b in zip(xs,xs[1:])) and any(b<a for a,b in zip(xs,xs[1:])), "wave cup did not reverse"
                tilts=[float(r["fluid_surface_delta"]) for r in fluid_rows]
                assert min(tilts)<-1 and max(tilts)>1, "wave surface did not respond in both directions"
                assert max(float(r["fluid_divergence"]) for r in fluid_rows)<1e-3, "wave projection divergence"
                paused=[r for r in fluid_rows if r["paused"]=="true"]
                assert len(paused)>=2 and len({(r["cup_x"],r["simulation_tick"]) for r in paused})==1, "paused wave scene moved"
                with Image.open(frames[-1][1]) as im:
                    pixels=list(im.convert("RGB").get_flattened_data())
                    assert sum(r<80 and g>90 and b>180 for r,g,b in pixels)>100, "water absent from rendered wave scene"
                    assert sum(r>170 and g>140 and b<100 for r,g,b in pixels)>100, "oil absent from rendered wave scene"
                replay=subprocess.run([str(ROOT/"target/debug"/package),"--replay",str(trace_path),"--workers","1"],env=env,text=True,capture_output=True,check=True,timeout=90)
                shutil.copyfile(trace_path,MEDIA/f"{prefix}.trace")
                checks={"water_volume_cells":water,"oil_volume_cells":oil[-1],"surface_delta_range":[min(tilts),max(tilts)],"cup_x_range":[min(xs),max(xs)],"replay":replay.stdout.strip(),"checks":["real left/right movement and reversal", "entire cup inside the camera", "inertial surface slopes in both directions", "water and oil visible", "separate phase conservation", "bounded projection divergence", "pause freezes movement and liquid", "one-worker authoritative replay", "Escape shutdown"],"scope":"Short wave/momentum demonstration; the longer CPU cup acceptance separately checks resting phase layers, deep interior holes and sleep."}
            elif sample == "cup":
                cup_rows=[r for r in rows if r["cup_x"]]
                assert cup_rows, "cup was not created"
                water=int(cup_rows[0]["water"])
                assert water>0 and all(int(r["water"])==water for r in cup_rows), "cup motion lost or created water"
                oil=[int(r["oil"]) for r in cup_rows if int(r["oil"])>0]
                assert oil and min(oil)==max(oil), "oil injection or motion lost/created oil"
                xs=[int(r["cup_x"]) for r in cup_rows]
                assert max(xs)-min(xs)>20 and any(b>a for a,b in zip(xs,xs[1:])) and any(b<a for a,b in zip(xs,xs[1:])), "cup did not move both ways"
                assert all(r["cup_blocked"]=="0" for r in cup_rows), "cup movement was blocked"
                paused=[r for r in cup_rows if r["paused"]=="true"]
                assert len(paused)>=2 and len({(r["cup_x"],r["simulation_tick"]) for r in paused})==1, "paused cup moved"
                assert all(r["active_tiles"]=="0" and r["converted_cells"]=="0" and r["upload_bytes"]=="0" for r in cup_rows[-2:]), "stopped cup contents did not sleep"
                with Image.open(frames[-1][1]) as im:
                    rgb=im.convert("RGB")
                    blue=[]; yellow=[]
                    last=cup_rows[-1]
                    cx,cy,cw,ch=[float(last[k]) for k in ("cup_x","cup_y","cup_width","cup_height")]
                    ox,oy,vw,vh=[float(last[k]) for k in ("camera_x","camera_y","camera_width","camera_height")]
                    left=max(0,math.ceil((cx+2-ox)/vw*960)); right=min(960,math.floor((cx+cw-2-ox)/vw*960))
                    top=max(0,math.ceil((cy-oy)/vh*540)); bottom=min(540,math.floor((cy+ch-2-oy)/vh*540))
                    for y in range(top,bottom):
                        for x in range(left,right):
                            r,g,b=rgb.getpixel((x,y))
                            if r<80 and g>90 and b>180: blue.append(y)
                            if r>170 and g>140 and b<100: yellow.append(y)
                    assert blue and yellow and max(yellow)<=min(blue)+math.ceil(540/vh), "rendered oil did not form an upper layer"
                    layer_bands={"oil_y":[min(yellow),max(yellow)],"water_y":[min(blue),max(blue)]}
                assert "Fluid step rejected" not in (local / f"{sample}.log").read_text(), "CFD step failed"
                assert all(abs(float(r["fluid_water_volume"])-water)<1e-6 for r in cup_rows), "CFD water volume drift"
                assert all(abs(float(r["fluid_oil_volume"])-int(r["oil"]))<1e-6 for r in cup_rows), "CFD oil volume drift"
                assert max(float(r["fluid_divergence"]) for r in cup_rows)<1e-3, "pressure projection left divergence"
                tilts=[float(r["fluid_surface_delta"]) for r in cup_rows]
                assert min(tilts)<-1 and max(tilts)>1, "surface did not slosh in both directions"
                # Use the intersection of all cup interiors, avoiding wall/CSV
                # sampling offsets. Falling oil above the water is allowed; air
                # below the first water pixel in a column is not.
                left=math.ceil((max(xs)+2-ox)/vw*960)
                right=math.floor((min(xs)+cw-2-ox)/vw*960)
                checked_frames=0
                for timestamp,path in frames:
                    if not 3 <= timestamp-start <= 15:
                        continue
                    with Image.open(path) as im:
                        rgb=im.convert("RGB")
                        for x in range(left,right):
                            submerged=True
                            deep_top=math.ceil((cy+ch-14-oy)/vh*540)
                            for y in range(deep_top,bottom):
                                r,g,b=rgb.getpixel((x,y))
                                submerged |= r<80 and g>90 and b>180
                                assert not (submerged and (r,g,b)==(16,23,36)), f"rendered air pocket at ({x},{y}), t={timestamp-start:.2f}s"
                    checked_frames+=1
                assert checked_frames>=50, "insufficient moving-cup frames checked"
                replay=subprocess.run([str(ROOT / "target/debug" / package),"--replay",str(trace_path),"--workers","1"],env=env,text=True,capture_output=True,check=True,timeout=50)
                shutil.copyfile(trace_path,MEDIA / f"{prefix}.trace")
                checks={"water_cells":water,"oil_cells":oil[-1],"rendered_layers":layer_bands,"air_pocket_frames_checked":checked_frames,"sleep_tolerance_cells":0.005,"fluid_settings":{"viscosity_cell2_s":0.5,"pressure_cutoff":0.01,"drag_s_inverse":0.8},"volume_unit":"simulation cell area","simulation_grid":[int(v) for v in trace_path.read_text().splitlines()[0].split()[1:3]],"surface_delta_range":[min(tilts),max(tilts)],"solver":"Eulerian MAC / geometric VOF, cut-cell pressure, MIC-PCG","cup_x_range":[min(xs),max(xs)],"replay":replay.stdout.strip(),"checks":["game-owned cup shape", "left/right real input", "separate water/oil mass conservation", "oil injection into air", "no artificial holes in the deep interior during gentle sloshing", "oil above water in rendered output", "pause freezes cup and liquid", "resting contents sleep", "authoritative replay with one worker", "Escape shutdown"]}
            elif sample=="fluid-dam":
                fluid_rows=[r for r in rows if r["cup_x"]]
                assert fluid_rows, "dam scene did not open"
                water=float(fluid_rows[0]["fluid_water_volume"])
                assert water>0 and all(abs(float(r["fluid_water_volume"])-water)<1e-6 for r in fluid_rows), "dam lost water"
                assert max(float(r["fluid_divergence"]) for r in fluid_rows)<1e-3, "dam projection divergence"
                assert "Fluid step rejected" not in (local / f"{sample}.log").read_text(), "CFD dam solve failed"
                paused=[r for r in fluid_rows if r["paused"]=="true"]
                assert paused and len({r["simulation_tick"] for r in paused})==1, "dam pause did not freeze"
                last=fluid_rows[-1]
                cx,cy,cw,ch=[float(last[k]) for k in ("cup_x","cup_y","cup_width","cup_height")]
                ox,oy,vw,vh=[float(last[k]) for k in ("camera_x","camera_y","camera_width","camera_height")]
                box=(math.ceil((cx+cw/2+4-ox)/vw*960),math.ceil((cy-oy)/vh*540),math.floor((cx+cw-2-ox)/vw*960),math.floor((cy+ch-2-oy)/vh*540))
                with Image.open(frames[-1][1]) as im:
                    spread=sum(r<80 and g>90 and b>180 for r,g,b in im.convert("RGB").crop(box).get_flattened_data())
                assert spread>100, "dam water did not pass the baffle in rendered output"
                replay=subprocess.run([str(ROOT/"target/debug"/package),"--replay",str(trace_path),"--workers","1"],env=env,text=True,capture_output=True,check=True,timeout=90)
                shutil.copyfile(trace_path,MEDIA/f"{prefix}.trace")
                checks={"solver":"Eulerian MAC / geometric VOF, cut-cell pressure, MIC-PCG","fluid_settings":{"viscosity_cell2_s":0.5,"pressure_cutoff":0.01,"drag_s_inverse":0.8,"sleep_tolerance":0.005},"water_volume_cells":water,"water_pixels_beyond_baffle":spread,"replay":replay.stdout.strip(),"checks":["collapse of a water column", "concave domain with an internal baffle", "separate conserved phase volume", "bounded divergence after projection", "pause and resume", "real rendered water beyond the baffle", "one-worker authoritative replay", "Escape shutdown"]}
            elif sample == "sea":
                sea_rows=[r for r in rows if int(r["liquid"])>0]
                assert sea_rows, "sea was not created"
                water=int(sea_rows[0]["liquid"])
                assert all(int(r["liquid"])==water for r in sea_rows), "painting or flow lost/created water"
                assert max(int(r["sand"]) for r in sea_rows)-int(sea_rows[0]["sand"])>1500, "not enough sand fell into sea"
                assert any(r["paused"]=="true" for r in sea_rows), "pause input was not applied"
                replay=subprocess.run([str(ROOT / "target/debug" / package), "--replay", str(trace_path), "--workers", "1"],env=env,text=True,capture_output=True,check=True,timeout=40)
                shutil.copyfile(trace_path,MEDIA / f"{prefix}.trace")
                def water_on_right(path):
                    with Image.open(path) as im:
                        rgb=im.convert("RGB")
                        return sum(b>180 and r<80 and g>90 for y in range(200,530) for x in range(510,950) for r,g,b in [rgb.getpixel((x,y))])
                before=water_on_right(frames[8][1])
                after=max(water_on_right(p) for _,p in frames[-24:])
                assert after>before+50, f"water not visible over the sand bank: before={before} after={after}"
                assert all(r["active_tiles"]=="0" and r["visited_cells"]=="0" and r["converted_cells"]=="0" and r["upload_bytes"]=="0" for r in sea_rows[-3:]), "sea did not sleep after settling"
                checks={"water_cells":water,"water_pixels_right_before":before,"water_pixels_right_after":after,"replay":replay.stdout.strip(),"checks":["left sea and right sand scene", "real mouse sand rain", "water mass conserved throughout", "water visible beyond bank", "settled sea sleeps with zero cell conversion/upload", "pause/resume", "authoritative replay with one worker", "Escape shutdown"]}
            elif sample == "dialogue":
                pages = [page for page, _ in itertools.groupby(row["page"] for row in rows)]
                assert pages == ["Intro", "Choice", "River", "Intro", "Choice", "Hill", "Intro", "Hill"], pages
                first_choice = next(float(r["elapsed_s"]) for r in rows if r["page"]=="Choice")
                first_river = next(float(r["elapsed_s"]) for r in rows if r["page"]=="River")
                assert first_river-first_choice >= 1.5, "release outside button activated an early choice"
                assert any(r["window_width"]=="1280" and r["window_height"]=="720" for r in rows), "dialogue client did not resize"
                assert any(r["focused"] == "Some(Choose(1))" and r["page"] == "Choice" for r in rows), "keyboard focus did not reach second choice"
                assert max(int(r["cached_glyphs"]) for r in rows)>80, "Japanese font glyphs were not cached"
                assert any(a["page"]==b["page"] and a["ui_redraws"]==b["ui_redraws"] for a,b in zip(rows,rows[1:])), "idle UI rerendered every sample"
                assert any(r["saves"] == "1" for r in rows), "save did not commit"
                assert any(r["volume"] == "40" and r["muted"] == "true" and r["page"] == "Intro" for r in rows), "settings did not change before load"
                assert any(r["loads"] == "1" and r["page"] == "Hill" and r["volume"] == "70" and r["muted"] == "false" for r in rows), "checkpoint did not restore progress/settings"
                assert all(r["audio"] == "Ready" for r in rows[1:]), "ALSA device did not stay ready"
                audio = audio_probe.evidence()
                # Buffered driver output trails input slightly; use stable windows.
                def peak_between(a, b):
                    return max(r["peak"] for r in audio_probe.rows if a <= r["at"] - start <= b)
                assert peak_between(16.8, 17.5) > .001, "loaded audio remained muted"
                assert peak_between(18.8, 19.3) == 0, "paused BGM was not silent"
                assert peak_between(20.2, 20.7) > .001, "BGM did not resume"
                assert peak_between(21.8, 22.5) == 0, "mute did not silence output"
                checks = {"pages": pages, "audio_output": audio, "checks": ["both choices", "mouse button hit testing", "cancel release outside button", "keyboard choice and focus", "restart button", "resize and pointer remapping", "Japanese font cache", "idle UI texture reuse", "F5/F9 progress and volume restoration", "ALSA PCM output/pause/resume/mute", "Escape shutdown"]}
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
            elif sample == "placement":
                assert any(r["inspect_x"] == "110" and r["inspect_y"] == "69" and r["material"] == "2" for r in rows), "grid block not placed under preview"
                assert any(r["placed_images"] == "1" and float(r["last_image_x"]) == 123.25 and float(r["last_image_y"]) == 83.75 for r in rows), "free image lost fractional mouse coordinates"
                assert max(int(r["placed_images"]) for r in rows) == 3, "image click generated duplicates or missed a placement"
                assert any(r["placed_images"] == "3" and float(r["last_image_x"]) == 232.5 and float(r["last_image_y"]) == 117.5 for r in rows), "free image placement ignored camera zoom"
                assert any(r["placed_images"] == "2" and 8.5 < float(r["elapsed_s"]) < 10 for r in rows), "image erase failed"
                assert rows[-1]["placed_images"] == "0", "reset did not remove placed images"
                replay = subprocess.run([str(ROOT / "target/debug" / package), "--replay", str(trace_path), "--workers", "1"], env=env, text=True, capture_output=True, check=True, timeout=30)
                shutil.copyfile(trace_path, MEDIA / f"{prefix}.trace")
                records = trace_path.read_text().splitlines()
                assert any("rect:96:64:8:8:2" in line for line in records) and any("rect:104:64:8:8:2" in line for line in records), "blocks did not snap to the configured grid"
                checks = {"replay": replay.stdout.strip(), "checks": ["8x8 grid blocks", "fractional image placement", "one image per click", "image placement with camera zoom", "image erase", "reset", "authoritative replay with 1 worker", "Escape shutdown"]}
            elif sample == "camera":
                assert any(float(r["camera_x"]) > 60 for r in rows), "camera did not follow moving actor"
                assert any(abs(float(r["camera_width"])-192) < 0.01 for r in rows), "zoom did not change view extent"
                assert any(r["window_width"] == "1280" and r["window_height"] == "720" for r in rows), "real window resize missing"
                inspected = [r for r in rows if r["paused"] == "true" and r["material"] == "2" and abs(float(r["camera_width"])-240)<0.01]
                assert any(int(r["inspect_x"]) == math.floor(float(r["camera_x"])+180) and int(r["inspect_y"]) == math.floor(float(r["camera_y"])+67.5) for r in inspected), "zoomed mouse paint/inspection coordinates disagree"
                assert any(r["facing"] == "-1" for r in rows), "left-facing state missing"
                assert rows[-1]["facing"] == "1", "reset retained previous facing"
                paused = [r for r in rows if r["paused"] == "true"]
                assert all(abs(float(r["display_player_x"])-float(r["player_x"]))<0.001 and abs(float(r["display_player_y"])-float(r["player_y"]))<0.001 for r in paused), "paused/stepped display interpolated away from exact state"
                assert any(0.001 < abs(float(r["display_player_x"])-float(r["player_x"])) < 1.51 for r in rows if r["paused"] == "false"), "no interpolated render positions observed"
                assert float(rows[-1]["camera_width"]) == 480, "full world view not restored"
                replay = subprocess.run([str(ROOT / "target/debug" / package), "--replay", str(trace_path), "--workers", "1"], env=env, text=True, capture_output=True, check=True, timeout=30)
                shutil.copyfile(trace_path, MEDIA / f"{prefix}.trace")
                checks = {"replay": replay.stdout.strip(), "checks": ["camera follow", "zoom", "real window resize", "mouse paint/inspection in world coordinates", "interpolated player presentation", "exact pause/step/reset positions", "facing reset", "client-only capture during resize", "authoritative replay with 1 worker", "Escape shutdown"]}
            elif sample == "world-combat":
                live = [r for r in rows if r["world_x"] and r["world_waiting"] == "false"]
                assert {"Ready", "Playing"}.issubset({r["run_phase"] for r in rows})
                assert all(r["run_phase"] != "Defeated" for r in rows), "combat ended before roundtrip"
                assert max(float(r["world_x"]) for r in live) > 768, "second region boundary not reached"
                assert len({r["world_origin_x"] for r in live}) >= 3
                first = [r for r in live if r["enemy_id"] == "1"]
                despawned = [i for i, r in enumerate(rows) if r["world_origin_x"] == "64" and int(r["despawned_enemies"] or 0) >= 1]
                assert despawned, "enemy outside the region was not released"
                returned = [r for r in rows[max(despawned)+1:] if r["world_origin_x"] == "-960" and r["enemy_id"] not in ("", "1")]
                assert returned and all(r["enemy_health"] == "3" for r in returned), "return did not spawn a fresh encounter"
                assert all(abs(float(r["enemy_x"]) + int(r["world_origin_x"]) + 176.5) < 0.001 for r in first), "enemy global position shifted"
                assert max(int(r["kills"] or 0) for r in rows) == 1, "streamed chasing encounter was not defeated"
                assert max(int(r["resident_chunks"]) for r in live) <= 32
                assert all(r["embedded"] == "false" for r in rows)
                assert all(int(r["rejected_enemy_shots"] or 0) == 0 for r in rows)
                paused = [r["simulation_tick"] for r in rows if r["paused"] == "true"]
                assert len(paused) >= 2 and len(set(paused)) == 1
                replay = subprocess.run([str(ROOT / "target/debug" / package), "--replay", str(trace_path), "--workers", "1"], env=env, text=True, capture_output=True, check=True, timeout=60)
                shutil.copyfile(trace_path, MEDIA / f"{prefix}.trace")
                checks = {"restart": resume_evidence, "resident_peak": max(int(r["resident_chunks"]) for r in live), "replay": replay.stdout.strip(), "checks": ["start screen", "terrain-aware aimed shot", "two region boundaries and return", "outside enemy released and fresh encounter on return", "chasing encounter and kill", "pause freezes combat", "bounded resident chunks", "authoritative replay with 1 worker", "Escape shutdown"]}
            elif sample == "world":
                live=[r for r in rows if r["world_x"]]
                assert live, "no world coordinates"
                origins={r["world_origin_x"] for r in live}
                assert len(origins)>=2, "player never crossed an active-region boundary"
                assert max(int(r["world_x"]) for r in live)>256, "forward exploration did not cross boundary"
                assert min(int(r["world_x"]) for r in live[-15:])<100, "return exploration failed"
                assert max(int(r["resident_chunks"]) for r in live)<=32, "resident budget exceeded"
                assert all(r["embedded"]=="false" for r in live), "world handoff embedded actor"
                assert max(float(r["player_y"]) for r in live)-min(float(r["player_y"]) for r in live)>10, "jump/dig missing"
                chunks=list((temporary/"world"/"42").glob("*.chunk"))
                assert chunks, "exit did not persist terrain changes"
                replay = subprocess.run([str(ROOT / "target/debug" / package), "--replay", str(trace_path), "--workers", "1"], env=env, text=True, capture_output=True, check=True, timeout=60)
                shutil.copyfile(trace_path, MEDIA / f"{prefix}.trace")
                checks={"resident_peak":max(int(r["resident_chunks"]) for r in live),"world_origins_x":sorted(map(int,origins)),"saved_chunks":len(chunks),"replay":replay.stdout.strip(),"checks":["global world coordinates", "active region boundary crossing and return", "resident budget", "jump/dig", "terrain persisted on exit", "explicit region payload replay without disk IO", "Escape shutdown"]}
            elif sample == "combat":
                assert {"Ready", "Playing", "Defeated"}.issubset({r["run_phase"] for r in rows}), "start/play/defeat flow missing"
                assert all(r["simulation_tick"] == "0" for r in rows if r["run_phase"] == "Ready"), "title advanced simulation"
                assert max(int(r["kills"] or 0) for r in rows) == 1, "aimed shots did not defeat enemy"
                assert any(int(r["enemy_shots"] or 0) > 0 for r in rows), "enemy shots missing"
                ended = [r for r in rows if r["run_phase"] == "Defeated"]
                assert len(ended) >= 2 and len({r["simulation_tick"] for r in ended}) == 1, "defeated game did not freeze"
                assert all(r["player_health"] == "0" for r in ended), "defeat without zero health"
                last_defeat = max(i for i, r in enumerate(rows) if r["run_phase"] == "Defeated")
                assert any(r["run_phase"] == "Playing" and r["player_health"] == "3" and r["enemies"] == "1" for r in rows[last_defeat + 1:]), "retry did not restore run"
                inspected = [r for r in rows if r["inspect_x"] == "120" and r["inspect_y"] == "168"]
                assert any(r["material"] == "2" for r in inspected) and any(r["material"] == "0" for r in inspected), "grid placement/erase missing"
                assert max(float(r["player_x"]) for r in rows) > 90, "exploration missing"
                assert max(float(r["player_y"]) for r in rows) > 180, "digging missing"
                assert all(r["embedded"] == "false" for r in rows), "actor embedded"
                hunters = [float(r["enemy_x"]) for r in rows if r.get("enemy_x") and r["run_phase"] == "Playing"]
                assert max(hunters) - min(hunters) > 30, "enemy chase missing"
                arcs = [r for r in rows if float(r.get("shot_gravity") or 0) > 0]
                assert arcs and min(float(r["shot_vy"]) for r in arcs) < 0 and max(float(r["shot_vy"]) for r in arcs) > 0, "rising/falling gravity shot missing"
                replay = subprocess.run([str(ROOT / "target/debug" / package), "--replay", str(trace_path), "--workers", "1"], env=env, text=True, capture_output=True, check=True, timeout=30)
                shutil.copyfile(trace_path, MEDIA / f"{prefix}.trace")
                checks = {"seed": 42, "replay": replay.stdout.strip(), "checks": ["start screen", "aimed ranged combat", "chasing enemy", "rising/falling gravity shot", "enemy defeat", "enemy fire and player damage", "defeated game freezes", "same-seed retry restores health and enemy", "exploration", "digging", "grid placement/erasure", "authoritative replay with 1 worker", "Escape shutdown"]}
            elif sample == "exploration":
                assert {"42", "43"}.issubset({r["exploration_seed"] for r in rows}), "seed generation/switch missing"
                inspected = [r for r in rows if r["inspect_x"] == "120" and r["inspect_y"] == "160"]
                assert any(r["material"] == "2" for r in inspected) and any(r["material"] == "0" for r in inspected), "grid placement/erasure missing"
                assert max(float(r["player_x"]) for r in rows) > 90, "exploration movement missing"
                assert max(float(r["player_y"]) for r in rows if r["exploration_seed"] == "42") > 180, "digging did not open ground"
                retried = [r for r in rows if r["exploration_seed"] == "43" and r["paused"] == "true" and r["simulation_tick"] == "0"]
                assert len(retried) >= 2 and all(float(r["player_x"]) == 60 and float(r["player_y"]) == 168 for r in retried), "paused generation/retry lost safe spawn"
                assert all(r["embedded"] == "false" for r in rows), "actor became embedded"
                replay = subprocess.run([str(ROOT / "target/debug" / package), "--replay", str(trace_path), "--workers", "1"], env=env, text=True, capture_output=True, check=True, timeout=30)
                shutil.copyfile(trace_path, MEDIA / f"{prefix}.trace")
                checks = {"seeds": [42, 43], "replay": replay.stdout.strip(), "checks": ["generated map", "safe spawn", "exploration movement", "digging", "grid placement/erasure", "same-seed retry", "new seed", "authoritative replay with 1 worker", "Escape shutdown"]}
            elif sample == "projectiles":
                assert max(int(r["impacts"]) for r in rows) >= 2, "shots did not hit terrain"
                assert any(int(r["sparks"]) > 0 for r in rows), "impact particles missing"
                assert any(r["material"] == "2" for r in rows) and any(r["material"] == "0" and int(r["impacts"]) > 0 for r in rows), "hit did not destroy inspected wall"
                assert any(int(r["shots"]) > 0 for r in rows), "moving shots missing"
                assert rows[-1]["impacts"] == "0" and rows[-1]["sparks"] == "0", "reset did not clear projectile state"
                replay = subprocess.run([str(ROOT / "target/debug" / package), "--replay", str(trace_path), "--workers", "1"], env=env, text=True, capture_output=True, check=True, timeout=30)
                shutil.copyfile(trace_path, MEDIA / f"{prefix}.trace")
                checks = {"peak_impacts": max(int(r["impacts"]) for r in rows), "replay": replay.stdout.strip(),
                          "checks": ["paused fire waits for step", "moving shots", "material collision", "wall destruction", "cosmetic sparks", "reset", "authoritative replay with 1 worker", "Escape shutdown"]}
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
            if sample == "camera":
                assert all(all(image.getpixel(point) != (0,0,0) for point in [(0,0),(959,0),(0,539),(959,539)]) for image in images), "black desktop margin leaked into client capture"
                checks["checks"].append("no black desktop margin at client corners")
            if sample == "sandbox":
                paused = [r for r in rows if r["paused"] == "true" and r["material"] == "3"]
                assert len({r["simulation_tick"] for r in paused}) == 1, "physics advanced during texture animation"
                patches = {image.crop((494,114,508,128)).tobytes() for (timestamp,_),image in zip(frames,images) if 7.2 <= timestamp-start <= 7.95}
                assert len(patches) >= 2, "material texture did not animate while physics was paused"
                checks["paused_simulation_tick"] = int(paused[0]["simulation_tick"])
                checks["texture_frames_while_paused"] = len(patches)
                checks["checks"].append("material texture animation while physics tick stays fixed")
            if sample in ("world-combat", "world", "combat", "exploration", "platformer", "debugger", "projectiles", "camera", "placement"):
                assert any(int(row.get("sprite_upload_bytes") or 0) > 0 for row in rows), "actor sprites not submitted"
                assert any(any(color == (36,198,225) for _,color in (image.getcolors(maxcolors=100000) or [])) for image in images), "PNG actor pixels missing from real window"
                checks["checks"].append("PNG+JSON actor sprite rendered through shared GPU pipeline")
            durations = [max(10, round((b[0] - a[0]) * 100) * 10) for a, b in zip(frames, frames[1:])]
            durations.append(130)
            images[0].save(MEDIA / f"{prefix}.gif", save_all=True, append_images=images[1:], duration=durations, loop=0, optimize=True, disposal=2)
            with Image.open(MEDIA / f"{prefix}.gif") as gif:
                encoded_frames = gif.n_frames
            assert source_fingerprint()==built_source, "source changed during capture; retain private preview and recapture"
            report = {"source_sha256": built_source,"compiler":subprocess.check_output(["rustc","--version"],text=True).strip(), "sample": sample, "base_revision": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
                      "platform": "Linux Xorg dummy", "adapter": adapter_line, "build_profile": "dev",
                      "target_capture_fps": 8,
                      "observed_capture_fps": round((len(frames) - 1) / (frames[-1][0] - frames[0][0]), 3),
                      "gif_encoded_frames": encoded_frames,
                      "draw_submission_fps_range": [min(rates), max(rates)],
                      "simulation_hz_range": [min(simulation_rates), max(simulation_rates)],
                      "measurement_scope": "On-screen DRAW counts completed present-submission calls per wall-clock interval; SIM counts simulation ticks. GIF rate is capture rate. Render GPU execution time and physical presentation are not measured; CFD compute queries are reported separately when available.",
                      "captured_frames": len(frames), "gif_duration_ms": sum(durations),
                      "capture_target": "game client window (desktop excluded)",
                      "source_size": "actual client size; see window_width/window_height in CSV", "gif_size": [960, 540],
                      **({"font_source": json.loads((ROOT / "samples/dialogue/assets/font-source.json").read_text())} if sample == "dialogue" else {}),
                      "fluid_backend": args.fluid_backend if sample in ("cup", "fluid-wave", "fluid-dam") else None,
                      "fluid_compute_adapter": next((line for line in (local / f"{sample}.log").read_text().splitlines() if line.startswith("fluid_compute=")), None),
                      "simulation_grid": [int(v) for v in args.grid.split("x")],
                      **checks}
            if sample in ("cup", "fluid-wave", "fluid-dam") and args.fluid_backend == "gpu":
                report["solver"] = "Eulerian MAC / GPU two-level Schwarz-PCG (bounded coarse grid), distributed aggregation multigrid-PCG / f64 defect correction / CPU geometric VOF"
            if sample in ("cup", "fluid-wave", "fluid-dam"):
                # Exclude scene entry and paused/sleeping rows. A one-second
                # counter interval can still straddle an input transition.
                active=[r for r in rows if r["cup_x"]][2:]
                active=[r for r in active if r["paused"]=="false" and r["fluid_sleeping"]=="false"]
                if active:
                    report["fluid_active_simulation_hz_range"]=[min(float(r["sim_hz"]) for r in active),max(float(r["sim_hz"]) for r in active)]
                    report["fluid_active_tick_work_ms_range"]=[min(float(r["tick_work_ms"]) for r in active),max(float(r["tick_work_ms"]) for r in active)]
            if sample in ("cup", "fluid-wave", "fluid-dam") and args.fluid_backend == "gpu":
                active=[r for r in rows if r["cup_x"] and r["paused"]=="false" and r["fluid_sleeping"]=="false"]
                active=list({r["simulation_tick"]:r for r in active}.values())
                def measured_range(key,convert=float):
                    values=[convert(r[key]) for r in active if r.get(key)]
                    return [min(values),max(values)] if values else None
                report["fluid_gpu_measurements"]={
                    "timestamp_available": any(r.get("fluid_gpu_pressure_ms") for r in active),
                    "velocity_kernel_ms_range": measured_range("fluid_gpu_velocity_ms"),
                    "velocity_extension_ms_range": measured_range("fluid_velocity_extension_ms"),
                    "pressure_kernel_ms_range": measured_range("fluid_gpu_pressure_ms"),
                    "pressure_iterations_per_tick_range": measured_range("fluid_pressure_iterations",int),
                    "pressure_assembly_ms_range": measured_range("fluid_pressure_assembly_ms"),
                    "pressure_setup_ms_range": measured_range("fluid_pressure_setup_ms"),
                    "readbacks_per_tick_range": measured_range("fluid_gpu_readbacks",int),
                    "upload_bytes_per_tick_range": measured_range("fluid_gpu_upload_bytes",int),
                    "readback_bytes_per_tick_range": measured_range("fluid_gpu_readback_bytes",int),
                    "dispatches_per_tick_range": measured_range("fluid_gpu_dispatches",int),
                    "velocity_workgroups_per_dispatch_range": measured_range("fluid_gpu_velocity_workgroups",int),
                    "pressure_workgroups_per_dispatch_range": measured_range("fluid_gpu_pressure_workgroups",int),
                    "scope": "GPU queries time compute passes, excluding transfer and CPU wait. Host stage durations include CPU work, transfer and synchronization. Latest completed CFD tick per sample; repeated tick IDs deduplicated. Software-adapter times do not measure hardware GPU performance."
                }
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
            if audio_probe:
                audio_probe.close()


if __name__ == "__main__":
    main()
