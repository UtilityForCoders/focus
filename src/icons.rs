//! Íconos de los accesos.
//!
//! Cuatro fuentes, según el acceso:
//!
//! - **Apps, programas, carpetas y archivos**: el shell (`IShellItemImageFactory`), que entiende
//!   rutas y apps de `shell:AppsFolder` (también las de la Store). Se piden a 256 px en dos
//!   variantes y se queda la que no tenga halo blanco (ver `halo`).
//! - **Webs**: el favicon del sitio (`web.rs`), descargado una vez y guardado en disco. Mientras no
//!   llega, se usa el ícono del navegador predeterminado.
//! - **Un ícono dentro de un archivo** (`archivo:C:\...\shell32.dll,12`): `PrivateExtractIconsW`,
//!   que lo extrae al tamaño pedido.
//! - **Lucide** (`lucide:globe`): un glifo vectorial; no tiene píxeles, lo dibuja el pintor.
//!
//! Los píxeles se guardan BGRA premultiplicado (lo que espera Direct2D) junto con copias a mitad
//! de tamaño: reducir 256 px a 30 de una vez se ve dentado, y así el pintor usa la copia más
//! cercana al tamaño en pantalla.

use crate::config::{expandir, Acceso};
use crate::{lucide, web};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use windows::core::{w, Interface, HSTRING, PCWSTR};
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::Storage::FileSystem::{SearchPathW, FILE_ATTRIBUTE_NORMAL};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, PrivateExtractIconsW, HICON, ICONINFO};

/// Lado al que se normalizan los íconos: lo más grande que suele traer una app, y suficiente
/// para la rueda al 140 % en una pantalla al 200 %.
pub const LADO: i32 = 256;
/// Por debajo de este lado ya no se hacen más copias reducidas.
const LADO_MIN: u32 = 16;
/// Un halo menor que esto se considera limpio y no hace falta probar otra variante.
const HALO_LIMPIO: f32 = 0.15;

pub struct Nivel {
    pub ancho: u32,
    pub alto: u32,
    pub pixeles: Vec<u8>,
}

/// Píxeles de un ícono, BGRA premultiplicado, con sus copias reducidas (cada una a la mitad).
pub struct Icono {
    /// Identifica el ícono en las cachés de bitmaps de Direct2D.
    pub clave: String,
    pub niveles: Vec<Nivel>,
}

impl Icono {
    pub fn nuevo(clave: String, ancho: u32, alto: u32, pixeles: Vec<u8>) -> Self {
        let mut niveles = vec![Nivel { ancho, alto, pixeles }];
        while let Some(u) = niveles.last() {
            if u.ancho / 2 < LADO_MIN || u.alto / 2 < LADO_MIN {
                break;
            }
            let r = reducir(u);
            niveles.push(r);
        }
        Icono { clave, niveles }
    }

    /// La copia más pequeña que todavía cubre `lado` píxeles (o la mayor, si ninguna).
    pub fn nivel_para(&self, lado: f32) -> (usize, &Nivel) {
        let mut elegido = 0;
        for (i, n) in self.niveles.iter().enumerate() {
            if n.ancho as f32 >= lado {
                elegido = i;
            }
        }
        (elegido, &self.niveles[elegido])
    }
}

/// Mitad de tamaño promediando cada bloque de 2x2 (en premultiplicado se puede promediar directo).
fn reducir(n: &Nivel) -> Nivel {
    let (an, al) = (n.ancho / 2, n.alto / 2);
    let mut p = vec![0u8; (an * al * 4) as usize];
    let fila = (n.ancho * 4) as usize;
    for y in 0..al as usize {
        for x in 0..an as usize {
            let o = (2 * y) * fila + (2 * x) * 4;
            for c in 0..4 {
                let s = n.pixeles[o + c] as u32
                    + n.pixeles[o + 4 + c] as u32
                    + n.pixeles[o + fila + c] as u32
                    + n.pixeles[o + fila + 4 + c] as u32;
                p[(y * an as usize + x) * 4 + c] = ((s + 2) / 4) as u8;
            }
        }
    }
    Nivel { ancho: an, alto: al, pixeles: p }
}

pub type IconoRef = Arc<Icono>;

/// Ancho, alto y píxeles BGRA premultiplicados, recién extraídos.
pub type Pixeles = (u32, u32, Vec<u8>);

/// Lo que se dibuja para un acceso: una imagen o un glifo de Lucide.
#[derive(Clone)]
pub enum Visual {
    Imagen(IconoRef),
    Lucide(&'static str),
}

thread_local! {
    /// Íconos por clave. Se guarda también el fracaso (`None`) para no volver a pedirle al shell
    /// un ícono que no existe.
    static CACHE: RefCell<HashMap<String, Option<IconoRef>>> = RefCell::new(HashMap::new());
}

fn de_cache(clave: &str, extraer: impl FnOnce() -> Option<Pixeles>) -> Option<IconoRef> {
    if let Some(v) = CACHE.with(|c| c.borrow().get(clave).cloned()) {
        return v;
    }
    let icono = extraer().map(|(an, al, px)| Arc::new(Icono::nuevo(clave.to_string(), an, al, px)));
    CACHE.with(|c| c.borrow_mut().insert(clave.to_string(), icono.clone()));
    icono
}

/// Olvida los íconos de las webs (llegó un favicon nuevo y hay que releerlos).
pub fn olvidar_webs() {
    CACHE.with(|c| c.borrow_mut().retain(|k, _| web::host(k).is_none()));
}

/// Qué dibujar para un acceso: su ícono propio si lo eligió el usuario, si no, el automático.
pub fn visual(acceso: &Acceso) -> Option<Visual> {
    match acceso.icono.as_deref().map(str::trim) {
        Some(propio) if !propio.is_empty() => visual_propio(propio).or_else(|| automatico(&acceso.destino)),
        _ => automatico(&acceso.destino),
    }
}

/// Un ícono elegido: `lucide:<nombre>` o `archivo:<ruta>,<índice>`.
pub fn visual_propio(propio: &str) -> Option<Visual> {
    if let Some(n) = propio.strip_prefix("lucide:") {
        return lucide::nombre(n.trim()).map(Visual::Lucide);
    }
    if let Some(resto) = propio.strip_prefix("archivo:") {
        let (ruta, indice) = partir_archivo(resto)?;
        return de_cache(propio, || de_archivo(&ruta, indice, LADO)).map(Visual::Imagen);
    }
    None
}

/// `C:\a\b.dll,12` → (`C:\a\b.dll`, 12). Sin índice, el primero.
pub fn partir_archivo(s: &str) -> Option<(String, i32)> {
    let s = s.trim();
    match s.rsplit_once(',') {
        Some((r, i)) if i.trim().parse::<i32>().is_ok() => Some((r.trim().to_string(), i.trim().parse().ok()?)),
        _ => (!s.is_empty()).then(|| (s.to_string(), 0)),
    }
}

/// El ícono automático de un destino.
pub fn automatico(destino: &str) -> Option<Visual> {
    let img = match fuente(destino) {
        Fuente::Elemento(ruta) => de_cache(destino, || {
            let item: IShellItem = unsafe { SHCreateItemFromParsingName(&HSTRING::from(ruta), None).ok()? };
            mejor_de_item(&item)
        }),
        Fuente::Web(host) => match web::en_disco(&host) {
            Some(bytes) => de_cache(destino, || decodificar(&bytes)),
            None => {
                // Mientras llega el favicon, el ícono del navegador (sin guardarlo como definitivo).
                web::pedir(host);
                de_cache("ext:.html", || desde_extension(w!(".html")))
            }
        },
        Fuente::Ninguna => None,
    };
    img.map(Visual::Imagen)
}

enum Fuente {
    /// Algo que el shell sabe abrir como elemento: ruta, carpeta o `shell:AppsFolder\...`.
    Elemento(String),
    /// Una URL web, con su host.
    Web(String),
    /// Otra cosa (`ms-settings:`, un programa que no está en el PATH...): sin ícono.
    Ninguna,
}

fn fuente(destino: &str) -> Fuente {
    let d = expandir(destino.trim()).trim_matches('"').to_string();
    if d.is_empty() {
        return Fuente::Ninguna;
    }
    let l = d.to_ascii_lowercase();
    if l.starts_with("http://") || l.starts_with("https://") {
        return web::host(&d).map_or(Fuente::Ninguna, Fuente::Web);
    }
    if l.starts_with("shell:") || d.contains('\\') || d.contains('/') {
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

/// El mejor ícono de un elemento del shell. Primero sin escalar hacia arriba (así llega el más
/// grande que la app trae de verdad); si tiene halo, se prueba la otra variante y gana la más
/// limpia.
pub fn mejor_de_item(item: &IShellItem) -> Option<Pixeles> {
    let mut mejor: Option<(Pixeles, f32)> = None;
    for flags in [SIIGBF_BIGGERSIZEOK | SIIGBF_ICONONLY, SIIGBF_ICONONLY | SIIGBF_RESIZETOFIT] {
        let Some(img) = de_item(item, LADO, flags) else { continue };
        let h = halo(&img.2, img.0, img.1);
        if mejor.as_ref().is_none_or(|(_, hm)| h < *hm) {
            mejor = Some((img, h));
        }
        if h < HALO_LIMPIO {
            break;
        }
    }
    mejor.map(|(img, _)| normalizar(img))
}

/// Ícono de un elemento del shell a un tamaño (lo usa también la lista de apps).
pub fn de_item(item: &IShellItem, lado: i32, flags: SIIGBF) -> Option<Pixeles> {
    unsafe {
        let fabrica: IShellItemImageFactory = item.cast().ok()?;
        let bitmap = fabrica.GetImage(SIZE { cx: lado, cy: lado }, flags).ok()?;
        let r = pixeles(bitmap);
        let _ = DeleteObject(bitmap.into());
        r
    }
}

/// Nada mayor que `LADO`: el shell a veces devuelve imágenes enormes con `BIGGERSIZEOK`.
fn normalizar((mut an, mut al, mut px): Pixeles) -> Pixeles {
    while an > LADO as u32 && an % 2 == 0 && al % 2 == 0 {
        let n = reducir(&Nivel { ancho: an, alto: al, pixeles: px });
        (an, al, px) = (n.ancho, n.alto, n.pixeles);
    }
    (an, al, px)
}

/// Un ícono que está dentro de un archivo (.dll, .exe, .ico), por posición.
pub fn de_archivo(ruta: &str, indice: i32, lado: i32) -> Option<Pixeles> {
    let ruta = expandir(ruta);
    let mut nombre = [0u16; 260];
    let u: Vec<u16> = ruta.encode_utf16().collect();
    // Una ruta más larga no se recorta: recortada nombraría otro archivo.
    if u.len() >= nombre.len() {
        return None;
    }
    nombre[..u.len()].copy_from_slice(&u);
    let mut iconos = [HICON::default()];
    unsafe {
        let n = PrivateExtractIconsW(&nombre, indice, lado, lado, Some(&mut iconos), None, 0);
        if n == 0 || n == u32::MAX || iconos[0].is_invalid() {
            return None;
        }
        let r = pixeles_de_icono(iconos[0]);
        let _ = DestroyIcon(iconos[0]);
        r
    }
}

/// Cuántos íconos tiene un archivo.
pub fn cantidad_en_archivo(ruta: &str) -> u32 {
    let ruta = expandir(ruta);
    let mut nombre = [0u16; 260];
    let u: Vec<u16> = ruta.encode_utf16().collect();
    if u.len() >= nombre.len() {
        return 0;
    }
    nombre[..u.len()].copy_from_slice(&u);
    let n = unsafe { PrivateExtractIconsW(&nombre, 0, 32, 32, None, None, 0) };
    if n == u32::MAX { 0 } else { n }
}

/// Decodifica un PNG, ICO, JPG... con WIC a BGRA premultiplicado. En un ICO elige la imagen más
/// grande.
pub fn decodificar(bytes: &[u8]) -> Option<Pixeles> {
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let flujo = wic.CreateStream().ok()?;
        flujo.InitializeFromMemory(bytes).ok()?;
        let dec = wic.CreateDecoderFromStream(&flujo, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
        let mut mejor = (0u32, 0u32);
        for i in 0..dec.GetFrameCount().ok()? {
            let f = dec.GetFrame(i).ok()?;
            let (mut an, mut al) = (0, 0);
            f.GetSize(&mut an, &mut al).ok()?;
            if an > mejor.1 {
                mejor = (i, an);
            }
        }
        let cuadro = dec.GetFrame(mejor.0).ok()?;
        let conv = wic.CreateFormatConverter().ok()?;
        conv.Initialize(&cuadro, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)
            .ok()?;
        let (mut an, mut al) = (0, 0);
        conv.GetSize(&mut an, &mut al).ok()?;
        if an == 0 || al == 0 || an > 4096 || al > 4096 {
            return None;
        }
        let mut px = vec![0u8; (an * al * 4) as usize];
        conv.CopyPixels(std::ptr::null(), an * 4, &mut px).ok()?;
        Some(normalizar((an, al, px)))
    }
}

fn desde_extension(ext: PCWSTR) -> Option<Pixeles> {
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

/// Píxeles de un HICON. Los íconos antiguos no traen alfa: su transparencia está en la máscara
/// (bit a 1 = transparente). Sin aplicarla salen sobre un cuadro negro.
unsafe fn pixeles_de_icono(icono: HICON) -> Option<Pixeles> {
    let mut info = ICONINFO::default();
    unsafe {
        GetIconInfo(icono, &mut info).ok()?;
        let color = leer_dib(info.hbmColor);
        let mascara = leer_dib(info.hbmMask);
        let _ = DeleteObject(info.hbmColor.into());
        let _ = DeleteObject(info.hbmMask.into());
        let (an, al, mut px) = color?;
        let sin_alfa = px.as_chunks::<4>().0.iter().all(|p| p[3] == 0);
        // La máscara de un ícono con color mide lo mismo que él.
        match mascara.filter(|(man, mal, _)| sin_alfa && *man == an && *mal >= al) {
            Some((_, _, m)) => {
                // Alfa 0 o 255 y color a cero donde es transparente: ya queda premultiplicado.
                // (Pasarlo por `premultiplicar` volvería opaco un ícono vacío, todo alfa 0.)
                aplicar_mascara(&mut px, &m);
            }
            None => premultiplicar(&mut px),
        }
        Some((an, al, px))
    }
}

/// Transparencia de una máscara de ícono: bit a 1 (blanco) es transparente.
fn aplicar_mascara(px: &mut [u8], mascara: &[u8]) {
    for (p, q) in px.as_chunks_mut::<4>().0.iter_mut().zip(mascara.as_chunks::<4>().0) {
        if q[0] > 127 {
            *p = [0, 0, 0, 0];
        } else {
            p[3] = 255;
        }
    }
}

/// Copia un HBITMAP de 32 bits a BGRA premultiplicado.
fn pixeles(bitmap: HBITMAP) -> Option<Pixeles> {
    let (an, al, mut px) = leer_dib(bitmap)?;
    premultiplicar(&mut px);
    Some((an, al, px))
}

/// Lee cualquier HBITMAP como BGRA de 32 bits, tal cual (sin tocar el alfa).
fn leer_dib(bitmap: HBITMAP) -> Option<Pixeles> {
    if bitmap.is_invalid() {
        return None;
    }
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

/// Cuánto halo blanco tiene un ícono, de 0 a 1.
///
/// Un halo es lo que queda cuando un ícono se compuso sobre un fondo claro y luego se recortó por
/// alfa: los píxeles semitransparentes del borde son casi blancos. Sobre las baldosas oscuras se
/// ve como una pegatina. Para no castigar un logo que de verdad es blanco, se descuenta lo claro
/// que sea el interior opaco.
pub fn halo(px: &[u8], ancho: u32, alto: u32) -> f32 {
    let (mut borde, mut borde_claro, mut opaco, mut opaco_claro) = (0u32, 0u32, 0u32, 0u32);
    for p in px.as_chunks::<4>().0 {
        let a = p[3] as u32;
        if a < 16 {
            continue;
        }
        // Color sin premultiplicar.
        let minimo = (0..3).map(|c| p[c] as u32 * 255 / a).min().unwrap_or(0);
        let claro = minimo >= 220;
        if a < 230 {
            borde += 1;
            borde_claro += claro as u32;
        } else {
            opaco += 1;
            opaco_claro += claro as u32;
        }
    }
    // Muy poco borde para juzgar (un ícono cuadrado y opaco no tiene halo).
    if borde < (ancho * alto / 200).max(8) {
        return 0.0;
    }
    let f_borde = borde_claro as f32 / borde as f32;
    let f_opaco = if opaco > 0 { opaco_claro as f32 / opaco as f32 } else { 0.0 };
    (f_borde - f_opaco).max(0.0)
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
        assert!(matches!(fuente("https://github.com/x"), Fuente::Web(h) if h == "github.com"));
        assert!(matches!(fuente("shell:AppsFolder\\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"), Fuente::Elemento(_)));
        assert!(matches!(fuente("C:\\Windows"), Fuente::Elemento(_)));
        assert!(matches!(fuente("ms-settings:"), Fuente::Ninguna));
        assert!(matches!(fuente("notepad.exe"), Fuente::Elemento(_)));
    }

    #[test]
    fn parte_la_referencia_a_un_icono_de_archivo() {
        assert_eq!(partir_archivo("C:\\W\\shell32.dll,12"), Some(("C:\\W\\shell32.dll".into(), 12)));
        assert_eq!(partir_archivo("C:\\a,b\\x.ico"), Some(("C:\\a,b\\x.ico".into(), 0)));
        assert_eq!(partir_archivo("x.dll,-3"), Some(("x.dll".into(), -3)));
        assert_eq!(partir_archivo("  "), None);
    }

    /// Ícono sintético de 32x32: interior opaco del color dado y un anillo semitransparente.
    fn sintetico(interior: [u8; 3], borde: [u8; 3]) -> Vec<u8> {
        let mut px = vec![0u8; 32 * 32 * 4];
        for y in 0..32 {
            for x in 0..32 {
                let d = ((x as f32 - 15.5).powi(2) + (y as f32 - 15.5).powi(2)).sqrt();
                let (c, a) = if d < 12.0 { (interior, 255u32) } else if d < 15.0 { (borde, 120) } else { continue };
                let o = (y * 32 + x) * 4;
                for i in 0..3 {
                    px[o + i] = (c[i] as u32 * a / 255) as u8;
                }
                px[o + 3] = a as u8;
            }
        }
        px
    }

    #[test]
    fn detecta_el_halo_blanco() {
        assert!(halo(&sintetico([200, 30, 30], [255, 255, 255]), 32, 32) > 0.5);
    }

    #[test]
    fn un_icono_de_color_limpio_no_tiene_halo() {
        assert!(halo(&sintetico([200, 30, 30], [200, 30, 30]), 32, 32) < HALO_LIMPIO);
    }

    #[test]
    fn un_logo_blanco_no_es_un_halo() {
        assert!(halo(&sintetico([250, 250, 250], [250, 250, 250]), 32, 32) < HALO_LIMPIO);
    }

    #[test]
    fn la_mascara_deja_transparente_lo_que_marca() {
        // Dos píxeles negros sin alfa; la máscara marca el segundo como transparente.
        let mut px = vec![10, 20, 30, 0, 40, 50, 60, 0];
        aplicar_mascara(&mut px, &[0, 0, 0, 255, 255, 255, 255, 255]);
        assert_eq!(px, vec![10, 20, 30, 255, 0, 0, 0, 0]);
        // Un ícono vacío (todo transparente) sigue vacío.
        let mut vacio = vec![0u8; 8];
        aplicar_mascara(&mut vacio, &[255u8; 8]);
        assert!(vacio.iter().all(|&v| v == 0));
    }

    #[test]
    fn un_icono_opaco_no_tiene_halo() {
        assert_eq!(halo(&vec![255u8; 64 * 64 * 4], 64, 64), 0.0);
    }

    #[test]
    fn las_copias_reducidas_bajan_a_la_mitad() {
        let ic = Icono::nuevo("x".into(), 256, 256, vec![128u8; 256 * 256 * 4]);
        let lados: Vec<u32> = ic.niveles.iter().map(|n| n.ancho).collect();
        assert_eq!(lados, [256, 128, 64, 32, 16]);
        assert_eq!(ic.nivel_para(40.0).1.ancho, 64);
        assert_eq!(ic.nivel_para(500.0).1.ancho, 256);
        assert!(ic.niveles[2].pixeles.iter().all(|&v| v == 128));
    }
}

