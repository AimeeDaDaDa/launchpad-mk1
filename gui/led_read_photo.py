#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""led_read_photo.py — 从 Launchpad 照片里采样每个点亮键的实际 RGB

用法：python led_read_photo.py <图片路径>
思路：亮点比未点亮的灰键更"有颜色"（饱和度 + 亮度），先把它们找出来，
按 x/y 投影聚成 4 列 4 行，再在每个键中心取小窗口的平均颜色。
"""
import sys

import numpy as np
from PIL import Image

path = sys.argv[1]
im = Image.open(path).convert("RGB")
# 缩放到便于分析的尺寸（保持比例）
scale = 900.0 / max(im.size)
if scale < 1:
    im = im.resize((int(im.size[0] * scale), int(im.size[1] * scale)))
a = np.asarray(im).astype(np.float32)
H, W, _ = a.shape
print("分析尺寸: %dx%d" % (W, H))

mx = a.max(axis=2)
mn = a.min(axis=2)
colorful = mx - mn
mask = (mx > 80) & (colorful > 22)
# 只看画面中左上的 pad 区（去掉顶部圆键、右侧和底部）
mask[:, int(W * 0.62):] = False
mask[: int(H * 0.06), :] = False
if mask.sum() < 50:
    print("没找到亮点（阈值太严？）")
    sys.exit(1)


def clusters(idx, gap):
    vals = np.unique(idx)
    out = []
    start = prev = vals[0]
    for v in vals[1:]:
        if v - prev > gap:
            out.append((int(start), int(prev)))
            start = v
        prev = v
    out.append((int(start), int(prev)))
    return [c for c in out if c[1] - c[0] > 4]


ys, xs = np.nonzero(mask)
xc = clusters(xs, gap=max(4, int(W * 0.01)))
yc = clusters(ys, gap=max(4, int(H * 0.01)))
print("检测到列数 %d, 行数 %d" % (len(xc), len(yc)))
if len(xc) != 4 or len(yc) != 4:
    print("警告：不是 4x4，聚类结果 x=%s y=%s" % (xc, yc))

# 每个格子中心取窗口平均
names = {}
for r, (y0, y1) in enumerate(yc[:4]):
    for g, (x0, x1) in enumerate(xc[:4]):
        cy = (y0 + y1) // 2
        cx = (x0 + x1) // 2
        win = a[max(0, cy - 6):cy + 7, max(0, cx - 6):cx + 7]
        col = win.reshape(-1, 3).mean(axis=0)
        R, G, B = [int(round(v)) for v in col]
        vel = 16 * g + r + 12
        names[(r, g)] = (vel, R, G, B)

print()
print("表：行=红档 r，列=绿档 g，velocity = 16g + r + 12")
print("%-8s %-8s %-8s %-8s" % ("", "第1列", "第2列", "第3列", "第4列") if False else "")
hdr = "%-10s" % "位置"
for g in range(4):
    hdr += "%-22s" % ("第%d列(g=%d)" % (g + 1, g))
print(hdr)
for r in range(4):
    row = "第%d行(r=%d) " % (r + 1, r)
    for g in range(4):
        vel, R, G, B = names[(r, g)]
        row += "%-22s" % ("v=%-3d rgb(%3d,%3d,%3d)" % (vel, R, G, B))
    print(row)

print()
print("各 velocity 详情（按绿/红档）:")
for r in range(4):
    for g in range(4):
        vel, R, G, B = names[(r, g)]
        mxv = max(R, G, B)
        tag = "灭" if mxv < 40 else ""
        if not tag:
            if R > G * 1.6:
                tag = "偏红"
            elif G > R * 1.6:
                tag = "偏绿"
            elif B > 120 and abs(R - G) < 40:
                tag = "偏白"
            elif R > 200 and G > 150:
                tag = "黄/橙"
            else:
                tag = "混合"
        print("  绿%d 红%d  v=%-3d  rgb(%3d,%3d,%3d)  %s" % (g, r, vel, R, G, B, tag))
