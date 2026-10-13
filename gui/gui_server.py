#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
gui_server.py — launch-soundpad 本地控制台（Web 界面后端）

用法：
  python gui_server.py [端口]       默认 8787

界面功能：
  - 播放设备手动选择（本地监听 / VoiceMeeter 虚拟设备 / MIDI 设备），存回 config.yaml
  - 8x8 按键网格，每个按键的播放内容（音频文件）可自定义，存回 pages/
  - 试听、上传音频、整页从语言包文件夹生成、页面增删改名
  - 一键启动 / 停止 launch-soundpad.exe 并查看日志

零配置依赖：Python 标准库 + （可选）sounddevice/soundfile/numpy 用于试听与设备枚举。
未装这些库时界面仍可正常编辑配置，只是没有设备下拉和试听。
"""

import ctypes
import json
import os
import re
import subprocess
import sys
import threading
import time
import traceback
from collections import deque
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse, parse_qs

HERE = Path(__file__).resolve().parent          # .../mfloy_launchpad/gui
PROJECT_ROOT = HERE.parent                       # .../mfloy_launchpad
CONFIG_PATH = PROJECT_ROOT / "config.yaml"
PAGES_DIR = PROJECT_ROOT / "pages"
SOUNDS_DIR = PROJECT_ROOT / "sounds"
EXE_PATH = PROJECT_ROOT / "target" / "release" / "launch-soundpad.exe"
INDEX_HTML = HERE / "index.html"

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8787

AUDIO_EXTS = (".wav", ".mp3", ".ogg", ".flac", ".aiff", ".aif", ".m4a")

# MK1 三色 LED：velocity = 16*green + red + 12
COLORS = {
    "green": 60,   # 绿（亮）
    "amber": 63,   # 琥珀（亮）
    "yellow": 62,  # 黄
    "red": 15,     # 红（亮）
    "off": 12,     # 灭
}
DEFAULT_COLOR = COLORS["green"]

# ---------------------------------------------------------------- 可选音频库
#
# 按需加载：PortAudio（sounddevice）初始化偶尔会卡很久，不能拖住启动——
# 服务必须先把端口绑上、页面能打开，设备枚举/试听等第一次用到时再加载。

_audio = None    # None=还没加载, False=不可用, (numpy, sounddevice, soundfile)
AUDIO_OK = None  # 加载后才知道


def load_audio():
    global _audio, AUDIO_OK
    if _audio is None:
        try:
            import numpy
            import sounddevice
            import soundfile
            _audio = (numpy, sounddevice, soundfile)
        except Exception:
            _audio = False
        AUDIO_OK = bool(_audio)
        if not AUDIO_OK:
            print("[!] 未安装 sounddevice/soundfile/numpy：设备下拉与试听不可用，编辑功能正常")
    return _audio


def _audio_libs():
    return _audio   # 仅供展示，不触发加载


# ---------------------------------------------------------------- 工具

def grid_notes():
    """MK1 的 64 个网格 note，行优先：note = 16*行 + 列"""
    return [16 * row + col for row in range(8) for col in range(8)]


def note_label(note):
    row, col = divmod(note, 16)
    return "第%d行 第%d列" % (row + 1, col + 1)


def list_audio_devices():
    """输出设备列表（优先 WASAPI——与 Rust cpal 看到的端点一致；去重）"""
    libs = load_audio()
    if not libs:
        return []
    _, sd, _ = libs
    try:
        apis = sd.query_hostapis()
        default_idx = sd.default.device[1]

        def collect(api_filter):
            by_name = {}
            for i, d in enumerate(sd.query_devices()):
                if d["max_output_channels"] <= 0:
                    continue
                api = apis[d["hostapi"]]["name"] if 0 <= d["hostapi"] < len(apis) else "?"
                if api_filter and api_filter not in api:
                    continue
                rec = {"index": i, "name": d["name"], "api": api,
                       "is_default": i == default_idx}
                prev = by_name.get(d["name"])
                if prev is None:
                    by_name[d["name"]] = rec
            return sorted(by_name.values(), key=lambda r: r["name"].lower())

        devs = collect("WASAPI")
        if not devs:
            devs = collect(None)
        return devs
    except Exception:
        return []


def device_index_by_name(name):
    """按名字（子串）找输出设备 index，供试听用"""
    libs = load_audio()
    if not libs or not name:
        return None
    _, sd, _ = libs
    try:
        for i, d in enumerate(sd.query_devices()):
            if d["max_output_channels"] > 0 and (d["name"] == name or name in d["name"]):
                return i
    except Exception:
        pass
    return None


def list_midi_devices():
    """Windows winmm 枚举 MIDI 输入/输出端口（零依赖 ctypes）"""
    out = {"in": [], "out": []}
    if os.name != "nt":
        return out
    try:
        winmm = ctypes.windll.winmm
        winmm.midiInGetDevCapsA.argtypes = [ctypes.c_uint, ctypes.c_void_p, ctypes.c_uint]
        winmm.midiOutGetDevCapsA.argtypes = [ctypes.c_uint, ctypes.c_void_p, ctypes.c_uint]
        winmm.midiInGetDevCapsA.restype = ctypes.c_uint
        winmm.midiOutGetDevCapsA.restype = ctypes.c_uint

        def _name(buf):
            # MIDIINCAPS/MIDIOUTCAPS: wMid(2) wPid(2) vDriverVersion(4) szPname[32]
            raw = buf.raw[8:40]
            return raw.split(b"\x00")[0].decode("mbcs", "replace")

        n = winmm.midiInGetNumDevs()
        for i in range(n):
            caps = ctypes.create_string_buffer(64)
            if winmm.midiInGetDevCapsA(i, caps, ctypes.sizeof(caps)) == 0:
                out["in"].append(_name(caps))
        n = winmm.midiOutGetNumDevs()
        for i in range(n):
            caps = ctypes.create_string_buffer(64)
            if winmm.midiOutGetDevCapsA(i, caps, ctypes.sizeof(caps)) == 0:
                out["out"].append(_name(caps))
    except Exception:
        pass
    return out


# ---------------------------------------------------------------- config.yaml

# 界面可改的键（其余键如 bookmark_*、debug_mode 原样保留）
EDITABLE_KEYS = ("midi_in_device", "midi_out_device", "output_device",
                 "virtual_device", "hold_to", "local_monitor", "virtual_volume",
                 "sounds_dir")


def _yaml_quote(value):
    v = str(value)
    if v == "":
        return ""
    if re.search(r'[:#]', v) or v != v.strip() or v.startswith(("-", "{", "[")):
        return '"%s"' % v.replace('"', "'")
    return v


def read_config():
    cfg = {}
    if not CONFIG_PATH.exists():
        return cfg
    for line in CONFIG_PATH.read_text(encoding="utf-8").splitlines():
        m = re.match(r"^([A-Za-z0-9_]+)\s*:\s*(.*)$", line)
        if m:
            cfg[m.group(1)] = m.group(2).strip().strip('"').strip("'")
    return cfg


def write_config(updates):
    """按行改写 config.yaml：只动给定键，保留注释与其他键的顺序"""
    lines = []
    if CONFIG_PATH.exists():
        lines = CONFIG_PATH.read_text(encoding="utf-8").splitlines()
    before = read_config()
    remaining = dict(updates)
    out = []
    for line in lines:
        m = re.match(r"^([A-Za-z0-9_]+)\s*:\s*(.*)$", line)
        if m and m.group(1) in remaining:
            key = m.group(1)
            val = _yaml_quote(remaining.pop(key))
            out.append("%s: %s" % (key, val) if val else "%s:" % key)
        else:
            out.append(line)
    # 新增的键追加到末尾
    for key, val in remaining.items():
        q = _yaml_quote(val)
        out.append("%s: %s" % (key, q) if q else "%s:" % key)
    CONFIG_PATH.write_text("\n".join(out) + "\n", encoding="utf-8")
    # 审计日志：记录每次配置写入，便于排查“值被清空”类问题
    try:
        with open(PROJECT_ROOT / "config_write.log", "a", encoding="utf-8") as fh:
            fh.write("[%s] updates=%s\n  before=%s\n  after=%s\n" % (
                time.strftime("%Y-%m-%d %H:%M:%S"), updates, before, read_config()))
    except Exception:
        pass
    return read_config()


# ---------------------------------------------------------------- pages

def parse_page_file(path):
    """-> {note(int): {"path": str, "color": int}}"""
    pads = {}
    try:
        text = path.read_text(encoding="utf-8")
    except Exception:
        return pads
    for line in text.splitlines():
        line = line.strip()
        if not line:
            continue
        parts = line.split(";")
        if len(parts) < 3:
            continue
        try:
            note = int(parts[0])
            color = int(parts[2])
        except ValueError:
            continue
        pads[note] = {"path": parts[1], "color": color}
    return pads


def list_pages():
    pages = []
    if not PAGES_DIR.is_dir():
        return pages
    for f in sorted(PAGES_DIR.iterdir(), key=lambda p: p.name):
        if not f.is_file():
            continue
        pads = parse_page_file(f)
        pages.append({
            "name": f.name,
            "label": re.sub(r"^\d+_", "", f.name) or f.name,
            "pads": {str(n): p for n, p in pads.items()},
            "count": len(pads),
        })
    return pages


def portable_path(p):
    """数据目录内的绝对路径 -> 相对路径（页面文件存相对路径，整个文件夹搬走也能用）"""
    try:
        path = Path(p)
        if not path.is_absolute():
            return str(p)
        rel = path.relative_to(PROJECT_ROOT)
        return str(rel).replace("\\", "/")
    except Exception:
        return str(p)


def resolve_path(p):
    """相对路径按数据目录补全（试听用）"""
    path = Path(p)
    return path if path.is_absolute() else (PROJECT_ROOT / path)


def save_page(name, pads):
    """pads: {note(str/int): {"path": str, "color": int}}"""
    PAGES_DIR.mkdir(exist_ok=True)
    safe = re.sub(r"[\\/:*?\"<>|]", "_", name)
    path = PAGES_DIR / safe
    lines = []
    for note in sorted(int(n) for n in pads):
        spec = pads[str(note)] if str(note) in pads else pads[note]
        p = (spec.get("path") or "").strip()
        if not p:
            continue
        color = int(spec.get("color") or DEFAULT_COLOR)
        lines.append("%d;%s;%d" % (note, portable_path(p), color))
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return safe


def next_page_number():
    best = 0
    if PAGES_DIR.is_dir():
        for f in PAGES_DIR.iterdir():
            m = re.match(r"^(\d+)_", f.name)
            if m:
                best = max(best, int(m.group(1)))
    return best + 1


def sounds_root():
    """语音包根目录：config 里的 sounds_dir（相对数据目录），缺省 sounds"""
    d = (read_config().get("sounds_dir") or "").strip()
    p = Path(d) if d else Path("sounds")
    return p if p.is_absolute() else (PROJECT_ROOT / p)


def pack_dirs():
    """[(包名, 目录)]：设了 sounds_dir 就那一个文件夹算一个语音包；否则 sounds/ 下每个子文件夹各一个"""
    root = sounds_root()
    if (read_config().get("sounds_dir") or "").strip():
        return [(root.name or "语音包", root)]
    out = []
    if root.is_dir():
        for d in sorted(root.iterdir(), key=lambda x: x.name):
            if d.is_dir():
                out.append((d.name, d))
    return out


def pack_dir_by_name(name):
    for n, d in pack_dirs():
        if n == name:
            return d
    return None


def scan_packs():
    """语音包 -> [{name, dir, files:[{name, path, size}]}]"""
    packs = []
    for name, d in pack_dirs():
        files = []
        if d.is_dir():
            for f in sorted(d.iterdir(), key=lambda p: p.name.lower()):
                if f.is_file() and f.suffix.lower() in AUDIO_EXTS:
                    files.append({"name": f.name, "path": str(f), "size": f.stat().st_size})
        if files:
            packs.append({"name": name, "dir": str(d), "files": files})
    return packs


# ---------------------------------------------------------------- 试听

class Preview:
    """单文件试听：读取 -> 单声道 -> 重采样 -> 输出到指定设备"""

    def __init__(self):
        self.lock = threading.Lock()
        self.stream = None
        self.stop_flag = False

    def stop(self):
        with self.lock:
            self.stop_flag = True
            s, self.stream = self.stream, None
        if s is not None:
            try:
                s.stop()
                s.close()
            except Exception:
                pass

    def play(self, path, device_index=None, gain=0.9):
        self.stop()
        libs = load_audio()
        if not libs:
            return {"ok": False, "error": "未安装 sounddevice/soundfile，无法试听"}
        np, sd, sf = libs
        try:
            data, sr = sf.read(path, dtype="float32", always_2d=False)
        except Exception as e:
            return {"ok": False, "error": "无法读取音频: %s" % e}
        if data.ndim > 1:
            data = data.mean(axis=1)
        data = np.ascontiguousarray(data, dtype=np.float32)
        try:
            if device_index is None:
                out = sd.default.device[1] if sd.default.device[1] is not None else None
            else:
                out = device_index
            info = sd.query_devices(out) if out is not None else sd.query_devices()
            dev_sr = int(info["default_samplerate"]) if out is not None else 48000
            if sr != dev_sr and len(data) > 1:
                n = max(1, int(round(len(data) * dev_sr / sr)))
                data = np.interp(np.linspace(0, 1, n), np.linspace(0, 1, len(data)),
                                 data).astype(np.float32)
            channels = min(2, max(1, int(info["max_output_channels"])))

            pos = [0]
            done = threading.Event()

            def cb(outdata, frames, time_info, status):
                end = pos[0] + frames
                if end <= len(data):
                    chunk = data[pos[0]:end]
                    pos[0] = end
                else:
                    chunk = np.zeros(frames, dtype=np.float32)
                    n = max(0, len(data) - pos[0])
                    chunk[:n] = data[pos[0]:]
                    pos[0] = len(data)
                    done.set()
                if channels == 1:
                    outdata[:, 0] = chunk * gain
                else:
                    outdata[:, 0] = chunk * gain
                    outdata[:, 1] = chunk * gain

            with self.lock:
                self.stop_flag = False
                self.stream = sd.OutputStream(device=out, samplerate=dev_sr,
                                              channels=channels, dtype="float32",
                                              callback=cb)
                self.stream.start()
            threading.Thread(target=self._wait_done, args=(done,), daemon=True).start()
            return {"ok": True, "seconds": round(len(data) / dev_sr, 2)}
        except Exception as e:
            return {"ok": False, "error": "播放失败: %s" % e}

    def _wait_done(self, done):
        done.wait(60)
        if done.is_set():
            with self.lock:
                if not self.stop_flag:
                    s, self.stream = self.stream, None
            try:
                if s:
                    s.stop()
                    s.close()
            except Exception:
                pass


preview = Preview()


# ---------------------------------------------------------------- Launchpad 熄灯

# MK1：网格 note（16*行+列）+ 右列圆键 + 顶排 CC 104-111；velocity 12 = 灭
MK1_LED_NOTES = ([16 * r + c for r in range(8) for c in range(8)]
                 + [8, 24, 40, 56, 72, 88, 120]
                 + list(range(104, 112)))
LED_OFF = 12


def _find_midi_out_id(hint):
    """winmm 里按名字找 MIDI 输出端口 -> (id, name)"""
    winmm = ctypes.windll.winmm
    winmm.midiOutGetDevCapsA.argtypes = [ctypes.c_uint, ctypes.c_void_p, ctypes.c_uint]
    winmm.midiOutGetDevCapsA.restype = ctypes.c_uint
    found = []
    for i in range(winmm.midiOutGetNumDevs()):
        caps = ctypes.create_string_buffer(64)
        if winmm.midiOutGetDevCapsA(i, caps, ctypes.sizeof(caps)) == 0:
            found.append((i, caps.raw[8:40].split(b"\x00")[0].decode("mbcs", "replace")))
    h = (hint or "").lower()
    for i, nm in found:
        if h and h in nm.lower():
            return i, nm
    for i, nm in found:
        if "launchpad" in nm.lower():
            return i, nm
    return None, None


def clear_launchpad_leds(hint=None):
    """关闭 Launchpad 所有 LED。

    进程被强杀（或崩溃）时 Rust 端没机会清灯，pad 会一直亮着，
    所以由 GUI 在进程结束后直接发一遍「灭」的 MIDI 消息。
    """
    if os.name != "nt":
        return {"ok": False, "error": "仅支持 Windows"}
    if hint is None:
        hint = read_config().get("midi_out_device") or "Launchpad"
    try:
        winmm = ctypes.windll.winmm
        winmm.midiOutOpen.argtypes = [ctypes.c_void_p, ctypes.c_uint,
                                      ctypes.c_void_p, ctypes.c_void_p, ctypes.c_uint]
        winmm.midiOutShortMsg.argtypes = [ctypes.c_void_p, ctypes.c_uint]
        idx, dev = _find_midi_out_id(hint)
        if idx is None:
            return {"ok": False, "error": "找不到 MIDI 输出设备（%s）" % hint}
        handle = ctypes.c_void_p()
        if winmm.midiOutOpen(ctypes.byref(handle), idx, None, 0, 0) != 0:
            return {"ok": False, "error": "midiOutOpen 失败: %s" % dev}
        try:
            for note in MK1_LED_NOTES:
                winmm.midiOutShortMsg(handle, 0x90 | (note << 8) | (LED_OFF << 16))
        finally:
            winmm.midiOutClose(handle)
        return {"ok": True, "device": dev, "count": len(MK1_LED_NOTES)}
    except Exception as e:
        return {"ok": False, "error": "熄灯失败: %s" % e}


# ---------------------------------------------------------------- MIDI 按键监视

MIDIINPROC = ctypes.WINFUNCTYPE(
    None, ctypes.c_void_p, ctypes.c_uint,
    ctypes.c_void_p, ctypes.c_void_p, ctypes.c_void_p,
)
CALLBACK_FUNCTION = 0x30000
MIM_DATA = 0x3C3


class MidiWatcher:
    """直接监听 Launchpad 的 MIDI 输入，把按键事件交给网页显示。

    用途：判断「按实体键没反应」是硬件/驱动没进来，还是声音板程序的问题。
    和 launch-soundpad.exe 可以同时监听同一个端口（Windows MMAPI 允许）。
    """

    def __init__(self):
        self.lock = threading.Lock()
        self.handle = ctypes.c_void_p()
        self.cb = None          # 必须持有回调引用，否则被 GC 会崩
        self.active = False
        self.device = None
        self.error = None
        self.events = deque(maxlen=300)
        self.seq = 0

    def _on_event(self, h, wmsg, inst, p1, p2):
        if not self.active or wmsg != MIM_DATA:
            return
        try:
            p1 = p1 or 0
            status = p1 & 0xFF
            data1 = (p1 >> 8) & 0xFF
            data2 = (p1 >> 16) & 0xFF
            cmd = status & 0xF0
            if cmd == 0x90 and data2 > 0:
                kind = "note_on"
            elif cmd == 0x80 or (cmd == 0x90 and data2 == 0):
                kind = "note_off"
            elif cmd == 0xB0:
                kind = "cc"
            else:
                return
            with self.lock:
                self.seq += 1
                self.events.append({
                    "seq": self.seq,
                    "t": time.strftime("%H:%M:%S"),
                    "kind": kind,
                    "note": data1,
                    "velocity": data2,
                    "raw": "0x%02X 0x%02X 0x%02X" % (status, data1, data2),
                })
        except Exception:
            pass

    def start(self, hint=None):
        with self.lock:
            if self.active:
                return {"ok": True, "already": True, "device": self.device}
        if os.name != "nt":
            return {"ok": False, "error": "仅支持 Windows"}
        if hint is None:
            hint = read_config().get("midi_in_device") or "Launchpad"
        try:
            winmm = ctypes.windll.winmm
            winmm.midiInGetDevCapsA.argtypes = [ctypes.c_uint, ctypes.c_void_p, ctypes.c_uint]
            winmm.midiInGetDevCapsA.restype = ctypes.c_uint
            winmm.midiInOpen.argtypes = [ctypes.POINTER(ctypes.c_void_p), ctypes.c_uint,
                                         MIDIINPROC, ctypes.c_void_p, ctypes.c_uint]
            winmm.midiInOpen.restype = ctypes.c_uint
            winmm.midiInStart.argtypes = [ctypes.c_void_p]
            winmm.midiInStart.restype = ctypes.c_uint
            found = []
            for i in range(winmm.midiInGetNumDevs()):
                caps = ctypes.create_string_buffer(64)
                if winmm.midiInGetDevCapsA(i, caps, ctypes.sizeof(caps)) == 0:
                    found.append((i, caps.raw[8:40].split(b"\x00")[0].decode("mbcs", "replace")))
            if not found:
                return {"ok": False, "error": "系统里没有 MIDI 输入端口"}
            h = (hint or "").lower()
            idx, dev = None, None
            for i, nm in found:
                if h and h in nm.lower():
                    idx, dev = i, nm
                    break
            if idx is None:
                return {"ok": False, "error": "找不到 MIDI 输入端口（%s），现有：%s"
                        % (hint, [n for _, n in found])}
            self.cb = MIDIINPROC(self._on_event)
            handle = ctypes.c_void_p()
            res = winmm.midiInOpen(ctypes.byref(handle), idx, self.cb, None, CALLBACK_FUNCTION)
            if res != 0:
                return {"ok": False, "error": "midiInOpen 失败，错误码 %d（端口可能被独占）" % res}
            self.handle = handle
            self.device = dev
            self.error = None
            with self.lock:
                self.active = True
                self.events.clear()
                self.seq = 0
            res = winmm.midiInStart(handle)
            if res != 0:
                self.stop()
                return {"ok": False, "error": "midiInStart 失败，错误码 %d" % res}
            return {"ok": True, "device": dev}
        except Exception as e:
            self.stop()
            return {"ok": False, "error": "监视启动失败: %s" % e}

    def stop(self):
        with self.lock:
            was = self.active
            self.active = False
            handle, self.handle = self.handle, ctypes.c_void_p()
        if was and handle:
            try:
                winmm = ctypes.windll.winmm
                winmm.midiInStop.argtypes = [ctypes.c_void_p]
                winmm.midiInClose.argtypes = [ctypes.c_void_p]
                winmm.midiInStop(handle)
                winmm.midiInClose(handle)
            except Exception:
                pass
        return {"ok": True}

    def poll(self, since):
        with self.lock:
            evs = [e for e in self.events if e["seq"] > since]
            return {"ok": True, "watching": self.active, "device": self.device,
                    "error": self.error, "events": evs,
                    "last_seq": self.seq}


watcher = MidiWatcher()


# ---------------------------------------------------------------- 声音板进程

class Runner:
    def __init__(self):
        self.proc = None
        self.lines = deque(maxlen=800)
        self.lock = threading.Lock()
        self.stopping = False

    def _log(self, line):
        with self.lock:
            self.lines.append(line)

    def running(self):
        return self.proc is not None and self.proc.poll() is None

    def start(self):
        if self.running():
            return {"ok": False, "error": "已在运行"}
        if not EXE_PATH.exists():
            return {"ok": False, "error": "找不到 %s，先跑 build.bat 编译" % EXE_PATH}
        env = dict(os.environ)
        cfg = read_config()
        env["RUST_LOG"] = "debug" if cfg.get("debug_mode", "").lower() in ("true", "1", "yes") else "info"
        try:
            self.proc = subprocess.Popen(
                [str(EXE_PATH)], cwd=str(PROJECT_ROOT), env=env,
                stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                stdin=subprocess.DEVNULL, text=True, encoding="utf-8",
                errors="replace", bufsize=1,
            )
        except Exception as e:
            return {"ok": False, "error": "启动失败: %s" % e}
        with self.lock:
            self.stopping = False
            self.lines.clear()
            self.lines.append("[GUI] 已启动 launch-soundpad.exe (pid %d)" % self.proc.pid)
        threading.Thread(target=self._pump, daemon=True).start()
        return {"ok": True, "pid": self.proc.pid}

    def _pump(self):
        proc = self.proc
        try:
            for line in proc.stdout:
                self._log(line.rstrip("\n"))
        except Exception:
            pass
        # 等进程真正结束再取退出码（此前直接 poll() 往往拿到 None）
        try:
            code = proc.wait(timeout=8)
        except Exception:
            code = proc.poll()
        if self.stopping:
            self._log("[GUI] 已停止声音板 (exit code=%s)" % code)
        else:
            tail = "" if code == 0 else "，请检查上方日志中的报错"
            self._log("[GUI] 声音板已退出 (exit code=%s)%s" % (code, tail))
        # 强杀/崩溃时 Rust 端来不及清灯，这里补一遍
        r = clear_launchpad_leds()
        self._log("[GUI] 熄灯: %s" % ("已关闭 %d 个 LED (%s)" % (r["count"], r["device"])
                                      if r.get("ok") else "跳过（%s）" % r.get("error")))

    def stop(self):
        if not self.running():
            return {"ok": False, "error": "未在运行"}
        self.stopping = True
        try:
            self.proc.terminate()
        except Exception as e:
            return {"ok": False, "error": "停止失败: %s" % e}
        try:
            self.proc.wait(timeout=3)
        except Exception:
            try:
                self.proc.kill()
            except Exception:
                pass
        return {"ok": True}

    def log(self):
        with self.lock:
            return list(self.lines)


runner = Runner()


# ---------------------------------------------------------------- HTTP 层

class SingleServer(ThreadingHTTPServer):
    """Windows 上 HTTPServer 默认 allow_reuse_address=1，多个进程能绑同一端口，
    导致请求随机落到不同实例（各自有独立的 Runner 状态），端口回退也失效。
    关掉它，端口被占就直接报错，交给 main() 顺延。"""
    allow_reuse_address = False


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):  # 静音默认访问日志
        pass

    # ---- 基础 ----
    def _send_json(self, obj, code=200):
        body = json.dumps(obj, ensure_ascii=False).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _send_html(self, text):
        body = text.encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _read_body(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n) if n else b""
        try:
            return json.loads(raw.decode("utf-8"))
        except Exception:
            return {}

    def _read_raw(self):
        n = int(self.headers.get("Content-Length") or 0)
        return self.rfile.read(n) if n else b""

    def do_GET(self):
        try:
            self._route_get()
        except BrokenPipeError:
            pass
        except Exception:
            traceback.print_exc()
            try:
                self._send_json({"ok": False, "error": "server error"}, 500)
            except Exception:
                pass

    def do_POST(self):
        try:
            self._route_post()
        except BrokenPipeError:
            pass
        except Exception:
            traceback.print_exc()
            try:
                self._send_json({"ok": False, "error": "server error"}, 500)
            except Exception:
                pass

    # ---- GET ----
    def _route_get(self):
        path = urlparse(self.path).path
        if path in ("/", "/index.html"):
            self._send_html(INDEX_HTML.read_text(encoding="utf-8"))
            return
        if path == "/api/bootstrap":
            devices = list_audio_devices()   # 这里会触发音频库加载（首次可能慢几秒）
            midi = list_midi_devices()
            self._send_json({
                "ok": True,
                "embedded": False,
                "audio_libs": bool(AUDIO_OK),
                # Python 版没有实现 LED 颜色测试（那是 exe 内嵌版才有的），页面据此隐藏那张卡片
                "led_test": False,
                "exe_exists": EXE_PATH.exists(),
                "config": read_config(),
                "devices": devices,
                "midi": midi,
                "pages": list_pages(),
                "packs": scan_packs(),
                "running": runner.running(),
                "server": {"pid": os.getpid(), "port": self.server.server_address[1],
                           "root": str(PROJECT_ROOT)},
            })
            return
        if path == "/api/devices":
            self._send_json({"ok": True, "devices": list_audio_devices(),
                             "midi": list_midi_devices(), "audio_libs": AUDIO_OK})
            return
        if path == "/api/pages":
            self._send_json({"ok": True, "pages": list_pages(), "packs": scan_packs()})
            return
        if path == "/api/sounds":
            self._send_json({"ok": True, "packs": scan_packs()})
            return
        if path == "/api/midi/events":
            since = int((parse_qs(urlparse(self.path).query).get("since") or ["0"])[0] or 0)
            self._send_json(watcher.poll(since))
            return
        if path == "/api/status":
            self._send_json({"ok": True, "running": runner.running(),
                             "log": runner.log(), "exe_exists": EXE_PATH.exists(),
                             "pid": os.getpid(),
                             "port": self.server.server_address[1],
                             "watching": watcher.active,
                             "watch_device": watcher.device,
                             "watch_seq": watcher.seq})
            return
        self._send_json({"ok": False, "error": "not found: %s" % path}, 404)

    # ---- POST ----
    def _route_post(self):
        parsed = urlparse(self.path)
        path = parsed.path
        qs = parse_qs(parsed.query)

        if path == "/api/config":
            body = self._read_body()
            updates = {k: str(v) for k, v in body.items() if k in EDITABLE_KEYS}
            cfg = write_config(updates)
            self._send_json({"ok": True, "config": cfg})
            return

        if path == "/api/pages/save":
            body = self._read_body()
            name = (body.get("name") or "").strip()
            if not name:
                self._send_json({"ok": False, "error": "缺少页面名"}, 400)
                return
            saved = save_page(name, body.get("pads") or {})
            self._send_json({"ok": True, "name": saved, "pages": list_pages()})
            return

        if path == "/api/pages/create":
            body = self._read_body()
            label = (body.get("label") or "新页面").strip()
            name = "%02d_%s" % (next_page_number(), re.sub(r"[\\/:*?\"<>|]", "_", label))
            save_page(name, {})
            self._send_json({"ok": True, "name": name, "pages": list_pages()})
            return

        if path == "/api/pages/fill":
            # 从语言包文件夹整页填充（顺序填满 64 个 pad）
            body = self._read_body()
            label = (body.get("label") or "").strip()
            pack = (body.get("pack") or "").strip()
            src = pack_dir_by_name(pack) or sounds_root()
            files = [f for f in sorted(src.iterdir(), key=lambda p: p.name.lower())
                     if f.is_file() and f.suffix.lower() in AUDIO_EXTS] if src.is_dir() else []
            if not files:
                self._send_json({"ok": False, "error": "语言包 %r 为空或不存在" % pack}, 400)
                return
            name = body.get("name") or ("%02d_%s" % (next_page_number(),
                                        re.sub(r"[\\/:*?\"<>|]", "_", label or pack)))
            pads = {}
            for i, f in enumerate(files[:64]):
                pads[grid_notes()[i]] = {"path": str(f), "color": DEFAULT_COLOR}
            save_page(name, pads)
            self._send_json({"ok": True, "name": name, "pages": list_pages(),
                             "filled": min(len(files), 64)})
            return

        if path == "/api/pages/rename":
            body = self._read_body()
            old = (body.get("old") or "").strip()
            new = (body.get("new") or "").strip()
            if not old or not new:
                self._send_json({"ok": False, "error": "缺少页面名"}, 400)
                return
            new = re.sub(r"[\\/:*?\"<>|]", "_", new)
            src, dst = PAGES_DIR / old, PAGES_DIR / new
            if not src.is_file():
                self._send_json({"ok": False, "error": "页面不存在"}, 400)
                return
            if dst.exists():
                self._send_json({"ok": False, "error": "目标页面已存在"}, 400)
                return
            src.rename(dst)
            self._send_json({"ok": True, "name": new, "pages": list_pages()})
            return

        if path == "/api/pages/delete":
            body = self._read_body()
            name = (body.get("name") or "").strip()
            target = PAGES_DIR / name
            if not target.is_file():
                self._send_json({"ok": False, "error": "页面不存在"}, 400)
                return
            target.unlink()
            self._send_json({"ok": True, "pages": list_pages()})
            return

        if path == "/api/upload":
            # ?pack=xx   body = 音频字节
            pack = (qs.get("pack") or ["自定义"])[0]
            fname = (qs.get("name") or [""])[0]
            pack = re.sub(r"[\\/:*?\"<>|]", "_", pack)
            fname = os.path.basename(fname)
            if not fname or Path(fname).suffix.lower() not in AUDIO_EXTS:
                self._send_json({"ok": False, "error": "只支持音频文件: %s" % ",".join(AUDIO_EXTS)}, 400)
                return
            dest_dir = pack_dir_by_name(pack) or sounds_root()
            dest_dir.mkdir(parents=True, exist_ok=True)
            data = self._read_raw()
            if not data:
                self._send_json({"ok": False, "error": "空文件"}, 400)
                return
            dest = dest_dir / fname
            if dest.exists():  # 重名加时间戳
                stem = dest.stem
                dest = dest_dir / ("%s_%s%s" % (stem, time.strftime("%H%M%S"), dest.suffix))
            dest.write_bytes(data)
            self._send_json({"ok": True, "path": str(dest), "packs": scan_packs()})
            return

        if path == "/api/preview":
            body = self._read_body()
            dev = body.get("device_index")
            if dev is None and body.get("device"):
                dev = device_index_by_name(body["device"])
            target = body.get("path") or ""
            if target:
                target = str(resolve_path(target))
            r = preview.play(target, dev)
            self._send_json(r)
            return

        if path == "/api/preview/stop":
            preview.stop()
            self._send_json({"ok": True})
            return

        if path == "/api/open":
            body = self._read_body()
            target = body.get("path") or str(sounds_root())
            p = Path(target)
            if not p.is_absolute():
                p = PROJECT_ROOT / p
            try:
                os.startfile(str(p))  # noqa
                self._send_json({"ok": True})
            except Exception as e:
                self._send_json({"ok": False, "error": str(e)})
            return

        if path == "/api/midi/watch":
            body = self._read_body()
            action = body.get("action")
            if action == "start":
                self._send_json(watcher.start(body.get("device")))
            elif action == "stop":
                watcher.stop()
                self._send_json({"ok": True})
            else:
                self._send_json({"ok": False, "error": "unknown action"}, 400)
            return

        if path == "/api/led/clear":
            # 手动熄灯（比如上次崩溃后灯常亮）
            r = clear_launchpad_leds()
            self._send_json(r)
            return

        if path == "/api/reload":
            # 独立 exe 里是「就地重载」；Python 控制台里等价于重启声音板
            was_running = runner.running()
            if was_running:
                runner.stop()
                time.sleep(0.5)
                r = runner.start()
            else:
                r = {"ok": True, "note": "声音板本来就没在运行"}
            self._send_json(r)
            return

        if path == "/api/quit":
            runner.stop()
            self._send_json({"ok": True})
            return

        if path == "/api/run":
            body = self._read_body()
            action = body.get("action")
            if action == "start":
                self._send_json(runner.start())
            elif action == "stop":
                self._send_json(runner.stop())
            else:
                self._send_json({"ok": False, "error": "unknown action"}, 400)
            return

        self._send_json({"ok": False, "error": "not found: %s" % path}, 404)


def main():
    if not INDEX_HTML.exists():
        print("[!] 缺少 index.html：%s" % INDEX_HTML)
        sys.exit(1)
    if not EXE_PATH.exists():
        print("[!] 未找到 %s，请先运行 build.bat" % EXE_PATH)
    server = None
    last_err = None
    port = PORT
    for candidate in range(PORT, PORT + 10):
        try:
            server = SingleServer(("127.0.0.1", candidate), Handler)
            port = candidate
            break
        except OSError as e:
            last_err = e
    if server is None:
        print("[!] 端口 %d-%d 都被占用：%s" % (PORT, PORT + 9, last_err))
        sys.exit(1)
    url = "http://127.0.0.1:%d/" % port
    print("=" * 56)
    print(" Launchpad 声音板控制台")
    print(" 地址   : %s" % url)
    print(" 项目   : %s" % PROJECT_ROOT)
    print(" 声音板 : %s" % ("就绪" if EXE_PATH.exists() else "未编译"))
    print(" 试听   : 按需加载（首次打开页面时）")
    print(" 停止   : 关掉本窗口，或 Ctrl+C")
    print("=" * 56)
    try:
        import webbrowser
        if not os.environ.get("LAUNCHPAD_NO_BROWSER"):
            threading.Timer(0.8, lambda: webbrowser.open(url)).start()
    except Exception:
        pass
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\n再见")
    finally:
        runner.stop()
        watcher.stop()
        server.server_close()


if __name__ == "__main__":
    main()
