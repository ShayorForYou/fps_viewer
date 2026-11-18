//! Notification-area icon with a native (dark-mode) context menu.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Controls::{LoadIconMetric, LIM_SMALL};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::settings::OverlayStyle;
use crate::util::{copy_wide, hinstance, wide};

pub const CMD_SETTINGS: u32 = 1;
pub const CMD_TOGGLE_OVERLAY: u32 = 2;
pub const CMD_MOVE: u32 = 3;
pub const CMD_STYLE_MINIMAL: u32 = 10;
pub const CMD_STYLE_COMPACT: u32 = 11;
pub const CMD_STYLE_DETAILED: u32 = 12;
pub const CMD_QUIT: u32 = 20;

pub struct Tray {
    hwnd: HWND,
    callback_msg: u32,
    icon: HICON,
    tip: String,
    /// Broadcast when Explorer restarts — the icon must be re-added.
    pub taskbar_created: u32,
}

impl Tray {
    pub fn new(hwnd: HWND, callback_msg: u32) -> Tray {
        let icon = unsafe { LoadIconMetric(Some(hinstance()), PCWSTR(1 as *const u16), LIM_SMALL).unwrap_or_default() };
        let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        let tray = Tray { hwnd, callback_msg, icon, tip: "FPS Viewer".into(), taskbar_created };
        tray.add();
        tray
    }

    fn data(&self) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            ..Default::default()
        }
    }

    pub fn add(&self) {
        let mut nid = self.data();
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP;
        nid.uCallbackMessage = self.callback_msg;
        nid.hIcon = self.icon;
        copy_wide(&mut nid.szTip, &self.tip);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_ADD, &nid);
            nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
        }
    }

    pub fn remove(&self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.data());
        }
    }

    pub fn set_tooltip(&mut self, text: &str) {
        if self.tip == text {
            return;
        }
        self.tip = text.to_string();
        let mut nid = self.data();
        nid.uFlags = NIF_TIP | NIF_SHOWTIP;
        copy_wide(&mut nid.szTip, text);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

    pub fn balloon(&self, title: &str, text: &str) {
        let mut nid = self.data();
        nid.uFlags = NIF_INFO;
        copy_wide(&mut nid.szInfoTitle, title);
        copy_wide(&mut nid.szInfo, text);
        nid.dwInfoFlags = NIIF_NONE | NIIF_RESPECT_QUIET_TIME;
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.remove();
        if !self.icon.is_invalid() {
            unsafe {
                let _ = DestroyIcon(self.icon);
            }
        }
    }
}

/// Shows the tray menu at the cursor and returns the chosen command (0 = dismissed).
/// Runs a modal loop, so call it while no app state is borrowed.
pub fn show_menu(owner: HWND, overlay_on: bool, move_on: bool, style: OverlayStyle) -> u32 {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return 0 };
        let Ok(styles) = CreatePopupMenu() else {
            let _ = DestroyMenu(menu);
            return 0;
        };
        let item = |m: HMENU, flags: MENU_ITEM_FLAGS, id: usize, text: &str| {
            let t = wide(text);
            let _ = AppendMenuW(m, flags, id, PCWSTR(t.as_ptr()));
        };
        let check = |on: bool| if on { MF_CHECKED } else { MF_UNCHECKED };

        item(styles, MF_STRING, CMD_STYLE_MINIMAL as usize, "Minimal");
        item(styles, MF_STRING, CMD_STYLE_COMPACT as usize, "Compact");
        item(styles, MF_STRING, CMD_STYLE_DETAILED as usize, "Detailed");
        let current = match style {
            OverlayStyle::Minimal => CMD_STYLE_MINIMAL,
            OverlayStyle::Compact => CMD_STYLE_COMPACT,
            OverlayStyle::Detailed => CMD_STYLE_DETAILED,
        };
        let _ = CheckMenuRadioItem(styles, CMD_STYLE_MINIMAL, CMD_STYLE_DETAILED, current, MF_BYCOMMAND.0);

        item(menu, MF_STRING | MF_GRAYED, 0, "FPS Viewer");
        item(menu, MF_SEPARATOR, 0, "");
        item(menu, MF_STRING | check(overlay_on), CMD_TOGGLE_OVERLAY as usize, "Show overlay\tAlt+Shift+F");
        item(menu, MF_STRING | MF_POPUP, styles.0 as usize, "Style");
        item(menu, MF_STRING | check(move_on), CMD_MOVE as usize, "Move overlay\tAlt+Shift+M");
        item(menu, MF_SEPARATOR, 0, "");
        item(menu, MF_STRING, CMD_SETTINGS as usize, "Settings…\tAlt+Shift+O");
        item(menu, MF_STRING, CMD_QUIT as usize, "Quit");
        let _ = SetMenuDefaultItem(menu, CMD_SETTINGS, 0); // bold, like the original

        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        // Required so the menu closes when the user clicks elsewhere.
        let _ = SetForegroundWindow(owner);
        let cmd = TrackPopupMenuEx(
            menu,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY | TPM_BOTTOMALIGN).0,
            p.x,
            p.y,
            owner,
            None,
        );
        let _ = PostMessageW(Some(owner), WM_NULL, Default::default(), Default::default());
        let _ = DestroyMenu(menu); // also destroys the submenu
        cmd.0 as u32
    }
}
