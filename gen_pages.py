#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
gen_pages.py — 把「语言包」文件夹生成 Mcfloy launchpad 的 pages 页面

每个语言包文件夹 = 一个 page（用顶排 CC 106/107 翻页切换语言）。
MK1 网格 note = 16*行 + 列（左上角 note 0）。

用法：
  python gen_pages.py                 # 扫描 ./sounds，生成到 ./pages
  python gen_pages.py <音频目录>      # 指定其他目录
"""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
PAGES = os.path.join(HERE, "pages")
AUDIO_EXTS = (".wav", ".mp3", ".ogg", ".flac", ".aiff", ".aif", ".m4a")
DEMO_PAGE = "00_demo"

# MK1 三色 LED：velocity = 16*green + red + 12
COLOR_GREEN_FULL = 60


def grid_notes():
    """MK1 的 64 个网格 note，行优先"""
    return [16 * row + col for row in range(8) for col in range(8)]


def main():
    src = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, "sounds")
    src = os.path.abspath(src)
    if not os.path.isdir(src):
        print("音频目录不存在: %s" % src)
        print("先建几个语言包文件夹，例如：")
        print("  %s/中文/xxx.wav" % src)
        print("  %s/日本語/xxx.wav" % src)
        return

    os.makedirs(PAGES, exist_ok=True)
    # 清掉旧页面，保留演示页
    for f in os.listdir(PAGES):
        if f != DEMO_PAGE:
            os.remove(os.path.join(PAGES, f))

    packs = []
    for name in sorted(os.listdir(src)):
        p = os.path.join(src, name)
        if not os.path.isdir(p):
            continue
        files = [
            os.path.join(p, f)
            for f in sorted(os.listdir(p))
            if f.lower().endswith(AUDIO_EXTS)
        ]
        if files:
            packs.append((name, files))

    if not packs:
        print("在 %s 下没找到语言包（需要子文件夹 + 音频文件）" % src)
        print("演示页 pages/%s 仍可使用" % DEMO_PAGE)
        return

    notes = grid_notes()
    print("生成 %d 个页面（MK1 网格 note 0-119）:" % len(packs))
    for i, (name, files) in enumerate(packs):
        lines = []
        for j, path in enumerate(files[:64]):
            lines.append("%d;%s;%d" % (notes[j], os.path.abspath(path), COLOR_GREEN_FULL))
        out = os.path.join(PAGES, "%02d_%s" % (i + 1, name))
        with open(out, "w", encoding="utf-8") as fh:
            fh.write("\n".join(lines) + "\n")
        print("  %-24s %d 个音频 -> %s" % (name, min(len(files), 64), os.path.basename(out)))

    print("\n完成。顶排 CC 104=首页 105=末页 106=上一页 107=下一页 108=退出 109=停止全部")


if __name__ == "__main__":
    main()
