#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""led_map.py — 把照片上半部分打成字符颜色地图，用来目测按键几何

字符： . 暗/灰  R 红  r 暗红  G 绿  g 暗绿  O 橙  Y 黄  W 白/奶白
"""
import sys

import numpy as np
from PIL import Image

im = Image.open(sys.argv[1]).convert("RGB")
a = np.asarray(im).astype(np.float32)
H, W, _ = a.shape
step = int(sys.argv[2]) if len(sys.argv) > 2 else 15
print("原图 %dx%d, 采样步长 %d" % (W, H, step))


def classify(R, G, B):
    mx = max(R, G, B)
    if mx < 70:
        return "."
    if mx - min(R, G, B) < 25:
        return "W" if mx > 140 else "."
    if R > G and R > B:
        if G > R * 0.75 and mx > 150:
            return "Y"      # 红多绿也不少 -> 黄
        if G > R * 0.45:
            return "O"      # 橙
        return "R" if mx > 130 else "r"
    if G >= R and G > B:
        return "G" if mx > 130 else "g"
    return "."


print("    " + "".join([str((x // step) % 10) for x in range(0, min(W, 560), step)]))
for y in range(0, min(H, 760), step):
    row = ""
    for x in range(0, min(W, 560), step):
        win = a[max(0, y - 3):y + 4, max(0, x - 3):x + 4].reshape(-1, 3).mean(axis=0)
        row += classify(*[float(v) for v in win])
    print("%3d %s" % (y, row))
