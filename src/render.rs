//! Dibujo de la rueda con Direct2D sobre un DIB de 32 bits, que luego se entrega a una ventana
//! "layered" con `UpdateLayeredWindow`. Así la transparencia es por píxel (las sombras, el
//! oscurecido del fondo) sin depender del compositor para nada más.
//!
//! Diseño: baldosas redondeadas con el ícono de cada acceso, repartidas en círculo alrededor de
//! un botón central (soltar ahí cancela). La baldosa a la que apuntas crece un poco, se aclara y
//! muestra su nombre en una etiqueta hacia fuera de la rueda.
//!
//! Todo se mide en píxeles físicos: la escala (DPI del monitor por el tamaño elegido) llega en
//! `Escena::escala` y se aplica a las medidas, no al render target (que trabaja a 96 ppp).

use crate::icons::Visual;
use crate::ui::{self, glifo, rect, rgba, tema, Alin, Peso, Pintor, Rect};
use crate::wheel::{self, BALDOSA};
use windows::core::{Interface, Result};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows_numerics::Matrix3x2;

/// Lo que hace falta para pintar un fotograma.
pub struct Escena<'a> {
    /// Centro de la rueda, relativo a la esquina de la ventana.
    pub centro: (f32, f32),
    /// DPI del monitor / 96, multiplicado por el tamaño elegido en ajustes.
    pub escala: f32,
    pub nombres: &'a [String],
    /// Ícono de cada acceso (mismo orden que `nombres`); sin ícono se dibuja la inicial.
    pub iconos: &'a [Option<Visual>],
    pub seleccion: Option<usize>,
    /// Progreso de la animación de apertura, de 0 a 1 (ya suavizado).
    pub progreso: f32,
    pub oscurecer: bool,
    /// Radio de la zona central (cancelar), en píxeles físicos.
    pub radio_central: f32,
}

pub struct Renderer {
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    target: Option<ID2D1DCRenderTarget>,
    pintor: Option<Pintor>,
    dc: HDC,
    bitmap: HBITMAP,
    anterior: HGDIOBJ,
    pub ancho: i32,
    pub alto: i32,
}

const BALDOSA_FONDO: ui::Color = rgba(26, 26, 30, 0.88);
const BALDOSA_FONDO_ACTIVA: ui::Color = rgba(64, 64, 74, 0.96);
const BALDOSA_BORDE: ui::Color = rgba(255, 255, 255, 0.12);
const BALDOSA_BORDE_ACTIVA: ui::Color = rgba(255, 255, 255, 0.40);
const CENTRO_FONDO: ui::Color = rgba(20, 20, 24, 0.90);

impl Renderer {
    pub fn nuevo() -> Result<Self> {
        unsafe {
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let dc = CreateCompatibleDC(None);
            Ok(Renderer {
                factory,
                dwrite,
                target: None,
                pintor: None,
                dc,
                bitmap: HBITMAP::default(),
                anterior: HGDIOBJ::default(),
                ancho: 0,
                alto: 0,
            })
        }
    }

    pub fn dc(&self) -> HDC {
        self.dc
    }

    /// Asegura un DIB del tamaño pedido; solo se recrea si cambia.
    pub fn preparar(&mut self, ancho: i32, alto: i32) -> Result<()> {
        if ancho == self.ancho && alto == self.alto && !self.bitmap.is_invalid() {
            return Ok(());
        }
        unsafe {
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: ancho,
                    biHeight: -alto, // negativo: filas de arriba hacia abajo
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let nuevo = CreateDIBSection(Some(self.dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
            let previo = SelectObject(self.dc, nuevo.into());
            if self.bitmap.is_invalid() {
                self.anterior = previo;
            } else {
                let _ = DeleteObject(self.bitmap.into());
            }
            self.bitmap = nuevo;
        }
        self.ancho = ancho;
        self.alto = alto;
        Ok(())
    }

    fn target(&mut self) -> Result<ID2D1DCRenderTarget> {
        if let Some(t) = &self.target {
            return Ok(t.clone());
        }
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_GDI_COMPATIBLE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let t = unsafe { self.factory.CreateDCRenderTarget(&props)? };
        self.pintor = Some(Pintor::nuevo(t.cast()?, self.dwrite.clone())?);
        self.target = Some(t.clone());
        Ok(t)
    }

    /// Pinta la escena en el DIB. Después, el DIB está listo para `UpdateLayeredWindow`.
    pub fn dibujar(&mut self, escena: &Escena) -> Result<()> {
        let resultado = self.dibujar_en(escena);
        if resultado.is_err() {
            // Dispositivo perdido o similar: el próximo fotograma recrea el render target.
            self.target = None;
            self.pintor = None;
        }
        resultado
    }

    fn dibujar_en(&mut self, e: &Escena) -> Result<()> {
        let target = self.target()?;
        let area = RECT { left: 0, top: 0, right: self.ancho, bottom: self.alto };
        let (ancho, alto) = (self.ancho as f32, self.alto as f32);
        let Some(p) = self.pintor.as_mut() else { return Ok(()) };
        unsafe {
            target.BindDC(self.dc, &area)?;
            target.BeginDraw();
            target.SetTransform(&Matrix3x2::identity());
            target.Clear(Some(&tema::TRANSPARENTE));
        }
        if e.oscurecer {
            p.rellenar(rect(0.0, 0.0, ancho, alto), 0.0, rgba(0, 0, 0, 0.28 * e.progreso));
        }
        pintar_rueda(p, e);
        unsafe { target.EndDraw(None, None) }
    }
}

/// Dibuja la rueda con cualquier pintor: la ventana de la rueda y la vista previa de los ajustes
/// usan esta misma función. No toca el fondo; deja la transformación como la encontró.
pub fn pintar_rueda(p: &mut Pintor, e: &Escena) {
    let (cx, cy) = e.centro;
    let s = e.escala;
    let mut previa = Matrix3x2::identity();
    unsafe { p.rt.GetTransform(&mut previa) };
    // La rueda crece desde el 90 % alrededor de su centro mientras aparece.
    let z = 0.90 + 0.10 * e.progreso;
    let m = Matrix3x2 { M11: z, M12: 0.0, M21: 0.0, M22: z, M31: cx * (1.0 - z), M32: cy * (1.0 - z) };
    unsafe { p.rt.SetTransform(&(m * previa)) };

    let n = e.nombres.len();
    // Las baldosas salen desde el centro hacia su sitio.
    let radio = wheel::radio_anillo(n) * s * (0.55 + 0.45 * e.progreso);

    // Botón central: soltar aquí cancela.
    let rc = e.radio_central.clamp(18.0 * s, 26.0 * s);
    let en_centro = e.seleccion.is_none();
    p.circulo(cx, cy + 1.5 * s, rc + 1.0 * s, rgba(0, 0, 0, 0.25));
    p.circulo(cx, cy, rc, CENTRO_FONDO);
    p.borde_circulo(cx, cy, rc - 0.5, if en_centro { tema::BORDE_FUERTE } else { tema::BORDE }, 1.0);
    p.glifo(glifo::CERRAR, cx, cy, 12.0 * s, if en_centro { tema::TEXTO } else { tema::TEXTO_2 });

    let mut etiqueta: Option<(Rect, f32, f32)> = None;
    for i in 0..n {
        let activa = e.seleccion == Some(i);
        let a = wheel::angulo_centro(i, n);
        let (bx, by) = (cx + radio * a.cos(), cy + radio * a.sin());
        let lado = BALDOSA * s * if activa { 1.14 } else { 1.0 };
        let r = rect(bx - lado / 2.0, by - lado / 2.0, lado, lado);
        let esquina = lado * 0.27;

        p.sombra(r, esquina, if activa { 1.6 } else { 1.0 });
        p.rellenar(r, esquina, if activa { BALDOSA_FONDO_ACTIVA } else { BALDOSA_FONDO });
        p.bordear(r, esquina, if activa { BALDOSA_BORDE_ACTIVA } else { BALDOSA_BORDE }, 1.0);

        match e.iconos.get(i).and_then(|x| x.as_ref()) {
            Some(Visual::Imagen(icono)) => {
                let li = lado * 0.58;
                p.imagen(icono, rect(bx - li / 2.0, by - li / 2.0, li, li), 1.0);
            }
            Some(Visual::Lucide(nombre)) => {
                let li = lado * 0.46;
                p.lucide(nombre, rect(bx - li / 2.0, by - li / 2.0, li, li), tema::TEXTO);
            }
            None => {
                let inicial: String = e.nombres[i].chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
                p.texto(&inicial, r, 20.0 * s, Peso::Semi, Alin::Centro, tema::TEXTO);
            }
        }
        if activa {
            etiqueta = Some((r, a.cos(), a.sin()));
        }
    }

    // Etiqueta con el nombre, hacia fuera de la rueda: a un lado si la baldosa está a la
    // izquierda o a la derecha, encima o debajo si está arriba o abajo.
    if let (Some((r, cos, sin)), Some(i)) = (etiqueta, e.seleccion) {
        let nombre = &e.nombres[i];
        let tam = 12.5 * s;
        let (alto_e, pad) = (26.0 * s, 11.0 * s);
        let ancho_e = (p.medir(nombre, tam, Peso::Semi) + 2.0 * pad).min(260.0 * s);
        let sep = 10.0 * s;
        let (mx, my) = ((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0);
        let (x, y) = if cos > 0.35 {
            (r.right + sep, my - alto_e / 2.0)
        } else if cos < -0.35 {
            (r.left - sep - ancho_e, my - alto_e / 2.0)
        } else if sin < 0.0 {
            (mx - ancho_e / 2.0, r.top - sep - alto_e)
        } else {
            (mx - ancho_e / 2.0, r.bottom + sep)
        };
        let caja = rect(x, y, ancho_e, alto_e);
        p.sombra(caja, alto_e / 2.0, 0.8);
        p.rellenar(caja, alto_e / 2.0, tema::CLARO);
        p.texto(nombre, rect(x + pad, y, ancho_e - 2.0 * pad, alto_e), tam, Peso::Semi, Alin::Centro, tema::TEXTO_OSCURO);
    }

    if n == 0 {
        let caja = rect(cx - 200.0 * s, cy + 40.0 * s, 400.0 * s, 24.0 * s);
        p.texto("Sin accesos: agrégalos en los ajustes", caja, 12.5 * s, Peso::Medio, Alin::Centro, tema::TEXTO);
    }
    unsafe { p.rt.SetTransform(&previa) };
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            if !self.bitmap.is_invalid() {
                SelectObject(self.dc, self.anterior);
                let _ = DeleteObject(self.bitmap.into());
            }
            let _ = DeleteDC(self.dc);
        }
    }
}
