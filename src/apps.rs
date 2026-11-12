//! Todas las apps instaladas, tal como las muestra el menú Inicio.
//!
//! Se leen de la carpeta virtual `shell:AppsFolder`, que reúne los programas de escritorio y las
//! apps de la Microsoft Store en un solo sitio. Cada elemento tiene un nombre de análisis (un AUMID
//! como `Microsoft.WindowsCalculator_8wekyb3d8bbwe!App`, o una ruta con un GUID de carpeta
//! conocida) que sirve para abrirlo después como `shell:AppsFolder\<id>`, sea del tipo que sea.
//!
//! Recorrer la carpeta tarda menos de un segundo, pero pedir el ícono de cada app puede llevar
//! varios (unos 250 elementos en un equipo normal). Por eso se hace en un hilo aparte y en dos
//! pasos: primero se envía la lista con los nombres, y después los íconos por tandas.

use crate::icons;
use std::sync::Mutex;
use windows::core::{w, PWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

#[derive(Debug, Clone)]
pub struct AppInstalada {
    pub nombre: String,
    /// Listo para usar como destino de un acceso: `shell:AppsFolder\<id>`.
    pub destino: String,
    /// Índice en la lista de imágenes del sistema; -1 mientras no se conoce.
    pub icono: i32,
}

/// Última lista completa (con íconos), para que reabrir la ventana sea instantáneo.
static CACHE: Mutex<Option<Vec<AppInstalada>>> = Mutex::new(None);

pub fn en_cache() -> Option<Vec<AppInstalada>> {
    CACHE.lock().ok().and_then(|c| c.clone())
}

/// `lParam` de `msg_lista`: `Box<Vec<AppInstalada>>`.
/// `lParam` de `msg_iconos`: `Box<Vec<(usize, i32)>>` con (posición en la lista, ícono).
/// Quien recibe los mensajes se queda con la caja (`Box::from_raw`).
pub fn cargar_en_segundo_plano(ventana: HWND, msg_lista: u32, msg_iconos: u32) {
    let ventana = ventana.0 as isize;
    std::thread::spawn(move || {
        let hwnd = HWND(ventana as *mut _);
        let enviar = |msg: u32, caja: isize| unsafe {
            PostMessageW(Some(hwnd), msg, WPARAM(0), LPARAM(caja)).is_ok()
        };

        unsafe {
            // Hilo propio: necesita su propio COM.
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let mut elementos = recorrer();
        elementos.sort_by_key(|a| a.0.nombre.to_lowercase());
        elementos.dedup_by(|a, b| a.0.nombre == b.0.nombre && a.0.destino == b.0.destino);

        let lista: Vec<AppInstalada> = elementos.iter().map(|(a, _)| a.clone()).collect();
        let caja = Box::into_raw(Box::new(lista)) as isize;
        if !enviar(msg_lista, caja) {
            // La ventana ya se cerró: se libera la caja y no tiene sentido seguir.
            drop(unsafe { Box::from_raw(caja as *mut Vec<AppInstalada>) });
            return;
        }

        let mut completa = Vec::with_capacity(elementos.len());
        let mut tanda = Vec::new();
        let enviar_tanda = |tanda: Vec<(usize, i32)>| {
            let caja = Box::into_raw(Box::new(tanda)) as isize;
            if !enviar(msg_iconos, caja) {
                drop(unsafe { Box::from_raw(caja as *mut Vec<(usize, i32)>) });
            }
        };
        for (i, (mut app, item)) in elementos.into_iter().enumerate() {
            app.icono = indice_de(&item);
            tanda.push((i, app.icono));
            completa.push(app);
            if tanda.len() == 24 {
                enviar_tanda(std::mem::take(&mut tanda));
            }
        }
        if !tanda.is_empty() {
            enviar_tanda(tanda);
        }
        if let Ok(mut c) = CACHE.lock() {
            *c = Some(completa);
        }
    });
}

fn texto(p: PWSTR) -> String {
    unsafe {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

fn recorrer() -> Vec<(AppInstalada, IShellItem)> {
    let mut apps = Vec::new();
    unsafe {
        let Ok(carpeta) = SHCreateItemFromParsingName::<_, _, IShellItem>(w!("shell:AppsFolder"), None) else {
            return apps;
        };
        let Ok(lista) = carpeta.BindToHandler::<_, IEnumShellItems>(None, &BHID_EnumItems) else {
            return apps;
        };
        loop {
            let mut item = [None];
            let mut leidos = 0u32;
            if lista.Next(&mut item, Some(&mut leidos)).is_err() || leidos == 0 {
                break;
            }
            let Some(item) = item[0].take() else { break };
            let (Ok(nombre), Ok(id)) = (
                item.GetDisplayName(SIGDN_NORMALDISPLAY),
                item.GetDisplayName(SIGDN_PARENTRELATIVEPARSING),
            ) else {
                continue;
            };
            let (nombre, id) = (texto(nombre), texto(id));
            if nombre.is_empty() || id.is_empty() {
                continue;
            }
            apps.push((AppInstalada { nombre, destino: format!("shell:AppsFolder\\{id}"), icono: -1 }, item));
        }
    }
    apps
}

fn indice_de(item: &IShellItem) -> i32 {
    unsafe {
        match SHGetIDListFromObject(item) {
            Ok(pidl) => {
                let i = icons::indice_de_pidl(pidl);
                CoTaskMemFree(Some(pidl as *const _));
                i
            }
            Err(_) => -1,
        }
    }
}

/// ¿Coincide la app con el texto de búsqueda? Cada palabra tiene que aparecer en el nombre.
pub fn coincide(app: &AppInstalada, busqueda: &str) -> bool {
    let nombre = app.nombre.to_lowercase();
    busqueda.to_lowercase().split_whitespace().all(|p| nombre.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(nombre: &str) -> AppInstalada {
        AppInstalada { nombre: nombre.into(), destino: format!("shell:AppsFolder\\{nombre}"), icono: 0 }
    }

    #[test]
    fn busqueda_por_palabras_sin_mayusculas() {
        let a = app("Visual Studio Code");
        assert!(coincide(&a, "code"));
        assert!(coincide(&a, "VISUAL code"));
        assert!(coincide(&a, ""));
        assert!(!coincide(&a, "studio 2022"));
    }

    /// Lee la carpeta de apps real del equipo. Lento; se ejecuta con `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn recorre_las_apps_del_equipo() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let t = std::time::Instant::now();
        let apps = recorrer();
        println!("{} apps en {:?}", apps.len(), t.elapsed());
        assert!(!apps.is_empty());
    }
}
