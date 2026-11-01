"""Generate Nook's original, tiny event sounds as 22.05 kHz mono PCM WAV."""

import math
import os
import struct
import wave


RATE = 22_050
OUT = os.path.join(os.path.dirname(__file__), "wav")
os.makedirs(OUT, exist_ok=True)


def write(name, duration, sample):
    count = round(RATE * duration)
    with wave.open(os.path.join(OUT, name + ".wav"), "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(RATE)
        wav.writeframes(b"".join(
            struct.pack("<h", max(-32767, min(32767, round(sample(i / RATE) * 32767))))
            for i in range(count)
        ))


def tone(freq, duration, volume=0.16, decay=24.0):
    return lambda t: volume * math.exp(-decay * t) * math.sin(2 * math.pi * freq * t)


def sweep(start, end, duration, volume=0.14):
    def sample(t):
        phase = 2 * math.pi * (start * t + (end - start) * t * t / (2 * duration))
        envelope = min(1.0, t / 0.008) * math.exp(-10 * t / duration)
        return volume * envelope * math.sin(phase)
    return sample


write("nook_toggle_on", 0.14,
      lambda t: tone(660, 0.14, 0.12, 16)(t) + tone(990, 0.14, 0.09, 22)(t - 0.045) if t >= 0.045 else tone(660, 0.14, 0.12, 16)(t))
write("nook_expand", 0.24, sweep(420, 980, 0.24, 0.11))
write("nook_collapse", 0.18, sweep(880, 460, 0.18, 0.10))
write("nook_scrub", 0.022, tone(1650, 0.022, 0.08, 125))
