//! Ganchos globales de ratón y teclado (`WH_MOUSE_LL`, `WH_KEYBOARD_LL`).
//!
//! Hay que usar un gancho y no consultar el estado del botón: solo devolviendo 1 desde el gancho se
//! "traga" el clic central, y si no se traga, la ventana de debajo empieza el autodesplazamiento y
//! arrastra la página mientras apuntas.
//!
//! Los ganchos se ejecutan en el hilo que los instaló (el principal, con su bucle de mensajes) y
//! tienen que volver enseguida, así que no deciden nada: actualizan `ESTADO` y avisan a la ventana
//! con `PostMessageW`. La ventana hace el trabajo cuando el gancho ya respondió.

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU8, Ordering};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use crate::config::Boton;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Mensajes que el gancho envía a la ventana principal.
pub const WM_ORBYT_DOWN: u32 = WM_APP + 1;
pub const WM_ORBYT_TAP: u32 = WM_APP + 2;
pub const WM_ORBYT_RELEASE: u32 = WM_APP + 3;
pub const WM_ORBYT_CANCEL: u32 = WM_APP + 4;
/// En modo clic, un clic con la rueda abierta. `lparam` es 0 si es el izquierdo, 1 el derecho.
pub const WM_ORBYT_CLIC: u32 = WM_APP + 5;

/// Sin nada en curso.
pub const INACTIVO: u8 = 0;
/// Botón pulsado y tragado; aún no se sabe si es un clic o una pulsación larga.
pub const PENDIENTE: u8 = 1;
/// La rueda está abierta.
pub const ABIERTA: u8 = 2;
/// Se cerró con el botón aún pulsado: falta tragar su liberación para que no llegue suelta.
pub const CANCELADA: u8 = 3;

pub static ESTADO: AtomicU8 = AtomicU8::new(INACTIVO);
/// Modo clic: la rueda sigue abierta al soltar el botón y se elige con clics.
static MODO_CLIC: AtomicBool = AtomicBool::new(false);
/// El botón que abre la rueda está pulsado ahora mismo.
static BOTON_ABAJO: AtomicBool = AtomicBool::new(false);
/// Se tragó la pulsación de un clic: su liberación también hay que tragarla.
static TRAGAR_IZQ: AtomicBool = AtomicBool::new(false);
static TRAGAR_DER: AtomicBool = AtomicBool::new(false);

pub fn poner_modo_clic(clic: bool) {
    MODO_CLIC.store(clic, Ordering::Relaxed);
}

/// La rueda se cerró: si el botón sigue pulsado, falta tragar su liberación.
pub fn terminar() {
    let estado = if BOTON_ABAJO.load(Ordering::SeqCst) { CANCELADA } else { INACTIVO };
    ESTADO.store(estado, Ordering::SeqCst);
}
static VENTANA: AtomicIsize = AtomicIsize::new(0);
static GANCHO_RATON: AtomicIsize = AtomicIsize::new(0);
static GANCHO_TECLADO: AtomicIsize = AtomicIsize::new(0);

pub fn instalar(ventana: HWND) -> windows::core::Result<()> {
    VENTANA.store(ventana.0 as isize, Ordering::SeqCst);
    unsafe {
        let r = SetWindowsHookExW(WH_MOUSE_LL, Some(raton), None, 0)?;
        GANCHO_RATON.store(r.0 as isize, Ordering::SeqCst);
        let t = SetWindowsHookExW(WH_KEYBOARD_LL, Some(teclado), None, 0)?;
        GANCHO_TECLADO.store(t.0 as isize, Ordering::SeqCst);
    }
    Ok(())
}

/// Quita los ganchos. Hay que hacerlo antes de salir del bucle de mensajes: un gancho de bajo
/// nivel se atiende en este hilo, y mientras siga puesto sin que nadie bombee mensajes, cada
/// movimiento del ratón de todo el sistema espera a que Windows se canse de esperarlo.
pub fn quitar() {
    for gancho in [&GANCHO_RATON, &GANCHO_TECLADO] {
        let h = gancho.swap(0, Ordering::SeqCst);
        if h != 0 {
            unsafe {
                let _ = UnhookWindowsHookEx(HHOOK(h as *mut _));
            }
        }
    }
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
        // Los eventos inyectados (incluido el clic que Orbyt reenvía) pasan sin tocar.
        let inyectado = info.flags & LLMHF_INJECTED != 0;
        if !inyectado {
            let msg = wparam.0 as u32;
            let clic = MODO_CLIC.load(Ordering::Relaxed);
            match es_el_boton(msg, info.mouseData) {
                Some(true) => {
                    BOTON_ABAJO.store(true, Ordering::SeqCst);
                    if ESTADO
                        .compare_exchange(INACTIVO, PENDIENTE, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                    {
                        avisar(WM_ORBYT_DOWN, empaquetar(info.pt.x, info.pt.y));
                        return LRESULT(1);
                    }
                    // Modo clic, con la rueda abierta: volver a pulsar el botón la cierra.
                    if ESTADO
                        .compare_exchange(ABIERTA, CANCELADA, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                    {
                        avisar(WM_ORBYT_CANCEL, 0);
                        return LRESULT(1);
                    }
                }
                Some(false) => {
                    BOTON_ABAJO.store(false, Ordering::SeqCst);
                    match ESTADO.load(Ordering::SeqCst) {
                        PENDIENTE => {
                            ESTADO.store(INACTIVO, Ordering::SeqCst);
                            avisar(WM_ORBYT_TAP, 0);
                            return LRESULT(1);
                        }
                        // En modo clic, soltar no elige nada: la rueda queda abierta.
                        ABIERTA if clic => return LRESULT(1),
                        ABIERTA => {
                            ESTADO.store(INACTIVO, Ordering::SeqCst);
                            avisar(WM_ORBYT_RELEASE, 0);
                            return LRESULT(1);
                        }
                        CANCELADA => {
                            ESTADO.store(INACTIVO, Ordering::SeqCst);
                            return LRESULT(1);
                        }
                        _ => {}
                    }
                }
                None => {
                    // Modo clic, con la rueda abierta: los clics son para la rueda y no llegan a
                    // la ventana de debajo (ni su pulsación ni su liberación).
                    if clic && ESTADO.load(Ordering::SeqCst) == ABIERTA {
                        match msg {
                            WM_LBUTTONDOWN => {
                                TRAGAR_IZQ.store(true, Ordering::SeqCst);
                                avisar(WM_ORBYT_CLIC, 0);
                                return LRESULT(1);
                            }
                            WM_RBUTTONDOWN => {
                                TRAGAR_DER.store(true, Ordering::SeqCst);
                                avisar(WM_ORBYT_CLIC, 1);
                                return LRESULT(1);
                            }
                            _ => {}
                        }
                    }
                    let tragar = match msg {
                        WM_LBUTTONUP => TRAGAR_IZQ.swap(false, Ordering::SeqCst),
                        WM_RBUTTONUP => TRAGAR_DER.swap(false, Ordering::SeqCst),
                        _ => false,
                    };
                    if tragar {
                        return LRESULT(1);
                    }
                }
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
            if pulsada && ESTADO.load(Ordering::SeqCst) == ABIERTA {
                // Con el botón aún pulsado queda CANCELADA (falta tragar su liberación); en modo
                // clic, ya suelto, vuelve directamente a INACTIVO.
                terminar();
                avisar(WM_ORBYT_CANCEL, 0);
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
