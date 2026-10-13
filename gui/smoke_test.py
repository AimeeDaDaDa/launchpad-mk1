#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""smoke_test.py — gui_server API 冒烟测试（本地 127.0.0.1）

测试会临时改动 config.yaml（回写同一批值），结束时自动还原；
建议先确保只有**一个**控制台实例在跑（否则请求会落到别的实例上）。
"""
import atexit
import json
import os
import sys
import urllib.request

BASE = "http://127.0.0.1:8787"
# 不走代理
os.environ["no_proxy"] = "*"
os.environ["NO_PROXY"] = "*"
proxy_handler = urllib.request.ProxyHandler({})
opener = urllib.request.build_opener(proxy_handler)

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))  # gui/ 的上级 = 项目根
CFG_PATH = os.path.join(ROOT, "config.yaml")

# 配置备份：无论测试怎么退出（含异常），都还原
_cfg_backup = open(CFG_PATH, "rb").read() if os.path.exists(CFG_PATH) else None


@atexit.register
def _restore_config():
    if _cfg_backup is not None:
        with open(CFG_PATH, "wb") as fh:
            fh.write(_cfg_backup)


def file_config():
    """直接按文件解析，用于和服务端返回的配置比对"""
    cfg = {}
    for line in open(CFG_PATH, encoding="utf-8"):
        line = line.strip()
        if not line or line.startswith("#") or ":" not in line:
            continue
        k, v = line.split(":", 1)
        cfg[k.strip()] = v.strip().strip('"').strip("'")
    return cfg


def call(path, payload=None, method=None):
    data = None
    headers = {}
    if payload is not None:
        data = json.dumps(payload).encode("utf-8")
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(BASE + path, data=data, headers=headers,
                                 method=method or ("POST" if data else "GET"))
    with opener.open(req, timeout=15) as r:
        return json.loads(r.read().decode("utf-8"))


fails = []


def check(name, cond, extra=""):
    print(("PASS" if cond else "FAIL"), name, extra)
    if not cond:
        fails.append(name)


# 1. bootstrap
j = call("/api/bootstrap")
check("bootstrap", j.get("ok"))
check("midi_in 识别 Launchpad", "Launchpad Mini" in j["midi"]["in"], str(j["midi"]["in"]))
check("设备列表非空", len(j["devices"]) > 0, "%d devices" % len(j["devices"]))
check("页面加载", len(j["pages"]) >= 3, str([p["name"] for p in j["pages"]]))
check("服务实例信息", bool(j.get("server", {}).get("pid")), str(j.get("server")))

# 服务读到的配置必须和磁盘文件一致（防止请求落到别的实例/别的路径上）
fc = file_config()
same = all((j["config"].get(k) or "") == (fc.get(k) or "")
           for k in ("midi_in_device", "midi_out_device", "virtual_device", "output_device"))
check("服务配置 == 磁盘文件", same,
      "server: %s | file: %s" % (j["config"].get("virtual_device"), fc.get("virtual_device")))

# 2. config 回写（写当前值，不改变语义）
j2 = call("/api/config", {
    "output_device": j["config"].get("output_device", ""),
    "virtual_device": j["config"].get("virtual_device", ""),
    "midi_in_device": j["config"].get("midi_in_device", ""),
    "midi_out_device": j["config"].get("midi_out_device", ""),
    "hold_to": j["config"].get("hold_to", "Normal"),
    "local_monitor": j["config"].get("local_monitor", "true"),
    "virtual_volume": j["config"].get("virtual_volume", "0.10"),
})
check("config 保存", j2.get("ok"))
cfg = j2["config"]
check("bookmark 保留", cfg.get("bookmark_1") == "pages", str(cfg.get("bookmark_1")))
check("debug_mode 保留", cfg.get("debug_mode") == "true")
check("非空值没被写空", all((cfg.get(k) or "") == (fc.get(k) or "")
                          for k in ("midi_in_device", "midi_out_device", "virtual_device")),
      "after: %s" % {k: cfg.get(k) for k in ("midi_in_device", "midi_out_device", "virtual_device")})

# 3. 建测试页 -> 保存 -> 删除
j3 = call("/api/pages/create", {"label": "冒烟测试"})
check("创建页面", j3.get("ok"), j3.get("name", ""))
test_page = j3.get("name")
pads = {"0": {"path": os.path.join(ROOT, "sounds", "中文", "clip_0.wav"), "color": 60},
        "17": {"path": os.path.join(ROOT, "sounds", "中文", "clip_1.wav"), "color": 63}}
j4 = call("/api/pages/save", {"name": test_page, "pads": pads})
check("保存页面", j4.get("ok"))
page_file = os.path.join(ROOT, "pages", test_page)
content = open(page_file, encoding="utf-8").read()
check("页面文件格式", "0;" in content and ";60" in content and "17;" in content,
      repr(content[:80]))
j5 = call("/api/pages/delete", {"name": test_page})
check("删除页面", j5.get("ok") and not os.path.exists(page_file))

# 4. 从文件夹生成整页
j6 = call("/api/pages/fill", {"pack": "中文", "label": "冒烟生成"})
check("文件夹生成整页", j6.get("ok") and j6.get("filled") == 3, str(j6.get("filled")))
if j6.get("ok"):
    call("/api/pages/delete", {"name": j6["name"]})

# 5. 试听
wav = os.path.join(ROOT, "sounds", "中文", "clip_0.wav")
j7 = call("/api/preview", {"path": wav, "device_index": None})
check("试听播放", j7.get("ok"), j7.get("seconds", j.get("error", "")))
call("/api/preview/stop", method="POST")

# 清理可能残留的测试页
for p in call("/api/pages")["pages"]:
    if "冒烟" in p["name"]:
        call("/api/pages/delete", {"name": p["name"]})
        print("清理残留页面:", p["name"])

# 6. 启动/停止声音板
j8 = call("/api/run", {"action": "start"})
check("启动声音板", j8.get("ok"), j8.get("error", ""))
if j8.get("ok"):
    import time
    time.sleep(2.5)
    st = call("/api/status")
    check("运行状态", st.get("running"))
    log = "\n".join(st.get("log", []))
    check("日志有内容", "launch_soundpad" in log or "MiniMk1" in log,
          log.strip().splitlines()[-1][:100] if log.strip() else "(empty)")
    j9 = call("/api/run", {"action": "stop"})
    check("停止声音板", j9.get("ok"))
    time.sleep(2)
    st2 = call("/api/status")
    log2 = st2.get("log", [])
    tail2 = [l for l in log2 if l.startswith("[GUI]")]
    check("退出码不再是 None", any("exit code=" in l and "exit code=None" not in l for l in tail2),
          str(tail2[-2:]))
    check("停止后自动熄灯", any(l.startswith("[GUI] 熄灯:") for l in tail2),
          str([l for l in tail2 if l.startswith("[GUI] 熄灯:")]))

# 7. 手动熄灯
j10 = call("/api/led/clear", method="POST")
check("手动熄灯接口", j10.get("ok"), str(j10.get("device") or j10.get("error")))

# 8. HTML 可访问
with opener.open(BASE + "/", timeout=10) as r:
    html = r.read().decode("utf-8")
check("首页 HTML", "Launchpad 声音板控制台" in html and "</html>" in html)

print()
if fails:
    print("失败 %d 项: %s" % (len(fails), fails))
    sys.exit(1)
print("全部通过 ✓")
