//! Favicons de los accesos web.
//!
//! Se descargan con WinHTTP (el cliente HTTP de Windows: sigue redirecciones, respeta el proxy
//! del sistema y valida certificados) en un hilo aparte, nunca mientras se dibuja. La respuesta se
//! guarda en `%APPDATA%\Focus\favicons\<host>.png`, así que cada sitio se pide una sola vez y
//! después funciona sin red.
//!
//! **Qué sale del equipo:** solo el nombre del host (`github.com`), y solo hacia el servicio de
//! favicons de Google o, si falla, el de DuckDuckGo. Nunca la ruta ni la consulta de la URL.
//! Antes de pedir nada se comprueba que el host tenga forma de dominio de verdad, para no hacer
//! una petición por cada letra mientras se escribe una dirección en el editor.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Mutex;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Networking::WinHttp::*;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Se envía a la ventana de aviso cuando llega un favicon nuevo.
pub const WM_FOCUS_FAVICON: u32 = WM_APP + 40;

static AVISO: AtomicIsize = AtomicIsize::new(0);
/// Hosts ya pedidos en esta sesión (con éxito o no): no se vuelven a pedir.
static PEDIDOS: Mutex<Option<HashSet<String>>> = Mutex::new(None);

pub fn poner_aviso(ventana: HWND) {
    AVISO.store(ventana.0 as isize, Ordering::Relaxed);
}

/// El host de una URL web, en minúsculas, si tiene forma de dominio real.
pub fn host(url: &str) -> Option<String> {
    let u = url.trim();
    let resto = u.strip_prefix("https://").or_else(|| u.strip_prefix("http://")).or_else(|| {
        let l = u.to_ascii_lowercase();
        (l.starts_with("https://") || l.starts_with("http://")).then(|| &u[u.find("://").unwrap() + 3..])
    })?;
    let autoridad = resto.split(['/', '?', '#']).next()?;
    // Sin usuario ni contraseña, y sin puerto.
    let sin_usuario = autoridad.rsplit('@').next()?;
    let h = sin_usuario.split(':').next()?.trim_end_matches('.').to_ascii_lowercase();
    let valido = h.len() >= 4
        && h.len() <= 253
        && h.contains('.')
        && h.split('.').all(|e| !e.is_empty() && e.len() <= 63 && !e.starts_with('-') && !e.ends_with('-'))
        && h.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        // El último tramo (com, es, io...) tiene letras: así no se piden "1.2" ni "10.0.0.1".
        && h.rsplit('.').next().is_some_and(|tld| tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic()));
    valido.then_some(h)
}

fn carpeta() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("Focus").join("favicons")
}

fn archivo(host: &str) -> PathBuf {
    carpeta().join(format!("{host}.png"))
}

/// El favicon ya guardado en disco, si lo hay.
pub fn en_disco(host: &str) -> Option<Vec<u8>> {
    std::fs::read(archivo(host)).ok().filter(|b| !b.is_empty())
}

/// Pide el favicon en segundo plano, una sola vez por host y sesión. Al terminar avisa con
/// `WM_FOCUS_FAVICON` (si lo consiguió).
pub fn pedir(host: String) {
    {
        let Ok(mut g) = PEDIDOS.lock() else { return };
        let pedidos = g.get_or_insert_with(HashSet::new);
        if !pedidos.insert(host.clone()) {
            return;
        }
    }
    std::thread::spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let fuentes = [
            ("www.google.com", format!("/s2/favicons?domain={host}&sz=128")),
            ("icons.duckduckgo.com", format!("/ip3/{host}.ico")),
        ];
        for (servidor, ruta) in fuentes {
            let Some(bytes) = descargar(servidor, &ruta) else { continue };
            // Solo se guarda si de verdad es una imagen que Windows sabe leer.
            if crate::icons::decodificar(&bytes).is_none() {
                continue;
            }
            let _ = std::fs::create_dir_all(carpeta());
            if std::fs::write(archivo(&host), &bytes).is_ok() {
                let v = AVISO.load(Ordering::Relaxed);
                if v != 0 {
                    unsafe {
                        let _ = PostMessageW(Some(HWND(v as *mut _)), WM_FOCUS_FAVICON, WPARAM(0), LPARAM(0));
                    }
                }
            }
            return;
        }
    });
}

/// Un GET por HTTPS con tiempos límite cortos. Devuelve el cuerpo si la respuesta es 200.
fn descargar(servidor: &str, ruta: &str) -> Option<Vec<u8>> {
    struct Asa(*mut core::ffi::c_void);
    impl Drop for Asa {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let _ = WinHttpCloseHandle(self.0);
                }
            }
        }
    }
    unsafe {
        let sesion = Asa(WinHttpOpen(
            w!("Focus/1.0"),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ));
        if sesion.0.is_null() {
            return None;
        }
        let _ = WinHttpSetTimeouts(sesion.0, 4000, 4000, 4000, 6000);
        let conexion = Asa(WinHttpConnect(sesion.0, &HSTRING::from(servidor), INTERNET_DEFAULT_HTTPS_PORT, 0));
        if conexion.0.is_null() {
            return None;
        }
        let pedido = Asa(WinHttpOpenRequest(
            conexion.0,
            w!("GET"),
            &HSTRING::from(ruta),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ));
        if pedido.0.is_null() {
            return None;
        }
        WinHttpSendRequest(pedido.0, None, None, 0, 0, 0).ok()?;
        WinHttpReceiveResponse(pedido.0, std::ptr::null_mut()).ok()?;

        let mut estado = 0u32;
        let mut tam = 4u32;
        let mut indice = 0u32;
        WinHttpQueryHeaders(
            pedido.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut estado as *mut u32 as *mut _),
            &mut tam,
            &mut indice,
        )
        .ok()?;
        if estado != 200 {
            return None;
        }

        let mut cuerpo = Vec::new();
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let mut leidos = 0u32;
            WinHttpReadData(pedido.0, buf.as_mut_ptr().cast(), buf.len() as u32, &mut leidos).ok()?;
            if leidos == 0 {
                break;
            }
            cuerpo.extend_from_slice(&buf[..leidos as usize]);
            // Un favicon no pesa megas: si sí, no es un favicon.
            if cuerpo.len() > 2 * 1024 * 1024 {
                return None;
            }
        }
        (!cuerpo.is_empty()).then_some(cuerpo)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extrae_el_host_sin_ruta_usuario_ni_puerto() {
        assert_eq!(host("https://github.com/UtilityForCoders/focus").as_deref(), Some("github.com"));
        assert_eq!(host("http://Example.com:8080/a?b=c").as_deref(), Some("example.com"));
        assert_eq!(host("https://user:pw@secreto.example.com/x").as_deref(), Some("secreto.example.com"));
        assert_eq!(host("HTTPS://WWW.Google.com").as_deref(), Some("www.google.com"));
    }

    #[test]
    fn no_pide_hosts_que_no_pueden_tener_favicon() {
        assert_eq!(host("https://g"), None);
        assert_eq!(host("https://git"), None);
        assert_eq!(host("https://10.0.0.1"), None);
        assert_eq!(host("https://localhost:3000"), None);
        assert_eq!(host("ms-settings:"), None);
        assert_eq!(host("C:\\Windows"), None);
        assert_eq!(host("https://-mal.com"), None);
    }
}
