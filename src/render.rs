//! Dibujo de la rueda con Direct2D sobre un DIB de 32 bits, que luego se entrega a una ventana
//! "layered" con `UpdateLayeredWindow`. Así la transparencia es por píxel (las sombras, el
//! oscurecido del fondo) sin depender del compositor para nada más.
//!
//! Diseño: pétalos radiales con esquinas redondeadas alrededor de un botón central (soltar ahí cancela).
//! El pétalo al que apuntas se resalta en hover, y muestra su nombre en una etiqueta hacia fuera de la rueda.
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
use windows_numerics::{Matrix3x2, Vector2};

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

/// Crea la geometría vectorial de un segmento de pétalo curvado con esquinas redondeadas.
fn crear_segmento(
    factory: &ID2D1Factory,
    cx: f32,
    cy: f32,
    ang_ini: f32,
    ang_fin: f32,
    r_int: f32,
    r_ext: f32,
    escala: f32,
) -> Result<ID2D1PathGeometry> {
    let geo = unsafe { factory.CreatePathGeometry()? };
    let sink = unsafe { geo.Open()? };
    let v = |r: f32, a: f32| Vector2 { X: cx + r * a.cos(), Y: cy + r * a.sin() };

    let span = (ang_fin - ang_ini).abs();
    let rc_ext = (wheel::REDONDEO_ESQUINA * escala)
        .min((r_ext - r_int) * 0.35)
        .min(r_ext * span * 0.35);
    let rc_int = (7.0 * escala)
        .min((r_ext - r_int) * 0.25)
        .min(r_int * span * 0.30);

    unsafe {
        if rc_ext >= 1.0 && rc_int >= 1.0 {
            let da_ext = rc_ext / r_ext;
            let da_int = rc_int / r_int;
            sink.BeginFigure(v(r_int + rc_int, ang_ini), D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLine(v(r_ext - rc_ext, ang_ini));
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: v(r_ext, ang_ini + da_ext),
                size: D2D_SIZE_F { width: rc_ext, height: rc_ext },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: D2D1_ARC_SIZE_SMALL,
            });
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: v(r_ext, ang_fin - da_ext),
                size: D2D_SIZE_F { width: r_ext, height: r_ext },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: D2D1_ARC_SIZE_SMALL,
            });
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: v(r_ext - rc_ext, ang_fin),
                size: D2D_SIZE_F { width: rc_ext, height: rc_ext },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: D2D1_ARC_SIZE_SMALL,
            });
            sink.AddLine(v(r_int + rc_int, ang_fin));
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: v(r_int, ang_fin - da_int),
                size: D2D_SIZE_F { width: rc_int, height: rc_int },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: D2D1_ARC_SIZE_SMALL,
            });
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: v(r_int, ang_ini + da_int),
                size: D2D_SIZE_F { width: r_int, height: r_int },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE,
                arcSize: D2D1_ARC_SIZE_SMALL,
            });
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: v(r_int + rc_int, ang_ini),
                size: D2D_SIZE_F { width: rc_int, height: rc_int },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: D2D1_ARC_SIZE_SMALL,
            });
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        } else {
            sink.BeginFigure(v(r_int, ang_ini), D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLine(v(r_ext, ang_ini));
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: v(r_ext, ang_fin),
                size: D2D_SIZE_F { width: r_ext, height: r_ext },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: D2D1_ARC_SIZE_SMALL,
            });
            sink.AddLine(v(r_int, ang_fin));
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: v(r_int, ang_ini),
                size: D2D_SIZE_F { width: r_int, height: r_int },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE,
                arcSize: D2D1_ARC_SIZE_SMALL,
            });
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        }
        sink.Close()?;
    }
    Ok(geo)
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

    let n = e.baldosas.len();
    let r_int = wheel::radio_int(n) * s;
    let r_ext_base = wheel::radio_ext(n) * s;
    let op = if e.opacidad_fondo.is_finite() && e.opacidad_fondo > 0.0 { e.opacidad_fondo } else { 0.88 };

    let Ok(factory) = (unsafe { p.rt.GetFactory() }) else { return };

    let mut etiqueta: Option<(f32, f32)> = None;
    for i in 0..n {
        let activa = e.seleccion == Some(i);
        let (ang_ini, ang_fin) = wheel::angulos_sector(i, n);
        let ang_centro = (ang_ini + ang_fin) / 2.0;

        let r_ext = if activa { r_ext_base + 3.0 * s } else { r_ext_base };
        let geo = match crear_segmento(&factory, cx, cy, ang_ini, ang_fin, r_int, r_ext, s) {
            Ok(g) => g,
            Err(_) => continue,
        };

        // 1. Sombra suave para separar del escritorio o resplandor violeta en hover:
        if activa {
            p.bordear_geometria(&geo, rgba(142, 107, 196, 0.22), 8.0 * s);
            p.bordear_geometria(&geo, rgba(158, 122, 214, 0.38), 4.0 * s);
        } else {
            p.bordear_geometria(&geo, rgba(0, 0, 0, 0.20), 4.0 * s);
            if let Ok(geo_sombra) = crear_segmento(&factory, cx, cy + 2.5 * s, ang_ini, ang_fin, r_int, r_ext, s) {
                p.rellenar_geometria(&geo_sombra, rgba(0, 0, 0, 0.22));
            }
        }

        // 2. Relleno con gradiente elegante (glassmorphism):
        let p_int = Vector2 { X: cx + r_int * ang_centro.cos(), Y: cy + r_int * ang_centro.sin() };
        let p_ext = Vector2 { X: cx + r_ext * ang_centro.cos(), Y: cy + r_ext * ang_centro.sin() };

        if activa {
            let col_int = if e.color_hover != [0, 0, 0] {
                rgba(e.color_hover[0].saturating_sub(20), e.color_hover[1].saturating_sub(20), e.color_hover[2].saturating_sub(20), 0.94)
            } else {
                rgba(92, 60, 145, 0.95)
            };
            let col_ext = if e.color_hover != [0, 0, 0] {
                rgba(e.color_hover[0], e.color_hover[1], e.color_hover[2], 0.98)
            } else {
                rgba(156, 118, 212, 0.98)
            };
            p.rellenar_geometria_gradiente(&geo, p_int, col_int, p_ext, col_ext);
            p.bordear_geometria(&geo, rgba(225, 202, 255, 0.85), 1.5 * s);
        } else {
            let col_int = rgba(22, 26, 36, 0.85 * op);
            let col_ext = rgba(38, 46, 62, 0.92 * op);
            p.rellenar_geometria_gradiente(&geo, p_int, col_int, p_ext, col_ext);
            p.bordear_geometria(&geo, rgba(255, 255, 255, 0.16), 1.1 * s);
        }

        // 3. Icono y texto:
        let b = &e.baldosas[i];
        let r_medio = (r_int + r_ext) / 2.0;
        let (px, py) = (cx + r_medio * ang_centro.cos(), cy + r_medio * ang_centro.sin());
        // Los nombres dentro de cada pétalo se solapaban y desplazaban los íconos. El ícono
        // permanece centrado y el nombre seleccionado se dibuja en una sola etiqueta exterior.
        let (ix, iy, lado) = (px, py, 34.0 * s * if activa { 1.06 } else { 1.0 });

        match (&b.visual, &b.grupo) {
            (Some(v), _) => pintar_visual(p, v, ix, iy, lado),
            (None, Some(sub)) if !sub.is_empty() => {
                let celda = lado * 0.32;
                let paso = celda + lado * 0.08;
                let filas = sub.len().min(4).div_ceil(2) as f32;
                for (k, v) in sub.iter().enumerate().take(4) {
                    let (fx, fy) = ((k % 2) as f32, (k / 2) as f32);
                    let mx = ix + (fx - 0.5) * paso;
                    let my = iy + (fy - (filas - 1.0) / 2.0) * paso;
                    match v {
                        Some(v) => pintar_visual(p, v, mx, my, celda / 0.58),
                        None => p.rellenar(rect(mx - celda / 2.0, my - celda / 2.0, celda, celda), celda * 0.25, tema::CONTROL_ACTIVO),
                    }
                }
            }
            (None, Some(_)) => {
                let li = lado * 0.52;
                if let Some(n) = crate::lucide::nombre("folder") {
                    p.lucide(n, rect(ix - li / 2.0, iy - li / 2.0, li, li), tema::TEXTO);
                }
            }
            (None, None) => {
                let inicial: String = b.nombre.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
                let r_ini = rect(ix - lado / 2.0, iy - lado / 2.0, lado, lado);
                p.texto(&inicial, r_ini, 18.0 * s, Peso::Semi, Alin::Centro, tema::TEXTO);
            }
        }

        if activa && e.mostrar_nombres {
            etiqueta = Some((ang_centro, r_ext));
        }
    }

    // Botón central: soltar aquí cancela.
    let rc = e.radio_central.clamp(19.0 * s, 23.0 * s);
    let en_centro = e.seleccion.is_none();
    p.circulo(cx, cy + 2.0 * s, rc + 1.0 * s, rgba(0, 0, 0, 0.35));
    p.circulo(cx, cy, rc, rgba(16, 18, 24, 0.95));
    p.borde_circulo(cx, cy, rc, if en_centro { rgba(255, 255, 255, 0.35) } else { rgba(255, 255, 255, 0.12) }, 1.0 * s);
    if e.grupo.is_some() {
        let d = 5.0 * s;
        let col = if en_centro { tema::TEXTO } else { tema::TEXTO_2 };
        p.linea(cx + d * 0.6, cy - d, cx - d * 0.6, cy, col, 1.6 * s);
        p.linea(cx - d * 0.6, cy, cx + d * 0.6, cy + d, col, 1.6 * s);
    } else {
        let d = 4.8 * s;
        let col = if en_centro { rgba(255, 255, 255, 1.0) } else { rgba(255, 255, 255, 0.85) };
        p.linea(cx - d, cy - d, cx + d, cy + d, col, 1.6 * s);
        p.linea(cx + d, cy - d, cx - d, cy + d, col, 1.6 * s);
    }

    // Dentro de un grupo, su nombre bajo el botón central.
    if let Some(nombre) = e.grupo {
        let tam = 11.0 * s;
        let (alto_e, pad) = (22.0 * s, 9.0 * s);
        let ancho_e = (p.medir(nombre, tam, Peso::Semi) + 2.0 * pad).min(180.0 * s);
        let caja = rect(cx - ancho_e / 2.0, cy + rc + 10.0 * s, ancho_e, alto_e);
        p.rellenar(caja, alto_e / 2.0, CENTRO_FONDO);
        p.bordear(caja, alto_e / 2.0, tema::BORDE, 1.0);
        p.texto(nombre, rect(caja.left + pad, caja.top, ancho_e - 2.0 * pad, alto_e), tam, Peso::Semi, Alin::Centro, tema::TEXTO);
    }

    // Etiqueta con el nombre, hacia fuera de la rueda.
    if let (Some((a, r_borde)), Some(i)) = (etiqueta, e.seleccion) {
        let nombre = &e.baldosas[i].nombre;
        let tam = 12.0 * s;
        let (alto_e, pad) = (26.0 * s, 12.0 * s);
        let ancho_e = (p.medir(nombre, tam, Peso::Semi) + 2.0 * pad).min(260.0 * s);
        let sep = 16.0 * s;
        let cos = a.cos();
        let sin = a.sin();
        let (ox, oy) = (cx + (r_borde + sep) * cos, cy + (r_borde + sep) * sin);
        let (x, y) = if cos > 0.35 {
            (ox, oy - alto_e / 2.0)
        } else if cos < -0.35 {
            (ox - ancho_e, oy - alto_e / 2.0)
        } else if sin < 0.0 {
            (ox - ancho_e / 2.0, oy - alto_e)
        } else {
            (ox - ancho_e / 2.0, oy)
        };
        // La rueda puede abrirse junto al borde del monitor. Mantener la etiqueta dentro del
        // render target evita que los nombres laterales o largos queden recortados.
        let tam_area = unsafe { p.rt.GetSize() };
        let margen = 8.0 * s;
        let x = x.clamp(margen, (tam_area.width - ancho_e - margen).max(margen));
        let y = y.clamp(margen, (tam_area.height - alto_e - margen).max(margen));
        let caja = rect(x, y, ancho_e, alto_e);
        p.sombra(caja, alto_e / 2.0, 0.9);
        p.rellenar(caja, alto_e / 2.0, rgba(28, 32, 44, 0.95));
        p.bordear(caja, alto_e / 2.0, rgba(255, 255, 255, 0.20), 1.0);
        p.texto(nombre, rect(x + pad, y, ancho_e - 2.0 * pad, alto_e), tam, Peso::Semi, Alin::Centro, tema::TEXTO);
    }

    if n == 0 {
        let caja = rect(cx - 200.0 * s, cy + 70.0 * s, 400.0 * s, 24.0 * s);
        let aviso = if e.grupo.is_some() { "Grupo vacío: vuelve al centro para salir" } else { "Sin accesos: agrégalos en los ajustes" };
        p.texto(aviso, caja, 12.5 * s, Peso::Medio, Alin::Centro, tema::TEXTO);
    }
    unsafe { p.rt.SetTransform(&previa) };
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
