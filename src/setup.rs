//! setup.rs — 首次运行自动准备
//!
//! 目标：把 exe 拷到任何地方（或给别人），双击就能用，不用先手写配置。
//!   - config.yaml 不存在  -> 按自动探测到的设备生成一份
//!   - sounds/ 不存在      -> 建好语言包目录结构 + 说明文件
//!   - pages/ 为空         -> 用 sounds/ 里的音频生成页面；没有音频就先放一个空演示页
//!
//! 返回的提示会显示在设置页的「活动日志」里，用户能看见做了什么。

use std::path::Path;

use log::{info, warn};

/// 自动准备，返回写给用户看的说明（会进设置页活动日志）
pub fn ensure_all() -> Vec<String> {
    let mut notes = Vec::new();
    notes.extend(ensure_sounds());
    let created_config = ensure_config();
    notes.extend(created_config.clone());
    notes.extend(ensure_pages(created_config.iter().any(|_| true)));
    notes
}

/// 建立语音包目录（只在完全没有时做）。
/// 默认就给一个：`sounds/语音包/` —— 音频直接丢进去就行，对应 config 的 `sounds_dir`。
fn ensure_sounds() -> Vec<String> {
    let mut notes = Vec::new();
    if Path::new("sounds").is_dir() {
        return notes;
    }
    let dir = Path::new("sounds").join("语音包");
    let hint = "把音频文件（wav / mp3 / ogg / flac / m4a）直接放进这个文件夹。\n\
                这个文件夹就是 config.yaml 里 sounds_dir 指的「语音包路径」。\n\
                想放多个语音包（例如按语言分），可以在 sounds/ 下再建子文件夹，\
                并把 sounds_dir 那行删掉或留空——那时每个子文件夹各是一个语音包。\n";
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(dir.join("把音频放这里.txt"), hint);
        notes.push("首次运行：已创建语音包目录 sounds/语音包/".to_string());
    }
    notes
}

/// 自动探测设备名：MIDI 端口 + 适合当虚拟设备的输出设备
fn detect_devices() -> (String, String, String, bool) {
    use cpal::traits::{DeviceTrait, HostTrait};

    // MIDI：优先名字里带 launchpad 的
    let mut midi_names: Vec<String> = Vec::new();
    if let Ok(mi) = midir::MidiInput::new("launch-soundpad-setup") {
        for p in mi.ports() {
            if let Ok(n) = mi.port_name(&p) {
                midi_names.push(n);
            }
        }
    }
    let midi = midi_names
        .iter()
        .find(|n| n.to_lowercase().contains("launchpad"))
        .or_else(|| midi_names.first())
        .cloned()
        .unwrap_or_default();

    // 音频输出：优先虚拟混音设备（VoiceMeeter / VB-Cable 之类）
    let host = cpal::default_host();
    let mut outputs: Vec<String> = Vec::new();
    let mut default_output = String::new();
    if let Some(d) = host.default_output_device() {
        default_output = d.name().unwrap_or_default();
    }
    if let Ok(devices) = host.devices() {
        for d in devices {
            if let Ok(name) = d.name() {
                if d.supported_output_configs().map(|mut c| c.next().is_some()).unwrap_or(false)
                    && !outputs.contains(&name)
                {
                    outputs.push(name);
                }
            }
        }
    }
    let lower = |s: &str| s.to_lowercase();
    let virtual_dev = outputs
        .iter()
        .find(|n| lower(n).contains("voicemeeter in 3"))
        .or_else(|| outputs.iter().find(|n| lower(n).contains("voicemeeter")))
        .or_else(|| outputs.iter().find(|n| lower(n).contains("cable input")))
        .or_else(|| outputs.iter().find(|n| lower(n).contains("virtual")))
        .cloned();
    let found_virtual = virtual_dev.is_some();
    let virtual_dev = virtual_dev.unwrap_or_else(|| default_output.clone());

    (midi.clone(), midi, virtual_dev, found_virtual)
}

/// 生成默认 config.yaml（只在文件不存在时）
fn ensure_config() -> Vec<String> {
    let mut notes = Vec::new();
    if Path::new("config.yaml").exists() {
        return notes;
    }
    let (midi_in, midi_out, virtual_dev, found_virtual) = detect_devices();

    // 没找到虚拟混音设备时，就别开本地监听，只往默认设备播一遍（否则会重一遍）
    let local_monitor = found_virtual;
    let text = format!(
        "# 首次运行自动生成。设备名可在设置页里改（改完点「重新加载配置」）。\n\
         midi_in_device: {midi_in}\n\
         midi_out_device: {midi_out}\n\
         output_device:\n\
         virtual_device: {virtual_dev}\n\
         bookmark_1: pages\n\
         bookmark_2:\n\
         bookmark_3:\n\
         bookmark_4:\n\
         bookmark_5:\n\
         bookmark_6:\n\
         bookmark_7:\n\
         debug_mode: true\n\
         hold_to: Normal\n\
         local_monitor: {local_monitor}\n\
         virtual_volume: {vol}\n\
         sounds_dir: sounds/语音包\n",
        midi_in = if midi_in.is_empty() { "Launchpad Mini" } else { &midi_in },
        midi_out = if midi_out.is_empty() { "Launchpad Mini" } else { &midi_out },
        virtual_dev = virtual_dev,
        local_monitor = local_monitor,
        vol = if found_virtual { "1.0" } else { "1.0" },
    );
    match std::fs::write("config.yaml", text) {
        Ok(_) => {
            info!("created default config.yaml");
            notes.push(format!(
                "首次运行：已生成 config.yaml（MIDI: {} / 虚拟设备: {}）",
                if midi_in.is_empty() { "未探测到" } else { &midi_in },
                if virtual_dev.is_empty() { "未探测到" } else { &virtual_dev }
            ));
            if !found_virtual {
                notes.push("提示：没找到 VoiceMeeter 之类的虚拟设备，先按默认音箱输出、关掉了本地监听".into());
            }
            if midi_in.is_empty() {
                notes.push("警告：没探测到 Launchpad 的 MIDI 端口，插好设备后到设置页选一下".into());
            }
        }
        Err(e) => {
            warn!("write config.yaml failed: {}", e);
            notes.push(format!("生成 config.yaml 失败：{}", e));
        }
    }
    notes
}

/// 建 pages/（为空时）：能用 sounds/ 生成就用，否则放一个空演示页
fn ensure_pages(_config_was_created: bool) -> Vec<String> {
    let mut notes = Vec::new();
    let existing: Vec<_> = std::fs::read_dir("pages")
        .map(|rd| rd.filter_map(|e| e.ok()).filter(|e| e.path().is_file()).collect())
        .unwrap_or_default();
    if !existing.is_empty() {
        return notes;
    }
    let _ = std::fs::create_dir_all("pages");

    // 有哪些语音包（config 设了 sounds_dir 就只有一个；否则是 sounds/ 下的子文件夹）
    let mut packs: Vec<(String, Vec<String>)> = Vec::new();
    for (name, dir) in crate::webui::pack_dirs() {
        let mut files: Vec<String> = Vec::new();
        if let Ok(frd) = std::fs::read_dir(&dir) {
            for f in frd.filter_map(|e| e.ok()).map(|e| e.path()) {
                let ext = f.extension().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
                if f.is_file() && ["wav", "mp3", "ogg", "flac", "aiff", "aif", "m4a"].contains(&ext.as_str()) {
                    // 存相对路径（用 / 分隔），整个文件夹搬走也能用
                    files.push(crate::webui::portable_path(&f.to_string_lossy())
                        .to_string_lossy()
                        .replace('\\', "/"));
                }
            }
        }
        files.sort();
        if !files.is_empty() {
            packs.push((name, files));
        }
    }
    packs.sort_by(|a, b| a.0.cmp(&b.0));

    if packs.is_empty() {
        if std::fs::write("pages/00_demo", "").is_ok() {
            notes.push("首次运行：已创建空的 pages/00_demo（往 sounds/ 放音频后在设置页里绑定按键即可）".into());
        }
        return notes;
    }

    let grid: Vec<u8> = (0..8u8).flat_map(|r| (0..8u8).map(move |c| 16 * r + c)).collect();
    for (i, (pack, files)) in packs.iter().enumerate() {
        let mut lines: Vec<String> = Vec::new();
        for (j, path) in files.iter().take(64).enumerate() {
            lines.push(format!("{};{};60", grid[j], path));
        }
        let name = format!("{:02}_{}", i + 1, pack);
        if std::fs::write(Path::new("pages").join(&name), format!("{}\n", lines.join("\n"))).is_ok() {
            notes.push(format!("首次运行：已用 sounds/{} 生成页面 {}（{} 个键）", pack, name, files.len().min(64)));
        }
    }
    notes
}
