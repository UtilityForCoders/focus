//! Dibujo de la rueda con Direct2D sobre un DIB de 32 bits, que luego se entrega a una ventana
//! "layered" con `UpdateLayeredWindow`. Así la transparencia es por píxel (las sombras, el
//! oscurecido del fondo) sin depender del compositor para nada más.
//!
//! Diseño: botones independientes distribuidos sobre una órbita alrededor de un control central.
//! El botón apuntado se resalta y muestra su nombre hacia fuera de la rueda.
//!
//! Todo se mide en píxeles físicos: la escala (DPI del monitor por el tamaño elegido) llega en
//! `Escena::escala` y se aplica a las medidas, no al render target (que trabaja a 96 ppp).

use crate::config::{Config, Grupo};
use crate::icons::{self, Visual};
use crate::ui::{self, rect, rgba, tema, Alin, Peso, Pintor};
use crate::wheel;
use windows::core::{Interface, Result};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows_numerics::Matrix3x2;

/// Una baldosa de la rueda: un acceso o un grupo.
#[derive(Clone)]
pub struct Baldosa {
    pub nombre: String,
    /// Sin ícono se dibuja la inicial (o, en un grupo, las miniaturas de sus accesos).
    pub visual: Option<Visual>,
    /// En un grupo, los íconos de sus primeros accesos; `None` si es un acceso.
    pub grupo: Option<Vec<Option<Visual>>>,
}

impl Baldosa {
    pub fn es_grupo(&self) -> bool {
        self.grupo.is_some()
    }
}

/// Baldosas de la rueda principal: los grupos (y accesos sueltos, si un config viejo los trae).
pub fn baldosas_raiz(cfg: &Config) -> Vec<Baldosa> {
    let grupos = cfg.grupos.iter().map(|g| Baldosa {
        nombre: g.nombre.clone(),
        visual: g.icono.as_deref().and_then(icons::visual_propio),
        grupo: Some(g.accesos.iter().take(4).map(icons::visual).collect()),
    });
    let sueltos = cfg.accesos.iter().map(|a| Baldosa { nombre: a.nombre.clone(), visual: icons::visual(a), grupo: None });
    grupos.chain(sueltos).collect()
}

/// Baldosas de un grupo abierto.
pub fn baldosas_grupo(g: &Grupo) -> Vec<Baldosa> {
    g.accesos.iter().map(|a| Baldosa { nombre: a.nombre.clone(), visual: icons::visual(a), grupo: None }).collect()
}

/// Lo que hace falta para pintar un fotograma.
pub struct Escena<'a> {
    /// Centro de la rueda, relativo a la esquina de la ventana.
    pub centro: (f32, f32),
    /// DPI del monitor / 96, multiplicado por el tamaño elegido en ajustes.
    pub escala: f32,
    pub baldosas: &'a [Baldosa],
    /// Nombre del grupo abierto; `None` en la rueda principal. Con un grupo abierto, el botón
    /// central muestra "volver".
    pub grupo: Option<&'a str>,
    pub seleccion: Option<usize>,
    /// Progreso de la animación de apertura, de 0 a 1 (ya suavizado).
    pub progreso: f32,
    /// Progreso de la espera para entrar al grupo seleccionado, de 0 a 1.
    pub progreso_grupo: f32,
    pub oscurecer: bool,
    /// Radio de la zona central (cancelar), en píxeles físicos.
    pub radio_central: f32,
    pub mostrar_nombres: bool,
    pub color_hover: [u8; 3],
    pub opacidad_fondo: f32,
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

/// Dibuja el avance de permanencia como un arco alrededor de un botón orbital.
fn pintar_progreso(p: &Pintor, cx: f32, cy: f32, radio: f32, progreso: f32, escala: f32) {
    let pasos = 48usize;
    let visibles = (progreso.clamp(0.0, 1.0) * pasos as f32).ceil() as usize;
    for i in 0..visibles {
        let a0 = -std::f32::consts::PI / 2.0 + std::f32::consts::TAU * i as f32 / pasos as f32;
        let a1 = -std::f32::consts::PI / 2.0 + std::f32::consts::TAU * (i + 1) as f32 / pasos as f32;
        p.linea(
            cx + radio * a0.cos(),
            cy + radio * a0.sin(),
            cx + radio * a1.cos(),
            cy + radio * a1.sin(),
            rgba(220, 196, 255, 0.95),
            2.6 * escala,
        );
    }
}

/// Dibuja la rueda con cualquier pintor: la ventana de la rueda y la vista previa de los ajustes
/// usan esta misma función.
pub fn pintar_rueda(p: &mut Pintor, e: &Escena) {
    let (cx, cy) = e.centro;
    let s = e.escala;
    let n = e.baldosas.len();
    let op = if e.opacidad_fondo.is_finite() && e.opacidad_fondo > 0.0 { e.opacidad_fondo } else { 0.88 };
    let radio_final = wheel::radio_anillo(n) * s;
    let radio_orbita = radio_final * (0.58 + 0.42 * e.progreso);

    // Una guía tenue da estructura sin convertir cada opción en un sector pesado.
    if n > 1 {
        p.borde_circulo(cx, cy, radio_orbita, rgba(255, 255, 255, 0.07), 1.0 * s);
        p.borde_circulo(cx, cy, radio_orbita - 2.0 * s, rgba(0, 0, 0, 0.16), 1.0 * s);
    }

    let hover = if e.color_hover == [0, 0, 0] { [112, 78, 168] } else { e.color_hover };
    let mut etiqueta: Option<(f32, f32)> = None;

    for (i, b) in e.baldosas.iter().enumerate() {
        let activa = e.seleccion == Some(i);
        let angulo = wheel::angulo_centro(i, n);
        let (px, py) = (
            cx + radio_orbita * angulo.cos(),
            cy + radio_orbita * angulo.sin(),
        );
        let radio = (wheel::BOTON / 2.0 + if activa { 4.0 } else { 0.0 }) * s;

        // Sombra, resplandor y superficie del botón independiente.
        p.circulo(px, py + 3.0 * s, radio + 3.0 * s, rgba(0, 0, 0, 0.28));
        if activa {
            p.circulo(px, py, radio + 9.0 * s, rgba(hover[0], hover[1], hover[2], 0.13));
            p.circulo(px, py, radio + 4.0 * s, rgba(hover[0], hover[1], hover[2], 0.24));
            p.circulo(
                px,
                py,
                radio,
                rgba(
                    hover[0].saturating_sub(22),
                    hover[1].saturating_sub(22),
                    hover[2].saturating_sub(22),
                    0.98,
                ),
            );
            p.borde_circulo(px, py, radio, rgba(236, 220, 255, 0.88), 1.5 * s);
        } else {
            p.circulo(px, py, radio, rgba(27, 32, 44, 0.54 + 0.42 * op));
            p.borde_circulo(px, py, radio, rgba(255, 255, 255, 0.17), 1.0 * s);
            p.borde_circulo(px, py, radio - 2.0 * s, rgba(255, 255, 255, 0.04), 1.0 * s);
        }

        let lado = 38.0 * s * if activa { 1.06 } else { 1.0 };
        match (&b.visual, &b.grupo) {
            (Some(v), _) => pintar_visual(p, v, px, py, lado),
            (None, Some(sub)) if !sub.is_empty() => {
                let celda = lado * 0.31;
                let paso = celda + lado * 0.09;
                let filas = sub.len().min(4).div_ceil(2) as f32;
                for (k, v) in sub.iter().enumerate().take(4) {
                    let (fx, fy) = ((k % 2) as f32, (k / 2) as f32);
                    let mx = px + (fx - 0.5) * paso;
                    let my = py + (fy - (filas - 1.0) / 2.0) * paso;
                    match v {
                        Some(v) => pintar_visual(p, v, mx, my, celda / 0.58),
                        None => p.circulo(mx, my, celda / 2.0, rgba(255, 255, 255, 0.18)),
                    }
                }
            }
            (None, Some(_)) => {
                let li = lado * 0.58;
                if let Some(nombre) = crate::lucide::nombre("folder") {
                    p.lucide(nombre, rect(px - li / 2.0, py - li / 2.0, li, li), tema::TEXTO);
                }
            }
            (None, None) => {
                let inicial: String = b.nombre.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
                p.texto(
                    &inicial,
                    rect(px - lado / 2.0, py - lado / 2.0, lado, lado),
                    18.0 * s,
                    Peso::Semi,
                    Alin::Centro,
                    tema::TEXTO,
                );
            }
        }

        // Un distintivo pequeño separa visualmente los grupos de las aplicaciones.
        if b.es_grupo() {
            let (bx, by) = (px + radio * 0.68, py + radio * 0.68);
            let rb = 7.0 * s;
            p.circulo(bx, by, rb + 1.0 * s, rgba(12, 14, 20, 0.92));
            p.circulo(bx, by, rb, rgba(hover[0], hover[1], hover[2], 1.0));
            let d = 2.3 * s;
            p.linea(bx - d, by - d, bx + d, by, tema::TEXTO, 1.4 * s);
            p.linea(bx + d, by, bx - d, by + d, tema::TEXTO, 1.4 * s);
        }

        if activa && b.es_grupo() && e.progreso_grupo > 0.0 {
            pintar_progreso(p, px, py, radio + 6.0 * s, e.progreso_grupo, s);
        }
        if activa && e.mostrar_nombres {
            etiqueta = Some((angulo, radio_orbita + radio));
        }
    }

    // El centro conserva una acción inequívoca: cerrar en la raíz y volver dentro de un grupo.
    let rc = e.radio_central.clamp(20.0 * s, 25.0 * s);
    let en_centro = e.seleccion.is_none();
    p.circulo(cx, cy + 2.5 * s, rc + 2.0 * s, rgba(0, 0, 0, 0.38));
    p.circulo(cx, cy, rc, rgba(18, 21, 29, 0.97));
    p.borde_circulo(
        cx,
        cy,
        rc,
        if en_centro { rgba(211, 190, 244, 0.65) } else { rgba(255, 255, 255, 0.14) },
        1.2 * s,
    );
    if e.grupo.is_some() {
        let d = 5.0 * s;
        let col = if en_centro { tema::TEXTO } else { tema::TEXTO_2 };
        p.linea(cx + d * 0.6, cy - d, cx - d * 0.6, cy, col, 1.7 * s);
        p.linea(cx - d * 0.6, cy, cx + d * 0.6, cy + d, col, 1.7 * s);
    } else {
        let d = 4.8 * s;
        let col = if en_centro { tema::TEXTO } else { tema::TEXTO_2 };
        p.linea(cx - d, cy - d, cx + d, cy + d, col, 1.7 * s);
        p.linea(cx + d, cy - d, cx - d, cy + d, col, 1.7 * s);
    }

    if let Some(nombre) = e.grupo {
        let tam = 11.0 * s;
        let (alto, pad) = (22.0 * s, 9.0 * s);
        let ancho = (p.medir(nombre, tam, Peso::Semi) + 2.0 * pad).min(180.0 * s);
        let caja = rect(cx - ancho / 2.0, cy + rc + 10.0 * s, ancho, alto);
        p.rellenar(caja, alto / 2.0, CENTRO_FONDO);
        p.bordear(caja, alto / 2.0, tema::BORDE, 1.0);
        p.texto(nombre, rect(caja.left + pad, caja.top, ancho - 2.0 * pad, alto), tam, Peso::Semi, Alin::Centro, tema::TEXTO);
    }

    if let (Some((angulo, borde)), Some(i)) = (etiqueta, e.seleccion) {
        let nombre = &e.baldosas[i].nombre;
        let tam = 12.0 * s;
        let (alto, pad) = (28.0 * s, 13.0 * s);
        let ancho = (p.medir(nombre, tam, Peso::Semi) + 2.0 * pad).min(260.0 * s);
        let sep = 13.0 * s;
        let cos = angulo.cos();
        let sin = angulo.sin();
        let (ox, oy) = (cx + (borde + sep) * cos, cy + (borde + sep) * sin);
        let (x, y) = if cos > 0.35 {
            (ox, oy - alto / 2.0)
        } else if cos < -0.35 {
            (ox - ancho, oy - alto / 2.0)
        } else if sin < 0.0 {
            (ox - ancho / 2.0, oy - alto)
        } else {
            (ox - ancho / 2.0, oy)
        };
        let area = unsafe { p.rt.GetSize() };
        let margen = 8.0 * s;
        let x = x.clamp(margen, (area.width - ancho - margen).max(margen));
        let y = y.clamp(margen, (area.height - alto - margen).max(margen));
        let caja = rect(x, y, ancho, alto);
        p.sombra(caja, alto / 2.0, 0.9);
        p.rellenar(caja, alto / 2.0, rgba(25, 29, 40, 0.97));
        p.bordear(caja, alto / 2.0, rgba(255, 255, 255, 0.18), 1.0);
        p.texto(nombre, rect(x + pad, y, ancho - 2.0 * pad, alto), tam, Peso::Semi, Alin::Centro, tema::TEXTO);
    }

    if n == 0 {
        let caja = rect(cx - 200.0 * s, cy + 70.0 * s, 400.0 * s, 24.0 * s);
        let aviso = if e.grupo.is_some() { "Grupo vacío: vuelve al centro para salir" } else { "Sin accesos: agrégalos en los ajustes" };
        p.texto(aviso, caja, 12.5 * s, Peso::Medio, Alin::Centro, tema::TEXTO);
    }
}

/// Ícono de una baldosa de lado `lado` centrada en (`cx`, `cy`).
fn pintar_visual(p: &mut Pintor, v: &Visual, cx: f32, cy: f32, lado: f32) {
    match v {
        Visual::Imagen(icono) => {
            let li = lado * 0.70;
            p.imagen(icono, rect(cx - li / 2.0, cy - li / 2.0, li, li), 1.0);
        }
        Visual::Lucide(nombre) => {
            let li = lado * 0.62;
            p.lucide(nombre, rect(cx - li / 2.0, cy - li / 2.0, li, li), tema::TEXTO);
        }
    }
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
