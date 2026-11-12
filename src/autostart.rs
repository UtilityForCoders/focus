//! Iniciar con Windows: un valor en `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` con la
//! ruta de este ejecutable. Es por usuario, así que no pide permisos de administrador.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::*;

const CLAVE: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALOR: PCWSTR = w!("Focus");

fn comando() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(format!("\"{}\"", exe.display()))
}

/// ¿Está registrado para iniciar con Windows, y apunta a este mismo ejecutable?
pub fn activo() -> bool {
    let mut buf = [0u16; 1024];
    let mut tam = std::mem::size_of_val(&buf) as u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, CLAVE, VALOR, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut tam))
    };
    if r != ERROR_SUCCESS {
        return false;
    }
    let fin = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let valor = String::from_utf16_lossy(&buf[..fin]);
    comando().is_some_and(|c| c.eq_ignore_ascii_case(&valor))
}

pub fn poner(activar: bool) {
    unsafe {
        if activar {
            let Some(c) = comando() else { return };
            let datos: Vec<u16> = c.encode_utf16().chain(std::iter::once(0)).collect();
            let _ = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                CLAVE,
                VALOR,
                REG_SZ.0,
                Some(datos.as_ptr().cast()),
                (datos.len() * 2) as u32,
            );
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, CLAVE, VALOR);
        }
    }
}
