//! Host system tray untuk Rail.
//!
//! Aplikasi mengirim ikon tray (Shell_NotifyIcon) ke jendela pertama berkelas
//! `Shell_TrayWnd` yang ditemukan `FindWindow`. ABRA membuat jendela berkelas sama
//! yang selalu paling atas, membaca setiap pesan ikon, lalu meneruskan semuanya ke
//! taskbar Explorer yang asli. Jadi Explorer tetap tahu semua ikon, dan saat ABRA
//! keluar ikon-ikon itu tetap ada di taskbar Windows.

use nacre_platform::log;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;
use windows::core::w;
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS,
};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

const NIM_ADD: u32 = 0;
const NIM_MODIFY: u32 = 1;
const NIM_DELETE: u32 = 2;
const NIM_SETVERSION: u32 = 4;
const NIF_MESSAGE: u32 = 0x1;
const NIF_ICON: u32 = 0x2;
const NIF_TIP: u32 = 0x4;
const NIF_STATE: u32 = 0x8;
const NIF_GUID: u32 = 0x20;
const NIS_HIDDEN: u32 = 0x1;
const NIN_SELECT: u32 = WM_USER;
const SIGNATURE: u32 = 0x3475_3423;

/// Pesan ke Rail saat daftar ikon berubah.
pub const WM_TRAY_CHANGED: u32 = WM_APP + 90;

#[repr(C)]
#[derive(Clone, Copy)]
struct NotifyData32 {
    signature: u32,
    message: u32,
    cb_size: u32,
    hwnd: u32,
    uid: u32,
    flags: u32,
    callback: u32,
    hicon: u32,
    tip: [u16; 128],
    state: u32,
    state_mask: u32,
    info: [u16; 256],
    version: u32,
    info_title: [u16; 64],
    info_flags: u32,
    guid: [u8; 16],
    balloon_icon: u32,
}

#[derive(Clone)]
pub struct Icon {
    pub hwnd: isize,
    pub uid: u32,
    pub callback: u32,
    pub hicon: isize,
    pub tip: String,
    pub version: u32,
    pub hidden: bool,
    guid: Option<[u8; 16]>,
}

static ICONS: Mutex<Vec<Icon>> = Mutex::new(Vec::new());
static HOST: AtomicIsize = AtomicIsize::new(0);
static NOTIFY: AtomicIsize = AtomicIsize::new(0);

fn icons_lock() -> std::sync::MutexGuard<'static, Vec<Icon>> {
    ICONS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Ikon yang terlihat, urut sesuai waktu ditambahkan.
pub fn icons() -> Vec<Icon> {
    unsafe {
        icons_lock().retain(|i| {
            let alive = IsWindow(Some(HWND(i.hwnd as *mut _))).as_bool();
            if !alive && i.hicon != 0 {
                let _ = DestroyIcon(HICON(i.hicon as *mut _));
            }
            alive
        });
    }
    // Ikon sistem Windows (baterai, speaker, jaringan) dimiliki Explorer; Rail sudah punya versinya sendiri.
    let explorer_pid = nacre_platform::shell::taskbar::explorer_tray()
        .map(|h| {
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
            pid
        })
        .unwrap_or(0);
    icons_lock()
        .iter()
        .filter(|i| !i.hidden && i.hicon != 0)
        .filter(|i| {
            let mut pid = 0u32;
            unsafe { GetWindowThreadProcessId(HWND(i.hwnd as *mut _), Some(&mut pid)) };
            pid == 0 || pid != explorer_pid
        })
        .cloned()
        .collect()
}

pub fn init(notify: HWND) {
    NOTIFY.store(notify.0 as isize, Ordering::Relaxed);
    if HOST.load(Ordering::Relaxed) != 0 {
        return;
    }
    // Host punya thread sendiri: meneruskan pesan ke Explorer bisa lambat dan
    // tidak boleh menahan tampilan Rail.
    std::thread::spawn(|| unsafe {
        host_thread();
        let mut m = MSG::default();
        while GetMessageW(&mut m, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    });
}

unsafe fn host_thread() {
    {
        let Ok(hi) = GetModuleHandleW(None) else { return };
        let hi: HINSTANCE = hi.into();
        let class = w!("Shell_TrayWnd");
        RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(host_proc), hInstance: hi, lpszClassName: class, ..Default::default() });
        let Ok(hwnd) = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE, class, w!(""), WS_POPUP, 0, 0, 0, 0, None, None, Some(hi), None) else {
            log::line("tray", "gagal membuat host");
            return;
        };
        HOST.store(hwnd.0 as isize, Ordering::Relaxed);
        raise();
        SetTimer(Some(hwnd), 1, 2000, None);
        // Minta semua aplikasi mendaftarkan ulang ikonnya (sekarang ke host ABRA).
        let msg = RegisterWindowMessageW(w!("TaskbarCreated"));
        let _ = PostMessageW(Some(HWND_BROADCAST), msg, WPARAM(0), LPARAM(0));
        log::line("tray", "host aktif");
    }
}

/// Lepas host saat ABRA keluar, lalu minta aplikasi mendaftar ulang ke Explorer.
pub fn shutdown() {
    let h = HOST.swap(0, Ordering::Relaxed);
    if h == 0 {
        return;
    }
    unsafe {
        let _ = SendMessageTimeoutW(HWND(h as *mut _), WM_CLOSE, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG, 1000, None);
        if let Some(real) = nacre_platform::shell::taskbar::explorer_tray() {
            let _ = SetWindowPos(real, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
        let msg = RegisterWindowMessageW(w!("TaskbarCreated"));
        let _ = PostMessageW(Some(HWND_BROADCAST), msg, WPARAM(0), LPARAM(0));
    }
}

fn raise() {
    let h = HOST.load(Ordering::Relaxed);
    if h != 0 {
        unsafe {
            // Taskbar Explorer disembunyikan ABRA; taruh paling bawah supaya FindWindow menemukan host ini dulu.
            if let Some(real) = nacre_platform::shell::taskbar::explorer_tray() {
                let _ = SetWindowPos(real, Some(HWND_NOTOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                let _ = SetWindowPos(real, Some(HWND_BOTTOM), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
            let _ = SetWindowPos(HWND(h as *mut _), Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

fn notify_rail() {
    let n = NOTIFY.load(Ordering::Relaxed);
    if n != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(n as *mut _)), WM_TRAY_CHANGED, WPARAM(0), LPARAM(0));
        }
    }
}

fn wide(s: &[u16]) -> String {
    let n = s.iter().position(|c| *c == 0).unwrap_or(s.len());
    String::from_utf16_lossy(&s[..n])
}

fn handle_notify(bytes: &[u8]) {
    const MIN: usize = 8 + 6 * 4;
    if bytes.len() < MIN {
        return;
    }
    let mut buf = [0u8; std::mem::size_of::<NotifyData32>()];
    let n = bytes.len().min(buf.len());
    buf[..n].copy_from_slice(&bytes[..n]);
    let d: NotifyData32 = unsafe { std::ptr::read_unaligned(buf.as_ptr() as *const NotifyData32) };
    if d.signature != SIGNATURE {
        return;
    }
    let hwnd = d.hwnd as i32 as isize;
    let guid = if d.flags & NIF_GUID != 0 { Some(d.guid) } else { None };
    let mut list = icons_lock();
    let pos = list.iter().position(|i| match (guid, i.guid) {
        (Some(g), Some(h)) => g == h,
        _ => i.hwnd == hwnd && i.uid == d.uid,
    });
    match d.message {
        NIM_ADD | NIM_MODIFY => {
            let idx = match pos {
                Some(p) => p,
                None => {
                    list.push(Icon { hwnd, uid: d.uid, callback: 0, hicon: 0, tip: String::new(), version: 0, hidden: false, guid });
                    list.len() - 1
                }
            };
            let it = &mut list[idx];
            it.hwnd = hwnd;
            if d.flags & NIF_MESSAGE != 0 {
                it.callback = d.callback;
            }
            if d.flags & NIF_ICON != 0 {
                let copy = unsafe { CopyIcon(HICON(d.hicon as i32 as isize as *mut _)) }.map(|h| h.0 as isize).unwrap_or(0);
                if it.hicon != 0 {
                    unsafe {
                        let _ = DestroyIcon(HICON(it.hicon as *mut _));
                    }
                }
                it.hicon = copy;
            }
            if d.flags & NIF_TIP != 0 {
                it.tip = wide(&d.tip);
            }
            if d.flags & NIF_STATE != 0 && d.state_mask & NIS_HIDDEN != 0 {
                it.hidden = d.state & NIS_HIDDEN != 0;
            }
        }
        NIM_DELETE => {
            if let Some(p) = pos {
                let it = list.remove(p);
                if it.hicon != 0 {
                    unsafe {
                        let _ = DestroyIcon(HICON(it.hicon as *mut _));
                    }
                }
            }
        }
        NIM_SETVERSION => {
            if let Some(p) = pos {
                list[p].version = d.version;
            }
        }
        _ => {}
    }
    drop(list);
    notify_rail();
}

unsafe extern "system" fn host_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            raise();
            LRESULT(0)
        }
        WM_COPYDATA => {
            let cds = &*(lp.0 as *const COPYDATASTRUCT);
            let tray_msg = cds.dwData == 1 && !cds.lpData.is_null();
            if tray_msg {
                let bytes = std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize);
                handle_notify(bytes);
            }
            let r = forward(msg, wp, lp);
            if tray_msg {
                LRESULT(1)
            } else {
                r
            }
        }
        m if m >= WM_USER => forward(m, wp, lp),
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn forward(msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let Some(real) = nacre_platform::shell::taskbar::explorer_tray() else { return LRESULT(0) };
    let mut out = 0usize;
    unsafe {
        SendMessageTimeoutW(real, msg, wp, lp, SMTO_ABORTIFHUNG | SMTO_NORMAL, 300, Some(&mut out));
    }
    LRESULT(out as isize)
}

pub enum Button {
    Left,
    Right,
}

/// Teruskan klik ke aplikasi pemilik ikon, seperti yang dilakukan taskbar Windows.
pub fn invoke(icon: &Icon, b: Button, x: i32, y: i32) {
    unsafe {
        let h = HWND(icon.hwnd as *mut _);
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        let _ = windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(pid);
        let send = |m: u32| {
            let (wp, lp) = if icon.version >= 4 {
                let wp = ((y as u32 & 0xFFFF) << 16) | (x as u32 & 0xFFFF);
                (WPARAM(wp as usize), LPARAM((((icon.uid & 0xFFFF) << 16) | (m & 0xFFFF)) as isize))
            } else {
                (WPARAM(icon.uid as usize), LPARAM(m as isize))
            };
            let _ = PostMessageW(Some(h), icon.callback, wp, lp);
        };
        match b {
            Button::Left => {
                send(WM_LBUTTONDOWN);
                send(WM_LBUTTONUP);
                if icon.version >= 4 {
                    send(NIN_SELECT);
                }
            }
            Button::Right => {
                send(WM_RBUTTONDOWN);
                send(WM_RBUTTONUP);
                if icon.version >= 4 {
                    send(WM_CONTEXTMENU);
                }
            }
        }
    }
}

/// Gambar ikon ke BGRA premultiplied ukuran `px`×`px`.
pub fn bitmap(hicon: isize, px: i32) -> Option<Vec<u8>> {
    unsafe {
        let dc = CreateCompatibleDC(None);
        let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
        let bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: px,
                biHeight: -px,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let Ok(bmp) = CreateDIBSection(Some(dc), &bi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(dc);
            return None;
        };
        let old = SelectObject(dc, bmp.into());
        let ok = DrawIconEx(dc, 0, 0, HICON(hicon as *mut _), px, px, 0, None, DI_NORMAL).is_ok();
        let n = (px * px * 4) as usize;
        let mut out = std::slice::from_raw_parts(bits as *const u8, n).to_vec();
        SelectObject(dc, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(dc);
        if !ok {
            return None;
        }
        // Ikon lama tanpa alpha: anggap piksel berwarna sebagai buram.
        if out.chunks(4).all(|p| p[3] == 0) {
            for p in out.chunks_mut(4) {
                if p[0] | p[1] | p[2] != 0 {
                    p[3] = 255;
                }
            }
        }
        Some(out)
    }
}
