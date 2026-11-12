//! Íconos de Windows para los accesos.
//!
//! Dos usos distintos:
//! - **La rueda** necesita píxeles (BGRA premultiplicado) para crear bitmaps de Direct2D. Se piden
//!   al shell con `IShellItemImageFactory`, que entiende rutas, carpetas y apps de la carpeta
//!   virtual `shell:AppsFolder` (también las de la Microsoft Store).
//! - **La ventana de configuración** usa la lista de imágenes del sistema, que ya tiene los íconos
//!   cargados y es mucho más rápida para cientos de apps: solo hace falta el índice.

use crate::config::expandir;
use std::collections::HashMap;
use std::rc::Rc;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::{SearchPathW, FILE_ATTRIBUTE_NORMAL, FILE_FLAGS_AND_ATTRIBUTES};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Controls::HIMAGELIST;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

/// Lado en píxeles del ícono pedido para la rueda: alcanza hasta 225 % de escala.
const LADO: i32 = 96;

/// Píxeles de un ícono, BGRA premultiplicado, filas de arriba hacia abajo.
pub struct Icono {
    /// Destino del acceso; identifica el ícono en la caché de bitmaps del renderer.
    pub clave: String,
    pub ancho: u32,
    pub alto: u32,
    pub pixeles: Vec<u8>,
}

/// Caché de íconos por destino. Se guarda también el fracaso (`None`) para no volver a pedirle
/// al shell un ícono que no existe en cada pulsación.
#[derive(Default)]
pub struct Iconos {
    mapa: HashMap<String, Option<Rc<Icono>>>,
}

impl Iconos {
    pub fn obtener(&mut self, destino: &str) -> Option<Rc<Icono>> {
        if let Some(v) = self.mapa.get(destino) {
            return v.clone();
        }
        let icono = extraer(destino).map(|(ancho, alto, pixeles)| {
            Rc::new(Icono { clave: destino.to_string(), ancho, alto, pixeles })
        });
        self.mapa.insert(destino.to_string(), icono.clone());
        icono
    }
}

enum Fuente {
    /// Algo que el shell sabe abrir como elemento: ruta, carpeta o `shell:AppsFolder\...`.
    Elemento(String),
    /// Una URL web: se usa el ícono del navegador predeterminado (el de los .html).
    Web,
    /// Otra cosa (`ms-settings:`, un programa que no está en el PATH...): sin ícono.
    Ninguna,
}

fn es_web(d: &str) -> bool {
    let d = d.to_ascii_lowercase();
    d.starts_with("http://") || d.starts_with("https://")
}

fn fuente(destino: &str) -> Fuente {
    let d = expandir(destino.trim()).trim_matches('"').to_string();
    if d.is_empty() {
        return Fuente::Ninguna;
    }
    if es_web(&d) {
        return Fuente::Web;
    }
    if d.to_ascii_lowercase().starts_with("shell:") || d.contains('\\') || d.contains('/') {
        return Fuente::Elemento(d);
    }
    // Un nombre suelto ("notepad.exe", "calc"): se busca como lo haría el shell, en el PATH.
    match buscar_en_path(&d) {
        Some(ruta) => Fuente::Elemento(ruta),
        None => Fuente::Ninguna,
    }
}

fn buscar_en_path(nombre: &str) -> Option<String> {
    let mut buf = vec![0u16; 1024];
    let n = unsafe { SearchPathW(PCWSTR::null(), &HSTRING::from(nombre), w!(".exe"), Some(&mut buf), None) };
    (n > 0 && (n as usize) < buf.len()).then(|| String::from_utf16_lossy(&buf[..n as usize]))
}

fn extraer(destino: &str) -> Option<(u32, u32, Vec<u8>)> {
    match fuente(destino) {
        Fuente::Elemento(ruta) => desde_shell(&ruta),
        Fuente::Web => desde_extension(w!(".html")),
        Fuente::Ninguna => None,
    }
}

fn desde_shell(ruta: &str) -> Option<(u32, u32, Vec<u8>)> {
    unsafe {
        let fabrica: IShellItemImageFactory = SHCreateItemFromParsingName(&HSTRING::from(ruta), None).ok()?;
        let bitmap = fabrica.GetImage(SIZE { cx: LADO, cy: LADO }, SIIGBF_ICONONLY).ok()?;
        let r = pixeles(bitmap);
        let _ = DeleteObject(bitmap.into());
        r
    }
}

fn desde_extension(ext: PCWSTR) -> Option<(u32, u32, Vec<u8>)> {
    unsafe {
        let mut info = SHFILEINFOW::default();
        SHGetFileInfoW(
            ext,
            FILE_ATTRIBUTE_NORMAL,
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON | SHGFI_USEFILEATTRIBUTES,
        );
        if info.hIcon.is_invalid() {
            return None;
        }
        let r = pixeles_de_icono(info.hIcon);
        let _ = DestroyIcon(info.hIcon);
        r
    }
}

unsafe fn pixeles_de_icono(icono: HICON) -> Option<(u32, u32, Vec<u8>)> {
    let mut info = ICONINFO::default();
    unsafe {
        GetIconInfo(icono, &mut info).ok()?;
        let r = pixeles(info.hbmColor);
        let _ = DeleteObject(info.hbmColor.into());
        let _ = DeleteObject(info.hbmMask.into());
        r
    }
}

/// Copia un HBITMAP de 32 bits a BGRA premultiplicado, que es lo que espera Direct2D.
fn pixeles(bitmap: HBITMAP) -> Option<(u32, u32, Vec<u8>)> {
    unsafe {
        let mut bm = BITMAP::default();
        let leido = GetObjectW(bitmap.into(), std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut BITMAP as *mut _));
        if leido == 0 || bm.bmWidth <= 0 || bm.bmHeight == 0 {
            return None;
        }
        let (an, al) = (bm.bmWidth, bm.bmHeight.abs());
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: an,
                biHeight: -al,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut datos = vec![0u8; (an * al * 4) as usize];
        let dc = CreateCompatibleDC(None);
        let lineas = GetDIBits(dc, bitmap, 0, al as u32, Some(datos.as_mut_ptr().cast()), &mut bmi, DIB_RGB_COLORS);
        let _ = DeleteDC(dc);
        if lineas == 0 {
            return None;
        }
        premultiplicar(&mut datos);
        Some((an as u32, al as u32, datos))
    }
}

/// El shell a veces entrega el alfa premultiplicado y a veces no, y algunos íconos antiguos no
/// traen alfa. Sin alfa (todo 0) se vuelve opaco; si algún color supera a su alfa, los datos no
/// estaban premultiplicados y se premultiplican.
fn premultiplicar(datos: &mut [u8]) {
    let con_alfa = datos.as_chunks::<4>().0.iter().any(|p| p[3] != 0);
    let directo = datos.as_chunks::<4>().0.iter().any(|p| p[0] > p[3] || p[1] > p[3] || p[2] > p[3]);
    for p in datos.as_chunks_mut::<4>().0 {
        if !con_alfa {
            p[3] = 255;
        } else if directo {
            let a = p[3] as u16;
            for c in &mut p[..3] {
                *c = ((*c as u16 * a + 127) / 255) as u8;
            }
        }
    }
}

// ─── Lista de imágenes del sistema (ventana de configuración) ───────────────────────────────

fn sfi_flags_indice() -> SHGFI_FLAGS {
    SHGFI_SYSICONINDEX | SHGFI_LARGEICON
}

/// La lista de imágenes grande del sistema (32 px a 100 %). Es compartida: no se destruye.
pub fn lista_sistema() -> HIMAGELIST {
    unsafe {
        let mut info = SHFILEINFOW::default();
        let h = SHGetFileInfoW(
            w!(".exe"),
            FILE_ATTRIBUTE_NORMAL,
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            sfi_flags_indice() | SHGFI_USEFILEATTRIBUTES,
        );
        HIMAGELIST(h as isize)
    }
}

/// Índice en la lista del sistema para un PIDL ya resuelto.
pub fn indice_de_pidl(pidl: *const ITEMIDLIST) -> i32 {
    unsafe {
        let mut info = SHFILEINFOW::default();
        let ok = SHGetFileInfoW(
            PCWSTR(pidl as *const u16),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            sfi_flags_indice() | SHGFI_PIDL,
        );
        if ok == 0 { -1 } else { info.iIcon }
    }
}

fn indice_de_extension(ext: PCWSTR) -> i32 {
    unsafe {
        let mut info = SHFILEINFOW::default();
        let ok = SHGetFileInfoW(
            ext,
            FILE_ATTRIBUTE_NORMAL,
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            sfi_flags_indice() | SHGFI_USEFILEATTRIBUTES,
        );
        if ok == 0 { -1 } else { info.iIcon }
    }
}

/// Índice en la lista del sistema para el destino de un acceso.
pub fn indice(destino: &str) -> i32 {
    match fuente(destino) {
        Fuente::Elemento(ruta) => unsafe {
            let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
            if SHParseDisplayName(&HSTRING::from(ruta), None, &mut pidl, 0, None).is_ok() && !pidl.is_null() {
                let i = indice_de_pidl(pidl);
                CoTaskMemFree(Some(pidl as *const _));
                i
            } else {
                indice_de_extension(w!(".exe"))
            }
        },
        Fuente::Web => indice_de_extension(w!(".html")),
        Fuente::Ninguna => indice_de_extension(w!(".exe")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sin_alfa_queda_opaco() {
        let mut d = vec![10, 20, 30, 0, 40, 50, 60, 0];
        premultiplicar(&mut d);
        assert_eq!(d, vec![10, 20, 30, 255, 40, 50, 60, 255]);
    }

    #[test]
    fn alfa_directo_se_premultiplica() {
        let mut d = vec![200, 100, 50, 128];
        premultiplicar(&mut d);
        assert_eq!(d, vec![100, 50, 25, 128]);
    }

    #[test]
    fn ya_premultiplicado_no_cambia() {
        let mut d = vec![60, 30, 10, 128, 0, 0, 0, 0];
        premultiplicar(&mut d);
        assert_eq!(d, vec![60, 30, 10, 128, 0, 0, 0, 0]);
    }

    #[test]
    fn clasifica_destinos() {
        assert!(matches!(fuente("https://github.com"), Fuente::Web));
        assert!(matches!(fuente("shell:AppsFolder\\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"), Fuente::Elemento(_)));
        assert!(matches!(fuente("C:\\Windows"), Fuente::Elemento(_)));
        assert!(matches!(fuente("ms-settings:"), Fuente::Ninguna));
        assert!(matches!(fuente("notepad.exe"), Fuente::Elemento(_)));
    }
}
