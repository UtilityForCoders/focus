//! Ícono en el área de notificación, con un menú para editar los accesos y salir.

use windows::core::w;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const WM_FOCUS_TRAY: u32 = WM_APP + 10;

pub const CMD_EDITAR: usize = 1;
pub const CMD_CARPETA: usize = 2;
pub const CMD_SALIR: usize = 3;

fn datos(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    };
    let tip: Vec<u16> = "Focus: mantén el botón central".encode_utf16().collect();
    nid.szTip[..tip.len()].copy_from_slice(&tip);
    nid
}

pub fn agregar(hwnd: HWND) {
    unsafe {
        let mut nid = datos(hwnd);
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_FOCUS_TRAY;
        nid.hIcon = LoadIconW(None, IDI_APPLICATION).unwrap_or_default();
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
    }
}

pub fn quitar(hwnd: HWND) {
    unsafe {
        let nid = datos(hwnd);
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

/// Muestra el menú en el puntero y devuelve el comando elegido (0 si se cerró sin elegir).
pub fn menu(hwnd: HWND) -> usize {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return 0 };
        let _ = AppendMenuW(menu, MF_STRING, CMD_EDITAR, w!("Editar accesos"));
        let _ = AppendMenuW(menu, MF_STRING, CMD_CARPETA, w!("Abrir carpeta de configuración"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, CMD_SALIR, w!("Salir"));
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        // Sin esto el menú no se cierra al hacer clic fuera de él.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
            p.x,
            p.y,
            None,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
        cmd.0 as usize
    }
}
