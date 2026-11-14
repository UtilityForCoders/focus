//! Abre un acceso con el shell: programas, carpetas, archivos, URLs y URIs pasan todos por
//! `ShellExecuteW("open")`, que ya sabe qué hacer con cada uno.

use crate::config::{expandir, Acceso};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow, ASFW_ANY, SW_SHOWNORMAL};

pub fn abrir(acceso: &Acceso) {
    let destino = HSTRING::from(expandir(acceso.destino.trim()));
    let argumentos = acceso.argumentos.as_deref().map(|a| HSTRING::from(expandir(a)));
    unsafe {
        // Orbyt nunca toma el foco (la rueda no se activa), así que Windows no dejaría que la
        // ventana nueva pase al frente. Esto le cede el permiso al proceso que se abre.
        let _ = AllowSetForegroundWindow(ASFW_ANY);
        let resultado = ShellExecuteW(
            None,
            w!("open"),
            &destino,
            argumentos.as_ref().map_or(PCWSTR::null(), |a| PCWSTR(a.as_ptr())),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        // Según la documentación, un valor <= 32 es un código de error.
        if resultado.0 as isize <= 32 {
            eprintln!("no se pudo abrir {:?} (código {})", acceso.destino, resultado.0 as isize);
        }
    }
}
