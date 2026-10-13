//! tray.rs — 系统托盘（仅 Windows）
//!
//! 让 launch-soundpad.exe 变成托盘程序：没有控制台窗口，托盘图标 + 右键菜单：
//!   打开设置页 / 重新加载配置 / 熄灭 Launchpad 灯 / 退出
//! 双击托盘图标 = 打开设置页。
//!
//! 实现方式是自带一个隐藏窗口 + 消息循环（Shell_NotifyIconW），不依赖额外 GUI 框架。
//! 整个模块只在 Windows 编译（main.rs 里 #[cfg(windows)] 引入）。

// 这里全是 Win32 FFI，函数本身就是 unsafe 语境
#![allow(unsafe_op_in_unsafe_fn)]

use std::sync::atomic::{AtomicIsize, AtomicU16, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use log::{info, warn};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi as gdi;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell as shell;
use windows_sys::Win32::UI::WindowsAndMessaging as wam;

use crate::webui;
use crate::{NoteEvent, QUIT_NOTE, RELOAD_NOTE};

const WM_TRAYICON: u32 = wam::WM_APP + 1;
const IDI_APPLICATION_RES: usize = 32512;

const CMD_OPEN: usize = 1;
const CMD_RELOAD: usize = 2;
const CMD_LEDS: usize = 3;
const CMD_QUIT: usize = 4;

static TX: Mutex<Option<Sender<NoteEvent>>> = Mutex::new(None);
static UI: Mutex<Option<Arc<webui::Shared>>> = Mutex::new(None);
static PORT: AtomicU16 = AtomicU16::new(8787);
static HWND_I: AtomicIsize = AtomicIsize::new(0);

/// 把托盘动作写进设置页的「活动日志」，用户能看见，也方便排查
fn act(msg: &str) {
    if let Ok(g) = UI.lock() {
        if let Some(ui) = g.as_ref() {
            ui.push_activity(msg.to_string());
        }
    }
}

fn send_cmd(note: u8) {
    if let Some(tx) = TX.lock().ok().and_then(|g| g.clone()) {
        let _ = tx.send((note, true));
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn fill_wide(dst: &mut [u16], s: &str) {
    let w = wide(s);
    let n = w.len().min(dst.len());
    dst[..n].copy_from_slice(&w[..n]);
    if n < dst.len() {
        dst[n] = 0;
    }
}

fn hwnd_to_isize(h: HWND) -> isize {
    h as isize
}

fn isize_to_hwnd(v: isize) -> HWND {
    v as HWND
}

/// 画一个 32x32 的图标（深色底 + 4 个彩色格子，像个 launchpad）
unsafe fn make_icon() -> wam::HICON {
    const S: i32 = 32;
    let mut px: Vec<u32> = vec!(0; (S * S) as usize);
    // 0xAARRGGBB -> 内存里是 BGRA（BI_RGB 32bpp）
    let bg = 0xFF20_2226u32;
    let cells = [0xFF35_C759u32, 0xFFF2_A900u32, 0xFFE8_3E3Eu32, 0xFF4F_8BF7u32];
    for y in 0..S {
        for x in 0..S {
            let i = (y * S + x) as usize;
            let inside = x >= 3 && y >= 3 && x < S - 3 && y < S - 3;
            if !inside {
                px[i] = 0; // 透明
                continue;
            }
            // 2x2 格子，格子之间留缝
            let cx = (x - 5) / 12; // 0..2
            let cy = (y - 5) / 12;
            let gap = ((x - 5) % 12) >= 10 || ((y - 5) % 12) >= 10 || x < 5 || y < 5 || x >= 27 || y >= 27;
            if gap {
                px[i] = bg;
            } else {
                let idx = ((cy * 2 + cx).clamp(0, 3)) as usize;
                px[i] = cells[idx];
            }
        }
    }

    let hdc_screen = gdi::GetDC(0 as HWND);
    let hdc = gdi::CreateCompatibleDC(hdc_screen);
    let bmi = gdi::BITMAPINFO {
        bmiHeader: gdi::BITMAPINFOHEADER {
            biSize: std::mem::size_of::<gdi::BITMAPINFOHEADER>() as u32,
            biWidth: S,
            biHeight: -S, // 负数 = 自上而下，和我们的像素顺序一致
            biPlanes: 1,
            biBitCount: 32,
            biCompression: gdi::BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [std::mem::zeroed(); 1],
    };
    let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
    let hbm_color = gdi::CreateDIBSection(hdc, &bmi, gdi::DIB_RGB_COLORS, &mut bits, 0 as _, 0);
    if !bits.is_null() {
        std::ptr::copy_nonoverlapping(px.as_ptr() as *const u8, bits as *mut u8, px.len() * 4);
    }
    // 掩码：全 0 = 不透明（透明区域靠 alpha 处理，Windows 会用颜色位图的 alpha）
    let mask_bits = vec![0u8; (S * S / 8) as usize];
    let hbm_mask = gdi::CreateBitmap(S, S, 1, 1, mask_bits.as_ptr() as *const std::ffi::c_void);
    let info = wam::ICONINFO {
        fIcon: 1,
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: hbm_mask,
        hbmColor: hbm_color,
    };
    let icon = wam::CreateIconIndirect(&info);
    gdi::DeleteObject(hbm_color as _);
    gdi::DeleteObject(hbm_mask as _);
    gdi::DeleteDC(hdc);
    gdi::ReleaseDC(0 as HWND, hdc_screen);
    if icon.is_null() {
        // 画失败就退回系统图标
        wam::LoadIconW(0 as _, IDI_APPLICATION_RES as *const u16)
    } else {
        icon
    }
}

fn open_settings() {
    let port = PORT.load(Ordering::Relaxed);
    let url = format!("http://127.0.0.1:{}/", port);
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", &url])
        .spawn();
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TRAYICON => {
            let ev = (lparam as u32) & 0xFFFF;
            match ev {
                wam::WM_LBUTTONUP | wam::WM_LBUTTONDBLCLK => open_settings(),
                wam::WM_RBUTTONUP => show_menu(hwnd),
                _ => {}
            }
            0
        }
        wam::WM_CLOSE => {
            remove_icon(hwnd);
            wam::DestroyWindow(hwnd);
            0
        }
        wam::WM_DESTROY => {
            wam::PostQuitMessage(0);
            0
        }
        _ => wam::DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn show_menu(hwnd: HWND) {
    let menu = wam::CreatePopupMenu();
    let items: [(&str, usize); 5] = [
        ("打开设置页", CMD_OPEN),
        ("重新加载配置 / 页面", CMD_RELOAD),
        ("熄灭 Launchpad 灯", CMD_LEDS),
        ("", 0), // 分隔线
        ("退出", CMD_QUIT),
    ];
    for (label, id) in items {
        if id == 0 {
            wam::AppendMenuW(menu, wam::MF_SEPARATOR, 0, std::ptr::null());
            continue;
        }
        let w = wide(label);
        wam::AppendMenuW(menu, wam::MF_STRING, id, w.as_ptr());
    }
    let mut pt = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    wam::GetCursorPos(&mut pt);
    // 不 SetForegroundWindow 的话点外面菜单不会消失
    wam::SetForegroundWindow(hwnd);
    let cmd = wam::TrackPopupMenu(
        menu,
        wam::TPM_RIGHTBUTTON | wam::TPM_RETURNCMD,
        pt.x,
        pt.y,
        0,
        hwnd,
        std::ptr::null(),
    );
    wam::DestroyMenu(menu);

    match cmd as usize {
        CMD_OPEN => {
            act("托盘菜单：打开设置页");
            open_settings();
        }
        CMD_RELOAD => {
            act("托盘菜单：重新加载配置 / 页面");
            send_cmd(RELOAD_NOTE);
        }
        CMD_LEDS => {
            act("托盘菜单：熄灭 Launchpad 灯");
            crate::webui::clear_leds_now();
        }
        CMD_QUIT => {
            act("托盘菜单：退出程序");
            send_cmd(QUIT_NOTE);
        }
        _ => {}
    }
}

unsafe fn remove_icon(hwnd: HWND) {
    let mut nid: shell::NOTIFYICONDATAW = std::mem::zeroed();
    nid.cbSize = std::mem::size_of::<shell::NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    shell::Shell_NotifyIconW(shell::NIM_DELETE, &nid);
}

/// 起托盘图标（在独立线程里跑消息循环）。失败也只是没有托盘图标，程序照常跑。
pub fn start(tx: Sender<NoteEvent>, port: u16, ui: Arc<webui::Shared>) {
    if let Ok(mut g) = TX.lock() {
        *g = Some(tx);
    }
    if let Ok(mut g) = UI.lock() {
        *g = Some(ui);
    }
    PORT.store(port, Ordering::Relaxed);

    std::thread::Builder::new()
        .name("tray".into())
        .spawn(move || unsafe {
            let hinst = GetModuleHandleW(std::ptr::null());
            let class_name = wide("launch_soundpad_tray_wnd");
            let icon = make_icon();

            let wc = wam::WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinst,
                hIcon: icon,
                hCursor: 0 as _,
                hbrBackground: 0 as _,
                lpszMenuName: std::ptr::null(),
                lpszClassName: class_name.as_ptr(),
            };
            if wam::RegisterClassW(&wc) == 0 {
                warn!("tray: RegisterClassW failed");
                return;
            }
            let title = wide("launch-soundpad");
            let hwnd = wam::CreateWindowExW(
                0,
                class_name.as_ptr(),
                title.as_ptr(),
                0, // 不显示
                0,
                0,
                0,
                0,
                0 as _,
                0 as _,
                hinst,
                std::ptr::null_mut(),
            );
            if hwnd.is_null() {
                warn!("tray: CreateWindowExW failed");
                return;
            }
            HWND_I.store(hwnd_to_isize(hwnd), Ordering::Relaxed);

            // 加图标 + 气泡提示
            let mut nid: shell::NOTIFYICONDATAW = std::mem::zeroed();
            nid.cbSize = std::mem::size_of::<shell::NOTIFYICONDATAW>() as u32;
            nid.hWnd = hwnd;
            nid.uID = 1;
            nid.uFlags = shell::NIF_MESSAGE | shell::NIF_ICON | shell::NIF_TIP | shell::NIF_INFO;
            nid.uCallbackMessage = WM_TRAYICON;
            nid.hIcon = icon;
            fill_wide(&mut nid.szTip, "Launchpad 声音板（双击打开设置页）");
            fill_wide(&mut nid.szInfoTitle, "Launchpad 声音板");
            fill_wide(&mut nid.szInfo, "已在后台运行。双击托盘图标打开设置页，右键有菜单。");
            nid.dwInfoFlags = shell::NIIF_INFO;
            if shell::Shell_NotifyIconW(shell::NIM_ADD, &nid) == 0 {
                warn!("tray: Shell_NotifyIconW(NIM_ADD) failed");
                act("托盘图标创建失败（右键菜单不可用，可用设置页里的按钮）");
                return;
            }
            info!("tray icon ready (port {})", PORT.load(Ordering::Relaxed));
            act("托盘图标已就绪（双击图标打开设置页）");

            let mut msg: wam::MSG = std::mem::zeroed();
            while wam::GetMessageW(&mut msg, 0 as _, 0, 0) > 0 {
                wam::TranslateMessage(&msg);
                wam::DispatchMessageW(&msg);
            }
            info!("tray loop ended");
        })
        .ok();
}

/// 退出前调用：把托盘图标撤掉并结束消息循环
pub fn shutdown() {
    let h = HWND_I.load(Ordering::Relaxed);
    if h != 0 {
        unsafe {
            wam::PostMessageW(isize_to_hwnd(h), wam::WM_CLOSE, 0, 0);
        }
    }
}
