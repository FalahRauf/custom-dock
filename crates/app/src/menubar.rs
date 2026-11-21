use crate::dock::menu::{Item, Menu, MENU_CLOSE, MENU_PICK};
use crate::popup::Popup;
use crate::{prefs, services};
use nacre_core::menubar::{self as lay, Gap, Item as LItem, Placed};
use nacre_platform::icons::Bitmap;
use nacre_platform::text::{measure, render, render_face, Style, ICON_FACE};
use nacre_platform::wins::{enumerate, focus, tracker};
use nacre_platform::{keys, launch, log, radio, shell, sys, time};
use nacre_ui::glass::Tier;
use nacre_ui::gfx::Gfx;
use std::cell::RefCell;
use std::collections::HashMap;
use windows::core::w;
use windows::UI::Color;
use windows::UI::Composition::{CompositionDrawingSurface, CompositionStretch, Compositor, ContainerVisual, SpriteVisual};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_numerics::{Vector2, Vector3};

const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_APPCHG: u32 = WM_APP + 70;
const WM_CLICKWIN: u32 = WM_APP + 74;
static MOUSE_HOOK: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
static BAR_CTL: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
const WM_NOTCH: u32 = WM_APP + 71;
const WM_TEST_NET: u32 = WM_APP + 75;
const T_CLOCK: usize = 1;
const T_STATUS: usize = 2;
const T_LIFT: usize = 41;
const T_APP: usize = 3;
const T_FS: usize = 4;
const T_SIG: usize = 5;
const T_AH: usize = 6;
const T_AH_OUT: usize = 7;
const T_AH_STEP: usize = 8;
const AH_STEP_MS: u32 = 24;
const AH_IN_MS: u32 = 220;
const AH_OUT_MS: u32 = 180;
const AH_HIDE_TICKS: u32 = 8;
const BAR_W: i32 = 600;
const APP_W: i32 = 800;

const LOGO: u32 = 1;
const APP: u32 = 2;
const M_FILE: u32 = 3;
const M_EDIT: u32 = 4;
const M_VIEW: u32 = 5;
const M_WIN: u32 = 6;
const M_HELP: u32 = 7;
const CLOCK: u32 = 10;
const CTRL: u32 = 11;
const SEARCH: u32 = 12;
const WIFI: u32 = 13;
const BATT: u32 = 14;
const STAGEI: u32 = 15;
const NATIVE0: u32 = 50;
const TRAY0: u32 = 200;
const TRAY_BTN: u32 = 199;
const STATUS: u32 = 198;
const VOL: u32 = 197;
const TRAY_MAX: usize = 16;
const NATIVE_CMD: usize = 100_000;

struct Bar {
    popup: Popup,
    comp: Compositor,
    gfx: Gfx,
    dpi: f32,
    screen: (i32, i32),
    h: i32,
    layer: ContainerVisual,
    capsule: SpriteVisual,
    bg: SpriteVisual,
    hair: SpriteVisual,
    cache: HashMap<String, (CompositionDrawingSurface, i32, i32)>,
    left: Vec<Placed>,
    right: Vec<Placed>,
    app: String,
    hidden: Vec<isize>,
    menu: Menu,
    open: Option<u32>,
    hover: Option<u32>,
    tracking: bool,
    wifi: Option<bool>,
    wifi_bars: Option<u8>,
    bt: Option<bool>,
    radios: Option<radio::Radios>,
    dark_text: bool,
    shadow: bool,
    native: Vec<nacre_platform::menus::Top>,
    native_hwnd: isize,
    fullscreen: bool,
    ah_shown: bool,
    ah_out: u32,
    ah_res: i32,
    ah_from: i32,
    ah_t0: std::time::Instant,
    tip: Option<crate::dock::tooltip::Tooltip>,
    tray: Vec<crate::tray::Icon>,
    corner: Option<(i32, i32, i32)>,
    flyout: Option<Popup>,
    fly_out: u32,
    menu_lift: u32,
    fly_cells: Vec<(f32, f32, f32, usize)>,
}

struct Ctl {
    hwnd: HWND,
    comp: Compositor,
}

thread_local! {
    static BAR: RefCell<Option<Box<Bar>>> = const { RefCell::new(None) };
    static CTL: RefCell<Option<Ctl>> = const { RefCell::new(None) };
}

fn with_bar<R>(f: impl FnOnce(&mut Bar) -> R) -> Option<R> {
    BAR.with(|c| c.try_borrow_mut().ok().and_then(|mut b| b.as_mut().map(|b| f(b))))
}

pub fn refresh_radios() {
    with_bar(|b| {
        if let Some(r) = &b.radios {
            let _ = r.commands.send(radio::Cmd::Refresh);
        }
    });
}

fn mt(key: &'static str) -> String {
    let t = prefs::tr(key);
    t.strip_prefix("Menu:").unwrap_or(t).to_string()
}

fn recolor(b: &Bitmap, rgb: [u8; 3]) -> Vec<u8> {
    let mut out = b.bgra.clone();
    for px in out.chunks_exact_mut(4) {
        let a = px[3] as u32;
        px[0] = (rgb[2] as u32 * a / 255) as u8;
        px[1] = (rgb[1] as u32 * a / 255) as u8;
        px[2] = (rgb[0] as u32 * a / 255) as u8;
    }
    out
}

fn noop_fg() {
    CTL.with(|c| {
        if let Some(ctl) = c.borrow().as_ref() {
            unsafe {
                let _ = PostMessageW(Some(ctl.hwnd), WM_APPCHG, WPARAM(0), LPARAM(0));
            }
        }
    });
}

unsafe extern "system" fn ll_mouse(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 {
        let m = wp.0 as u32;
        if m == WM_LBUTTONDOWN || m == WM_RBUTTONDOWN || m == WM_MBUTTONDOWN {
            let ctl = BAR_CTL.load(std::sync::atomic::Ordering::Relaxed);
            if ctl != 0 {
                let pt = (*(lp.0 as *const MSLLHOOKSTRUCT)).pt;
                let h = WindowFromPoint(pt);
                if !h.0.is_null() {
                    let root = GetAncestor(h, GA_ROOT);
                    if !root.0.is_null() {
                        let _ = PostMessageW(Some(HWND(ctl as *mut _)), WM_CLICKWIN, WPARAM(root.0 as usize), LPARAM(0));
                    }
                }
            }
        }
    }
    CallNextHookEx(None, code, wp, lp)
}

fn loc_hook(ev: u32, hwnd: isize) {
    if ev == tracker::EV_LOCATION && hwnd != 0 && hwnd == focus::foreground() {
        CTL.with(|c| {
            if let Some(ctl) = c.borrow().as_ref() {
                unsafe {
                    SetTimer(Some(ctl.hwnd), T_FS, 200, None);
                }
            }
        });
    }
}

fn is_fullscreen(hwnd: isize) -> bool {
    unsafe {
        let h = HWND(hwnd as *mut _);
        let mut cls = [0u16; 64];
        let n = GetClassNameW(h, &mut cls);
        let cls = String::from_utf16_lossy(&cls[..n.max(0) as usize]);
        if cls == "Progman" || cls == "WorkerW" || cls == "Shell_TrayWnd" {
            return false;
        }
        let mut r = windows::Win32::Foundation::RECT::default();
        if GetWindowRect(h, &mut r).is_err() {
            return false;
        }
        // Jendela maximize biasa (masih punya title bar) bukan fullscreen.
        if IsZoomed(h).as_bool() && (GetWindowLongW(h, GWL_STYLE) as u32 & WS_CAPTION.0) != 0 {
            return false;
        }
        let (sw, sh) = sys::screen_size();
        r.left <= 0 && r.top <= 0 && r.right >= sw && r.bottom >= sh
    }
}

pub fn init(comp: &Compositor) -> windows_core::Result<()> {
    unsafe {
        let hi: HINSTANCE = GetModuleHandleW(None)?.into();
        let class = w!("AbraBarCtl");
        RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(ctl_proc), hInstance: hi, lpszClassName: class, ..Default::default() });
        let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), class, w!("abra-bar-ctl"), WS_POPUP, 0, 0, 0, 0, Some(HWND_MESSAGE), None, Some(hi), None)?;
        CTL.with(|c| *c.borrow_mut() = Some(Ctl { hwnd, comp: comp.clone() }));
        prefs::listen(hwnd);
        tracker::install(noop_fg);
        tracker::install_fx(loc_hook);
        BAR_CTL.store(hwnd.0 as isize, std::sync::atomic::Ordering::Relaxed);
        if let Ok(h) = SetWindowsHookExW(WH_MOUSE_LL, Some(ll_mouse), None, 0) {
            MOUSE_HOOK.store(h.0 as isize, std::sync::atomic::Ordering::Relaxed);
        }
        apply_prefs();
        crate::tray::init(hwnd);
    }
    Ok(())
}

fn apply_prefs() {
    let want = prefs::get().menubar;
    let have = BAR.with(|c| c.borrow().is_some());
    if want && !have {
        let comp = CTL.with(|c| c.borrow().as_ref().map(|c| c.comp.clone()));
        if let Some(comp) = comp {
            match create(&comp) {
                Ok(b) => {
                    let hwnd = b.popup.hwnd;
                    BAR.with(|c| *c.borrow_mut() = Some(b));
                    services::register_bar(hwnd);
                    with_bar(|b| {
                        b.refresh_app();
                        b.relayout();
                        b.render();
                    });
                    services::post_island(WM_APP + 72);
                    CTL.with(|c| {
                        if let Some(ctl) = c.borrow().as_ref() {
                            unsafe {
                                SetTimer(Some(ctl.hwnd), T_APP, 900, None);
                            }
                        }
                    });
                    log::line("menubar", "dibuat");
                    crate::hotcorners::raise();
                }
                Err(e) => log::line("menubar", &format!("gagal: {e}")),
            }
        }
    } else if !want && have {
        let b = BAR.with(|c| c.borrow_mut().take());
        if let Some(b) = b {
            b.menu.hide();
            unsafe {
                let _ = DestroyWindow(b.popup.hwnd);
            }
        }
        shell::appbar::release_top();
        log::line("menubar", "dihancurkan");
        services::post_island(crate::prefs::WM_PREFS);
    } else if want {
        let rescale = with_bar(|b| {
            let want_dpi = unsafe { GetDpiForWindow(b.popup.hwnd) }.max(96) as f32 / 96.0 * prefs::ui_scale();
            (want_dpi - b.dpi).abs() > 0.01
        })
        .unwrap_or(false);
        if rescale {
            let b = BAR.with(|c| c.borrow_mut().take());
            if let Some(b) = b {
                b.menu.hide();
                unsafe {
                    let _ = DestroyWindow(b.popup.hwnd);
                }
            }
            shell::appbar::release_top();
            apply_prefs();
            return;
        }
        with_bar(|b| {
            b.apply_look();
            b.relayout();
            b.render();
            b.sync_autohide();
        });
    }
    sync_ah_timer();
}

fn create(comp: &Compositor) -> windows_core::Result<Box<Bar>> {
    let screen = sys::screen_size();
    let glass = prefs::apply(nacre_ui::glass::GlassParams::dark((10.0, 10.0), 0.0), &prefs::bar_look());
    let mut popup = Popup::new(comp, w!("AbraMenuBar"), Some(bar_proc), WINDOW_EX_STYLE(0), glass)?;
    let dpi = unsafe { GetDpiForWindow(popup.hwnd) }.max(96) as f32 / 96.0 * prefs::ui_scale();
    let notch_h = services::notch().map(|(_, _, h)| h as f32 / dpi);
    let h = (notch_h.unwrap_or(26.0) * dpi).round() as i32;
    let tier = {
        let c = sys::conditions();
        if c.remote_session { Tier::Solid } else { Tier::Full }
    };
    let _ = popup.panel.set_tier(tier);
    popup.set_rect(0, 0, screen.0, h)?;
    popup.panel.set_radius(0.0)?;
    popup.panel.set_rect(0.0, 0.0, 0.0, 0.0)?;
    let bg = comp.CreateSpriteVisual()?;
    popup.content.Children()?.InsertAtTop(&bg)?;
    let hair = comp.CreateSpriteVisual()?;
    hair.SetOpacity(0.0)?;
    popup.content.Children()?.InsertAtTop(&hair)?;
    let capsule = comp.CreateSpriteVisual()?;
    capsule.SetOpacity(0.0)?;
    popup.content.Children()?.InsertAtTop(&capsule)?;
    let layer = comp.CreateContainerVisual()?;
    popup.content.Children()?.InsertAtTop(&layer)?;
    let gfx = Gfx::shared(comp)?;
    let menu = Menu::new(comp, popup.hwnd, dpi, screen, tier)?;
    if !ah_on() {
        popup.show();
        shell::appbar::reserve_top(popup.hwnd, screen.0, h);
    }
    unsafe {
        SetTimer(Some(popup.hwnd), T_CLOCK, time::ms_to_next_minute().max(200), None);
        SetTimer(Some(popup.hwnd), T_STATUS, 60_000, None);
        SetTimer(Some(popup.hwnd), T_SIG, 4_000, None);
    }
    let hw = popup.hwnd.0 as isize;
    let radios = radio::spawn(move || unsafe {
        let _ = PostMessageW(Some(HWND(hw as *mut _)), WM_APP + 22, WPARAM(0), LPARAM(0));
    });
    if let Ok(v) = std::env::var("NACRE_TEST_NETMENU") {
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(2500));
            let code = match v.as_str() {
                "bt" => 1,
                "wifi-others" => 2,
                "cc" => 4,
                "bt-click" => 6,
                "cc-edit" => 8,
                _ => 0,
            };
            unsafe {
                let _ = PostMessageW(Some(HWND(hw as *mut _)), WM_TEST_NET, WPARAM(code), LPARAM(0));
            }
            if code == 2 || code == 4 || code == 6 || code == 8 {
                std::thread::sleep(std::time::Duration::from_millis(2500));
                unsafe {
                    let _ = PostMessageW(Some(HWND(hw as *mut _)), WM_TEST_NET, WPARAM(code + 1), LPARAM(0));
                }
            }
        });
    }
    let mut bar = Box::new(Bar {
        popup,
        comp: comp.clone(),
        gfx,
        dpi,
        screen,
        h,
        layer,
        capsule,
        bg,
        hair,
        cache: HashMap::new(),
        left: vec![],
        right: vec![],
        app: String::new(),
        hidden: vec![],
        menu,
        open: None,
        hover: None,
        tracking: false,
        wifi: None,
        wifi_bars: nacre_platform::wifi::link().map(|l| l.1),
        bt: None,
        radios: Some(radios),
        dark_text: false,
        shadow: false,
        native: vec![],
        native_hwnd: 0,
        fullscreen: false,
        ah_shown: !ah_on(),
        ah_out: 0,
        ah_res: if ah_on() { 0 } else { h },
        ah_from: 0,
        ah_t0: std::time::Instant::now(),
        tip: None,
        tray: crate::tray::icons(),
        corner: None,
        flyout: None,
        fly_out: 0,
        menu_lift: 0,
        fly_cells: vec![],
    });
    bar.apply_bg();
    Ok(bar)
}

unsafe extern "system" fn ctl_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        prefs::WM_PREFS => {
            apply_prefs();
            LRESULT(0)
        }
        WM_APPCHG => {
            SetTimer(Some(hwnd), T_APP, 90, None);
            LRESULT(0)
        }
        WM_CLICKWIN => {
            let root = wp.0 as isize;
            let mut cls = [0u16; 64];
            let n = GetClassNameW(HWND(root as *mut _), &mut cls);
            let cls = String::from_utf16_lossy(&cls[..n.max(0) as usize]);
            let mut pid = 0u32;
            GetWindowThreadProcessId(HWND(root as *mut _), Some(&mut pid));
            let shell = matches!(cls.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "NotifyIconOverflowWindow" | "TopLevelWindowForOverflowXamlIsland");
            if pid != std::process::id() && !shell {
                crate::dock::poke_click();
                with_bar(|b| {
                    if b.refresh_app_for(root) {
                        b.relayout();
                        b.render();
                    }
                });
            }
            LRESULT(0)
        }
        WM_TIMER if wp.0 == T_FS => {
            let _ = KillTimer(Some(hwnd), T_FS);
            with_bar(|b| {
                b.check_fullscreen();
                b.recheck_color();
            });
            LRESULT(0)
        }
        crate::tray::WM_TRAY_CHANGED => {
            with_bar(|b| {
                b.tray = crate::tray::icons();
                b.relayout();
                b.render();
            });
            LRESULT(0)
        }
        WM_TIMER if wp.0 == T_AH_STEP => {
            with_bar(|b| b.res_step());
            LRESULT(0)
        }
        WM_TIMER if wp.0 == T_AH_OUT => {
            let _ = KillTimer(Some(hwnd), T_AH_OUT);
            with_bar(|b| {
                if !b.ah_shown {
                    b.popup.hide();
                }
            });
            LRESULT(0)
        }
        WM_TIMER if wp.0 == T_AH => {
            with_bar(|b| b.autohide_tick());
            LRESULT(0)
        }
        WM_TIMER if wp.0 == T_APP => {
            let _ = KillTimer(Some(hwnd), T_APP);
            with_bar(|b| {
                if b.refresh_app() {
                    b.relayout();
                    b.render();
                } else {
                    b.recheck_color();
                }
            });
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

impl Bar {
    fn fg(&self) -> [u8; 3] {
        if self.shadow {
            [0, 0, 0]
        } else if self.dark_text {
            [18, 18, 22]
        } else {
            [255, 255, 255]
        }
    }

    fn apply_bg(&mut self) {
        use nacre_core::settings::BarStyle;
        let p = prefs::get();
        let d = self.dpi;
        let flat = if p.menubar_glass && !p.menubar_material { Some(match p.menubar_style { BarStyle::Light => (248, 248, 250), BarStyle::Dark => (0, 0, 0) }) } else { None };
        if p.menubar_material {
            let look = {
                let mut s = prefs::get();
                s.blur = s.rail_blur;
                s.transparency = s.rail_transparency;
                nacre_core::settings::look(&s, sys::apps_dark(), s.dock_material)
            };
            if let Some((_, _, w)) = self.corner {
                // Mode pojok: kaca Liquid Glass seperti Deck (dengan tepi dan kilau), bentuk kapsul.
                let base = nacre_ui::glass::GlassParams::dark((w as f32, self.h as f32), self.h as f32 / 2.0);
                let gp = prefs::apply(base, &look);
                let _ = self.popup.panel.set_fringe(gp.liquify);
                let _ = self.popup.panel.set_params(gp);
                let _ = self.popup.panel.set_rect(0.0, 0.0, w as f32, self.h as f32);
                let _ = self.popup.panel.set_radius(self.h as f32 / 2.0);
                let _ = self.bg.SetOpacity(0.0);
                let _ = self.hair.SetOpacity(0.0);
                return;
            }
            let base = nacre_ui::glass::GlassParams::dark((self.screen.0 as f32, self.h as f32), 0.0);
            // Kaca Liquid Glass penuh selebar layar, tanpa garis tepi.
            let mut gp = prefs::apply(base, &look);
            gp.liquify = false;
            gp.rim.A = 0;
            gp.highlight = 1.0;
            let _ = self.popup.panel.set_fringe(false);
            let _ = self.popup.panel.set_params(gp);
            let _ = self.popup.panel.set_rect(0.0, 0.0, self.screen.0 as f32, self.h as f32);
            let _ = self.popup.panel.set_radius(0.0);
        } else {
            let _ = self.popup.panel.set_rect(0.0, 0.0, 0.0, 0.0);
        }
        let _ = self.bg.SetSize(Vector2 { X: self.screen.0 as f32, Y: self.h as f32 });
        match flat {
            Some((r, g, b)) => {
                if let Ok(br) = self.comp.CreateColorBrushWithColor(Color { A: 255, R: r, G: g, B: b }) {
                    let _ = self.bg.SetBrush(&br);
                }
                let _ = self.bg.SetOpacity(1.0);
            }
            None if !p.menubar_material => {
                if let Ok(br) = self.top_shade() {
                    let _ = self.bg.SetBrush(&br);
                    let _ = self.bg.SetOpacity(1.0);
                }
            }
            None => {
                let _ = self.bg.SetOpacity(0.0);
            }
        }
        let line = d.round().max(1.0);
        let _ = self.hair.SetSize(Vector2 { X: self.screen.0 as f32, Y: line });
        let _ = self.hair.SetOffset(Vector3 { X: 0.0, Y: self.h as f32 - line, Z: 0.0 });
        let hc = match (p.menubar_glass && !p.menubar_material, p.menubar_style) {
            _ if p.menubar_material => {
                None
            }
            (false, _) => None,
            (true, BarStyle::Light) => Some(Color { A: 34, R: 0, G: 0, B: 0 }),
            (true, BarStyle::Dark) => Some(Color { A: 40, R: 255, G: 255, B: 255 }),
        };
        match hc.and_then(|c| self.comp.CreateColorBrushWithColor(c).ok()) {
            Some(br) => {
                let _ = self.hair.SetBrush(&br);
                let _ = self.hair.SetOpacity(1.0);
            }
            None => {
                let _ = self.hair.SetOpacity(0.0);
            }
        }
    }

    fn top_shade(&self) -> windows_core::Result<windows::UI::Composition::CompositionLinearGradientBrush> {
        let g = self.comp.CreateLinearGradientBrush()?;
        g.SetStartPoint(Vector2 { X: 0.0, Y: 0.0 })?;
        g.SetEndPoint(Vector2 { X: 0.0, Y: 1.0 })?;
        let stops = g.ColorStops()?;
        for (o, a) in [(0.0, 40u8), (0.45, 18), (1.0, 0)] {
            stops.Append(&self.comp.CreateColorGradientStopWithOffsetAndColor(o, Color { A: a, R: 0, G: 0, B: 0 })?)?;
        }
        Ok(g)
    }

    fn update_text_color(&mut self) -> bool {
        use nacre_core::settings::BarStyle;
        let before = self.dark_text;
        let p = prefs::get();
        self.dark_text = p.menubar_glass && !p.menubar_material && p.menubar_style == BarStyle::Light;
        before != self.dark_text
    }

    fn recheck_color(&mut self) {
        if self.update_text_color() {
            self.cache.clear();
            self.render();
        }
    }

    fn apply_look(&mut self) {
        self.update_text_color();
        self.apply_bg();
        self.cache.clear();
    }

    fn reveal(&mut self) {
        if self.ah_shown {
            return;
        }
        log::line("menubar", &format!("reveal fs={} corner={:?}", self.fullscreen, self.corner));
        self.ah_shown = true;
        self.ah_out = 0;
        if let Some(ctl) = CTL.with(|c| c.borrow().as_ref().map(|c| c.hwnd)) {
            unsafe {
                let _ = KillTimer(Some(ctl), T_AH_OUT);
            }
        }
        self.start_res_anim();
        if !self.fullscreen {
            let root = &self.popup.surface_root();
            let from = self.hidden_offset();
            let _ = crate::motion::vec3(&self.comp, root, "Offset", Vector3 { X: 0.0, Y: from, Z: 0.0 }, Vector3 { X: 0.0, Y: 0.0, Z: 0.0 }, AH_IN_MS);
            self.popup.show();
            crate::topmost::request_raise();
        }
    }

    fn hidden_offset(&self) -> f32 {
        if self.corner.is_some() {
            self.h as f32 + 10.0 * self.dpi
        } else {
            -(self.h as f32)
        }
    }

    fn conceal(&mut self) {
        if !self.ah_shown {
            return;
        }
        self.ah_shown = false;
        self.ah_out = 0;
        self.close_menu();
        self.close_flyout();
        if let Some(t) = self.tip.as_ref() {
            t.hide();
        }
        self.start_res_anim();
        let root = &self.popup.surface_root();
        let to = self.hidden_offset();
        let _ = crate::motion::vec3(&self.comp, root, "Offset", Vector3 { X: 0.0, Y: 0.0, Z: 0.0 }, Vector3 { X: 0.0, Y: to, Z: 0.0 }, AH_OUT_MS);
        if let Some(ctl) = CTL.with(|c| c.borrow().as_ref().map(|c| c.hwnd)) {
            unsafe {
                SetTimer(Some(ctl), T_AH_OUT, AH_OUT_MS + 20, None);
            }
        }
    }

    fn start_res_anim(&mut self) {
        // Mengubah area kerja Windows bisa makan ~0,5 dtk; jalankan di thread lain
        // supaya animasi Rail tidak tersendat.
        self.ah_from = self.ah_res;
        self.ah_t0 = std::time::Instant::now();
        // Saat auto-hide aktif, Rail cukup menimpa jendela (tanpa mengubah ukuran jendela lain).
        let target = if self.ah_shown && !ah_on() { self.h } else { 0 };
        self.ah_res = target;
        appbar_async(self.popup.hwnd.0 as isize, self.screen.0, target);
    }

    fn res_step(&mut self) {
        let target = if self.ah_shown { self.h } else { 0 };
        let dur = if self.ah_shown { AH_IN_MS } else { AH_OUT_MS } as f32;
        let t = (self.ah_t0.elapsed().as_millis() as f32 / dur).clamp(0.0, 1.0);
        let e = 1.0 - (1.0 - t).powi(3);
        let v = (self.ah_from as f32 + (target - self.ah_from) as f32 * e).round() as i32;
        if v != self.ah_res || t >= 1.0 {
            self.ah_res = v;
            if v <= 0 {
                shell::appbar::release_top();
            } else {
                shell::appbar::reserve_top(self.popup.hwnd, self.screen.0, v);
            }
        }
        if t >= 1.0 {
            if let Some(ctl) = CTL.with(|c| c.borrow().as_ref().map(|c| c.hwnd)) {
                unsafe {
                    let _ = KillTimer(Some(ctl), T_AH_STEP);
                }
            }
        }
    }

    fn battery_label() -> Option<String> {
        let (pct, charging, secs) = sys::battery_time()?;
        let en = prefs::get().lang == nacre_core::i18n::Lang::En;
        let left = secs.map(|s| {
            let (h, m) = (s / 3600, (s % 3600) / 60);
            if h > 0 { format!("{h}h {m}m") } else { format!("{m}m") }
        });
        Some(match (charging, pct >= 100, left) {
            (true, true, _) => if en { format!("{pct}% · Fully charged") } else { format!("{pct}% · Terisi penuh") },
            (true, false, _) => if en { format!("{pct}% · Charging") } else { format!("{pct}% · Sedang dicas") },
            (false, _, Some(l)) => if en { format!("{pct}% · {l} remaining") } else { format!("{pct}% · sisa {l}") },
            (false, _, None) => if en { format!("{pct}% · Calculating time…") } else { format!("{pct}% · Menghitung sisa waktu…") },
        })
    }

    fn update_tip(&mut self) {
        let tray_tip = self.hover.and_then(|id| self.tray_at(id)).map(|i| i.tip).filter(|t| !t.trim().is_empty());
        let corner = self.corner.is_some();
        let status_tip = match self.hover {
            Some(BATT) => Self::battery_label(),
            Some(VOL) if corner => {
                let v = nacre_platform::audio::latest();
                Some(if v.muted { "Volume: mute".to_string() } else { format!("Volume: {}%", (v.level * 100.0).round() as i32) })
            }
            Some(WIFI) if corner => Some(match self.wifi {
                Some(true) => format!("Wi-Fi: {}", nacre_platform::wifi::ssid().unwrap_or_else(|| "terhubung".into())),
                _ => "Wi-Fi: tidak terhubung".to_string(),
            }),
            _ => None,
        };
        if (status_tip.is_some() || tray_tip.is_some()) && self.open.is_none() {
            let label = match tray_tip.map(|t| t.lines().next().unwrap_or("").to_string()).or(status_tip) {
                Some(l) => l,
                None => return,
            };
            let at = self.hover.unwrap_or(BATT);
            if self.tip.is_none() {
                self.tip = crate::dock::tooltip::Tooltip::new(&self.comp, self.dpi, self.screen.0).ok();
            }
            let cx = self.rect_of(at).map(|(x, w)| (x + w / 2.0) as i32).unwrap_or(self.screen.0 - 80);
            let (top, comp, gfx) = (self.h, self.comp.clone(), self.gfx.clone());
            let corner = self.corner;
            if let Some(t) = self.tip.as_mut() {
                let _ = match corner {
                    Some((x0, y0, _)) => t.show(&comp, &gfx, &label, x0 + cx, y0 - (4.0 * self.dpi) as i32),
                    None => t.show_below(&comp, &gfx, &label, cx, top),
                };
            }
        } else if let Some(t) = self.tip.as_ref() {
            t.hide();
        }
    }

    fn sync_autohide(&mut self) {
        if ah_on() {
            if self.ah_shown && self.open.is_none() {
                self.conceal();
            }
        } else if !self.ah_shown {
            self.reveal();
        }
    }

    /// Monitor eksternal dicolok/dicabut: ukuran layar berubah, posisi kapsul harus dihitung ulang.
    fn check_screen(&mut self) {
        let s = sys::screen_size();
        let dpi = unsafe { GetDpiForWindow(self.popup.hwnd) }.max(96) as f32 / 96.0 * prefs::ui_scale();
        if s == self.screen && (dpi - self.dpi).abs() < 0.01 {
            return;
        }
        nacre_platform::log::line("menubar", &format!("layar berubah {:?} -> {:?} dpi {}", self.screen, s, dpi));
        self.screen = s;
        self.dpi = dpi;
        self.cache.clear();
        self.close_menu();
        self.close_flyout();
        self.corner = None;
        self.h = 0;
        self.relayout();
        if ah_on() && self.ah_shown {
            self.conceal();
        }
        self.render();
    }

    fn autohide_tick(&mut self) {
        self.check_screen();
        if self.corner.is_some() && rail_metrics(self.dpi).0 != self.h {
            self.relayout();
            self.render();
        }
        if !ah_on() {
            return;
        }
        // Alt+Tab / geser 3 jari: jangan ubah apa pun selama pengalih jendela terbuka.
        if nacre_platform::wins::overlap::is_switcher(focus::foreground()) {
            return;
        }
        self.check_fullscreen();
        let mut pt = windows::Win32::Foundation::POINT::default();
        if unsafe { GetCursorPos(&mut pt) }.is_err() {
            return;
        }
        if let Some((cx, cy, _)) = self.corner {
            let pad = (16.0 * self.dpi) as i32;
            if !self.ah_shown {
                if !self.fullscreen && pt.y >= self.screen.1 - 2 && pt.x >= cx - pad {
                    self.reveal();
                }
                return;
            }
            let near = pt.x >= cx - pad && pt.y >= cy - pad;
            let ours = unsafe {
                let w = WindowFromPoint(pt);
                let mut pid = 0u32;
                if !w.0.is_null() {
                    GetWindowThreadProcessId(GetAncestor(w, GA_ROOT), Some(&mut pid));
                }
                pid == std::process::id() && pt.x > self.screen.0 / 2
            };
            // Panel tray ikut tertutup sendiri bila kursor meninggalkannya (kecuali menu klik kanan aplikasi masih terbuka).
            if self.flyout_open() {
                let r = self.flyout.as_ref().map(|p| p.rect).unwrap_or((0, 0, 0, 0));
                let m = pad;
                let inside = pt.x >= r.0 - m && pt.x < r.0 + r.2 + m && pt.y >= r.1 - m && pt.y < r.1 + r.3 + m;
                if inside || near || ours || context_menu_open() {
                    self.fly_out = 0;
                } else {
                    self.fly_out += 1;
                    if self.fly_out >= 12 {
                        self.fly_out = 0;
                        self.close_flyout();
                        self.render();
                    }
                }
            }
            if near || ours || self.open.is_some() || self.flyout_open() || shell_flyout_fg() {
                self.ah_out = 0;
            } else {
                self.ah_out += 1;
                if self.ah_out >= AH_HIDE_TICKS {
                    self.conceal();
                }
            }
            return;
        }
        if prefs::get().menubar_smart_hide && !covered_foreground() {
            self.ah_out = 0;
            self.reveal();
            return;
        }
        if pt.x < 0 || pt.x >= self.screen.0 {
            return;
        }
        if !self.ah_shown {
            // Aplikasi fullscreen: jangan muncul dari tepi atas; cukup lewat tombol Windows.
            if self.fullscreen {
                return;
            }
            if pt.y <= 0 {
                self.reveal();
            }
            return;
        }
        let near = pt.y < self.h + (8.0 * self.dpi) as i32;
        let ours = unsafe {
            let w = WindowFromPoint(pt);
            let mut pid = 0u32;
            if !w.0.is_null() {
                GetWindowThreadProcessId(GetAncestor(w, GA_ROOT), Some(&mut pid));
            }
            pid == std::process::id() && pt.y < self.screen.1 / 2
        };
        if near || ours || self.open.is_some() {
            self.ah_out = 0;
        } else {
            self.ah_out += 1;
            if self.ah_out >= AH_HIDE_TICKS {
                self.conceal();
            }
        }
    }

    fn check_fullscreen(&mut self) {
        let fg = focus::foreground();
        if fg == 0 || fg == self.popup.hwnd.0 as isize || nacre_platform::wins::overlap::is_switcher(fg) {
            return;
        }
        let full = is_fullscreen(fg) && !prefs::get().above_fullscreen;
        if full != self.fullscreen {
            self.fullscreen = full;
            if full {
                self.close_menu();
                self.popup.hide();
            } else if self.ah_shown {
                self.popup.show();
                crate::topmost::request_raise();
            }
        }
    }

    fn refresh_app(&mut self) -> bool {
        self.refresh_app_for(focus::foreground())
    }

    fn refresh_app_for(&mut self, fg: isize) -> bool {
        if fg == 0 || fg == self.popup.hwnd.0 as isize {
            return false;
        }
        let mut changed = false;
        self.check_fullscreen();
        let native = nacre_platform::menus::read(fg).unwrap_or_default();
        let titles = |v: &Vec<nacre_platform::menus::Top>| v.iter().map(|t| t.title.clone()).collect::<Vec<_>>();
        if titles(&native) != titles(&self.native) {
            changed = true;
        }
        self.native = native;
        self.native_hwnd = fg;
        let wins = enumerate::list(std::process::id());
        let name = wins
            .iter()
            .find(|w| w.hwnd == fg)
            .map(|w| {
                if !w.name.is_empty() {
                    w.name.clone()
                } else {
                    nacre_core::dock::items::name_from_path(&w.path)
                }
            })
            .filter(|n| !n.is_empty());
        let Some(name) = name else { return changed };
        if name == self.app {
            return changed;
        }
        self.app = name;
        true
    }

    fn text_w(&self, s: &str, weight: i32) -> f32 {
        let px = (13.0 * self.dpi).round() as i32;
        measure(s, &Style { px, weight }).0 as f32 + 2.0
    }

    fn relayout(&mut self) {
        let d = self.dpi;
        let pad = 11.0 * d;
        let t = |s: &str, wgt: i32| self.text_w(s, wgt) + 2.0 * pad;
        let mut left = vec![
            LItem { id: LOGO, width: self.h as f32 * 1.05 },
            LItem { id: APP, width: t(&self.app_label(), APP_W) },
        ];
        if self.native.is_empty() {
            left.push(LItem { id: M_FILE, width: t(&mt("Berkas"), BAR_W) });
            left.push(LItem { id: M_EDIT, width: t(&mt("Edit"), BAR_W) });
            left.push(LItem { id: M_VIEW, width: t(&mt("Menu:Tampilan"), BAR_W) });
            left.push(LItem { id: M_WIN, width: t(&mt("Menu:Jendela"), BAR_W) });
            left.push(LItem { id: M_HELP, width: t(&mt("Bantuan"), BAR_W) });
        } else {
            for (i, top) in self.native.iter().enumerate().take(20) {
                left.push(LItem { id: NATIVE0 + i as u32, width: t(&top.title, BAR_W) });
            }
        }
        let icon_w = 30.0 * d;
        if corner_on() {
            // Seperti pojok taskbar Windows 11: [^] [wifi speaker baterai] [jam/tanggal].
            let (tm, dt) = self.clock_lines();
            let cw = self.small_w(&tm, 12.0, BAR_W).max(self.small_w(&dt, 12.0, BAR_W)) + 2.0 * pad;
            let mut right = vec![LItem { id: CLOCK, width: cw }];
            // Ikon status terpisah supaya hover-nya sendiri-sendiri; kliknya tetap membuka Quick Settings.
            if let Some(bw) = self.batt_w() {
                right.push(LItem { id: BATT, width: bw + 8.0 * d });
            }
            if nacre_platform::audio::latest().available {
                right.push(LItem { id: VOL, width: 30.0 * d });
            }
            if self.wifi.is_some() {
                right.push(LItem { id: WIFI, width: 30.0 * d });
            }
            if !self.tray.is_empty() {
                right.push(LItem { id: TRAY_BTN, width: 28.0 * d });
            }
            let pill_w = right.iter().map(|i| i.width).sum::<f32>() + 2.0 * 8.0 * d;
            let (l, r) = lay::layout(pill_w, 6.0 * d, &[], &right, None);
            self.left = l;
            self.right = r;
            let (h, m) = rail_metrics(d);
            let w = pill_w.ceil() as i32;
            let (x, y) = (self.screen.0 - w - m, self.screen.1 - h - m);
            if self.corner != Some((x, y, w)) || self.h != h {
                self.corner = Some((x, y, w));
                self.h = h;
                let _ = self.popup.set_rect(x, y, w, h);
                self.apply_bg();
            }
            return;
        }
        let mut right = vec![
            LItem { id: CLOCK, width: t(&self.clock_text(), BAR_W) },
            LItem { id: CTRL, width: icon_w },
        ];
        if !corner_on() {
            right.push(LItem { id: SEARCH, width: icon_w });
        }
        if prefs::get().stage {
            right.push(LItem { id: STAGEI, width: icon_w });
        }
        if self.wifi.is_some() {
            right.push(LItem { id: WIFI, width: icon_w });
        }
        if let Some((pct, _)) = sys::battery() {
            let extra = if prefs::get().battery_pct == 2 { self.small_w(&format!("{pct}%"), 12.0, BAR_W) + 5.0 * d } else { 0.0 };
            right.push(LItem { id: BATT, width: 40.0 * d + extra });
        }
        // Seperti Windows: ikon tray dilipat di balik tombol panah.
        if !self.tray.is_empty() {
            right.push(LItem { id: TRAY_BTN, width: 28.0 * d });
        }
        let corner = corner_on();
        if corner {
            left.clear();
        }
        let pill_w = right.iter().map(|i| i.width).sum::<f32>() + 2.0 * 12.0 * d;
        let gap = if corner { None } else { services::notch().map(|(l, r, _)| Gap { left: l as f32, right: r as f32 }) };
        let lay_w = if corner { pill_w } else { self.screen.0 as f32 };
        let (l, r) = lay::layout(lay_w, 6.0 * d, &left, &right, gap);
        self.left = l;
        self.right = r;
        if corner {
            // Pil kaca kecil di pojok kanan bawah, seperti sisi kanan taskbar Windows.
            let h = (36.0 * d).round() as i32;
            let m = (8.0 * d).round() as i32;
            let w = pill_w.ceil() as i32;
            let (x, y) = (self.screen.0 - w - m, self.screen.1 - h - m);
            if self.corner != Some((x, y, w)) || self.h != h {
                self.corner = Some((x, y, w));
                self.h = h;
                let _ = self.popup.set_rect(x, y, w, h);
                self.apply_bg();
            }
            return;
        }
        if self.corner.take().is_some() {
            self.h = 0;
        }
        let want_h = services::notch().map(|(_, _, h)| h).unwrap_or((26.0 * d).round() as i32);
        if want_h != self.h {
            self.h = want_h;
            let _ = self.popup.set_rect(0, 0, self.screen.0, self.h);
            let _ = self.popup.panel.set_radius(0.0);
            self.apply_bg();
            if self.ah_shown && !ah_on() {
                self.ah_res = self.h;
                shell::appbar::reserve_top(self.popup.hwnd, self.screen.0, self.h);
            }
        }
    }

    fn app_label(&self) -> String {
        if self.app.is_empty() {
            "CustomDock".to_string()
        } else {
            self.app.clone()
        }
    }

    fn clock_lines(&self) -> (String, String) {
        let n = time::local_now();
        let tm = if prefs::get().clock_24h {
            format!("{:02}.{:02}", n.hour, n.minute)
        } else {
            let (h12, ap) = (if n.hour % 12 == 0 { 12 } else { n.hour % 12 }, if n.hour < 12 { "AM" } else { "PM" });
            format!("{h12}:{:02} {ap}", n.minute)
        };
        (tm, format!("{:02}/{:02}/{}", n.day, n.month, n.year))
    }

    fn status_w(&self) -> f32 {
        let d = self.dpi;
        let mut w = 12.0 * d;
        if self.wifi.is_some() {
            w += 24.0 * d;
        }
        if nacre_platform::audio::latest().available {
            w += 24.0 * d;
        }
        if let Some(w0) = self.batt_w() {
            w += w0;
        }
        w
    }

    fn batt_w(&self) -> Option<f32> {
        let (pct, _) = sys::battery()?;
        let extra = if prefs::get().battery_pct == 2 { self.small_w(&format!("{pct}%"), 12.0, BAR_W) + 5.0 * self.dpi } else { 0.0 };
        Some(36.0 * self.dpi + extra)
    }


    fn draw_status(&mut self, x: f32) {
        let d = self.dpi;
        let mut cx = x + 6.0 * d;
        if self.wifi.is_some() {
            self.draw_wifi(cx + 12.0 * d);
            cx += 24.0 * d;
        }
        let v = nacre_platform::audio::latest();
        if v.available {
            let g = if v.muted || v.level <= 0.0 { "\u{E74F}" } else if v.level < 0.34 { "\u{E993}" } else if v.level < 0.67 { "\u{E994}" } else { "\u{E995}" };
            self.draw_glyph(g, cx + 12.0 * d, 15.0, 0.95);
            cx += 24.0 * d;
        }
        if let (Some(bw), false) = (self.batt_w(), self.shadow) {
            self.draw_battery(cx, bw);
        }
    }

    fn clock_text(&self) -> String {
        let n = time::local_now();
        let lang = prefs::get().lang;
        let wd = nacre_core::island::calendar::weekday(n.year, n.month, n.day) as usize;
        let days = nacre_core::i18n::weekday_labels(lang, false, nacre_core::island::calendar::weekday_labels(false));
        let months_en = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        let months_id = ["Jan", "Feb", "Mar", "Apr", "Mei", "Jun", "Jul", "Agu", "Sep", "Okt", "Nov", "Des"];
        let mon = match lang {
            nacre_core::i18n::Lang::En => months_en[(n.month - 1) as usize % 12],
            nacre_core::i18n::Lang::Id => months_id[(n.month - 1) as usize % 12],
        };
        let time = if prefs::get().clock_24h {
            format!("{:02}.{:02}", n.hour, n.minute)
        } else {
            let (h12, ap) = (if n.hour % 12 == 0 { 12 } else { n.hour % 12 }, if n.hour < 12 { "AM" } else { "PM" });
            format!("{h12}:{:02} {ap}", n.minute)
        };
        match lang {
            nacre_core::i18n::Lang::En => format!("{} {} {}  {}", days[wd % 7], mon, n.day, time),
            nacre_core::i18n::Lang::Id => format!("{} {} {}  {}", days[wd % 7], n.day, mon, time),
        }
    }

    fn sprite(&self, parent: &ContainerVisual, s: &CompositionDrawingSurface, x: f32, y: f32, w: f32, h: f32, op: f32) {
        let go = || -> windows_core::Result<()> {
            let b = self.comp.CreateSurfaceBrushWithSurface(s)?;
            b.SetStretch(CompositionStretch::None)?;
            let v = self.comp.CreateSpriteVisual()?;
            v.SetBrush(&b)?;
            v.SetSize(Vector2 { X: w, Y: h })?;
            let (dy, om) = if self.shadow { (1.0 * self.dpi, 0.38) } else { (0.0, 1.0) };
            v.SetOffset(Vector3 { X: x, Y: y + dy, Z: 0.0 })?;
            v.SetOpacity(op * om)?;
            parent.Children()?.InsertAtTop(&v)
        };
        let _ = go();
    }

    fn draw_text(&mut self, text: &str, weight: i32, x: f32, w: f32, op: f32) {
        let px = (13.0 * self.dpi).round() as i32;
        let rgb = self.fg();
        let key = format!("t|{px}|{weight}|{rgb:?}|{text}");
        let hit = self.cache.get(&key).cloned();
        let (surf, tw, th) = match hit {
            Some(v) => v,
            None => {
                let style = Style { px, weight };
                let (tw, th) = measure(text, &style);
                let tw = tw + 2;
                let Some(bmp) = render(text, &style, tw, th) else { return };
                let data = recolor(&bmp, rgb);
                let Ok(s) = self.gfx.surface_from_bgra(bmp.w, bmp.h, &data) else { return };
                if self.cache.len() > 120 {
                    self.cache.clear();
                }
                let v = (s, tw, th);
                self.cache.insert(key, v.clone());
                v
            }
        };
        let y = (self.h as f32 - th as f32) / 2.0;
        let layer = self.layer.clone();
        self.sprite(&layer, &surf, x + (w - tw as f32) / 2.0, y, tw as f32, th as f32, op);
    }

    fn draw_glyph(&mut self, glyph: &str, cx: f32, size: f32, op: f32) {
        let px = (size * self.dpi).round() as i32;
        let rgb = self.fg();
        if let Some(sym) = nacre_core::symbols::for_glyph(glyph) {
            let n = (size * 1.25 * self.dpi).round().max(8.0) as u32;
            let key = format!("sym|{sym:?}|{n}|{rgb:?}");
            let (surf, w, h) = match self.cache.get(&key).cloned() {
                Some(v) => v,
                None => {
                    let data = nacre_core::symbols::render(sym, n, rgb);
                    let Ok(s) = self.gfx.surface_from_bgra(n, n, &data) else { return };
                    let v = (s, n as i32, n as i32);
                    self.cache.insert(key, v.clone());
                    v
                }
            };
            let layer = self.layer.clone();
            self.sprite(&layer, &surf, (cx - w as f32 / 2.0).round(), ((self.h as f32 - h as f32) / 2.0).round(), w as f32, h as f32, op);
            return;
        }
        let key = format!("g|{px}|{rgb:?}|{glyph}");
        let hit = self.cache.get(&key).cloned();
        let (surf, w, h) = match hit {
            Some(v) => v,
            None => {
                let b = (size * 1.6 * self.dpi).round() as i32;
                let Some(bmp) = render_face(glyph, &Style { px, weight: 400 }, ICON_FACE, b, b) else { return };
                let (w, h) = (bmp.w as usize, bmp.h as usize);
                let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0usize, 0usize);
                for y in 0..h {
                    for x in 0..w {
                        if bmp.bgra[(y * w + x) * 4 + 3] > 8 {
                            x0 = x0.min(x);
                            x1 = x1.max(x);
                            y0 = y0.min(y);
                            y1 = y1.max(y);
                        }
                    }
                }
                if x0 > x1 {
                    return;
                }
                let (gw, gh) = (x1 - x0 + 1, y1 - y0 + 1);
                let mut data = vec![0u8; gw * gh * 4];
                let rec = recolor(&bmp, rgb);
                for y in 0..gh {
                    for x in 0..gw {
                        let s = ((y0 + y) * w + x0 + x) * 4;
                        let d = (y * gw + x) * 4;
                        data[d..d + 4].copy_from_slice(&rec[s..s + 4]);
                    }
                }
                let Ok(s) = self.gfx.surface_from_bgra(gw as u32, gh as u32, &data) else { return };
                let v = (s, gw as i32, gh as i32);
                self.cache.insert(key, v.clone());
                v
            }
        };
        let layer = self.layer.clone();
        self.sprite(&layer, &surf, cx - w as f32 / 2.0, (self.h as f32 - h as f32) / 2.0, w as f32, h as f32, op);
    }

    fn draw_logo(&mut self, x: f32, w: f32) {
        let size = (self.h as f32 * 0.54).round() as u32;
        let rgb = self.fg();
        let custom = crate::menulogo::path().is_some();
        let size = if custom { (self.h as f32 * 0.5).round() as u32 } else { size };
        let key = format!("logo|{size}|{rgb:?}|{}|{custom}", crate::menulogo::generation());
        let hit = self.cache.get(&key).cloned();
        let (surf, lw, lh) = match hit {
            Some(v) => v,
            None => {
                let (bw, bh, data) = match crate::menulogo::bitmap(size, rgb).filter(|_| custom) {
                    Some(b) => (b.w, b.h, b.bgra),
                    None => (size, size, nacre_core::logo::render_badge(size, rgb)),
                };
                let Ok(s) = self.gfx.surface_from_bgra(bw, bh, &data) else { return };
                let v = (s, bw as i32, bh as i32);
                self.cache.insert(key, v.clone());
                v
            }
        };
        let layer = self.layer.clone();
        self.sprite(&layer, &surf, x + (w - lw as f32) / 2.0, (self.h as f32 - lh as f32) / 2.0, lw as f32, lh as f32, 1.0);
    }

    fn small_w(&self, text: &str, size: f32, weight: i32) -> f32 {
        measure(text, &Style { px: (size * self.dpi).round() as i32, weight }).0 as f32 + 2.0
    }

    fn small_text(&mut self, text: &str, size: f32, weight: i32, rgb: [u8; 3], cx: f32, cy: f32, op: f32) {
        let px = (size * self.dpi).round() as i32;
        let key = format!("st|{px}|{weight}|{rgb:?}|{text}");
        let (surf, tw, th) = match self.cache.get(&key).cloned() {
            Some(v) => v,
            None => {
                let style = Style { px, weight };
                let (tw, th) = measure(text, &style);
                let Some(bmp) = render(text, &style, tw + 2, th) else { return };
                let Ok(s) = self.gfx.surface_from_bgra(bmp.w, bmp.h, &recolor(&bmp, rgb)) else { return };
                let v = (s, bmp.w as i32, bmp.h as i32);
                self.cache.insert(key, v.clone());
                v
            }
        };
        let layer = self.layer.clone();
        self.sprite(&layer, &surf, (cx - tw as f32 / 2.0).round(), (cy - th as f32 / 2.0).round(), tw as f32, th as f32, op);
    }

    fn draw_wifi(&mut self, cx: f32) {
        let d = self.dpi;
        let rgb = self.fg();
        let n = (17.0 * d).round() as u32;
        let level = if self.wifi == Some(false) { 0 } else { nacre_core::logo::wifi_level(self.wifi_bars) };
        let key = format!("wifi|{n}|{level}|{rgb:?}");
        let (surf, sw, sh) = match self.cache.get(&key).cloned() {
            Some(v) => v,
            None => {
                let data = nacre_core::logo::render_wifi(n, level, rgb, 0.32);
                let Ok(s) = self.gfx.surface_from_bgra(n, n, &data) else { return };
                let v = (s, n as i32, n as i32);
                self.cache.insert(key, v.clone());
                v
            }
        };
        let layer = self.layer.clone();
        let y = ((self.h as f32 - sh as f32) / 2.0 + 1.0 * d).round();
        self.sprite(&layer, &surf, (cx - sw as f32 / 2.0).round(), y, sw as f32, sh as f32, 1.0);
    }

    fn draw_battery(&mut self, x: f32, w: f32) {
        let Some((pct, charging)) = sys::battery() else { return };
        let d = self.dpi;
        let rgb = self.fg();
        let mode = prefs::get().battery_pct;
        let inside = mode == 1;
        let (bw, bh) = if inside { ((29.0 * d).round() as u32, (14.0 * d).round() as u32) } else { ((25.0 * d).round() as u32, (12.0 * d).round() as u32) };
        let fill = if charging { [52, 199, 89] } else if pct <= 10 { [255, 69, 58] } else { rgb };
        let key = format!("batt3|{bw}|{bh}|{pct}|{rgb:?}|{fill:?}|{inside}");
        let (surf, sw, sh) = match self.cache.get(&key).cloned() {
            Some(v) => v,
            None => {
                let mut data = nacre_core::logo::render_battery_pill(bw, bh, pct, rgb, fill, inside);
                if inside {
                    let style = Style { px: (9.5 * d).round() as i32, weight: BAR_W };
                    let label = pct.to_string();
                    let (tw, th) = measure(&label, &style);
                    if let Some(tb) = render(&label, &style, tw + 4, th + 2) {
                        knock_out(&mut data, bw, bh, &tb);
                    }
                }
                let Ok(s) = self.gfx.surface_from_bgra(bw, bh, &data) else { return };
                let v = (s, bw as i32, bh as i32);
                self.cache.insert(key, v.clone());
                v
            }
        };
        let text_w = if mode == 2 { self.small_w(&format!("{pct}%"), 12.0, BAR_W) + 5.0 * d } else { 0.0 };
        let bx = (x + text_w + (w - text_w - sw as f32) / 2.0).round();
        let by = ((self.h as f32 - sh as f32) / 2.0).round();
        let layer = self.layer.clone();
        self.sprite(&layer, &surf, bx, by, sw as f32, sh as f32, 1.0);
        let cy = self.h as f32 / 2.0;
        match mode {
            2 => {
                self.small_text(&format!("{pct}%"), 12.0, BAR_W, rgb, x + text_w / 2.0, cy, 0.95);
            }
            _ => {}
        }
    }

    fn rect(&self, parent: &ContainerVisual, x: f32, y: f32, w: f32, h: f32, r: f32, c: Color) {
        let go = || -> windows_core::Result<()> {
            let v = self.comp.CreateSpriteVisual()?;
            v.SetSize(Vector2 { X: w, Y: h })?;
            v.SetOffset(Vector3 { X: x, Y: y, Z: 0.0 })?;
            v.SetBrush(&self.comp.CreateColorBrushWithColor(c)?)?;
            if r > 0.0 {
                let g = self.comp.CreateRoundedRectangleGeometry()?;
                g.SetSize(Vector2 { X: w, Y: h })?;
                g.SetCornerRadius(Vector2 { X: r, Y: r })?;
                let clip = self.comp.CreateGeometricClip()?;
                clip.SetGeometry(&g)?;
                v.SetClip(&clip)?;
            }
            parent.Children()?.InsertAtTop(&v)
        };
        let _ = go();
    }

    fn draw_items(&mut self) {
        let lefts = self.left.clone();
        let rights = self.right.clone();
        let (app, clock) = (self.app_label(), self.clock_text());
        for p in &lefts {
            match p.id {
                LOGO => self.draw_logo(p.x, p.w),
                APP => self.draw_text(&app, APP_W, p.x, p.w, 1.0),
                M_FILE => self.draw_text(&mt("Berkas"), BAR_W, p.x, p.w, 1.0),
                M_EDIT => self.draw_text(&mt("Edit"), BAR_W, p.x, p.w, 1.0),
                M_VIEW => self.draw_text(&mt("Menu:Tampilan"), BAR_W, p.x, p.w, 1.0),
                M_WIN => self.draw_text(&mt("Menu:Jendela"), BAR_W, p.x, p.w, 1.0),
                M_HELP => self.draw_text(&mt("Bantuan"), BAR_W, p.x, p.w, 1.0),
                n if n >= NATIVE0 && n < NATIVE0 + 20 => {
                    let title = self.native.get((n - NATIVE0) as usize).map(|t| t.title.clone()).unwrap_or_default();
                    self.draw_text(&title, BAR_W, p.x, p.w, 1.0)
                }
                _ => {}
            }
        }
        for p in &rights {
            let cx = p.x + p.w / 2.0;
            match p.id {
                CLOCK if self.corner.is_some() => {
                    let (tm, dt) = self.clock_lines();
                    let (rgb, h) = (self.fg(), self.h as f32);
                    self.small_text(&tm, 12.0, BAR_W, rgb, cx, h / 2.0 - 7.5 * self.dpi, 1.0);
                    self.small_text(&dt, 12.0, BAR_W, rgb, cx, h / 2.0 + 8.0 * self.dpi, 1.0);
                }
                CLOCK => self.draw_text(&clock, BAR_W, p.x, p.w, 1.0),
                STATUS => self.draw_status(p.x),
                CTRL => self.draw_glyph("\u{E9E9}", cx, 13.0, 0.95),
                SEARCH => self.draw_glyph("\u{E721}", cx, 13.0, 0.95),
                STAGEI => self.draw_glyph("\u{E8A0}", cx, 13.0, 0.95),
                WIFI => self.draw_wifi(cx),
                BATT => {
                    if !self.shadow {
                        self.draw_battery(p.x, p.w)
                    }
                }
                VOL => {
                    let v = nacre_platform::audio::latest();
                    let g = if v.muted || v.level <= 0.0 { "\u{E74F}" } else if v.level < 0.34 { "\u{E993}" } else if v.level < 0.67 { "\u{E994}" } else { "\u{E995}" };
                    self.draw_glyph(g, cx, 15.0, 0.95);
                }
                TRAY_BTN => self.draw_glyph(if self.flyout_open() { "\u{E70D}" } else { "\u{E70E}" }, cx, 11.0, 0.95),
                n if n >= TRAY0 && n < TRAY0 + TRAY_MAX as u32 => self.draw_tray((n - TRAY0) as usize, cx),
                _ => {}
            }
        }
    }

    fn render(&mut self) {
        if let Ok(k) = self.layer.Children() {
            let _ = k.RemoveAll();
        }
        self.update_text_color();
        if !self.dark_text && !prefs::get().menubar_glass && !prefs::get().menubar_material {
            self.shadow = true;
            self.draw_items();
            self.shadow = false;
        }
        self.draw_items();
        self.update_capsule();
    }

    fn draw_tray(&mut self, i: usize, cx: f32) {
        let Some(icon) = self.tray.get(i).cloned() else { return };
        let px = (16.0 * self.dpi).round() as i32;
        let key = format!("tray:{}:{px}", icon.hicon);
        if !self.cache.contains_key(&key) {
            let Some(bgra) = crate::tray::bitmap(icon.hicon, px) else { return };
            let Ok(s) = self.gfx.surface_from_bgra(px as u32, px as u32, &bgra) else { return };
            self.cache.insert(key.clone(), (s, px, px));
        }
        let Some((s, w, h)) = self.cache.get(&key).cloned() else { return };
        let y = ((self.h - h) / 2) as f32;
        let layer = self.layer.clone();
        self.sprite(&layer, &s, (cx - w as f32 / 2.0).round(), y, w as f32, h as f32, 1.0);
    }

    fn flyout_open(&self) -> bool {
        self.flyout.as_ref().map(|p| p.is_visible()).unwrap_or(false)
    }

    fn close_flyout(&mut self) {
        if let Some(p) = self.flyout.as_ref() {
            p.hide();
        }
        if let Some(t) = self.tip.as_ref() {
            t.hide();
        }
    }

    fn open_flyout(&mut self) {
        self.tray = crate::tray::icons();
        log::line("tray", &format!("flyout buka: {} ikon", self.tray.len()));
        if self.tray.is_empty() {
            return;
        }
        let d = self.dpi;
        if self.flyout.is_none() {
            let glass = prefs::glass((10.0, 10.0), 16.0 * d, prefs::Role::Dock);
            self.flyout = Popup::new(&self.comp, w!("AbraTrayFlyout"), Some(flyout_proc), WINDOW_EX_STYLE(0), glass).ok();
        }
        let n = self.tray.len();
        let cols = n.min(5);
        let rows = n.div_ceil(cols);
        let (cell, pad) = (40.0 * d, 10.0 * d);
        let (w, h) = (cols as f32 * cell + 2.0 * pad, rows as f32 * cell + 2.0 * pad);
        let (bx, bw) = self.rect_of(TRAY_BTN).unwrap_or((0.0, 0.0));
        let (ox, oy) = self.corner.map(|(x, y, _)| (x, y)).unwrap_or((0, 0));
        let center = ox as f32 + bx + bw / 2.0;
        let x = (center - w / 2.0).clamp(8.0 * d, self.screen.0 as f32 - w - 8.0 * d) as i32;
        let y = if self.corner.is_some() { oy - h as i32 - (8.0 * d) as i32 } else { self.h + (6.0 * d) as i32 };
        let px = (20.0 * d).round() as i32;
        let mut cells = vec![];
        let mut sprites = vec![];
        for (i, icon) in self.tray.iter().enumerate() {
            let (cx, cy) = (pad + (i % cols) as f32 * cell, pad + (i / cols) as f32 * cell);
            cells.push((cx, cy, cell, i));
            let key = format!("tray:{}:{px}", icon.hicon);
            if !self.cache.contains_key(&key) {
                if let Some(bgra) = crate::tray::bitmap(icon.hicon, px) {
                    if let Ok(s) = self.gfx.surface_from_bgra(px as u32, px as u32, &bgra) {
                        self.cache.insert(key.clone(), (s, px, px));
                    }
                }
            }
            if let Some((s, _, _)) = self.cache.get(&key) {
                sprites.push((s.clone(), cx + (cell - px as f32) / 2.0, cy + (cell - px as f32) / 2.0));
            }
        }
        self.fly_cells = cells;
        let comp = self.comp.clone();
        let Some(p) = self.flyout.as_mut() else { return };
        let _ = p.set_rect(x, y, w as i32, h as i32);
        let _ = p.panel.set_radius(16.0 * d);
        if let Ok(k) = p.content.Children() {
            let _ = k.RemoveAll();
        }
        for (s, sx, sy) in sprites {
            let go = || -> windows_core::Result<()> {
                let b = comp.CreateSurfaceBrushWithSurface(&s)?;
                b.SetStretch(CompositionStretch::None)?;
                let v = comp.CreateSpriteVisual()?;
                v.SetBrush(&b)?;
                v.SetSize(Vector2 { X: px as f32, Y: px as f32 })?;
                v.SetOffset(Vector3 { X: sx.round(), Y: sy.round(), Z: 0.0 })?;
                p.content.Children()?.InsertAtTop(&v)
            };
            let _ = go();
        }
        p.show();
        crate::topmost::request_raise();
    }

    fn flyout_hit(&self, x: f32, y: f32) -> Option<(crate::tray::Icon, f32, f32)> {
        self.fly_cells
            .iter()
            .find(|(cx, cy, s, _)| x >= *cx && x < cx + s && y >= *cy && y < cy + s)
            .and_then(|(cx, _, s, i)| self.tray.get(*i).cloned().map(|ic| (ic, *cx, *s)))
    }

    fn tray_at(&self, id: u32) -> Option<crate::tray::Icon> {
        if id >= TRAY0 && id < TRAY0 + TRAY_MAX as u32 {
            self.tray.get((id - TRAY0) as usize).cloned()
        } else {
            None
        }
    }

    fn rect_of(&self, id: u32) -> Option<(f32, f32)> {
        self.left.iter().chain(self.right.iter()).find(|p| p.id == id).map(|p| (p.x, p.w))
    }

    fn update_capsule(&self) {
        use windows::Foundation::TimeSpan;
        use windows::UI::Composition::{CompositionObject, CompositionEasingFunction};
        use windows_core::{Interface, HSTRING};
        let cc = if crate::controlcenter::is_open() { Some(CTRL) } else { None };
        let active = self.open.or(cc).or(self.hover).filter(|id| *id != CLOCK || self.open.is_none());
        let d = self.dpi;
        let ms = |v: u32| TimeSpan { Duration: v as i64 * 10_000 };
        match active.and_then(|id| self.rect_of(id)) {
            Some((x, w)) => {
                let (ch, cy) = (self.h as f32 - 4.0 * d, 2.0 * d);
                let nw = w - 4.0 * d;
                let to = Vector3 { X: x + 2.0 * d, Y: cy, Z: 0.0 };
                let visible = self.capsule.Opacity().unwrap_or(0.0) > 0.5;
                let (old_off, old_w) = (self.capsule.Offset().unwrap_or(to), self.capsule.Size().map(|s| s.X).unwrap_or(nw));
                let moved = visible && ((old_off.X - to.X).abs() > 0.5 || (old_w - nw).abs() > 0.5);
                let _ = self.capsule.SetSize(Vector2 { X: nw, Y: ch });
                let _ = self.capsule.SetCenterPoint(Vector3 { X: nw / 2.0, Y: ch / 2.0, Z: 0.0 });
                let _ = self.capsule.SetOffset(to);
                let rgb = self.fg();
                let (a_top, a_bot) = if self.open.is_some() { (76u8, 46u8) } else { (52u8, 26u8) };
                if let Ok(gb) = self.comp.CreateLinearGradientBrush() {
                    let _ = gb.SetStartPoint(Vector2 { X: 0.0, Y: 0.0 });
                    let _ = gb.SetEndPoint(Vector2 { X: 0.0, Y: 1.0 });
                    if let (Ok(s0), Ok(s1), Ok(stops)) = (
                        self.comp.CreateColorGradientStopWithOffsetAndColor(0.0, Color { A: a_top, R: rgb[0], G: rgb[1], B: rgb[2] }),
                        self.comp.CreateColorGradientStopWithOffsetAndColor(1.0, Color { A: a_bot, R: rgb[0], G: rgb[1], B: rgb[2] }),
                        gb.ColorStops(),
                    ) {
                        let _ = stops.Append(&s0);
                        let _ = stops.Append(&s1);
                        let _ = self.capsule.SetBrush(&gb);
                    }
                }
                if let Ok(g) = self.comp.CreateRoundedRectangleGeometry() {
                    let _ = g.SetSize(Vector2 { X: nw, Y: ch });
                    let _ = g.SetCornerRadius(Vector2 { X: ch / 2.0, Y: ch / 2.0 });
                    if let Ok(clip) = self.comp.CreateGeometricClip() {
                        let _ = clip.SetGeometry(&g);
                        let _ = self.capsule.SetClip(&clip);
                    }
                }
                let ease = || -> windows_core::Result<CompositionEasingFunction> {
                    self.comp.CreateCubicBezierEasingFunction(Vector2 { X: 0.2, Y: 0.0 }, Vector2 { X: 0.0, Y: 1.0 })?.cast()
                };
                if !visible {
                    let go = || -> windows_core::Result<()> {
                        let e = ease()?;
                        let o = self.comp.CreateScalarKeyFrameAnimation()?;
                        o.InsertKeyFrame(0.0, 0.0)?;
                        o.InsertKeyFrameWithEasingFunction(1.0, 1.0, &e)?;
                        o.SetDuration(ms(150))?;
                        self.capsule.cast::<CompositionObject>()?.StartAnimation(&HSTRING::from("Opacity"), &o)?;
                        let sc = self.comp.CreateVector3KeyFrameAnimation()?;
                        sc.InsertKeyFrame(0.0, Vector3 { X: 0.90, Y: 0.86, Z: 1.0 })?;
                        sc.InsertKeyFrameWithEasingFunction(1.0, Vector3 { X: 1.0, Y: 1.0, Z: 1.0 }, &e)?;
                        sc.SetDuration(ms(190))?;
                        self.capsule.cast::<CompositionObject>()?.StartAnimation(&HSTRING::from("Scale"), &sc)
                    };
                    let _ = go();
                } else if moved {
                    let from_x = old_off.X + old_w / 2.0 - nw / 2.0;
                    let _ = crate::motion::vec3(&self.comp, &self.capsule, "Offset", Vector3 { X: from_x, Y: cy, Z: 0.0 }, to, crate::motion::SWITCHER_MOVE_MS + 50);
                    let _ = crate::motion::vec3(&self.comp, &self.capsule, "Scale", Vector3 { X: (old_w / nw).clamp(0.5, 2.0), Y: 1.0, Z: 1.0 }, Vector3 { X: 1.0, Y: 1.0, Z: 1.0 }, crate::motion::SWITCHER_MOVE_MS + 50);
                }
                let _ = self.capsule.SetOpacity(1.0);
            }
            None => {
                if self.capsule.Opacity().unwrap_or(0.0) > 0.01 {
                    let _ = crate::motion::scalar(&self.comp, &self.capsule, "Opacity", 1.0, 0.0, 150);
                    let _ = crate::motion::vec3(&self.comp, &self.capsule, "Scale", Vector3 { X: 1.0, Y: 1.0, Z: 1.0 }, Vector3 { X: 0.94, Y: 0.9, Z: 1.0 }, 150);
                    let _ = self.capsule.SetOpacity(0.0);
                }
            }
        }
    }

    fn press_feedback(&self) {
        use windows::Foundation::TimeSpan;
        use windows::UI::Composition::{CompositionEasingFunction, CompositionObject};
        use windows_core::{Interface, HSTRING};
        if self.capsule.Opacity().unwrap_or(0.0) < 0.5 {
            return;
        }
        let go = || -> windows_core::Result<()> {
            let e: CompositionEasingFunction = self.comp.CreateCubicBezierEasingFunction(Vector2 { X: 0.2, Y: 0.0 }, Vector2 { X: 0.0, Y: 1.0 })?.cast()?;
            let sc = self.comp.CreateVector3KeyFrameAnimation()?;
            sc.InsertKeyFrame(0.0, Vector3 { X: 1.0, Y: 1.0, Z: 1.0 })?;
            sc.InsertKeyFrameWithEasingFunction(0.25, Vector3 { X: 0.93, Y: 0.88, Z: 1.0 }, &e)?;
            sc.InsertKeyFrameWithEasingFunction(0.65, Vector3 { X: 1.035, Y: 1.04, Z: 1.0 }, &e)?;
            sc.InsertKeyFrameWithEasingFunction(1.0, Vector3 { X: 1.0, Y: 1.0, Z: 1.0 }, &e)?;
            sc.SetDuration(TimeSpan { Duration: 300 * 10_000 })?;
            self.capsule.cast::<CompositionObject>()?.StartAnimation(&HSTRING::from("Scale"), &sc)
        };
        let _ = go();
    }

    fn id_at(&self, x: f32) -> Option<u32> {
        lay::hit(&self.left, x).or_else(|| lay::hit(&self.right, x))
    }

    fn has_menu(id: u32) -> bool {
        matches!(id, LOGO | APP | M_FILE | M_EDIT | M_VIEW | M_WIN | M_HELP) || (NATIVE0..NATIVE0 + 20).contains(&id)
    }

    fn open_menu(&mut self, id: u32) {
        let items = self.menu_items(id);
        log::line("menubar", &format!("buka menu {id}: {} item", items.len()));
        let Some((x, _)) = self.rect_of(id) else { return };
        let (comp, gfx) = (self.comp.clone(), &self.gfx);
        let top = self.h;
        let r = self.menu.show_below(&comp, gfx, items, x as i32, top);
        if let Err(e) = &r {
            log::line("menubar", &format!("menu gagal: {e}"));
        }
        if r.is_ok() {
            self.open = Some(id);
            self.update_capsule();
        }
    }

    fn close_menu(&mut self) {
        self.menu.hide();
        self.open = None;
        self.update_capsule();
    }

    fn menu_items(&self, id: u32) -> Vec<Item> {
        let items = self.menu_items_raw(id);
        if (NATIVE0..NATIVE0 + 20).contains(&id) {
            return items;
        }
        items.into_iter().map(|it| { let s = nacre_core::symbols::for_command(it.id); it.with_sym(s) }).collect()
    }

    fn menu_items_raw(&self, id: u32) -> Vec<Item> {
        let a = |id: usize, label: &str| Item::action(id, label, true);
        if (NATIVE0..NATIVE0 + 20).contains(&id) {
            let tops = nacre_platform::menus::read(self.native_hwnd).unwrap_or_else(|| self.native.clone());
            let Some(top) = tops.get((id - NATIVE0) as usize) else { return vec![] };
            return top
                .entries
                .iter()
                .map(|e| {
                    if e.separator {
                        return Item::separator();
                    }
                    let pad = "    ".repeat(e.depth as usize);
                    let label = format!("{}{}{}", if e.checked { "\u{2713} " } else { "" }, pad, e.label);
                    let it = Item::action(if e.header { 0 } else { NATIVE_CMD + e.id as usize }, &label, e.enabled && !e.header);
                    if e.accel.is_empty() { it } else { it.with_accel(&e.accel) }
                })
                .collect();
        }
        match id {
            CTRL => {
                let on = |b: Option<bool>| match b {
                    Some(true) => format!("\u{2713} {}", prefs::tr("Aktif")),
                    Some(false) => prefs::tr("Mati").to_string(),
                    None => "-".to_string(),
                };
                let vol = nacre_platform::audio::latest();
                let vtxt = if vol.available { format!("{} {}%{}", prefs::tr("Volume"), (vol.level * 100.0).round() as i32, if vol.muted { format!(" {}", prefs::tr("(senyap)")) } else { String::new() }) } else { prefs::tr("Volume").to_string() };
                let btxt = match nacre_platform::brightness::latest() {
                    Some(v) => format!("{} {}%", prefs::tr("Kecerahan"), v),
                    None => prefs::tr("Kecerahan").to_string(),
                };
                vec![
                    a(800, &format!("Wi-Fi  \u{2013}  {}", on(self.wifi))).with_accel(""),
                    a(801, &format!("Bluetooth  \u{2013}  {}", on(self.bt))),
                    Item::separator(),
                    Item::action(0, &vtxt, false),
                    Item::action(802, &prefs::tr("Naikkan Volume"), vol.available),
                    Item::action(803, &prefs::tr("Turunkan Volume"), vol.available),
                    Item::action(804, &prefs::tr("Bisukan / Bunyikan"), vol.available),
                    Item::separator(),
                    Item::action(0, &btxt, false),
                    Item::action(805, &prefs::tr("Lebih Terang"), nacre_platform::brightness::latest().is_some()),
                    Item::action(806, &prefs::tr("Lebih Redup"), nacre_platform::brightness::latest().is_some()),
                    Item::separator(),
                    a(807, &format!("{}{}", if prefs::get().stage { "\u{2713} " } else { "" }, prefs::tr("Scene"))),
                    a(808, &prefs::tr("Overview")).with_accel("Ctrl+↑"),
                    a(809, &prefs::tr("Apps")).with_accel("Ctrl+Shift+Space"),
                    Item::separator(),
                    a(810, &prefs::tr("Pengaturan CustomDock…")),
                    a(811, &prefs::tr("Pengaturan Sistem…")),
                ]
            }
            LOGO => vec![
                a(100, &prefs::tr("Tentang PC Ini")),
                Item::separator(),
                a(101, &prefs::tr("Pengaturan Sistem…")),
                a(102, &prefs::tr("Toko Microsoft…")),
                Item::separator(),
                a(103, &prefs::tr("Pengaturan CustomDock…")),
                a(105, &prefs::tr("Apps")).with_accel("Ctrl+Shift+Space"),
                a(106, &prefs::tr("Overview")).with_accel("Ctrl+↑"),
                a(104, &prefs::tr("Paksa Keluar…")),
                Item::separator(),
                a(110, &prefs::tr("Tidur")),
                a(111, &prefs::tr("Mulai Ulang…")),
                a(112, &prefs::tr("Matikan…")),
                Item::separator(),
                a(113, &prefs::tr("Kunci Layar")),
                a(114, &prefs::tr("Keluar…")),
            ],
            APP => {
                let name = self.app_label();
                vec![
                    a(200, &format!("{} {}", prefs::tr("Tentang"), name)),
                    Item::separator(),
                    a(201, &format!("{} {}", prefs::tr("Sembunyikan"), name)).with_accel("Ctrl+Alt+H"),
                    a(202, &prefs::tr("Sembunyikan Lainnya")),
                    a(203, &prefs::tr("Tampilkan Semua")),
                    Item::separator(),
                    a(204, &format!("{} {}", prefs::tr("Keluar dari"), name)).with_accel("Ctrl+Alt+Q"),
                ]
            }
            M_FILE => vec![
                a(300, &prefs::tr("Baru")).with_accel("Ctrl+N"),
                a(301, &prefs::tr("Buka…")).with_accel("Ctrl+O"),
                Item::separator(),
                a(302, &prefs::tr("Simpan")).with_accel("Ctrl+S"),
                a(303, &prefs::tr("Tutup Jendela")).with_accel("Ctrl+W"),
                Item::separator(),
                a(304, &prefs::tr("Cetak…")).with_accel("Ctrl+P"),
            ],
            M_EDIT => vec![
                a(400, &prefs::tr("Urungkan")).with_accel("Ctrl+Z"),
                a(401, &prefs::tr("Ulangi")).with_accel("Ctrl+Y"),
                Item::separator(),
                a(402, &prefs::tr("Potong")).with_accel("Ctrl+X"),
                a(403, &prefs::tr("Salin")).with_accel("Ctrl+C"),
                a(404, &prefs::tr("Tempel")).with_accel("Ctrl+V"),
                a(405, &prefs::tr("Pilih Semua")).with_accel("Ctrl+A"),
                Item::separator(),
                a(406, &prefs::tr("Cari…")).with_accel("Ctrl+F"),
            ],
            M_VIEW => vec![
                a(500, &prefs::tr("Layar Penuh")).with_accel("F11"),
                Item::separator(),
                a(501, &prefs::tr("Perbesar")).with_accel("Ctrl++"),
                a(502, &prefs::tr("Perkecil")).with_accel("Ctrl+-"),
                a(503, &prefs::tr("Ukuran Asli")).with_accel("Ctrl+0"),
            ],
            M_WIN => vec![
                a(600, &prefs::tr("Minimalkan")).with_accel("Win+↓"),
                a(601, &prefs::tr("Zoom")).with_accel("Win+↑"),
            ],
            M_HELP => vec![a(700, &prefs::tr("Cari Bantuan")).with_accel("F1")],
            _ => vec![],
        }
    }

    fn click(&mut self, id: u32) {
        if self.corner.is_some() && matches!(id, WIFI | BATT | VOL) {
            self.close_menu();
            keys::chord(&[keys::WIN, 0x41]);
            return;
        }
        if id == TRAY_BTN {
            self.close_menu();
            if self.flyout_open() {
                self.close_flyout();
            } else {
                self.open_flyout();
            }
            self.render();
            return;
        }
        if let Some(icon) = self.tray_at(id) {
            self.close_menu();
            let mut pt = windows::Win32::Foundation::POINT::default();
            let _ = unsafe { GetCursorPos(&mut pt) };
            crate::tray::invoke(&icon, crate::tray::Button::Left, pt.x, pt.y);
            return;
        }
        if Self::has_menu(id) {
            if self.open == Some(id) {
                self.close_menu();
            } else {
                self.open_menu(id);
            }
            return;
        }
        match id {
            CTRL => {
                self.close_menu();
                match self.corner {
                    Some((_, y0, _)) => crate::controlcenter::toggle_above(y0),
                    None => crate::controlcenter::toggle(self.h),
                }
            }
            STAGEI => prefs::update(|st| st.stage = !st.stage),
            SEARCH => crate::spotlight::toggle(),
            WIFI => {
                if let Some((x, _)) = self.rect_of(WIFI) {
                    let anchor = match self.corner {
                        Some((_, y0, _)) => crate::netmenu::Anchor::Above(self.screen.0 - (8.0 * self.dpi) as i32, y0 - (6.0 * self.dpi) as i32),
                        None => crate::netmenu::Anchor::Below(x as i32, self.h),
                    };
                    crate::netmenu::show(&self.comp, crate::netmenu::Which::Wifi, anchor, self.dpi);
                }
            }
            // Menu asli Windows: Quick Settings (Win+A).
            STATUS | VOL => {
                self.close_menu();
                keys::chord(&[keys::WIN, 0x41]);
            }
            BATT => launch::open("ms-settings:batterysaver"),
            CLOCK => keys::chord(&[keys::WIN, 0x4E]),
            _ => {}
        }
    }

    fn pick(&mut self, cmd: usize) {
        self.close_menu();
        if cmd >= NATIVE_CMD {
            nacre_platform::menus::invoke(self.native_hwnd, (cmd - NATIVE_CMD) as u32);
            return;
        }
        match cmd {
            100 => launch::open("ms-settings:about"),
            101 => launch::open("ms-settings:"),
            102 => launch::open("ms-windows-store:"),
            103 => crate::settings_win::open(&self.comp),
            104 => launch::run("taskmgr.exe", ""),
            105 => crate::launchpad::toggle(),
            106 => crate::mission::toggle(),
            110 => confirm_power("Tidur sekarang?", "rundll32.exe", "powrprof.dll,SetSuspendState 0,1,0"),
            111 => confirm_power("Mulai ulang komputer sekarang?", "shutdown.exe", "/r /t 0"),
            112 => confirm_power("Matikan komputer sekarang?", "shutdown.exe", "/s /t 0"),
            113 => launch::run("rundll32.exe", "user32.dll,LockWorkStation"),
            114 => confirm_power("Keluar dari akun sekarang?", "shutdown.exe", "/l"),
            800 | 801 => {
                if let Some(rd) = &self.radios {
                    let (k, cur) = if cmd == 800 { (nacre_platform::radio::Kind::Wifi, self.wifi) } else { (nacre_platform::radio::Kind::Bluetooth, self.bt) };
                    let _ = rd.commands.send(nacre_platform::radio::Cmd::Set(k, !cur.unwrap_or(false)));
                }
            }
            802 | 803 => {
                let v = nacre_platform::audio::latest().level;
                services::set_volume((v + if cmd == 802 { 0.1 } else { -0.1 }).clamp(0.0, 1.0));
            }
            804 => services::toggle_mute(),
            805 | 806 => {
                if let Some(v) = nacre_platform::brightness::latest() {
                    services::set_brightness((v as f32 / 100.0 + if cmd == 805 { 0.1 } else { -0.1 }).clamp(0.0, 1.0));
                }
            }
            807 => prefs::update(|st| st.stage = !st.stage),
            808 => crate::mission::toggle(),
            809 => crate::launchpad::toggle(),
            810 => crate::settings_win::open(&self.comp),
            811 => launch::open("ms-settings:"),
            200 => crate::settings_win::open(&self.comp),
            201 => self.hide_app(false),
            202 => self.hide_app(true),
            203 => self.show_all(),
            204 => crate::macshortcuts::quit_foreground_app(),
            300 => keys::chord(&[keys::CTRL, 0x4E]),
            301 => keys::chord(&[keys::CTRL, 0x4F]),
            302 => keys::chord(&[keys::CTRL, 0x53]),
            303 => keys::chord(&[keys::CTRL, 0x57]),
            304 => keys::chord(&[keys::CTRL, 0x50]),
            400 => keys::chord(&[keys::CTRL, 0x5A]),
            401 => keys::chord(&[keys::CTRL, 0x59]),
            402 => keys::chord(&[keys::CTRL, 0x58]),
            403 => keys::chord(&[keys::CTRL, 0x43]),
            404 => keys::chord(&[keys::CTRL, 0x56]),
            405 => keys::chord(&[keys::CTRL, 0x41]),
            406 => keys::chord(&[keys::CTRL, 0x46]),
            500 => keys::chord(&[0x7A]),
            501 => keys::chord(&[keys::CTRL, 0xBB]),
            502 => keys::chord(&[keys::CTRL, 0xBD]),
            503 => keys::chord(&[keys::CTRL, 0x30]),
            600 => keys::chord(&[keys::WIN, 0x28]),
            601 => keys::chord(&[keys::WIN, 0x26]),
            700 => keys::chord(&[0x70]),
            _ => {}
        }
    }

    fn hide_app(&mut self, others: bool) {
        let fg = focus::foreground();
        let wins = enumerate::list(std::process::id());
        let Some(cur) = wins.iter().find(|w| w.hwnd == fg).map(|w| w.path.to_lowercase()) else { return };
        for w in &wins {
            let same = w.path.to_lowercase() == cur;
            if same != others && !focus::is_minimized(w.hwnd) {
                unsafe {
                    let _ = ShowWindow(HWND(w.hwnd as *mut _), SW_MINIMIZE);
                }
                self.hidden.push(w.hwnd);
            }
        }
    }

    fn show_all(&mut self) {
        for h in std::mem::take(&mut self.hidden) {
            if focus::is_alive(h) && focus::is_minimized(h) {
                unsafe {
                    let _ = ShowWindow(HWND(h as *mut _), SW_SHOWNOACTIVATE);
                }
            }
        }
    }
}

fn confirm_power(question: &'static str, file: &'static str, params: &'static str) {
    let text = prefs::tr(question).to_string();
    std::thread::spawn(move || unsafe {
        use windows::core::HSTRING;
        let r = MessageBoxW(None, &HSTRING::from(text), w!("CustomDock"), MB_YESNO | MB_ICONQUESTION | MB_TOPMOST | MB_SETFOREGROUND);
        if r == IDYES {
            launch::run(file, params);
        }
    });
}

fn xy(lp: LPARAM) -> (f32, f32) {
    ((lp.0 as i16) as f32, ((lp.0 >> 16) as i16) as f32)
}

unsafe extern "system" fn bar_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            let (x, _) = xy(lp);
            with_bar(|b| {
                if !b.tracking {
                    let mut t = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                    if TrackMouseEvent(&mut t).is_ok() {
                        b.tracking = true;
                    }
                }
                let id = b.id_at(x);
                if id != b.hover {
                    b.hover = id;
                    if let (Some(open), Some(new)) = (b.open, id) {
                        if new != open && Bar::has_menu(new) {
                            b.open_menu(new);
                        }
                    }
                    b.update_capsule();
                    b.update_tip();
                }
            });
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            with_bar(|b| {
                if let Some(t) = b.tip.as_ref() {
                    t.hide();
                }
                b.tracking = false;
                b.hover = None;
                b.update_capsule();
            });
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let (x, _) = xy(lp);
            with_bar(|b| {
                if let Some(id) = b.id_at(x) {
                    b.press_feedback();
                    b.click(id);
                }
            });
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            let (x, _) = xy(lp);
            with_bar(|b| {
                if let Some(icon) = b.id_at(x).and_then(|id| b.tray_at(id)) {
                    b.close_menu();
                    let mut pt = windows::Win32::Foundation::POINT::default();
                    let _ = GetCursorPos(&mut pt);
                    crate::tray::invoke(&icon, crate::tray::Button::Right, pt.x, pt.y);
                }
            });
            LRESULT(0)
        }
        MENU_PICK => {
            with_bar(|b| b.pick(wp.0));
            LRESULT(0)
        }
        MENU_CLOSE => {
            with_bar(|b| b.close_menu());
            LRESULT(0)
        }
        WM_NOTCH => {
            with_bar(|b| {
                b.relayout();
                b.render();
            });
            LRESULT(0)
        }
        WM_TEST_NET => {
            if wp.0 == 3 {
                crate::netmenu::test_toggle_others();
            } else if wp.0 == 4 || wp.0 == 8 {
                let h = with_bar(|b| b.h).unwrap_or(30);
                crate::controlcenter::toggle(h);
            } else if wp.0 == 5 {
                crate::controlcenter::test_open_picker();
            } else if wp.0 == 9 {
                crate::controlcenter::test_enter_edit();
            } else if wp.0 == 7 {
                crate::netmenu::test_click_audio();
            } else {
                let which = if wp.0 == 1 || wp.0 == 6 { crate::netmenu::Which::Bluetooth } else { crate::netmenu::Which::Wifi };
                with_bar(|b| {
                    if let Some((x, _)) = b.rect_of(WIFI) {
                        crate::netmenu::show(&b.comp, which, crate::netmenu::Anchor::Below(x as i32, b.h), b.dpi);
                    }
                });
            }
            LRESULT(0)
        }
        m if m == WM_APP + 22 => {
            with_bar(|b| {
                if let Some(r) = &b.radios {
                    while let Ok(s) = r.snapshots.try_recv() {
                        b.wifi = s.wifi;
                        b.bt = s.bluetooth;
                    }
                }
                b.relayout();
                b.render();
            });
            LRESULT(0)
        }
        WM_TIMER if wp.0 == T_SIG => {
            with_bar(|b| {
                let v = nacre_platform::wifi::link().map(|l| l.1);
                if v != b.wifi_bars {
                    b.wifi_bars = v;
                    b.render();
                }
            });
            LRESULT(0)
        }
        WM_TIMER => {
            let id = wp.0;
            if id == T_CLOCK {
                let _ = SetTimer(Some(hwnd), T_CLOCK, time::ms_to_next_minute().max(200), None);
            }
            with_bar(|b| {
                b.relayout();
                b.render();
            });
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn knock_out(data: &mut [u8], w: u32, h: u32, text: &nacre_platform::icons::Bitmap) {
    let (tw, th) = (text.w as usize, text.h as usize);
    let (mut x0, mut y0, mut x1, mut y1) = (tw, th, 0usize, 0usize);
    for y in 0..th {
        for x in 0..tw {
            if text.bgra[(y * tw + x) * 4 + 3] > 24 {
                x0 = x0.min(x);
                x1 = x1.max(x);
                y0 = y0.min(y);
                y1 = y1.max(y);
            }
        }
    }
    if x0 > x1 {
        return;
    }
    let nub = (w as f32 * 0.06).max(1.5);
    let body = w as f32 - nub - 1.0;
    let (iw, ih) = ((x1 - x0 + 1) as f32, (y1 - y0 + 1) as f32);
    let ox = ((body - iw) / 2.0).round() as i32 - x0 as i32;
    let oy = ((h as f32 - ih) / 2.0).round() as i32 - y0 as i32;
    for y in 0..th {
        for x in 0..tw {
            let m = text.bgra[(y * tw + x) * 4 + 3] as u32;
            if m == 0 {
                continue;
            }
            let (dx, dy) = (x as i32 + ox, y as i32 + oy);
            if dx < 0 || dy < 0 || dx >= w as i32 || dy >= h as i32 {
                continue;
            }
            let i = ((dy as u32 * w + dx as u32) * 4) as usize;
            for c in 0..4 {
                data[i + c] = (data[i + c] as u32 * (255 - m) / 255) as u8;
            }
        }
    }
}

fn appbar_async(hwnd: isize, screen_w: i32, h: i32) {
    use std::sync::mpsc::{channel, Sender};
    use std::sync::{Mutex, OnceLock};
    static TX: OnceLock<Mutex<Sender<(isize, i32, i32)>>> = OnceLock::new();
    let tx = TX.get_or_init(|| {
        let (tx, rx) = channel::<(isize, i32, i32)>();
        std::thread::spawn(move || {
            while let Ok(mut job) = rx.recv() {
                while let Ok(next) = rx.try_recv() {
                    job = next;
                }
                let (hw, w, h) = job;
                if h <= 0 {
                    shell::appbar::release_top();
                } else {
                    shell::appbar::reserve_top(HWND(hw as *mut _), w, h);
                }
            }
        });
        Mutex::new(tx)
    });
    let _ = tx.lock().unwrap_or_else(|e| e.into_inner()).send((hwnd, screen_w, h));
}

/// Auto-hide aktif: diminta pengguna, atau Rail sedang di pojok kanan bawah.
unsafe extern "system" fn flyout_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        // Menu klik kanan aplikasi tray harus tampil di depan panel tray, bukan di belakangnya.
        WM_TIMER if wp.0 == T_LIFT => {
            let left = with_bar(|b| {
                b.menu_lift = b.menu_lift.saturating_sub(1);
                b.menu_lift
            })
            .unwrap_or(0);
            lift_context_menus();
            if left == 0 && !context_menu_open() {
                let _ = KillTimer(Some(hwnd), T_LIFT);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP | WM_RBUTTONUP => {
            let (x, y) = xy(lp);
            let right = msg == WM_RBUTTONUP;
            with_bar(|b| {
                if let Some((icon, _, _)) = b.flyout_hit(x, y) {
                    let mut pt = windows::Win32::Foundation::POINT::default();
                    let _ = GetCursorPos(&mut pt);
                    if right {
                        b.menu_lift = 70;
                        let _ = SetTimer(Some(hwnd), T_LIFT, 30, None);
                        if let Some(t) = b.tip.as_ref() {
                            t.hide();
                        }
                    } else {
                        b.close_flyout();
                        b.render();
                    }
                    crate::tray::invoke(&icon, if right { crate::tray::Button::Right } else { crate::tray::Button::Left }, pt.x, pt.y);
                }
            });
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = xy(lp);
            with_bar(|b| {
                let r = b.flyout.as_ref().map(|p| p.rect).unwrap_or((0, 0, 0, 0));
                let hit = b.flyout_hit(x, y).filter(|(i, _, _)| !i.tip.trim().is_empty());
                if let Some((icon, cx, s)) = hit {
                    if b.tip.is_none() {
                        b.tip = crate::dock::tooltip::Tooltip::new(&b.comp, b.dpi, b.screen.0).ok();
                    }
                    let (comp, gfx) = (b.comp.clone(), b.gfx.clone());
                    let label = icon.tip.lines().next().unwrap_or("").to_string();
                    let dpi = b.dpi;
                    if let Some(tt) = b.tip.as_mut() {
                        let _ = tt.show(&comp, &gfx, &label, r.0 + (cx + s / 2.0) as i32, r.1 - (4.0 * dpi) as i32);
                    }
                } else if let Some(tt) = b.tip.as_ref() {
                    tt.hide();
                }
            });
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Quick Settings / kalender Windows sedang terbuka: jangan sembunyikan kapsul.
/// Menu klik kanan (kelas #32768) dari aplikasi tray sedang tampil.
fn lift_context_menus() {
    use windows::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        let mut after: Option<HWND> = None;
        for _ in 0..8 {
            let Ok(h) = FindWindowExW(None, after, w!("#32768"), None) else { break };
            if h.0.is_null() {
                break;
            }
            if IsWindowVisible(h).as_bool() {
                let _ = SetWindowPos(h, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
            after = Some(h);
        }
    }
}

fn context_menu_open() -> bool {
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::FindWindowW(w!("#32768"), None)
            .map(|h| !h.0.is_null() && windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(h).as_bool())
            .unwrap_or(false)
    }
}

/// Tinggi dan jarak bawah kapsul.
fn rail_metrics(d: f32) -> (i32, i32) {
    ((40.0 * d).round() as i32, (8.0 * d).round() as i32)
}

fn shell_flyout_fg() -> bool {
    unsafe {
        let w = windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow();
        if w.0.is_null() {
            return false;
        }
        let mut buf = [0u16; 64];
        let n = windows::Win32::UI::WindowsAndMessaging::GetClassNameW(w, &mut buf) as usize;
        let c = String::from_utf16_lossy(&buf[..n]);
        matches!(c.as_str(), "ControlCenterWindow" | "Windows.UI.Core.CoreWindow" | "XamlExplorerHostIslandWindow" | "NotifyIconOverflowWindow")
    }
}

fn ah_on() -> bool {
    let p = prefs::get();
    p.menubar_autohide || p.rail_corner
}

fn corner_on() -> bool {
    prefs::get().rail_corner
}

fn covered_foreground() -> bool {
    let fg = focus::foreground();
    if fg == 0 {
        return false;
    }
    unsafe {
        let h = HWND(fg as *mut _);
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        if pid == std::process::id() {
            return false;
        }
        let mut cls = [0u16; 64];
        let n = GetClassNameW(h, &mut cls);
        let cls = String::from_utf16_lossy(&cls[..n.max(0) as usize]);
        if matches!(cls.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd") {
            return false;
        }
        IsZoomed(h).as_bool() || is_fullscreen(fg)
    }
}

fn sync_ah_timer() {
    let on = prefs::get().menubar && ah_on();
    CTL.with(|c| {
        if let Some(ctl) = c.borrow().as_ref() {
            unsafe {
                if on {
                    SetTimer(Some(ctl.hwnd), T_AH, 60, None);
                } else {
                    let _ = KillTimer(Some(ctl.hwnd), T_AH);
                }
            }
        }
    });
}
