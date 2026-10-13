//! webui.rs — 内嵌设置页 + 本地 HTTP API（独立 exe 的一部分）
//!
//! 主程序启动时在 127.0.0.1 起一个迷你 HTTP 服务：
//!   - 设置页 gui/index.html 用 include_str! 直接编进二进制，不依赖外部文件
//!   - /api/* 提供设备枚举、配置读写、页面读写、试听、熄灯、按键事件、重载、退出
//!
//! 重载/退出是通过往主循环的 note 通道里塞哨兵 note 实现的（见 RELOAD_NOTE / QUIT_NOTE）。

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;

use log::{error, info, warn};
use serde_json::{json, Value};
use tiny_http::{Header, Method, Request, Response, Server};

use crate::audio;
use crate::{NoteEvent, QUIT_NOTE, RELOAD_NOTE};

/// 设置页：编译期嵌进二进制（改完页面要重新编译）
const INDEX_HTML: &str = include_str!("../gui/index.html");
const AUDIO_EXTS: [&str; 7] = ["wav", "mp3", "ogg", "flac", "aiff", "aif", "m4a"];
/// 界面能改的 config 键（其余键原样保留）
const EDITABLE_KEYS: [&str; 7] = [
    "midi_in_device", "midi_out_device", "output_device", "virtual_device",
    "hold_to", "local_monitor", "virtual_volume",
];
const DEFAULT_PORT: u16 = 8787;

/// MK1 全部 LED（网格 64 + 右列 7 + 顶排 8），velocity 12 = 灭
fn mk1_led_notes() -> Vec<u8> {
    let mut v: Vec<u8> = (0..8).flat_map(|r| (0..8).map(move |c| 16 * r + c)).collect();
    v.extend_from_slice(&[8, 24, 40, 56, 72, 88, 120]);
    v.extend(104..112);
    v
}

/// 与主循环共享的状态
pub struct Shared {
    tx: Sender<NoteEvent>,
    notes: Mutex<Notes>,
    activity: Mutex<VecDeque<String>>,
    preview: Mutex<Option<(rodio::OutputStream, rodio::Sink)>>,
    port: Mutex<u16>,
}

#[derive(Default)]
struct Notes {
    items: VecDeque<Value>,
    seq: u64,
}

impl Shared {
    /// 设置页端口（托盘菜单打开浏览器时用）
    pub fn port(&self) -> u16 {
        *self.port.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn push_activity(&self, line: impl Into<String>) {
        if let Ok(mut act) = self.activity.lock() {
            act.push_back(line.into());
            while act.len() > 300 {
                act.pop_front();
            }
        }
    }

    fn push_note(&self, note: u8, on: bool) {
        if let Ok(mut n) = self.notes.lock() {
            n.seq += 1;
            let seq = n.seq;
            n.items.push_back(json!({
                "seq": seq,
                "kind": if on { "note_on" } else { "note_off" },
                "note": note,
                "velocity": if on { 127 } else { 0 },
            }));
            while n.items.len() > 300 {
                n.items.pop_front();
            }
        }
    }

    fn activity_lines(&self) -> Vec<String> {
        self.activity.lock().map(|a| a.iter().cloned().collect()).unwrap_or_default()
    }
}

/// 由主循环调用：把收到的按键事件喂给设置页的「按键测试」
pub fn push_note(shared: &Arc<Shared>, note: u8, on: bool) {
    shared.push_note(note, on);
}

/// 由主循环调用：记一条活动日志
pub fn push_activity(shared: &Arc<Shared>, line: impl Into<String>) {
    shared.push_activity(line);
}

/// 启动内嵌设置页，返回共享状态（失败也只是没有界面，声音板照常工作）
pub fn start(tx: Sender<NoteEvent>, open_browser: bool) -> Arc<Shared> {
    let shared = Arc::new(Shared {
        tx,
        notes: Mutex::new(Notes::default()),
        activity: Mutex::new(VecDeque::new()),
        preview: Mutex::new(None),
        port: Mutex::new(DEFAULT_PORT),
    });

    let server = {
        let mut bound: Option<(u16, Server)> = None;
        for candidate in DEFAULT_PORT..DEFAULT_PORT + 10 {
            match Server::http(("127.0.0.1", candidate)) {
                Ok(s) => {
                    bound = Some((candidate, s));
                    break;
                }
                Err(e) => {
                    if candidate == DEFAULT_PORT {
                        warn!("web UI: port {} busy ({}), trying next", candidate, e);
                    }
                }
            }
        }
        bound
    };

    let Some((port, server)) = server else {
        error!("web UI: could not bind any port in {}..{}", DEFAULT_PORT, DEFAULT_PORT + 9);
        return shared;
    };
    if let Ok(mut p) = shared.port.lock() {
        *p = port;
    }
    let url = format!("http://127.0.0.1:{}/", port);
    shared.push_activity(format!("设置页已就绪: {}", url));
    info!("web UI listening on {}", url);

    // 打开浏览器（可用 --no-browser 关掉）
    if open_browser {
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", "", &url])
                .spawn();
        }
    }

    let loop_shared = Arc::clone(&shared);
    thread::spawn(move || {
        for request in server.incoming_requests() {
            let s = Arc::clone(&loop_shared);
            // 每个请求独立线程：某个请求出错不会拖垮整个设置页
            thread::spawn(move || {
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    handle(request, &s)
                }));
                if res.is_err() {
                    error!("web UI: handler panicked");
                }
            });
        }
    });

    shared
}

// ------------------------------------------------------------------ 小工具

fn json_response(v: Value) -> Response<std::io::Cursor<Vec<u8>>> {
    let body = v.to_string();
    Response::from_string(body)
        .with_header(Header::from_bytes(&b"Content-Type"[..], &b"application/json; charset=utf-8"[..]).unwrap())
        .with_header(Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).unwrap())
}

fn html_response(text: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(text.to_string())
        .with_header(Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap())
        .with_header(Header::from_bytes(&b"Cache-Control"[..], &b"no-store"[..]).unwrap())
}

fn not_found(what: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    json_response(json!({"ok": false, "error": format!("unknown endpoint: {}", what)}))
        .with_status_code(404)
}

fn read_body(req: &mut Request) -> String {
    let mut s = String::new();
    let _ = req.as_reader().read_to_string(&mut s);
    s
}

fn read_body_bytes(req: &mut Request) -> Vec<u8> {
    let mut v = Vec::new();
    let _ = req.as_reader().read_to_end(&mut v);
    v
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

/// "/api/x?a=1" -> ("/api/x", {a: 1})
fn split_url(url: &str) -> (String, HashMap<String, String>) {
    let mut it = url.splitn(2, '?');
    let path = it.next().unwrap_or("").to_string();
    let mut q = HashMap::new();
    if let Some(query) = it.next() {
        for pair in query.split('&') {
            if pair.is_empty() {
                continue;
            }
            let mut kv = pair.splitn(2, '=');
            let k = percent_decode(kv.next().unwrap_or(""));
            let v = percent_decode(kv.next().unwrap_or(""));
            q.insert(k, v);
        }
    }
    (path, q)
}

fn audio_ext_ok(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c })
        .collect()
}

// ------------------------------------------------------------------ config.yaml

/// 保序读取：[(key, value)]
fn read_config_pairs() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Ok(text) = std::fs::read_to_string("config.yaml") else {
        return out;
    };
    for line in text.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_string();
            if key.is_empty() || key.starts_with('#') || key.contains(' ') {
                continue;
            }
            let val = v.trim().trim_matches('"').trim_matches('\'').to_string();
            out.push((key, val));
        }
    }
    out
}

fn config_json() -> Value {
    let mut map = serde_json::Map::new();
    for (k, v) in read_config_pairs() {
        map.insert(k, Value::String(v));
    }
    Value::Object(map)
}

fn config_get(key: &str) -> Option<String> {
    read_config_pairs().into_iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn yaml_quote(v: &str) -> String {
    if v.is_empty() {
        return String::new();
    }
    let needs = v.contains(':')
        || v.contains('#')
        || v.trim() != v
        || v.starts_with('-')
        || v.starts_with('{')
        || v.starts_with('[');
    if needs {
        format!("\"{}\"", v.replace('"', "'"))
    } else {
        v.to_string()
    }
}

/// 按行改写：只动给定键，保留注释与其他键顺序；新键追加到末尾
fn write_config(updates: &serde_json::Map<String, Value>) -> Result<Value, String> {
    let text = std::fs::read_to_string("config.yaml").unwrap_or_default();
    let mut remaining: HashMap<String, String> = HashMap::new();
    for (k, v) in updates {
        if EDITABLE_KEYS.contains(&k.as_str()) {
            remaining.insert(k.clone(), value_to_string(v));
        }
    }
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let mut handled = false;
        if let Some((k, _)) = line.split_once(':') {
            let key = k.trim();
            if let Some(val) = remaining.remove(key) {
                out.push(if val.is_empty() { format!("{}:", key) } else { format!("{}: {}", key, yaml_quote(&val)) });
                handled = true;
            }
        }
        if !handled {
            out.push(line.to_string());
        }
    }
    for key in EDITABLE_KEYS {
        if let Some(val) = remaining.remove(key) {
            out.push(if val.is_empty() { format!("{}:", key) } else { format!("{}: {}", key, yaml_quote(&val)) });
        }
    }
    let body = format!("{}\n", out.join("\n").trim_end_matches(['\n', '\r']));
    std::fs::write("config.yaml", body).map_err(|e| e.to_string())?;
    Ok(config_json())
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

// ------------------------------------------------------------------ pages

fn color_of(v: &Value) -> u8 {
    match v {
        Value::Number(n) => n.as_u64().unwrap_or(60).min(255) as u8,
        Value::String(s) => s.trim().parse::<u8>().unwrap_or(60),
        _ => 60,
    }
}

fn parse_page(path: &Path) -> Vec<(u8, String, u8)> {
    let mut out = Vec::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return out;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split(';').collect();
        if parts.len() < 3 {
            continue;
        }
        let Ok(note) = parts[0].trim().parse::<u8>() else { continue };
        let color = parts[2].trim().parse::<u8>().unwrap_or(60);
        out.push((note, parts[1].to_string(), color));
    }
    out
}

fn pages_dir_entries() -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir("pages")
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_file()).collect())
        .unwrap_or_default();
    paths.sort();
    paths
}

fn pages_json() -> Value {
    let mut arr = Vec::new();
    for path in pages_dir_entries() {
        let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let notes = parse_page(&path);
        let mut pads = serde_json::Map::new();
        for (note, p, color) in &notes {
            pads.insert(note.to_string(), json!({"path": p, "color": color}));
        }
        // 去掉 "NN_" 前缀当显示名
        let label = match name.find('_') {
            Some(i) if name[..i].chars().all(|c| c.is_ascii_digit()) => name[i + 1..].to_string(),
            _ => name.clone(),
        };
        arr.push(json!({"name": name, "label": label, "pads": Value::Object(pads), "count": notes.len()}));
    }
    Value::Array(arr)
}

fn next_page_number() -> u32 {
    let mut best = 0u32;
    for path in pages_dir_entries() {
        let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        if let Some(i) = name.find('_') {
            if let Ok(n) = name[..i].parse::<u32>() {
                best = best.max(n);
            }
        }
    }
    best + 1
}

fn save_page(name: &str, pads: &Value) -> Result<String, String> {
    let safe = sanitize(name);
    let mut lines: Vec<String> = Vec::new();
    if let Value::Object(map) = pads {
        let mut keys: Vec<(u8, &Value)> = map
            .iter()
            .filter_map(|(k, v)| k.trim().parse::<u8>().ok().map(|n| (n, v)))
            .collect();
        keys.sort_by_key(|(n, _)| *n);
        for (note, spec) in keys {
            let p = spec.get("path").map(value_to_string).unwrap_or_default();
            if p.trim().is_empty() {
                continue;
            }
            let color = spec.get("color").map(color_of).unwrap_or(60);
            lines.push(format!("{};{};{}", note, p, color));
        }
    }
    std::fs::create_dir_all("pages").map_err(|e| e.to_string())?;
    std::fs::write(Path::new("pages").join(&safe), format!("{}\n", lines.join("\n")))
        .map_err(|e| e.to_string())?;
    Ok(safe)
}

// ------------------------------------------------------------------ sounds

fn packs_json() -> Value {
    let mut arr = Vec::new();
    let Ok(rd) = std::fs::read_dir("sounds") else {
        return Value::Array(arr);
    };
    let mut dirs: Vec<PathBuf> = rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    for dir in dirs {
        let mut files: Vec<(String, Value)> = Vec::new();
        if let Ok(frd) = std::fs::read_dir(&dir) {
            for f in frd.filter_map(|e| e.ok()).map(|e| e.path()) {
                if !f.is_file() {
                    continue;
                }
                let fname = f.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                if !audio_ext_ok(&fname) {
                    continue;
                }
                let size = std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0);
                files.push((
                    fname.to_lowercase(),
                    json!({"name": fname, "path": f.to_string_lossy(), "size": size}),
                ));
            }
        }
        files.sort_by(|a, b| a.0.cmp(&b.0));
        if files.is_empty() {
            continue;
        }
        arr.push(json!({
            "name": dir.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
            "files": files.into_iter().map(|(_, v)| v).collect::<Vec<_>>(),
        }));
    }
    Value::Array(arr)
}

// ------------------------------------------------------------------ 设备

fn audio_devices_json() -> Value {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let default_name = host.default_output_device().and_then(|d| d.name().ok());
    let mut arr = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    if let Ok(devices) = host.devices() {
        for d in devices {
            let Ok(name) = d.name() else { continue };
            // 只保留能当输出的设备
            if !d.supported_output_configs().map(|mut c| c.next().is_some()).unwrap_or(false) {
                continue;
            }
            if !seen.insert(name.clone()) {
                continue;
            }
            let is_default = default_name.as_deref() == Some(name.as_str());
            arr.push(json!({"index": arr.len(), "name": name, "api": "cpal", "is_default": is_default}));
        }
    }
    Value::Array(arr)
}

fn midi_devices_json() -> Value {
    let inputs = midir::MidiInput::new("launch-soundpad-webui")
        .map(|m| m.ports().iter().filter_map(|p| m.port_name(p).ok()).collect::<Vec<_>>())
        .unwrap_or_default();
    let outputs = midir::MidiOutput::new("launch-soundpad-webui")
        .map(|m| m.ports().iter().filter_map(|p| m.port_name(p).ok()).collect::<Vec<_>>())
        .unwrap_or_default();
    json!({"in": inputs, "out": outputs})
}

/// 给 Launchpad 发一遍「灭灯」（托盘菜单直接调用）
pub fn clear_leds_now() {
    let hint = config_get("midi_out_device").unwrap_or_default();
    match clear_leds(&hint) {
        Ok((dev, n)) => info!("cleared {} LEDs on {}", n, dev),
        Err(e) => warn!("clear leds failed: {}", e),
    }
}

/// 给 Launchpad 发一遍「灭灯」（进程被强杀后灯会常亮）
fn clear_leds(hint: &str) -> Result<(String, usize), String> {
    let midi_out = midir::MidiOutput::new("launch-soundpad-webui").map_err(|e| e.to_string())?;
    let notes = mk1_led_notes();
    for port in midi_out.ports() {
        let name = midi_out.port_name(&port).unwrap_or_default();
        let hit = if hint.is_empty() {
            name.to_lowercase().contains("launchpad")
        } else {
            name == hint || name.contains(hint)
        };
        if hit {
            let mut conn = midi_out.connect(&port, "led-clear").map_err(|e| e.to_string())?;
            for n in &notes {
                let _ = conn.send(&[0x90, *n, 12]);
            }
            thread::sleep(std::time::Duration::from_millis(30));
            return Ok((name, notes.len()));
        }
    }
    Err(format!("找不到 MIDI 输出端口（{}）", if hint.is_empty() { "launchpad" } else { hint }))
}

// ------------------------------------------------------------------ 试听

fn preview_play(shared: &Arc<Shared>, path: &str, device: &str) -> Value {
    // 先停掉上一条
    if let Ok(mut p) = shared.preview.lock() {
        *p = None;
    }
    if path.trim().is_empty() {
        return json!({"ok": false, "error": "没有指定音频文件"});
    }
    if !Path::new(path).is_file() {
        return json!({"ok": false, "error": format!("文件不存在: {}", path)});
    }
    let Some(stream) = audio::select_output_device(device) else {
        return json!({"ok": false, "error": format!("打不开输出设备: {}", if device.is_empty() { "(系统默认)" } else { device })});
    };
    let played = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| audio::play_sound(&stream, path, 0.9)));
    match played {
        Ok((sink, duration)) => {
            let secs = duration.map(|d| d.as_secs_f32()).unwrap_or(0.0);
            if let Ok(mut p) = shared.preview.lock() {
                *p = Some((stream, sink));
            }
            shared.push_activity(format!("试听 {}", Path::new(path).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()));
            json!({"ok": true, "seconds": (secs * 100.0).round() / 100.0})
        }
        Err(_) => json!({"ok": false, "error": "无法解码这个音频文件"}),
    }
}

fn preview_stop(shared: &Arc<Shared>) {
    if let Ok(mut p) = shared.preview.lock() {
        *p = None;
    }
}

// ------------------------------------------------------------------ 路由

fn handle(mut request: Request, shared: &Arc<Shared>) {
    let (path, query) = split_url(request.url());
    let method = request.method().clone();

    if method == Method::Get {
        match path.as_str() {
            "/" | "/index.html" => {
                let _ = request.respond(html_response(INDEX_HTML));
                return;
            }
            "/api/bootstrap" => {
                let port = *shared.port.lock().unwrap_or_else(|e| e.into_inner());
                let cwd = std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
                let _ = request.respond(json_response(json!({
                    "ok": true,
                    "embedded": true,
                    "audio_libs": true,
                    "exe_exists": true,
                    "config": config_json(),
                    "devices": audio_devices_json(),
                    "midi": midi_devices_json(),
                    "pages": pages_json(),
                    "packs": packs_json(),
                    "running": true,
                    "server": {"pid": std::process::id(), "port": port, "root": cwd, "embedded": true},
                })));
                return;
            }
            "/api/devices" => {
                let _ = request.respond(json_response(json!({
                    "ok": true, "devices": audio_devices_json(),
                    "midi": midi_devices_json(), "audio_libs": true,
                })));
                return;
            }
            "/api/pages" => {
                let _ = request.respond(json_response(json!({"ok": true, "pages": pages_json(), "packs": packs_json()})));
                return;
            }
            "/api/sounds" => {
                let _ = request.respond(json_response(json!({"ok": true, "packs": packs_json()})));
                return;
            }
            "/api/status" => {
                let port = *shared.port.lock().unwrap_or_else(|e| e.into_inner());
                let (seq, watching) = {
                    let n = shared.notes.lock().unwrap_or_else(|e| e.into_inner());
                    (n.seq, true)
                };
                let _ = request.respond(json_response(json!({
                    "ok": true,
                    "running": true,
                    "embedded": true,
                    "log": shared.activity_lines(),
                    "exe_exists": true,
                    "pid": std::process::id(),
                    "port": port,
                    "watching": watching,
                    "watch_device": config_get("midi_in_device").unwrap_or_default(),
                    "watch_seq": seq,
                })));
                return;
            }
            "/api/midi/events" => {
                let since: u64 = query.get("since").and_then(|s| s.parse().ok()).unwrap_or(0);
                let (events, seq) = {
                    let n = shared.notes.lock().unwrap_or_else(|e| e.into_inner());
                    (
                        n.items.iter().filter(|e| e.get("seq").and_then(|s| s.as_u64()).unwrap_or(0) > since).cloned().collect::<Vec<_>>(),
                        n.seq,
                    )
                };
                let _ = request.respond(json_response(json!({
                    "ok": true, "watching": true, "events": events, "last_seq": seq,
                    "device": config_get("midi_in_device").unwrap_or_default(),
                })));
                return;
            }
            other => {
                let _ = request.respond(not_found(other));
                return;
            }
        }
    }

    if method == Method::Post {
        // 支持两种传参：JSON body 或表单风格（都兼容前端）
        let body_text = if path == "/api/upload" { String::new() } else { read_body(&mut request) };
        let body: Value = serde_json::from_str(&body_text).unwrap_or(Value::Null);
        let body = if body.is_null() {
            // ?k=v 形式
            let mut m = serde_json::Map::new();
            for (k, v) in &query {
                m.insert(k.clone(), Value::String(v.clone()));
            }
            Value::Object(m)
        } else {
            body
        };

        match path.as_str() {
            "/api/config" => {
                let updates = match body.as_object() {
                    Some(o) => o.clone(),
                    None => serde_json::Map::new(),
                };
                match write_config(&updates) {
                    Ok(cfg) => {
                        let keys: Vec<String> = updates.keys().cloned().collect();
                        shared.push_activity(format!("配置已保存: {}", keys.join(", ")));
                        let _ = request.respond(json_response(json!({"ok": true, "config": cfg})));
                    }
                    Err(e) => {
                        let _ = request.respond(json_response(json!({"ok": false, "error": e})));
                    }
                }
                return;
            }
            "/api/pages/save" => {
                let name = body.get("name").map(value_to_string).unwrap_or_default();
                if name.trim().is_empty() {
                    let _ = request.respond(json_response(json!({"ok": false, "error": "缺少页面名"})));
                    return;
                }
                let pads = body.get("pads").cloned().unwrap_or(json!({}));
                match save_page(&name, &pads) {
                    Ok(saved) => {
                        shared.push_activity(format!("页面已保存: {}", saved));
                        let _ = request.respond(json_response(json!({"ok": true, "name": saved, "pages": pages_json()})));
                    }
                    Err(e) => {
                        let _ = request.respond(json_response(json!({"ok": false, "error": e})));
                    }
                }
                return;
            }
            "/api/pages/create" => {
                let label = body.get("label").map(value_to_string).unwrap_or_else(|| "新页面".into());
                let name = format!("{:02}_{}", next_page_number(), sanitize(&label));
                match save_page(&name, &json!({})) {
                    Ok(saved) => {
                        shared.push_activity(format!("新建页面: {}", saved));
                        let _ = request.respond(json_response(json!({"ok": true, "name": saved, "pages": pages_json()})));
                    }
                    Err(e) => {
                        let _ = request.respond(json_response(json!({"ok": false, "error": e})));
                    }
                }
                return;
            }
            "/api/pages/fill" => {
                let pack = body.get("pack").map(value_to_string).unwrap_or_default();
                let label = body.get("label").map(value_to_string).unwrap_or_else(|| pack.clone());
                let dir = Path::new("sounds").join(sanitize(&pack));
                let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
                    .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path())
                        .filter(|p| p.is_file() && audio_ext_ok(&p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()))
                        .collect())
                    .unwrap_or_default();
                files.sort();
                if files.is_empty() {
                    let _ = request.respond(json_response(json!({"ok": false, "error": format!("语言包 {} 为空或不存在", pack)})));
                    return;
                }
                let name = format!("{:02}_{}", next_page_number(), sanitize(&label));
                let grid: Vec<u8> = (0..8u8).flat_map(|r| (0..8u8).map(move |c| 16 * r + c)).collect();
                let mut pads = serde_json::Map::new();
                for (i, f) in files.iter().take(64).enumerate() {
                    pads.insert(grid[i].to_string(), json!({"path": f.to_string_lossy(), "color": 60}));
                }
                match save_page(&name, &Value::Object(pads)) {
                    Ok(saved) => {
                        shared.push_activity(format!("按文件夹生成页面: {} ({} 个键)", saved, files.len().min(64)));
                        let _ = request.respond(json_response(json!({"ok": true, "name": saved, "filled": files.len().min(64), "pages": pages_json()})));
                    }
                    Err(e) => {
                        let _ = request.respond(json_response(json!({"ok": false, "error": e})));
                    }
                }
                return;
            }
            "/api/pages/rename" => {
                let old = body.get("old").map(value_to_string).unwrap_or_default();
                let new = sanitize(&body.get("new").map(value_to_string).unwrap_or_default());
                if old.trim().is_empty() || new.trim().is_empty() {
                    let _ = request.respond(json_response(json!({"ok": false, "error": "缺少页面名"})));
                    return;
                }
                let src = Path::new("pages").join(&old);
                let dst = Path::new("pages").join(&new);
                if !src.is_file() {
                    let _ = request.respond(json_response(json!({"ok": false, "error": "页面不存在"})));
                    return;
                }
                if dst.exists() {
                    let _ = request.respond(json_response(json!({"ok": false, "error": "目标页面已存在"})));
                    return;
                }
                match std::fs::rename(&src, &dst) {
                    Ok(_) => {
                        shared.push_activity(format!("页面改名: {} -> {}", old, new));
                        let _ = request.respond(json_response(json!({"ok": true, "name": new, "pages": pages_json()})));
                    }
                    Err(e) => {
                        let _ = request.respond(json_response(json!({"ok": false, "error": e.to_string()})));
                    }
                }
                return;
            }
            "/api/pages/delete" => {
                let name = body.get("name").map(value_to_string).unwrap_or_default();
                let target = Path::new("pages").join(&name);
                if !target.is_file() {
                    let _ = request.respond(json_response(json!({"ok": false, "error": "页面不存在"})));
                    return;
                }
                match std::fs::remove_file(&target) {
                    Ok(_) => {
                        shared.push_activity(format!("删除页面: {}", name));
                        let _ = request.respond(json_response(json!({"ok": true, "pages": pages_json()})));
                    }
                    Err(e) => {
                        let _ = request.respond(json_response(json!({"ok": false, "error": e.to_string()})));
                    }
                }
                return;
            }
            "/api/upload" => {
                let pack = sanitize(&query.get("pack").cloned().unwrap_or_else(|| "自定义".into()));
                let fname = Path::new(query.get("name").cloned().unwrap_or_default().as_str())
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                if fname.is_empty() || !audio_ext_ok(&fname) {
                    let _ = request.respond(json_response(json!({"ok": false, "error": "只支持音频文件"})));
                    return;
                }
                let data = read_body_bytes(&mut request);
                if data.is_empty() {
                    let _ = request.respond(json_response(json!({"ok": false, "error": "空文件"})));
                    return;
                }
                let dir = Path::new("sounds").join(&pack);
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    let _ = request.respond(json_response(json!({"ok": false, "error": e.to_string()})));
                    return;
                }
                let dest = dir.join(&fname);
                if let Err(e) = std::fs::write(&dest, &data) {
                    let _ = request.respond(json_response(json!({"ok": false, "error": e.to_string()})));
                    return;
                }
                shared.push_activity(format!("上传音频: {}/{}", pack, fname));
                let _ = request.respond(json_response(json!({"ok": true, "path": dest.to_string_lossy(), "packs": packs_json()})));
                return;
            }
            "/api/preview" => {
                let p = body.get("path").map(value_to_string).unwrap_or_default();
                // 新前端传设备名（device），旧前端传 device_index
                let dev = body.get("device").map(value_to_string).unwrap_or_default();
                let _ = request.respond(json_response(preview_play(shared, &p, &dev)));
                return;
            }
            "/api/preview/stop" => {
                preview_stop(shared);
                let _ = request.respond(json_response(json!({"ok": true})));
                return;
            }
            "/api/led/clear" => {
                let hint = config_get("midi_out_device").unwrap_or_default();
                let _ = request.respond(match clear_leds(&hint) {
                    Ok((dev, n)) => {
                        shared.push_activity(format!("熄灯: {} 个 LED ({})", n, dev));
                        json_response(json!({"ok": true, "device": dev, "count": n}))
                    }
                    Err(e) => json_response(json!({"ok": false, "error": e})),
                });
                return;
            }
            "/api/midi/watch" => {
                let action = body.get("action").map(value_to_string).unwrap_or_default();
                if action == "start" {
                    if let Ok(mut n) = shared.notes.lock() {
                        n.items.clear();
                    }
                    let dev = config_get("midi_in_device").unwrap_or_default();
                    let _ = request.respond(json_response(json!({"ok": true, "device": dev})));
                } else {
                    let _ = request.respond(json_response(json!({"ok": true})));
                }
                return;
            }
            "/api/reload" => {
                if shared.tx.send((RELOAD_NOTE, true)).is_ok() {
                    shared.push_activity("请求重新加载 config.yaml 与页面");
                    let _ = request.respond(json_response(json!({"ok": true})));
                } else {
                    let _ = request.respond(json_response(json!({"ok": false, "error": "主循环已退出"})));
                }
                return;
            }
            "/api/quit" => {
                shared.push_activity("请求退出程序");
                let _ = shared.tx.send((QUIT_NOTE, true));
                let _ = request.respond(json_response(json!({"ok": true})));
                return;
            }
            "/api/open" => {
                let target = body.get("path").map(value_to_string).unwrap_or_else(|| "sounds".into());
                let p = PathBuf::from(&target);
                let p = if p.is_absolute() { p } else { std::env::current_dir().map(|d| d.join(&p)).unwrap_or(p) };
                #[cfg(target_os = "windows")]
                let spawned = std::process::Command::new("explorer").arg(&p).spawn();
                #[cfg(not(target_os = "windows"))]
                let spawned: Result<std::process::Child, std::io::Error> = Err(std::io::Error::other("unsupported"));
                let _ = request.respond(match spawned {
                    Ok(_) => json_response(json!({"ok": true})),
                    Err(e) => json_response(json!({"ok": false, "error": e.to_string()})),
                });
                return;
            }
            other => {
                let _ = request.respond(not_found(other));
                return;
            }
        }
    }

    let _ = request.respond(
        json_response(json!({"ok": false, "error": "method not allowed"}))
            .with_status_code(405),
    );
}
