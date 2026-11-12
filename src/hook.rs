//! Ganchos globales de ratón y teclado (`WH_MOUSE_LL`, `WH_KEYBOARD_LL`).
//!
//! Hay que usar un gancho y no consultar el estado del botón: solo devolviendo 1 desde el gancho se
//! "traga" el clic central, y si no se traga, la ventana de debajo empieza el autodesplazamiento y
//! arrastra la página mientras apuntas.
//!
//! Los ganchos se ejecutan en el hilo que los instaló (el principal, con su bucle de mensajes) y
//! tienen que volver enseguida, así que no deciden nada: actualizan `ESTADO` y avisan a la ventana
//! con `PostMessageW`. La ventana hace el trabajo cuando el gancho ya respondió.

use std::sync::atomic::{AtomicIsize, AtomicU8, Ordering};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use crate::config::Boton;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Mensajes que el gancho envía a la ventana principal.
pub const WM_FOCUS_DOWN: u32 = WM_APP + 1;
pub const WM_FOCUS_TAP: u32 = WM_APP + 2;
pub const WM_FOCUS_RELEASE: u32 = WM_APP + 3;
pub const WM_FOCUS_CANCEL: u32 = WM_APP + 4;

/// Sin nada en curso.
pub const INACTIVO: u8 = 0;
/// Botón pulsado y tragado; aún no se sabe si es un clic o una pulsación larga.
pub const PENDIENTE: u8 = 1;
/// La rueda está abierta.
pub const ABIERTA: u8 = 2;
/// Se canceló con Escape: falta tragar la liberación del botón para que no llegue suelta.
pub const CANCELADA: u8 = 3;

pub static ESTADO: AtomicU8 = AtomicU8::new(INACTIVO);
static VENTANA: AtomicIsize = AtomicIsize::new(0);

pub fn instalar(ventana: HWND) -> windows::core::Result<()> {
    VENTANA.store(ventana.0 as isize, Ordering::SeqCst);
    unsafe {
        SetWindowsHookExW(WH_MOUSE_LL, Some(raton), None, 0)?;
        SetWindowsHookExW(WH_KEYBOARD_LL, Some(teclado), None, 0)?;
    }
    Ok(())
}

fn avisar(msg: u32, lparam: isize) {
    let hwnd = HWND(VENTANA.load(Ordering::Relaxed) as *mut _);
    unsafe {
        let _ = PostMessageW(Some(hwnd), msg, WPARAM(0), LPARAM(lparam));
    }
}

/// Empaqueta un punto de pantalla (puede ser negativo con varios monitores) en un `LPARAM`.
pub fn empaquetar(x: i32, y: i32) -> isize {
    ((x as u16 as u32) | ((y as u16 as u32) << 16)) as isize
}

pub fn desempaquetar(l: isize) -> (i32, i32) {
    ((l as u32 & 0xFFFF) as u16 as i16 as i32, ((l as u32 >> 16) & 0xFFFF) as u16 as i16 as i32)
}

/// Botón que abre la rueda: 0 central, 1 lateral "atrás", 2 lateral "adelante".
static BOTON: AtomicU8 = AtomicU8::new(0);

pub fn poner_boton(boton: Boton) {
    let v = match boton {
        Boton::Central => 0,
        Boton::Atras => 1,
        Boton::Adelante => 2,
    };
    BOTON.store(v, Ordering::Relaxed);
}

pub fn boton() -> Boton {
    match BOTON.load(Ordering::Relaxed) {
        1 => Boton::Atras,
        2 => Boton::Adelante,
        _ => Boton::Central,
    }
}

/// ¿Es este mensaje del ratón el botón configurado? Devuelve `Some(true)` al pulsarlo,
/// `Some(false)` al soltarlo y `None` si es otro botón.
fn es_el_boton(msg: u32, datos: u32) -> Option<bool> {
    // En los botones laterales, la palabra alta de `mouseData` dice cuál es (1 o 2).
    let lateral = (datos >> 16) as u16;
    match (BOTON.load(Ordering::Relaxed), msg) {
        (0, WM_MBUTTONDOWN) => Some(true),
        (0, WM_MBUTTONUP) => Some(false),
        (1, WM_XBUTTONDOWN) if lateral == XBUTTON1 => Some(true),
        (1, WM_XBUTTONUP) if lateral == XBUTTON1 => Some(false),
        (2, WM_XBUTTONDOWN) if lateral == XBUTTON2 => Some(true),
        (2, WM_XBUTTONUP) if lateral == XBUTTON2 => Some(false),
        _ => None,
    }
}

unsafe extern "system" fn raton(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        // Los eventos inyectados (incluido el clic que Focus reenvía) pasan sin tocar.
        let inyectado = info.flags & LLMHF_INJECTED != 0;
        if !inyectado {
            match es_el_boton(wparam.0 as u32, info.mouseData) {
                Some(true) => {
                    if ESTADO
                        .compare_exchange(INACTIVO, PENDIENTE, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                    {
                        avisar(WM_FOCUS_DOWN, empaquetar(info.pt.x, info.pt.y));
                        return LRESULT(1);
                    }
                }
                Some(false) => match ESTADO.swap(INACTIVO, Ordering::SeqCst) {
                    PENDIENTE => {
                        avisar(WM_FOCUS_TAP, 0);
                        return LRESULT(1);
                    }
                    ABIERTA => {
                        avisar(WM_FOCUS_RELEASE, 0);
                        return LRESULT(1);
                    }
                    CANCELADA => return LRESULT(1),
                    _ => {}
                },
                None => {}
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn teclado(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && ESTADO.load(Ordering::Relaxed) == ABIERTA {
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if info.vkCode == VK_ESCAPE.0 as u32 {
            let pulsada = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            if pulsada
                && ESTADO
                    .compare_exchange(ABIERTA, CANCELADA, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
            {
                avisar(WM_FOCUS_CANCEL, 0);
            }
            // Escape no llega a la ventana de debajo mientras la rueda está abierta.
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empaquetar_conserva_coordenadas_negativas() {
        for (x, y) in [(0, 0), (1920, 1080), (-1280, 200), (-5, -3000)] {
            assert_eq!(desempaquetar(empaquetar(x, y)), (x, y));
        }
    }
}
