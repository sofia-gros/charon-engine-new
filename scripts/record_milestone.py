"""Record native sample windows using X11 input events and ffmpeg screen capture.

Requires Linux, Xvfb, libX11/libXtst, and ffmpeg. No synthetic game frames are used.
"""
import argparse
import ctypes as c
import ctypes.util
from datetime import datetime, timedelta, timezone
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
SAMPLES = {
    "m1": ("charon-m1-sample", "Charon M1 / Gem Run", "M1 implementation example"),
    "m2": ("charon-m2-sample", "Charon M2 / Falling Sand Lab", "M2 complete"),
}


class X11:
    def __init__(self, display):
        self.x = c.CDLL(ctypes.util.find_library("X11"))
        self.t = c.CDLL(ctypes.util.find_library("Xtst"))
        declarations = {
            "XOpenDisplay": ([c.c_char_p], c.c_void_p),
            "XDefaultRootWindow": ([c.c_void_p], c.c_ulong),
            "XQueryTree": ([c.c_void_p, c.c_ulong, c.POINTER(c.c_ulong), c.POINTER(c.c_ulong), c.POINTER(c.POINTER(c.c_ulong)), c.POINTER(c.c_uint)], c.c_int),
            "XFetchName": ([c.c_void_p, c.c_ulong, c.POINTER(c.c_void_p)], c.c_int),
            "XFree": ([c.c_void_p], c.c_int),
            "XSetInputFocus": ([c.c_void_p, c.c_ulong, c.c_int, c.c_ulong], c.c_int),
            "XStringToKeysym": ([c.c_char_p], c.c_ulong),
            "XKeysymToKeycode": ([c.c_void_p, c.c_ulong], c.c_uint),
            "XFlush": ([c.c_void_p], c.c_int),
            "XCloseDisplay": ([c.c_void_p], c.c_int),
            "XGetGeometry": ([c.c_void_p,c.c_ulong,c.POINTER(c.c_ulong),c.POINTER(c.c_int),c.POINTER(c.c_int),c.POINTER(c.c_uint),c.POINTER(c.c_uint),c.POINTER(c.c_uint),c.POINTER(c.c_uint)], c.c_int),
        }
        for name, (args, result) in declarations.items():
            method = getattr(self.x, name)
            method.argtypes, method.restype = args, result
        for name, args in {
            "XTestFakeKeyEvent": [c.c_void_p,c.c_uint,c.c_int,c.c_ulong],
            "XTestFakeButtonEvent": [c.c_void_p,c.c_uint,c.c_int,c.c_ulong],
            "XTestFakeMotionEvent": [c.c_void_p,c.c_int,c.c_int,c.c_int,c.c_ulong],
        }.items():
            method = getattr(self.t,name)
            method.argtypes, method.restype = args, c.c_int
        self.display = self.x.XOpenDisplay(display.encode())
        if not self.display:
            raise RuntimeError(f"cannot open {display}")
        self.root = self.x.XDefaultRootWindow(self.display)

    def find(self, title):
        root, parent = c.c_ulong(), c.c_ulong()
        children, count = c.POINTER(c.c_ulong)(), c.c_uint()
        self.x.XQueryTree(self.display,self.root,c.byref(root),c.byref(parent),c.byref(children),c.byref(count))
        result = None
        for i in range(count.value):
            name = c.c_void_p()
            if self.x.XFetchName(self.display,children[i],c.byref(name)) and name.value:
                text = c.string_at(name).decode(errors="replace")
                self.x.XFree(name)
                if text.startswith(title):
                    result = children[i]
                    break
        if children:
            self.x.XFree(children)
        return result

    def geometry(self, window):
        root, x, y = c.c_ulong(), c.c_int(), c.c_int()
        w,h,border,depth = c.c_uint(),c.c_uint(),c.c_uint(),c.c_uint()
        if not self.x.XGetGeometry(self.display,window,c.byref(root),c.byref(x),c.byref(y),c.byref(w),c.byref(h),c.byref(border),c.byref(depth)):
            raise RuntimeError("cannot read window geometry")
        return x.value,y.value,w.value,h.value

    def focus(self,window):
        self.x.XSetInputFocus(self.display,window,2,0)
        self.x.XFlush(self.display)

    def key(self,key,seconds=0.08):
        code = self.x.XKeysymToKeycode(self.display,self.x.XStringToKeysym(key.encode()))
        if not code:
            raise RuntimeError(f"unknown key {key}")
        self.t.XTestFakeKeyEvent(self.display,code,1,0)
        self.x.XFlush(self.display)
        time.sleep(seconds)
        self.t.XTestFakeKeyEvent(self.display,code,0,0)
        self.x.XFlush(self.display)
        time.sleep(0.1)

    def paint(self,x,y,seconds,button=1):
        self.t.XTestFakeMotionEvent(self.display,-1,x,y,0)
        self.t.XTestFakeButtonEvent(self.display,button,1,0)
        self.x.XFlush(self.display)
        time.sleep(seconds)
        self.t.XTestFakeButtonEvent(self.display,button,0,0)
        self.x.XFlush(self.display)


def stop(process):
    if process and process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


def record(milestone,output,xvfb,display):
    binary,title,status = SAMPLES[milestone]
    executable = ROOT / "target/release" / binary
    if not executable.exists():
        raise SystemExit(f"build first: cargo build --release -p {binary}")
    if not xvfb or not shutil.which("ffmpeg"):
        raise SystemExit("Xvfb and ffmpeg are required")
    output.parent.mkdir(parents=True,exist_ok=True)
    env = dict(os.environ,DISPLAY=display)
    actions = []
    server = app = recorder = connection = None
    with tempfile.TemporaryDirectory(prefix="charon-record-") as temporary:
        temporary = Path(temporary)
        with (temporary/"xvfb.log").open("w+") as server_log, (temporary/"app.log").open("w+") as app_log, (temporary/"capture.log").open("w+") as capture_log:
            try:
                Path("/tmp/.X11-unix").mkdir(mode=0o1777, exist_ok=True)
                server = subprocess.Popen([xvfb,display,"-screen","0","1280x900x24","-nolisten","tcp","-ac"],stdout=server_log,stderr=subprocess.STDOUT)
                for _ in range(50):
                    if server.poll() is not None:
                        raise RuntimeError("Xvfb exited; choose an unused display")
                    try:
                        connection = X11(display)
                        break
                    except RuntimeError:
                        time.sleep(0.1)
                if connection is None:
                    raise RuntimeError("Xvfb did not become ready")
                app = subprocess.Popen([str(executable)],cwd=ROOT,env=env,stdout=app_log,stderr=subprocess.STDOUT)
                window = None
                for _ in range(100):
                    if app.poll() is not None:
                        raise RuntimeError("sample exited before opening its window")
                    window = connection.find(title)
                    if window:
                        break
                    time.sleep(0.1)
                if not window:
                    raise RuntimeError("sample window not found")
                x,y,width,height = connection.geometry(window)
                connection.focus(window)
                video = temporary/"screen.mkv"
                recorder = subprocess.Popen([
                    "ffmpeg","-hide_banner","-loglevel","error","-y","-f","x11grab",
                    "-draw_mouse","1","-framerate","20","-video_size",f"{width}x{height}",
                    "-i",f"{display}+{x},{y}","-c:v","ffv1",str(video),
                ],stdin=subprocess.PIPE,stdout=capture_log,stderr=subprocess.STDOUT)
                start = time.monotonic()
                def action(label,operation):
                    actions.append({"seconds":round(time.monotonic()-start,2),"action":label})
                    operation()
                time.sleep(0.8)
                if milestone == "m1":
                    action("Move right (D)",lambda:connection.key("d",0.26))
                    action("Move down and collect gem (S)",lambda:connection.key("s",0.12))
                    time.sleep(0.5)
                    action("Move toward wall (D)",lambda:connection.key("d",0.55))
                    action("Pause (P)",lambda:connection.key("p"))
                    time.sleep(0.7)
                    action("Resume (P)",lambda:connection.key("p"))
                    action("Restart (R)",lambda:connection.key("r"))
                    time.sleep(0.8)
                else:
                    time.sleep(0.7)
                    action("Paint sand with left mouse button",lambda:connection.paint(x+32+160*3,y+88+65*3,0.7))
                    action("Select water (2)",lambda:connection.key("2"))
                    action("Paint water with left mouse button",lambda:connection.paint(x+32+210*3,y+88+75*3,0.6))
                    action("Open barrier (G)",lambda:connection.key("g"))
                    time.sleep(1.4)
                    action("Reset (R)",lambda:connection.key("r"))
                    time.sleep(0.35)
                    action("Convert barrier into sand (B)",lambda:connection.key("b"))
                    time.sleep(1.2)
                    action("Pause (P)",lambda:connection.key("p"))
                    time.sleep(0.6)
                    action("Resume (P)",lambda:connection.key("p"))
                    time.sleep(0.5)
                    action("Register-demo cloud: select C",lambda:connection.key("c"))
                    action("Paint cloud gas",lambda:connection.paint(x+32+200*3,y+88+155*3,0.4))
                    action("Register-demo nitrogen: select N",lambda:connection.key("n"))
                    action("Paint nitrogen gas",lambda:connection.paint(x+32+230*3,y+88+155*3,0.4))
                    time.sleep(0.6)
                    action("Select registered soil (9)",lambda:connection.key("9"))
                    action("Paint soil near floor",lambda:connection.paint(x+32+100*3,y+88+185*3,0.5))
                    action("Change selected material repose (A)",lambda:connection.key("a"))
                    time.sleep(0.8)
                if recorder.poll() is not None:
                    raise RuntimeError("screen recorder exited early")
                recorder.communicate(b"q\n",timeout=10)
                if recorder.returncode:
                    raise RuntimeError("screen recording failed")
                connection.key("Escape")
                app.wait(timeout=5)
                if app.returncode:
                    raise RuntimeError("sample did not shut down successfully")
                filters = "fps=12,scale=768:-1:flags=lanczos"
                palette = temporary/"palette.png"
                subprocess.run(["ffmpeg","-hide_banner","-loglevel","error","-y","-i",str(video),"-vf",filters+",palettegen=max_colors=128",str(palette)],check=True)
                subprocess.run(["ffmpeg","-hide_banner","-loglevel","error","-y","-i",str(video),"-i",str(palette),"-lavfi",filters+"[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=3","-loop","0",str(output)],check=True)
                metadata = {
                    "milestone":milestone,"status":status,
                    "sample_commit":subprocess.check_output(["git","rev-parse","HEAD"],cwd=ROOT,text=True).strip(),
                    "recorded_at":datetime.now(timezone(timedelta(hours=9))).isoformat(timespec="seconds"),
                    "environment":platform.platform(),"display":"Linux Xvfb; actual native window and XTest input",
                    "windows_11_verified":False,"sample_command":f"cargo run --release -p {binary}",
                    "gif_fps":12,"gif_width":768,"actions":actions,
                }
                output.with_suffix(".json").write_text(json.dumps(metadata,indent=2,ensure_ascii=False)+"\n",encoding="utf-8")
            except Exception:
                for label,log in [("Xvfb",server_log),("sample",app_log),("capture",capture_log)]:
                    log.flush();log.seek(0)
                    print(label+": "+log.read()[-3000:])
                raise
            finally:
                stop(recorder);stop(app)
                if connection:
                    connection.x.XCloseDisplay(connection.display)
                stop(server)
    label = output.relative_to(ROOT) if output.is_relative_to(ROOT) else output
    print(f"Recorded {label} ({output.stat().st_size} bytes)")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("milestone",choices=SAMPLES)
    parser.add_argument("--xvfb",default=shutil.which("Xvfb"))
    parser.add_argument("--display",default=":97")
    parser.add_argument("--output",type=Path)
    args = parser.parse_args()
    output = args.output or ROOT/"docs/demos"/f"{args.milestone}-demo.gif"
    record(args.milestone,output.resolve(),args.xvfb,args.display)
