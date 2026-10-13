// 托盘程序：release 版不带控制台窗口（想看输出就加 --console）
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

extern crate core;

use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, mpsc};
use std::sync::mpsc::{Receiver, Sender};
use std::thread;

use config_file::FromConfigFile;
use log::{debug, error, info};
use rodio::{OutputStream, Sink};

use crate::config::{Config, HoldMode};
use crate::launchpad::Launchpad;
use crate::referential::{Note, Referential};

mod config;
mod referential;
mod audio;
mod midi;
mod launchpad;
mod webui;
#[cfg(windows)]
mod tray;

const WHITE_COLOR: u8 = 3;
const RED_COLOR: u8 = 5;
const YELLOW_COLOR: u8 = 13;
const GREEN_COLOR: u8 = 87;
const VOICE_VOLUME: f32 = 1.0;
// 虚拟设备音量改由 config 的 virtual_volume 控制（缺省 0.10，见 config.rs）

// 内嵌设置页往主循环里发的哨兵 note（MK1 不会发这两个数）
pub const RELOAD_NOTE: u8 = 126;
pub const QUIT_NOTE: u8 = 127;

// Midi Output Event for the launchpad is a tuple of 3 elements:
// - The note id
// - The color
// - A boolean to know if the light should blink or not
type MidiOutputEvent = (u8, u8, bool);
type NoteEvent = (u8, bool);

/// 托盘程序默认没有控制台；带 --console 时挂到父控制台（在 cmd 里调试用）
#[cfg(windows)]
fn setup_console() {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::Console::{
        AllocConsole, AttachConsole, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
        STD_OUTPUT_HANDLE,
    };
    if !std::env::args().any(|a| a == "--console") {
        return;
    }
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            AllocConsole();
        }
        if let Ok(f) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
            let h = f.as_raw_handle() as isize;
            SetStdHandle(STD_OUTPUT_HANDLE, h as _);
            SetStdHandle(STD_ERROR_HANDLE, h as _);
            std::mem::forget(f); // 句柄要一直开着
        }
    }
}

#[cfg(not(windows))]
fn setup_console() {}

/// 已经在运行了吗？（命名互斥体；句柄故意不关，进程活着就等于占着）
#[cfg(windows)]
fn already_running() -> bool {
  use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
  use windows_sys::Win32::System::Threading::CreateMutexW;
  let name: Vec<u16> = "launch-soundpad-single-instance\0".encode_utf16().collect();
  unsafe {
    let handle = CreateMutexW(std::ptr::null(), 1, name.as_ptr());
    if !handle.is_null() && GetLastError() == ERROR_ALREADY_EXISTS {
      return true;
    }
  }
  false
}

#[cfg(not(windows))]
fn already_running() -> bool {
  false
}

/// 第二个实例：把已有实例的设置页打开，然后自己退出
fn focus_existing() {
  let _ = std::process::Command::new("cmd")
    .args(["/C", "start", "", "http://127.0.0.1:8787/"])
    .spawn();
}

/// 数据目录 = 含 config.yaml 的那个目录。
/// 找的顺序：当前目录 -> exe 同目录 -> 从 exe 目录往上几层（target/release -> 项目根）。
/// 这样双击 target/release 下的 exe 也能用，把 exe 拷到别处（连着 config.yaml/pages/sounds）也能用。
fn find_data_root() -> Option<std::path::PathBuf> {
  let has_cfg = |d: &std::path::Path| d.join("config.yaml").is_file();
  if let Ok(cwd) = std::env::current_dir() {
    if has_cfg(&cwd) {
      return Some(cwd);
    }
  }
  let exe = std::env::current_exe().ok()?;
  let mut dir = exe.parent()?.to_path_buf();
  for _ in 0..3 {
    if has_cfg(&dir) {
      return Some(dir);
    }
    dir = match dir.parent() {
      Some(p) => p.to_path_buf(),
      None => break,
    };
  }
  None
}

/// 出错提示：托盘程序没有控制台，光打印用户看不见，弹个对话框
fn fatal(msg: &str) {
  error!("{}", msg);
  #[cfg(windows)]
  unsafe {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    let text: Vec<u16> = msg.encode_utf16().chain(std::iter::once(0)).collect();
    let title: Vec<u16> = "Launchpad 声音板".encode_utf16().chain(std::iter::once(0)).collect();
    MessageBoxW(0 as _, text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR);
  }
}

fn main() -> Result<(), Box<dyn Error>> {

  setup_console();
  env_logger::init();

  // 单实例：已经在托盘里跑着的话，直接打开设置页就退出
  if already_running() {
    info!("another instance is already running, opening the settings page");
    focus_existing();
    return Ok(());
  }

  // 先定位数据目录（config.yaml / pages / sounds 都在那儿）
  match find_data_root() {
    Some(root) => {
      let _ = std::env::set_current_dir(&root);
      info!("working directory: {}", root.display());
    }
    None => {
      fatal("找不到 config.yaml。\n\n请把 launch-soundpad.exe 放在有 config.yaml、pages、sounds 的目录下运行\n（或者把 exe 拷到你放 config.yaml 的文件夹里）。");
      return Ok(());
    }
  }

  let mut config = match Config::from_config_file("config.yaml") {
    Ok(c) => c,
    Err(e) => {
      fatal(&format!("config.yaml 解析失败：{}\n\n检查一下 YAML 格式。", e));
      return Ok(());
    }
  };

  let mut midi_in = midir::MidiInput::new("midir reading input")?;
  midi_in.ignore(midir::Ignore::All);

  debug!("Available MIDI input ports:");
  for p in midi::get_midi_input_devices() {
    debug!("  - {}", p);
  }

  let mut conn_out;

  let Some(midi_out_name) = config.get_midi_out_device() else {
    fatal("config.yaml 里 midi_out_device 是空的。\n\n填入 Launchpad 的 MIDI 输出名（一般就是 \"Launchpad Mini\"）再启动。");
    return Ok(());
  };
  match midi::select_midi_output_device(midi_out_name) {
    Some(conn) => {
      conn_out = conn;
    }
    _ => {
      error!("No midi output port found, have you plugged your midi device ?");
      return Ok(());
    }
  }

  let Some(midi_in_device_name) = config.get_midi_in_device() else {
    fatal("config.yaml 里 midi_in_device 是空的。\n\n填入 Launchpad 的 MIDI 输入名（一般就是 \"Launchpad Mini\"）再启动。");
    return Ok(());
  };
  let launchpad = Arc::new(Launchpad::get_launchpad(midi_in_device_name));
  info!("Launchpad model detected: {:?}", launchpad);

  debug!("Available output ports:");
  for p in midi::get_midi_output_devices() {
    debug!("  - {}", p);
  }

  let mut output_handle: Option<OutputStream>;

  // 本地监听：config 里 local_monitor 为 false 时，完全不建本地输出流，
  // 声音只送虚拟设备（VoiceMeeter），避免本地和语音链路各听一遍。
  if config.local_monitor_enabled() {
    // An empty output_device falls back to the system default device.
    match audio::select_output_device(config.get_output_device().unwrap_or("")) {
      Some(stream) => {
        output_handle = Some(stream);
      }
      None => {
        error!("No audio device found");
        return Ok(());
      }
    }
  } else {
    info!("Local monitor disabled (local_monitor = false): sounds only go to the virtual device");
    output_handle = None;
  }

  debug!("Available output devices:");
  for device in audio::get_output_devices() {
    debug!("  - {}", device);
  }

  let mut virtual_handle;
  let Some(virtual_name) = config.get_virtual_device() else {
    fatal("config.yaml 里 virtual_device 是空的。\n\n填入要送声音的虚拟设备（例如 \"Voicemeeter In 3\"）再启动。");
    return Ok(());
  };
  match audio::select_output_device(&virtual_name) {
    Some(handle) => {
      virtual_handle = handle;
    }
    None => {
      error!("No virtual device found");
      return Ok(());
    }
  }

  let mut referential = Referential::new(&launchpad);
  referential.init(String::from("pages"));

  if referential.get_nb_pages() == 0 {
    error!("No pages found");
    return Ok(());
  }

  let (tx_note, rx_note): (Sender<NoteEvent>, Receiver<NoteEvent>) = mpsc::channel();
  let (tx_midi, rx_midi): (Sender<MidiOutputEvent>, Receiver<MidiOutputEvent>) = mpsc::channel();

  // 内嵌设置页：直接用浏览器打开 http://127.0.0.1:8787/，不需要 Python
  // 托盘程序默认不弹浏览器（从托盘图标/菜单打开）；想启动就弹加 --open
  let open_browser = std::env::args().any(|a| a == "--open");
  let ui = webui::start(tx_note.clone(), open_browser);
  #[cfg(windows)]
  tray::start(tx_note.clone(), ui.port(), Arc::clone(&ui));

  midi::refresh_grid(&launchpad, &config, &mut referential, &tx_midi, true);

  // Activate programmer mode
  conn_out.send(launchpad.programmer_mode_command()).unwrap();

  // Thread to manage the midi LEDs
  let led_launchpad = Arc::clone(&launchpad);
  thread::spawn(move || {
    for (note_id, color, should_blink) in rx_midi {
      let message = led_launchpad.led_message(note_id, color, should_blink);
      conn_out.send(&message).unwrap();
    }
  });

  let _conn_in = midi::listen_midi_input(midi_in_device_name, tx_note);
  // (本地监听 Sink 可选, 虚拟设备 Sink)
  let mut sinks: HashMap<u8, (Option<Sink>, Sink)> = HashMap::new();

  // Main loop to receive the midi events, also block the main thread from exiting.
  for (note_id, is_on) in rx_note {
    // 内嵌设置页发来的哨兵（note 126/127）
    if is_on && note_id == QUIT_NOTE {
      info!("Quit requested from the web UI");
      webui::push_activity(&ui, "收到退出指令，正在关闭");
      midi::actions::end_session(&launchpad, &tx_midi);
      break;
    }
    if is_on && note_id == RELOAD_NOTE {
      info!("Reloading config.yaml and pages (requested from the web UI)");
      match Config::from_config_file("config.yaml") {
        Ok(fresh) => config = fresh,
        Err(e) => error!("reload config failed: {}", e),
      }
      // 音频设备可能变了，重新开流
      output_handle = if config.local_monitor_enabled() {
        match audio::select_output_device(config.get_output_device().unwrap_or("")) {
          Some(stream) => Some(stream),
          None => {
            error!("No audio device found after reload");
            None
          }
        }
      } else {
        None
      };
      if let Some(stream) = audio::select_output_device(&config.get_virtual_device().unwrap_or_default()) {
        virtual_handle = stream;
      } else {
        error!("No virtual device found after reload");
      }
      midi::actions::stop_note(&mut sinks);
      let bookmark_path = config.get_bookmark(0).unwrap_or_else(|| String::from("pages"));
      referential.init(bookmark_path);
      midi::refresh_grid(&launchpad, &config, &mut referential, &tx_midi, true);
      webui::push_activity(&ui, "已重新加载 config.yaml 与页面");
      continue;
    }

    webui::push_note(&ui, note_id, is_on);

    if !is_on {
      match config.get_hold_to_mode() {
        HoldMode::Normal => {
          continue;
        }
        HoldMode::Stop => {
          if let Some((audio_sink, virtual_sink)) = sinks.remove(&note_id) {
            if let Some(sink) = audio_sink {
              sink.stop();
            }
            virtual_sink.stop();
          }
        }
        HoldMode::Pause => {
          if let Some((audio_sink, virtual_sink)) = sinks.get(&note_id) {
            if let Some(sink) = audio_sink {
              sink.pause();
            }
            virtual_sink.pause();
          }
        }
      }
      if let Some(note) = referential.get_note(note_id) {
        let thread_tx_midi = tx_midi.clone();
        thread_tx_midi.send((note.note_id, note.color, false)).unwrap();
      }
    }
    if is_on {
      if note_id == launchpad.end_session_note() {
        midi::actions::end_session(&launchpad, &tx_midi);
        break;
      }
      if note_id == launchpad.stop_note() {
        midi::actions::stop_note(&mut sinks);
        continue;
      }
      if note_id == launchpad.first_page_note() {
        referential.first_page();

        midi::refresh_grid(&launchpad, &config, &mut referential, &tx_midi, false);
        continue;
      }
      if note_id == launchpad.last_page_note() {
        referential.last_page();

        midi::refresh_grid(&launchpad, &config, &mut referential, &tx_midi, false);
        continue;
      }
      if note_id == launchpad.prev_page_note() {
        referential.previous_page();

        midi::refresh_grid(&launchpad, &config, &mut referential, &tx_midi, false);
        continue;
      }
      if note_id == launchpad.next_page_note() {
        referential.next_page();

        midi::refresh_grid(&launchpad, &config, &mut referential, &tx_midi, false);
        continue;
      }
      if note_id == launchpad.swap_hold_mode_note() {
        config.swap_hold_to_play();

        midi::refresh_grid(&launchpad, &config, &mut referential, &tx_midi, true);
        continue;
      }
      let bookmark_notes = launchpad.bookmark_notes();
      if bookmark_notes.contains(&note_id) {
        let index = bookmark_notes.iter().position(|&r| r == note_id).unwrap();
        if !config.bookmark_exists(index) {
          continue;
        }
        referential.set_current_bookmark(bookmark_notes[index]);
        // Get the bookmark parameter from Config based on index
        let bookmark_path = config.get_bookmark(index).expect("No path found for bookmark");
        referential.init(bookmark_path);

        midi::refresh_grid(&launchpad, &config, &mut referential, &tx_midi, true);
        continue;
      }
      if let Some(note) = &referential.get_note(note_id) {
        // Clone to ensure the value won't be freed until it's no longer used.
        // As without it,
        // the note would be tied with the temporary value from &referential.get_note(note_id)
        let note = note.clone();
        match config.get_hold_to_mode() {
          HoldMode::Normal | HoldMode::Stop => play_sound(&mut config, output_handle.as_ref(), &virtual_handle, &tx_midi, &mut sinks, note, &ui),
          HoldMode::Pause => {
            if let Some((audio_sink, virtual_sink)) = sinks.get(&note.note_id) {
              // 本地监听关闭时只有虚拟设备这一路，用它的状态判断
              let still_playing = match audio_sink {
                Some(sink) => !sink.empty(),
                None => !virtual_sink.empty(),
              };
              if still_playing {
                if let Some(sink) = audio_sink {
                  sink.play();
                }
                virtual_sink.play();
                continue;
              }
            }
            play_sound(&mut config, output_handle.as_ref(), &virtual_handle, &tx_midi, &mut sinks, note, &ui);
          }
        }
      }
    }
  }

  // 退出前撤掉托盘图标
  #[cfg(windows)]
  {
    tray::shutdown();
    thread::sleep(std::time::Duration::from_millis(200));
  }

  Ok(())
}

fn play_sound(config: &mut Config, output_handle: Option<&OutputStream>, virtual_handle: &OutputStream, tx_midi: &Sender<MidiOutputEvent>, sinks: &mut HashMap<u8, (Option<Sink>, Sink)>, note: Note, ui: &Arc<webui::Shared>) {
  // 本地监听关闭时 output_handle 为 None，只往虚拟设备（VoiceMeeter）送
  let audio_sink = output_handle.map(|handle| audio::play_sound(handle, note.path, VOICE_VOLUME).0);
  // 音量取 config 的 virtual_volume（缺省 0.10），时长以这一路为准（它一定会播）
  let (virtual_sink, duration) = audio::play_sound(virtual_handle, note.path, config.virtual_volume());

  // 活动日志：这把键播了什么文件
  let fname = note.path.rsplit(['\\', '/']).next().unwrap_or(note.path);
  webui::push_activity(ui, format!("▶ {}", fname));

  let thread_tx_midi = tx_midi.clone();
  thread_tx_midi.send((note.note_id, note.color, true)).unwrap();

  if config.get_hold_to_mode() == &HoldMode::Normal && let Some(duration) = duration {
    // Light on the note and light off after the duration
    thread::spawn(move || {
      thread::sleep(duration);
      thread_tx_midi.send((note.note_id, note.color, false)).unwrap();
    });
  }

  sinks.insert(note.note_id, (audio_sink, virtual_sink));
}