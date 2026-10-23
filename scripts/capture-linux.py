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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sample", choices=("sandbox", "dialogue", "platformer", "debugger", "projectiles", "camera", "placement", "sea", "cup"), default="sandbox")
    sample = parser.parse_args().sample
    package = "charon-sandbox" if sample != "dialogue" else "charon-dialogue-sample"
    prefix = f"{sample}-linux"
    local = LOCAL / sample
    duration = 23 if sample == "dialogue" else (40 if sample == "sea" else (36 if sample == "cup" else 14))
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
                client = subprocess.Popen([str(ROOT / "target/debug" / package), *(["--workers", "4"] if sample != "dialogue" else ["--save-file", str(local / "slot.json")]), *(["--play"] if sample in ("platformer", "debugger", "projectiles", "camera") else []), *(["--input-log", str(trace_path)] if sample != "dialogue" else []), "--seconds", str(duration + 20), "--record", str(local / "metrics.csv")], env=env, stdout=log, stderr=subprocess.STDOUT)
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
            if sample == "sandbox":
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
            elif sample == "cup":
                at(.5)
                inputs.key(ord("k"))
                at(1)
                inputs.key(ord("="))
                inputs.key(ord("="))
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
            rows = list(csv.DictReader((local / "metrics.csv").open()))
            if sample == "sandbox":
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
                            submerged=False
                            for y in range(top,bottom):
                                r,g,b=rgb.getpixel((x,y))
                                submerged |= r<80 and g>90 and b>180
                                assert not (submerged and (r,g,b)==(16,23,36)), f"rendered air pocket at ({x},{y}), t={timestamp-start:.2f}s"
                    checked_frames+=1
                assert checked_frames>=50, "insufficient moving-cup frames checked"
                replay=subprocess.run([str(ROOT / "target/debug" / package),"--replay",str(trace_path),"--workers","1"],env=env,text=True,capture_output=True,check=True,timeout=50)
                shutil.copyfile(trace_path,MEDIA / f"{prefix}.trace")
                checks={"water_cells":water,"oil_cells":oil[-1],"rendered_layers":layer_bands,"air_pocket_frames_checked":checked_frames,"cup_x_range":[min(xs),max(xs)],"replay":replay.stdout.strip(),"checks":["game-owned cup shape", "left/right real input", "separate water/oil mass conservation", "oil injection into air", "no rendered air pockets below water during motion", "oil above water in rendered output", "pause freezes cup and liquid", "resting contents sleep", "authoritative replay with one worker", "Escape shutdown"]}
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
            if sample in ("platformer", "debugger", "projectiles", "camera", "placement"):
                assert any(int(row.get("sprite_upload_bytes") or 0) > 0 for row in rows), "actor sprites not submitted"
                assert any(any(color == (36,198,225) for _,color in (image.getcolors(maxcolors=100000) or [])) for image in images), "PNG actor pixels missing from real window"
                checks["checks"].append("PNG+JSON actor sprite rendered through shared GPU pipeline")
            durations = [max(10, round((b[0] - a[0]) * 100) * 10) for a, b in zip(frames, frames[1:])]
            durations.append(130)
            images[0].save(MEDIA / f"{prefix}.gif", save_all=True, append_images=images[1:], duration=durations, loop=0, optimize=True, disposal=2)
            with Image.open(MEDIA / f"{prefix}.gif") as gif:
                encoded_frames = gif.n_frames
            source_hash = hashlib.sha256()
            for path in sorted(path for folder in ("engine", "plugins", "samples") for path in (ROOT / folder).rglob("*")):
                if path.is_file() and path.parts[len(ROOT.parts)] in ("engine", "plugins", "samples") and path.suffix in (".rs", ".wgsl", ".toml", ".png", ".jpg", ".json", ".ttf", ".otf"):
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
                      "capture_target": "game client window (desktop excluded)",
                      "source_size": "actual client size; see window_width/window_height in CSV", "gif_size": [960, 540],
                      **({"font_source": json.loads((ROOT / "samples/dialogue/assets/font-source.json").read_text())} if sample == "dialogue" else {}),
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
            if audio_probe:
                audio_probe.close()


if __name__ == "__main__":
    main()
