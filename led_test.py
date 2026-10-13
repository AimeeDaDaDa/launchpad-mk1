#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
led_test.py — Launchpad Mini MK1 LED 颜色测试

MK1 的 LED 速度值公式（三色：红 + 绿）：
    velocity = 12 + red + 16 * green        red / green 取 0..3
    red = 0 关, 1 暗, 2 中, 3 亮
    green = 0 关, 1 暗, 2 中, 3 亮
所以一共 16 档颜色（红/绿各 4 档亮度），速度值范围 12 ~ 63。

用法：
    python led_test.py              # 先逐个演示命名颜色，再显示 4x4 全色板
    python led_test.py --fast       # 快一点
    python led_test.py --matrix     # 只显示 4x4 全色板
    python led_test.py --clear      # 只熄灯

MIDI 用 ctypes 直调 winmm.dll，无第三方依赖。
"""

import argparse
import ctypes
import sys
import time

try:
    sys.stdout.reconfigure(encoding="utf-8")
except Exception:
    pass

winmm = ctypes.windll.winmm

# ---------------------------------------------------------------- MIDI 输出


def midi_out_names():
    winmm.midiOutGetDevCapsA.argtypes = [ctypes.c_uint, ctypes.c_void_p, ctypes.c_uint]
    winmm.midiOutGetDevCapsA.restype = ctypes.c_uint
    out = []
    for i in range(winmm.midiOutGetNumDevs()):
        buf = ctypes.create_string_buffer(64)
        if winmm.midiOutGetDevCapsA(i, buf, ctypes.sizeof(buf)) == 0:
            out.append((i, buf.raw[8:40].split(b"\x00")[0].decode("mbcs", "replace")))
    return out


class MidiOut:
    def __init__(self, index):
        winmm.midiOutOpen.argtypes = [ctypes.c_void_p, ctypes.c_uint,
                                      ctypes.c_void_p, ctypes.c_void_p, ctypes.c_uint]
        winmm.midiOutShortMsg.argtypes = [ctypes.c_void_p, ctypes.c_uint]
        self.handle = ctypes.c_void_p()
        res = winmm.midiOutOpen(ctypes.byref(self.handle), index, None, 0, 0)
        if res != 0:
            raise RuntimeError("midiOutOpen 失败，错误码 %d" % res)

    def note(self, note, velocity):
        if not (0 <= note <= 127 and 0 <= velocity <= 127):
            return
        winmm.midiOutShortMsg(self.handle, 0x90 | (note << 8) | (velocity << 16))

    def close(self):
        winmm.midiOutClose(self.handle)


def pick_launchpad(prefer="launchpad"):
    names = midi_out_names()
    if not names:
        return None, None
    for i, n in names:
        if prefer and prefer in n.lower():
            return i, n
    return names[0]


# ---------------------------------------------------------------- 颜色表

def vel(red, green):
    """MK1: velocity = 12 + red + 16*green"""
    return 12 + red + 16 * green


# README_MK1 里命名的几个常用色
NAMED = [
    ("灭 LED_OFF", 12, 0, 0),
    ("红·暗 red_low", 13, 1, 0),
    ("红·中 red_mid", 14, 2, 0),
    ("红·亮 red_full", 15, 3, 0),
    ("绿·暗 green_low", 28, 0, 1),
    ("绿·中 green_mid", 44, 0, 2),
    ("绿·亮 green_full", 60, 0, 3),
    ("黄·亮 yellow_full", 62, 2, 3),
    ("琥珀·暗 amber_low", 29, 1, 1),
    ("琥珀·亮 amber_full", 63, 3, 3),
]

# 4x4 全色板：行 = 绿的亮度，列 = 红的亮度
GRID_TOP_LEFT = [0, 1, 2, 3, 16, 17, 18, 19, 32, 33, 34, 35, 48, 49, 50, 51]


def all_off(mo, notes=None):
    for n in (notes or list(range(0, 120))):
        mo.note(n, 12)


def phase_named(mo, hold):
    print("\n=== 第 1 步：命名颜色逐个亮（看第 1 行 8 个 pad）===")
    row = [0, 1, 2, 3, 4, 5, 6, 7]
    for label, v, r, g in NAMED:
        for n in row:
            mo.note(n, v)
        print("  %-22s velocity=%-3d (red=%d green=%d)" % (label, v, r, g))
        time.sleep(hold)
    all_off(mo, row)
    print("  （已熄灭）")


def phase_matrix(mo, hold):
    print("\n=== 第 2 步：4x4 全色板（左上角 4 行 x 4 列）===")
    print("      红:  关    暗    中    亮")
    for g in range(4):
        gname = ["关", "暗", "中", "亮"][g]
        cells = []
        for r in range(4):
            note = 16 * g + r
            v = vel(r, g)
            mo.note(note, v)
            cells.append("(%2d)" % v)
        print("  绿 %s  %s" % (gname, "  ".join(cells)))
    print("\n  每格括号里是 velocity 值；pad 位置：第 1~4 行、第 1~4 列")
    print("  对应关系：note = 16*行 + 列，行号决定绿的亮度、列号决定红的亮度")
    time.sleep(hold)
    all_off(mo)
    print("  （已熄灭）")


def main():
    ap = argparse.ArgumentParser(description="Launchpad MK1 LED 颜色测试")
    ap.add_argument("--fast", action="store_true", help="每色停 1 秒")
    ap.add_argument("--matrix", action="store_true", help="只跑 4x4 全色板")
    ap.add_argument("--clear", action="store_true", help="只熄灯")
    args = ap.parse_args()

    idx, name = pick_launchpad()
    if idx is None:
        print("[!] 没找到 MIDI 输出端口")
        return
    print("MIDI 输出 -> [%d] %s" % (idx, name))
    mo = MidiOut(idx)
    try:
        if args.clear:
            all_off(mo)
            print("已熄灭全部 LED")
            return
        hold = 1.0 if args.fast else 2.2
        if not args.matrix:
            phase_named(mo, hold)
        phase_matrix(mo, hold + 6)
    except KeyboardInterrupt:
        pass
    finally:
        all_off(mo)
        mo.close()
        print("\n完成（已全部熄灭）。")


if __name__ == "__main__":
    main()
