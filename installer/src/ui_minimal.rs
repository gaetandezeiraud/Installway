//! Compact, auto-starting update UI for app-triggered self-updates.
//!
//! No license page, no path picker, no Install button — it starts immediately
//! and just shows progress. Layout:
//!
//! ```text
//!  ┌────────────────────────────────────────────┐
//!  │  ██      Applying update                    │
//!  │  ██      MyApp 1.2                          │
//!  │          [██████████░░░░░░░]  62%           │
//!  │          Updating bin/app.exe               │
//!  └────────────────────────────────────────────┘
//! ```
//! App icon on the left, text + progress on the right. Closes itself on
//! success; on failure it stays with the error message.

#![cfg(windows)]

use crate::extract::{InstallCtx, install};
use crate::payload::LoadedPayload;
use anyhow::Result;
use std::cell::RefCell;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::thread;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET,
    DEFAULT_PITCH, DeleteObject, FF_DONTCARE, FW_NORMAL, FW_SEMIBOLD, GetStockObject, HBRUSH, HFONT,
    OUT_DEFAULT_PRECIS, SetBkMode, SetTextColor, TRANSPARENT, WHITE_BRUSH,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::{
    ICC_PROGRESS_CLASS, INITCOMMONCONTROLSEX, InitCommonControlsEx, PBM_SETPOS, PBM_SETRANGE32,
    PROGRESS_CLASSW,
};
use windows::Win32::UI::Shell::ExtractIconW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

const ID_ICON: usize = 1;
const ID_TITLE: usize = 2;
const ID_SUB: usize = 3;
const ID_PROGRESS: usize = 4;
const ID_STATUS: usize = 5;

const WM_APP_PROGRESS: u32 = WM_APP + 1;
const WM_APP_DONE: u32 = WM_APP + 2;
const WM_APP_ERROR: u32 = WM_APP + 3;

const STM_SETICON: u32 = 0x0170;
const SS_ICON: u32 = 0x0003;

const WIN_W: i32 = 480;
const WIN_H: i32 = 168;
const PAD: i32 = 20;
const ICON_SZ: i32 = 48;
const COL_X: i32 = PAD + ICON_SZ + 20; // text column start

struct Prog {
    done: u64,
    total: u64,
    name: String,
}

struct State {
    cancel: Arc<AtomicBool>,
    prog: Arc<Mutex<Prog>>,
    error: String,
    font_title: HFONT,
    font_body: HFONT,
    bg: HBRUSH,
    hicon: HICON,
}

thread_local! {
    static STATE: RefCell<Option<Rc<RefCell<State>>>> = RefCell::new(None);
    static T: RefCell<common::i18n::Translator> = RefCell::new(common::i18n::Translator::default());
}

fn tr() -> common::i18n::Translator {
    T.with(|t| *t.borrow())
}

pub fn run(
    loaded: LoadedPayload,
    install_dir: PathBuf,
    launch_flag: bool,
    translator: common::i18n::Translator,
) -> Result<()> {
    T.with(|t| *t.borrow_mut() = translator);

    // Build window + register state (the only part that needs FFI).
    let win = unsafe { build_window(&loaded.payload)? };

    // Worker runs in safe code; only the message posts touch FFI.
    spawn_worker(win.hwnd_isize, install_dir, launch_flag, win.cancel, win.prog);

    // Standard message pump.
    unsafe { pump_messages() };
    Ok(())
}

struct Window {
    hwnd_isize: isize,
    cancel: Arc<AtomicBool>,
    prog: Arc<Mutex<Prog>>,
}

unsafe fn build_window(payload: &common::models::InstallerPayload) -> Result<Window> {
    let icc = INITCOMMONCONTROLSEX {
        dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_PROGRESS_CLASS,
    };
    let _ = unsafe { InitCommonControlsEx(&icc) };
    let hinstance = unsafe { GetModuleHandleW(PCWSTR::null()) }?;

    let class_name = w!("RustInstallerMiniWnd");
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: WNDCLASS_STYLES(0),
        lpfnWndProc: Some(wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: HINSTANCE(hinstance.0),
        hIcon: HICON::default(),
        hCursor: unsafe { LoadCursorW(None, IDC_ARROW) }?,
        hbrBackground: HBRUSH(unsafe { GetStockObject(WHITE_BRUSH) }.0),
        lpszMenuName: PCWSTR::null(),
        lpszClassName: class_name,
        hIconSm: HICON::default(),
    };
    unsafe { RegisterClassExW(&wc) };

    // App icon = our own embedded icon (copied from the product exe at build).
    let hicon = unsafe { own_icon() };

    let title_w = wide(&tr().get("install.minimal_title"));
    let state = Rc::new(RefCell::new(State {
        cancel: Arc::new(AtomicBool::new(false)),
        prog: Arc::new(Mutex::new(Prog { done: 0, total: 0, name: String::new() })),
        error: String::new(),
        font_title: create_font("Segoe UI Semibold", 20, FW_SEMIBOLD.0 as i32),
        font_body: create_font("Segoe UI", 15, FW_NORMAL.0 as i32),
        bg: unsafe { CreateSolidBrush(COLORREF(0x00FFFFFF)) },
        hicon,
    }));
    let cancel = state.borrow().cancel.clone();
    let prog = state.borrow().prog.clone();
    STATE.with(|s| *s.borrow_mut() = Some(state));

    // No min/max box, fixed small tool-like window (still has close).
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            PCWSTR(title_w.as_ptr()),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            WIN_W,
            WIN_H,
            None,
            None,
            Some(HINSTANCE(hinstance.0)),
            None,
        )
    }?;
    if !hicon.is_invalid() {
        unsafe {
            SendMessageW(hwnd, WM_SETICON, Some(WPARAM(1)), Some(LPARAM(hicon.0 as isize)));
            SendMessageW(hwnd, WM_SETICON, Some(WPARAM(0)), Some(LPARAM(hicon.0 as isize)));
        }
    }

    unsafe {
        center(hwnd);
        build_controls(hwnd, payload);
        let _ = ShowWindow(hwnd, SW_SHOW);
    }

    Ok(Window { hwnd_isize: hwnd.0 as isize, cancel, prog })
}

/// Auto-start the install worker (no button). Posts progress/done/error back
/// to the window thread.
fn spawn_worker(
    hwnd_isize: isize,
    install_dir: PathBuf,
    launch_flag: bool,
    cancel: Arc<AtomicBool>,
    prog: Arc<Mutex<Prog>>,
) {
    thread::spawn(move || {
        let loaded = match crate::payload::load_and_verify() {
            Ok(l) => l,
            Err(e) => return post_err(hwnd_isize, &format!("{e}")),
        };
        let prog_cb: Arc<dyn Fn(u64, u64, &str) + Send + Sync> = {
            let prog = prog.clone();
            Arc::new(move |done, total, name| {
                if let Ok(mut p) = prog.lock() {
                    p.done = done;
                    p.total = total;
                    p.name = name.to_string();
                }
                post(hwnd_isize, WM_APP_PROGRESS);
            })
        };
        let ctx = InstallCtx {
            install_dir: install_dir.clone(),
            payload: &loaded.payload,
            zip_bytes: &loaded.zip_bytes,
            cancel,
            on_progress: prog_cb,
        };
        if let Err(e) = install(ctx) {
            return post_err(hwnd_isize, &format!("{e}"));
        }
        if let Err(e) =
            crate::install::finalize(&install_dir, &loaded.payload, &loaded.uninstaller_bytes)
        {
            return post_err(hwnd_isize, &format!("finalize: {e}"));
        }
        if launch_flag && !loaded.payload.manifest.exe.is_empty() {
            let _ = crate::install::launch_product(&install_dir, &loaded.payload.manifest.exe);
        }
        post(hwnd_isize, WM_APP_DONE);
    });
}

unsafe fn pump_messages() {
    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Post a no-payload app message to the window thread (thread-safe FFI).
fn post(hwnd_isize: isize, msg: u32) {
    let _ = unsafe {
        PostMessageW(Some(HWND(hwnd_isize as *mut _)), msg, WPARAM(0), LPARAM(0))
    };
}

fn post_err(hwnd_isize: isize, msg: &str) {
    STATE.with(|s| {
        if let Some(st) = s.borrow().as_ref() {
            st.borrow_mut().error = msg.to_string();
        }
    });
    post(hwnd_isize, WM_APP_ERROR);
}

unsafe fn own_icon() -> HICON {
    unsafe {
        let Ok(exe) = std::env::current_exe() else {
            return HICON::default();
        };
        let w: Vec<u16> = exe.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let hmod = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
        // Index 0 = the application's primary icon (we embed the product's).
        ExtractIconW(Some(HINSTANCE(hmod.0)), PCWSTR(w.as_ptr()), 0)
    }
}

unsafe fn build_controls(hwnd: HWND, payload: &common::models::InstallerPayload) {
    let hinst = unsafe { GetModuleHandleW(PCWSTR::null()).unwrap_or_default() };
    let hinst = HINSTANCE(hinst.0);
    let tr = tr();

    let title_w = wide(&tr.get("install.minimal_title"));
    let sub_w = wide(&tr.fmt(
        "install.minimal_sub",
        &[("product", &payload.product), ("version", &payload.to_version)],
    ));

    unsafe {
        // Icon (static, owner sets via STM_SETICON)
        let icon_ctrl = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            PCWSTR::null(),
            WS_VISIBLE | WS_CHILD | WINDOW_STYLE(SS_ICON as u32),
            PAD,
            PAD,
            ICON_SZ,
            ICON_SZ,
            Some(hwnd),
            Some(HMENU(ID_ICON as *mut _)),
            Some(hinst),
            None,
        )
        .ok();
        if let Some(ic) = icon_ctrl {
            STATE.with(|s| {
                if let Some(st) = s.borrow().as_ref() {
                    let h = st.borrow().hicon;
                    if !h.is_invalid() {
                        SendMessageW(ic, STM_SETICON, Some(WPARAM(h.0 as usize)), Some(LPARAM(0)));
                    }
                }
            });
        }

        let _ = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            PCWSTR(title_w.as_ptr()),
            WS_VISIBLE | WS_CHILD,
            COL_X,
            PAD,
            WIN_W - COL_X - PAD,
            26,
            Some(hwnd),
            Some(HMENU(ID_TITLE as *mut _)),
            Some(hinst),
            None,
        );
        let _ = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            PCWSTR(sub_w.as_ptr()),
            WS_VISIBLE | WS_CHILD,
            COL_X,
            PAD + 28,
            WIN_W - COL_X - PAD,
            20,
            Some(hwnd),
            Some(HMENU(ID_SUB as *mut _)),
            Some(hinst),
            None,
        );
        let _ = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PROGRESS_CLASSW,
            PCWSTR::null(),
            WS_VISIBLE | WS_CHILD,
            COL_X,
            PAD + 56,
            WIN_W - COL_X - PAD,
            18,
            Some(hwnd),
            Some(HMENU(ID_PROGRESS as *mut _)),
            Some(hinst),
            None,
        );
        let _ = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            w!(""),
            WS_VISIBLE | WS_CHILD,
            COL_X,
            PAD + 80,
            WIN_W - COL_X - PAD,
            20,
            Some(hwnd),
            Some(HMENU(ID_STATUS as *mut _)),
            Some(hinst),
            None,
        );
    }

    STATE.with(|s| {
        if let Some(st) = s.borrow().as_ref() {
            let st = st.borrow();
            unsafe {
                set_font(hwnd, ID_TITLE, st.font_title);
                set_font(hwnd, ID_SUB, st.font_body);
                set_font(hwnd, ID_STATUS, st.font_body);
            }
        }
    });
}

unsafe fn set_font(hwnd: HWND, id: usize, font: HFONT) {
    unsafe {
        let h = GetDlgItem(Some(hwnd), id as i32).unwrap_or_default();
        if !h.is_invalid() {
            SendMessageW(h, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1)));
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CTLCOLORSTATIC => unsafe {
            let hdc = windows::Win32::Graphics::Gdi::HDC(wparam.0 as *mut core::ffi::c_void);
            let ctrl = HWND(lparam.0 as *mut _);
            let sub = GetDlgItem(Some(hwnd), ID_SUB as i32).unwrap_or_default();
            let status = GetDlgItem(Some(hwnd), ID_STATUS as i32).unwrap_or_default();
            let _ = SetBkMode(hdc, TRANSPARENT);
            // Subtitle + status in muted gray, title in near-black.
            if ctrl == sub || ctrl == status {
                SetTextColor(hdc, COLORREF(0x00777777));
            } else {
                SetTextColor(hdc, COLORREF(0x00202020));
            }
            return LRESULT(STATE.with(|s| {
                s.borrow().as_ref().map(|st| st.borrow().bg.0 as isize).unwrap_or(0)
            }));
        },
        m if m == WM_APP_PROGRESS => unsafe {
            update_progress(hwnd);
            LRESULT(0)
        },
        m if m == WM_APP_DONE => unsafe {
            set_text(hwnd, ID_STATUS, &tr().get("install.minimal_done"));
            set_bar(hwnd, 10000);
            // Brief pause so the user sees 100%, then close.
            let _ = SetTimer(Some(hwnd), 1, 900, None);
            LRESULT(0)
        },
        WM_TIMER => unsafe {
            let _ = KillTimer(Some(hwnd), 1);
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            LRESULT(0)
        },
        m if m == WM_APP_ERROR => unsafe {
            STATE.with(|s| {
                if let Some(st) = s.borrow().as_ref() {
                    let e = st.borrow().error.clone();
                    set_text(hwnd, ID_STATUS, &format!("{}{}", tr().get("install.err_prefix"), e));
                }
            });
            LRESULT(0)
        },
        WM_CLOSE => unsafe {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        },
        WM_DESTROY => unsafe {
            STATE.with(|s| {
                if let Some(st) = s.borrow().as_ref() {
                    let st = st.borrow();
                    let _ = DeleteObject(st.font_title.into());
                    let _ = DeleteObject(st.font_body.into());
                    let _ = DeleteObject(st.bg.into());
                }
            });
            PostQuitMessage(0);
            LRESULT(0)
        },
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

unsafe fn update_progress(hwnd: HWND) {
    STATE.with(|s| {
        let Some(st) = s.borrow().as_ref().cloned() else { return; };
        let st = st.borrow();
        let (done, total, name) = match st.prog.lock() {
            Ok(p) => (p.done, p.total, p.name.clone()),
            Err(_) => return,
        };
        let total = if total == 0 { 1 } else { total };
        let scaled = ((done as u128 * 10000u128) / total as u128) as i32;
        unsafe { set_bar(hwnd, scaled) };
        let pct = scaled / 100;
        let txt = format!("{}%  {}", pct, name);
        unsafe { set_text(hwnd, ID_STATUS, &txt) };
    });
}

unsafe fn set_bar(hwnd: HWND, scaled: i32) {
    unsafe {
        let bar = GetDlgItem(Some(hwnd), ID_PROGRESS as i32).unwrap_or_default();
        SendMessageW(bar, PBM_SETRANGE32, Some(WPARAM(0)), Some(LPARAM(10000)));
        SendMessageW(bar, PBM_SETPOS, Some(WPARAM(scaled as usize)), Some(LPARAM(0)));
    }
}

unsafe fn set_text(hwnd: HWND, id: usize, s: &str) {
    unsafe {
        let h = GetDlgItem(Some(hwnd), id as i32).unwrap_or_default();
        let w = wide(s);
        let _ = SetWindowTextW(h, PCWSTR(w.as_ptr()));
    }
}

unsafe fn center(hwnd: HWND) {
    let mut rect = RECT::default();
    unsafe { let _ = GetWindowRect(hwnd, &mut rect); };
    let w = rect.right - rect.left;
    let h = rect.bottom - rect.top;
    let sw = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let sh = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    unsafe {
        let _ = SetWindowPos(hwnd, None, (sw - w) / 2, (sh - h) / 2, 0, 0, SWP_NOSIZE | SWP_NOZORDER);
    }
}

fn create_font(name: &str, height: i32, weight: i32) -> HFONT {
    let name_w = wide(name);
    unsafe {
        CreateFontW(
            height, 0, 0, 0, weight, 0, 0, 0,
            DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY,
            ((DEFAULT_PITCH.0 as u32) | ((FF_DONTCARE.0 as u32) << 4)) as u32,
            PCWSTR(name_w.as_ptr()),
        )
    }
}

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}
