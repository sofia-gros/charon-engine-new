"""Private, paced ALSA file sink; validate backend PCM without physical speakers.

Writes only aggregate JSON evidence. The audio bytes stay in a temporary FIFO.
The plug converts the backend output to 48kHz stereo float32 before inspection.
"""
import array
import math
import os
from pathlib import Path
import threading
import time


class AudioProbe:
    def __init__(self, directory):
        directory = Path(directory)
        self.fifo = directory / "audio.fifo"
        os.mkfifo(self.fifo, 0o600)
        self.config = directory / "alsa.conf"
        self.config.write_text('''</usr/share/alsa/alsa.conf>
pcm.charon_file {
 type file
 slave.pcm "null"
 file "%s"
 format "raw"
}
pcm.!default {
 type plug
 slave { pcm "charon_file" format FLOAT_LE rate 48000 channels 2 }
}
''' % self.fifo)
        self.fd = os.open(self.fifo, os.O_RDONLY | os.O_NONBLOCK)
        self.stop = threading.Event()
        self.rows = []
        self.total = 0
        self.epoch = None
        self.error = None
        self.thread = threading.Thread(target=self._read, daemon=True)
        self.thread.start()

    def _read(self):
        try:
            while not self.stop.is_set():
                try:
                    data = os.read(self.fd, 4096)
                except BlockingIOError:
                    self.stop.wait(.002)
                    continue
                if not data:
                    self.stop.wait(.002)
                    continue
                if self.epoch is None:
                    self.epoch = time.monotonic()
                assert len(data) % 4 == 0, "unaligned ALSA float32 output"
                samples = array.array("f", data)
                assert all(math.isfinite(s) for s in samples), "nonfinite device output"
                self.total += len(data)
                self.rows.append({"at": time.monotonic(), "seconds": round(self.total / 384000, 4),
                                  "rms": math.sqrt(sum(s*s for s in samples)/len(samples)),
                                  "peak": max(map(abs, samples))})
                deadline = self.epoch + self.total / 384000
                self.stop.wait(max(0, deadline - time.monotonic()))
        except BaseException as error:
            self.error = error

    def close(self):
        self.stop.set()
        self.thread.join(timeout=3)
        os.close(self.fd)
        assert not self.thread.is_alive(), "ALSA probe did not stop"
        if self.error:
            raise self.error

    def evidence(self):
        assert self.rows and any(r["peak"] > .001 for r in self.rows), "no nonzero audio reached ALSA"
        return {"backend": "ALSA plug/file/null (virtual output; no physical speaker verification)",
                "format": "48000 Hz stereo float32", "bytes": self.total,
                "seconds": round(self.total / 384000, 3),
                "peak": max(r["peak"] for r in self.rows),
                "silent_chunks": sum(r["peak"] == 0 for r in self.rows),
                "nonzero_chunks": sum(r["peak"] > .001 for r in self.rows)}
