"""Screenshot VIctory's window (any run, not only the harness's) and optionally press keys first.

    python shot.py --out C:\\Users\\Corgan\\victory-mcp\\runs\\route\\03-down.png [--press Down] [--wait 2.5]

--press appends to the input file (VICTORY_INPUT_FILE, default <work>\\input.txt) and waits --wait seconds
before the capture. Prints the window title and the saved path.
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.wintypes as wt
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import prologue  # noqa: E402

user32 = ctypes.WinDLL("user32", use_last_error=True)
gdi32 = ctypes.WinDLL("gdi32")
try:
    user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
except Exception:
    pass


def victory_pid() -> int:
    out = subprocess.run(
        ["tasklist", "/FI", "IMAGENAME eq victory.exe", "/FO", "CSV", "/NH"], capture_output=True, text=True
    ).stdout
    for line in out.splitlines():
        parts = [p.strip('"') for p in line.split('","')]
        if len(parts) > 1 and parts[0].lower() == "victory.exe":
            return int(parts[1])
    raise SystemExit("victory.exe is not running")


def grab(pid: int, path: Path) -> str:
    from PIL import Image

    tree = prologue._children(pid)
    found: list[tuple[int, str, int]] = []
    Enum = ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)

    def cb(hwnd, _lp):
        owner = wt.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value in tree and user32.IsWindowVisible(hwnd):
            r = wt.RECT()
            user32.GetClientRect(hwnd, ctypes.byref(r))
            buf = ctypes.create_unicode_buffer(512)
            user32.GetWindowTextW(hwnd, buf, 512)
            found.append((hwnd, buf.value, r.right * r.bottom))
        return True

    user32.EnumWindows(Enum(cb), 0)
    if not found:
        raise SystemExit("no window")
    hwnd, title, _ = max(found, key=lambda t: t[2])
    r = wt.RECT()
    user32.GetClientRect(hwnd, ctypes.byref(r))
    w, h = r.right, r.bottom
    hdc = user32.GetDC(hwnd)
    mem = gdi32.CreateCompatibleDC(hdc)
    bmp = gdi32.CreateCompatibleBitmap(hdc, w, h)
    gdi32.SelectObject(mem, bmp)
    user32.PrintWindow(hwnd, mem, 3)  # client area, full content (DirectX)

    class BMI(ctypes.Structure):
        _fields_ = [("biSize", wt.DWORD), ("biWidth", ctypes.c_long), ("biHeight", ctypes.c_long),
                    ("biPlanes", wt.WORD), ("biBitCount", wt.WORD), ("biCompression", wt.DWORD),
                    ("biSizeImage", wt.DWORD), ("biXPelsPerMeter", ctypes.c_long),
                    ("biYPelsPerMeter", ctypes.c_long), ("biClrUsed", wt.DWORD), ("biClrImportant", wt.DWORD)]

    bmi = BMI(ctypes.sizeof(BMI), w, -h, 1, 32, 0, 0, 0, 0, 0, 0)
    data = ctypes.create_string_buffer(w * h * 4)
    gdi32.GetDIBits(mem, bmp, 0, h, data, ctypes.byref(bmi), 0)
    gdi32.DeleteObject(bmp)
    gdi32.DeleteDC(mem)
    user32.ReleaseDC(hwnd, hdc)
    img = Image.frombuffer("RGB", (w, h), data, "raw", "BGRX", 0, 1)
    if w > 1280:
        img = img.resize((1280, int(h * 1280 / w)))
    path.parent.mkdir(parents=True, exist_ok=True)
    img.save(path)
    return title


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--press", default="")
    ap.add_argument("--hold", type=int, default=150)
    ap.add_argument("--wait", type=float, default=2.5)
    ap.add_argument("--input", default=r"C:\Users\Corgan\victory-mcp\input.txt")
    a = ap.parse_args()
    pid = victory_pid()
    if a.press:
        for chord in prologue.check_keys(a.press):
            prologue.press(Path(a.input), chord, a.hold)
            time.sleep(0.6)
        time.sleep(a.wait)
    title = grab(pid, Path(a.out))
    print(f"{title}\n{a.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
