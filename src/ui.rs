//! Kit de dibujo compartido por la rueda y la ventana de ajustes: tema oscuro, rectángulos
//! redondeados, texto, glifos e imágenes sobre cualquier render target de Direct2D.
//!
//! Los glifos salen de la fuente de íconos del sistema: **Segoe Fluent Icons** en Windows 11 y
//! **Segoe MDL2 Assets** en Windows 10 (los mismos códigos sirven para los que se usan aquí).
//! Así no hace falta incluir íconos propios ni dibujarlos a mano.

use crate::icons::{Icono, Visual};
use crate::lucide::{self, Cmd};
use std::collections::HashMap;
use windows::core::{w, Result, HSTRING, PCWSTR};
use windows::core::BOOL;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows_numerics::{Matrix3x2, Vector2};

pub type Color = D2D1_COLOR_F;
pub type Rect = D2D_RECT_F;

pub const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color {
    Color { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a }
}

/// Colores del tema. Todo es blanco o negro con distinta opacidad, para que funcione igual sobre
/// el fondo translúcido de la ventana que sobre el escritorio.
pub mod tema {
    use super::{rgba, Color};
    pub const TEXTO: Color = rgba(242, 242, 245, 1.0);
    pub const TEXTO_2: Color = rgba(160, 160, 170, 1.0);
    pub const TEXTO_3: Color = rgba(112, 112, 122, 1.0);
    pub const TEXTO_OSCURO: Color = rgba(20, 20, 24, 1.0);
    pub const CONTROL: Color = rgba(255, 255, 255, 0.06);
    pub const CONTROL_HOVER: Color = rgba(255, 255, 255, 0.11);
    pub const CONTROL_ACTIVO: Color = rgba(255, 255, 255, 0.16);
    pub const BORDE: Color = rgba(255, 255, 255, 0.08);
    pub const BORDE_FUERTE: Color = rgba(255, 255, 255, 0.18);
    pub const SEPARADOR: Color = rgba(255, 255, 255, 0.06);
    pub const CLARO: Color = rgba(240, 240, 244, 0.95);
    pub const PELIGRO: Color = rgba(255, 110, 110, 1.0);
    pub const TRANSPARENTE: Color = rgba(0, 0, 0, 0.0);
}

/// Códigos de la fuente de íconos del sistema.
pub mod glifo {
    pub const AJUSTES: char = '\u{E713}';
    pub const RATON: char = '\u{E962}';
    pub const COLOR: char = '\u{E790}';
    pub const APPS: char = '\u{E71D}';
    pub const INFO: char = '\u{E946}';
    pub const BUSCAR: char = '\u{E721}';
    pub const ARRIBA: char = '\u{E70E}';
    pub const ABAJO: char = '\u{E70D}';
    pub const BORRAR: char = '\u{E74D}';
    pub const MAS: char = '\u{E710}';
    pub const CHECK: char = '\u{E73E}';
    pub const CERRAR: char = '\u{E711}';
    pub const CARPETA: char = '\u{E838}';
    pub const RECARGAR: char = '\u{E72C}';
    pub const ATRAS: char = '\u{E72B}';
    pub const IZQUIERDA: char = '\u{E76B}';
    pub const DERECHA: char = '\u{E76C}';
    pub const APAGAR: char = '\u{E7E8}';
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Alin {
    Izq,
    Centro,
    Der,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Peso {
    Normal,
    Medio,
    Semi,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct ClaveFormato {
    tam: u32,
    peso: Peso,
    alin: Alin,
    multilinea: bool,
    glifo: bool,
}

pub fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { left: x, top: y, right: x + w, bottom: y + h }
}

pub fn dentro(r: &Rect, x: f32, y: f32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

pub struct Pintor {
    pub rt: ID2D1RenderTarget,
    pincel: ID2D1SolidColorBrush,
    dwrite: IDWriteFactory,
    fuente_glifos: HSTRING,
    formatos: HashMap<ClaveFormato, IDWriteTextFormat>,
    bitmaps: HashMap<String, ID2D1Bitmap>,
    geometrias: HashMap<&'static str, ID2D1PathGeometry>,
    trazo: Option<ID2D1StrokeStyle>,
}

impl Pintor {
    pub fn nuevo(rt: ID2D1RenderTarget, dwrite: IDWriteFactory) -> Result<Self> {
        let pincel = unsafe { rt.CreateSolidColorBrush(&tema::TEXTO, None)? };
        let fuente_glifos = HSTRING::from(if existe_fuente(&dwrite, w!("Segoe Fluent Icons")) {
            "Segoe Fluent Icons"
        } else {
            "Segoe MDL2 Assets"
        });
        Ok(Pintor {
            rt,
            pincel,
            dwrite,
            fuente_glifos,
            formatos: HashMap::new(),
            bitmaps: HashMap::new(),
            geometrias: HashMap::new(),
            trazo: None,
        })
    }

    pub fn color(&self, c: &Color) -> &ID2D1SolidColorBrush {
        unsafe { self.pincel.SetColor(c) };
        &self.pincel
    }

    pub fn rellenar_geometria(&self, geo: &ID2D1Geometry, c: Color) {
        unsafe { self.rt.FillGeometry(geo, self.color(&c), None) };
    }

    pub fn rellenar_geometria_gradiente(
        &self,
        geo: &ID2D1Geometry,
        p0: Vector2,
        c0: Color,
        p1: Vector2,
        c1: Color,
    ) {
        let stops = [
            D2D1_GRADIENT_STOP { position: 0.0, color: c0 },
            D2D1_GRADIENT_STOP { position: 1.0, color: c1 },
        ];
        unsafe {
            if let Ok(col) = self.rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) {
                let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                    startPoint: p0,
                    endPoint: p1,
                };
                if let Ok(brush) = self.rt.CreateLinearGradientBrush(&props, None, &col) {
                    self.rt.FillGeometry(geo, &brush, None);
                }
            }
        }
    }

    pub fn bordear_geometria(&self, geo: &ID2D1Geometry, c: Color, grosor: f32) {
        unsafe { self.rt.DrawGeometry(geo, self.color(&c), grosor, None) };
    }

    pub fn rellenar(&self, r: Rect, radio: f32, c: Color) {
        unsafe {
            if radio <= 0.0 {
                self.rt.FillRectangle(&r, self.color(&c));
            } else {
                let rr = D2D1_ROUNDED_RECT { rect: r, radiusX: radio, radiusY: radio };
                self.rt.FillRoundedRectangle(&rr, self.color(&c));
            }
        }
    }

    pub fn bordear(&self, r: Rect, radio: f32, c: Color, grosor: f32) {
        // Medio píxel hacia dentro: el trazo queda nítido y dentro del rectángulo.
        let m = grosor / 2.0;
        let r = Rect { left: r.left + m, top: r.top + m, right: r.right - m, bottom: r.bottom - m };
        let rr = D2D1_ROUNDED_RECT { rect: r, radiusX: radio, radiusY: radio };
        unsafe { self.rt.DrawRoundedRectangle(&rr, self.color(&c), grosor, None) };
    }

    pub fn circulo(&self, cx: f32, cy: f32, radio: f32, c: Color) {
        let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: radio, radiusY: radio };
        unsafe { self.rt.FillEllipse(&e, self.color(&c)) };
    }

    pub fn borde_circulo(&self, cx: f32, cy: f32, radio: f32, c: Color, grosor: f32) {
        let e = D2D1_ELLIPSE { point: Vector2 { X: cx, Y: cy }, radiusX: radio, radiusY: radio };
        unsafe { self.rt.DrawEllipse(&e, self.color(&c), grosor, None) };
    }

    pub fn linea(&self, x0: f32, y0: f32, x1: f32, y1: f32, c: Color, grosor: f32) {
        unsafe {
            self.rt.DrawLine(Vector2 { X: x0, Y: y0 }, Vector2 { X: x1, Y: y1 }, self.color(&c), grosor, None);
        }
    }

    pub fn recortar(&self, r: Rect) {
        unsafe { self.rt.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_ALIASED) };
    }

    pub fn fin_recorte(&self) {
        unsafe { self.rt.PopAxisAlignedClip() };
    }

    fn formato(&mut self, clave: ClaveFormato) -> Option<IDWriteTextFormat> {
        if let Some(f) = self.formatos.get(&clave) {
            return Some(f.clone());
        }
        let peso = match clave.peso {
            Peso::Normal => DWRITE_FONT_WEIGHT_NORMAL,
            Peso::Medio => DWRITE_FONT_WEIGHT_MEDIUM,
            Peso::Semi => DWRITE_FONT_WEIGHT_SEMI_BOLD,
        };
        let familia: PCWSTR = if clave.glifo { PCWSTR(self.fuente_glifos.as_ptr()) } else { w!("Segoe UI Variable Text") };
        unsafe {
            let f = self
                .dwrite
                .CreateTextFormat(
                    familia,
                    None,
                    peso,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    clave.tam as f32 / 10.0,
                    w!("es-es"),
                )
                .ok()?;
            let alin = match clave.alin {
                Alin::Izq => DWRITE_TEXT_ALIGNMENT_LEADING,
                Alin::Centro => DWRITE_TEXT_ALIGNMENT_CENTER,
                Alin::Der => DWRITE_TEXT_ALIGNMENT_TRAILING,
            };
            let _ = f.SetTextAlignment(alin);
            if clave.multilinea {
                let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
                let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);
            } else {
                let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
                let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
                if let Ok(elipsis) = self.dwrite.CreateEllipsisTrimmingSign(&f) {
                    let recorte = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
                    let _ = f.SetTrimming(&recorte, &elipsis);
                }
            }
            self.formatos.insert(clave, f.clone());
            Some(f)
        }
    }

    fn clave(tam: f32, peso: Peso, alin: Alin, multilinea: bool, glifo: bool) -> ClaveFormato {
        ClaveFormato { tam: (tam * 10.0).round() as u32, peso, alin, multilinea, glifo }
    }

    /// Una línea de texto centrada verticalmente en `r`; si no cabe, termina en "…".
    pub fn texto(&mut self, s: &str, r: Rect, tam: f32, peso: Peso, alin: Alin, c: Color) {
        let Some(f) = self.formato(Self::clave(tam, peso, alin, false, false)) else { return };
        let t: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            self.rt.DrawText(&t, &f, &r, self.color(&c), D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
        }
    }

    /// Texto que salta de línea dentro de `r`, desde arriba.
    pub fn parrafo(&mut self, s: &str, r: Rect, tam: f32, c: Color) {
        let Some(f) = self.formato(Self::clave(tam, Peso::Normal, Alin::Izq, true, false)) else { return };
        let t: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            self.rt.DrawText(&t, &f, &r, self.color(&c), D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        }
    }

    /// Ancho de una línea de texto.
    pub fn medir(&mut self, s: &str, tam: f32, peso: Peso) -> f32 {
        self.medir_hasta(s, tam, peso, s.encode_utf16().count())
    }

    /// Ancho de las primeras `unidades` (UTF-16) de `s`: la posición del cursor de un campo.
    pub fn medir_hasta(&mut self, s: &str, tam: f32, peso: Peso, unidades: usize) -> f32 {
        let Some(f) = self.formato(Self::clave(tam, peso, Alin::Izq, false, false)) else { return 0.0 };
        let t: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let Ok(capa) = self.dwrite.CreateTextLayout(&t, &f, 10_000.0, 100.0) else { return 0.0 };
            if unidades >= t.len() {
                let mut m = DWRITE_TEXT_METRICS::default();
                let _ = capa.GetMetrics(&mut m);
                return m.widthIncludingTrailingWhitespace;
            }
            let (mut x, mut y) = (0.0, 0.0);
            let mut hm = DWRITE_HIT_TEST_METRICS::default();
            let _ = capa.HitTestTextPosition(unidades as u32, false, &mut x, &mut y, &mut hm);
            x
        }
    }

    pub fn glifo(&mut self, g: char, cx: f32, cy: f32, tam: f32, c: Color) {
        let Some(f) = self.formato(Self::clave(tam, Peso::Normal, Alin::Centro, false, true)) else { return };
        let mut buf = [0u16; 2];
        let t = g.encode_utf16(&mut buf);
        let r = rect(cx - tam, cy - tam, tam * 2.0, tam * 2.0);
        unsafe {
            self.rt.DrawText(t, &f, &r, self.color(&c), D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL);
        }
    }

    /// Escala real de dibujo: DPI del render target por la escala de la transformación actual.
    fn escala_real(&self) -> f32 {
        let (mut dx, mut dy) = (96.0f32, 96.0f32);
        let mut t = Matrix3x2::identity();
        unsafe {
            self.rt.GetDpi(&mut dx, &mut dy);
            self.rt.GetTransform(&mut t);
        }
        dx / 96.0 * (t.M11 * t.M11 + t.M12 * t.M12).sqrt()
    }

    /// Dibuja un ícono con la copia reducida más cercana al tamaño en pantalla. El bitmap de
    /// Direct2D se crea una vez por copia y render target.
    pub fn imagen(&mut self, icono: &Icono, r: Rect, opacidad: f32) {
        let lado = (r.right - r.left) * self.escala_real();
        let (nivel, n) = icono.nivel_para(lado);
        let clave = format!("{}#{nivel}", icono.clave);
        let b = match self.bitmaps.get(&clave) {
            Some(b) => b.clone(),
            None => {
                let props = D2D1_BITMAP_PROPERTIES {
                    pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                    dpiX: 96.0,
                    dpiY: 96.0,
                };
                let creado = unsafe {
                    self.rt.CreateBitmap(D2D_SIZE_U { width: n.ancho, height: n.alto }, Some(n.pixeles.as_ptr().cast()), n.ancho * 4, &props)
                };
                let Ok(b) = creado else { return };
                self.bitmaps.insert(clave, b.clone());
                b
            }
        };
        unsafe {
            self.rt.DrawBitmap(&b, Some(&r), opacidad, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
        }
    }

    /// Un glifo de Lucide dentro de `r` (cuadrado), con trazo redondeado como el original.
    pub fn lucide(&mut self, nombre: &'static str, r: Rect, c: Color) {
        let Some(geo) = self.geometria(nombre) else { return };
        if self.trazo.is_none() {
            let props = D2D1_STROKE_STYLE_PROPERTIES {
                startCap: D2D1_CAP_STYLE_ROUND,
                endCap: D2D1_CAP_STYLE_ROUND,
                dashCap: D2D1_CAP_STYLE_ROUND,
                lineJoin: D2D1_LINE_JOIN_ROUND,
                miterLimit: 10.0,
                dashStyle: D2D1_DASH_STYLE_SOLID,
                dashOffset: 0.0,
            };
            self.trazo = unsafe { self.rt.GetFactory().ok().and_then(|f| f.CreateStrokeStyle(&props, None).ok()) };
        }
        let lado = (r.right - r.left).min(r.bottom - r.top);
        let s = lado / 24.0;
        let mut previa = Matrix3x2::identity();
        unsafe {
            self.rt.GetTransform(&mut previa);
            let m = Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: r.left, M32: r.top };
            self.rt.SetTransform(&(m * previa));
            self.rt.DrawGeometry(&geo, self.color(&c), 2.0, self.trazo.as_ref());
            self.rt.SetTransform(&previa);
        }
    }

    /// Imagen o glifo, centrado en `r`.
    pub fn visual(&mut self, v: &Visual, r: Rect, c: Color) {
        match v {
            Visual::Imagen(ic) => self.imagen(ic, r, 1.0),
            Visual::Lucide(n) => self.lucide(n, r, c),
        }
    }

    fn geometria(&mut self, nombre: &'static str) -> Option<ID2D1PathGeometry> {
        if let Some(g) = self.geometrias.get(nombre) {
            return Some(g.clone());
        }
        let datos = lucide::trazo(nombre)?;
        let cmds = lucide::leer(datos);
        let v = |x: f32, y: f32| Vector2 { X: x, Y: y };
        unsafe {
            let geo = self.rt.GetFactory().ok()?.CreatePathGeometry().ok()?;
            let sink = geo.Open().ok()?;
            let mut abierta = false;
            let mut inicio = (0.0, 0.0);
            for cmd in cmds {
                match cmd {
                    Cmd::Mover(x, y) => {
                        if abierta {
                            sink.EndFigure(D2D1_FIGURE_END_OPEN);
                        }
                        sink.BeginFigure(v(x, y), D2D1_FIGURE_BEGIN_HOLLOW);
                        abierta = true;
                        inicio = (x, y);
                    }
                    // Un trazo sin M previo empieza en el último punto: se abre una figura ahí.
                    _ if !abierta => {
                        sink.BeginFigure(v(inicio.0, inicio.1), D2D1_FIGURE_BEGIN_HOLLOW);
                        abierta = true;
                        self.segmento(&sink, cmd, &mut abierta, &mut inicio);
                    }
                    _ => self.segmento(&sink, cmd, &mut abierta, &mut inicio),
                }
            }
            if abierta {
                sink.EndFigure(D2D1_FIGURE_END_OPEN);
            }
            sink.Close().ok()?;
            self.geometrias.insert(nombre, geo.clone());
            Some(geo)
        }
    }

    unsafe fn segmento(&self, sink: &ID2D1GeometrySink, cmd: Cmd, abierta: &mut bool, inicio: &mut (f32, f32)) {
        let v = |x: f32, y: f32| Vector2 { X: x, Y: y };
        unsafe {
            match cmd {
                Cmd::Linea(x, y) => sink.AddLine(v(x, y)),
                Cmd::Cubica(x1, y1, x2, y2, x, y) => {
                    sink.AddBezier(&D2D1_BEZIER_SEGMENT { point1: v(x1, y1), point2: v(x2, y2), point3: v(x, y) })
                }
                Cmd::Cuadratica(x1, y1, x, y) => sink.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT { point1: v(x1, y1), point2: v(x, y) }),
                Cmd::Arco { rx, ry, rot, grande, horario, x, y } => sink.AddArc(&D2D1_ARC_SEGMENT {
                    point: v(x, y),
                    size: D2D_SIZE_F { width: rx, height: ry },
                    rotationAngle: rot,
                    sweepDirection: if horario { D2D1_SWEEP_DIRECTION_CLOCKWISE } else { D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE },
                    arcSize: if grande { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
                }),
                Cmd::Cerrar => {
                    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                    *abierta = false;
                }
                Cmd::Mover(x, y) => *inicio = (x, y),
            }
        }
    }

    /// Sombra suave hecha con capas: suficiente para despegar una baldosa del fondo.
    pub fn sombra(&self, r: Rect, radio: f32, intensidad: f32) {
        for (i, a) in [(6.0, 0.10), (3.0, 0.14), (1.0, 0.18)] {
            let s = Rect { left: r.left - i * 0.5, top: r.top + i * 0.6, right: r.right + i * 0.5, bottom: r.bottom + i * 1.2 };
            self.rellenar(s, radio + i * 0.5, rgba(0, 0, 0, a * intensidad));
        }
    }
}

fn existe_fuente(dwrite: &IDWriteFactory, nombre: PCWSTR) -> bool {
    unsafe {
        let mut col = None;
        if dwrite.GetSystemFontCollection(&mut col, false).is_err() {
            return false;
        }
        let Some(col) = col else { return false };
        let (mut i, mut existe) = (0u32, BOOL(0));
        col.FindFamilyName(nombre, &mut i, &mut existe).is_ok() && existe.as_bool()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dentro_incluye_el_borde_superior_izquierdo() {
        let r = rect(10.0, 10.0, 20.0, 20.0);
        assert!(dentro(&r, 10.0, 10.0));
        assert!(dentro(&r, 29.9, 29.9));
        assert!(!dentro(&r, 30.0, 15.0));
        assert!(!dentro(&r, 9.9, 15.0));
    }
}
