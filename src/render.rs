//! Dibujo de la rueda con Direct2D sobre un DIB de 32 bits, que luego se entrega a una ventana
//! "layered" con `UpdateLayeredWindow`. Así la transparencia es por píxel (las esquinas del
//! anillo, el oscurecido del fondo) sin depender del compositor para nada más.
//!
//! Todo se mide en píxeles físicos: la escala del monitor llega en `Escena::escala` y se aplica a
//! las medidas, no al render target (que trabaja a 96 ppp).

use crate::wheel::{self, HUECO, RADIO_EXTERIOR, RADIO_INTERIOR};
use std::f32::consts::{PI, TAU};
use windows::core::{w, Result};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows_numerics::{Matrix3x2, Vector2};

/// Lo que hace falta para pintar un fotograma.
pub struct Escena<'a> {
    /// Centro de la rueda, relativo a la esquina de la ventana.
    pub centro: (f32, f32),
    pub escala: f32,
    pub nombres: &'a [String],
    pub seleccion: Option<usize>,
    /// Progreso de la animación de apertura, de 0 a 1 (ya suavizado).
    pub progreso: f32,
}

pub struct Renderer {
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    target: Option<ID2D1DCRenderTarget>,
    /// Formatos de texto creados para una escala concreta (cambian al pasar a otro monitor).
    textos: Option<(f32, IDWriteTextFormat, IDWriteTextFormat)>,
    dc: HDC,
    bitmap: HBITMAP,
    anterior: HGDIOBJ,
    pub ancho: i32,
    pub alto: i32,
}

const fn color(r: u8, g: u8, b: u8, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a }
}

const FONDO: D2D1_COLOR_F = color(0, 0, 0, 0.32);
const SECTOR: D2D1_COLOR_F = color(26, 27, 32, 0.94);
const SECTOR_ACTIVO: D2D1_COLOR_F = color(91, 140, 255, 0.97);
const BORDE: D2D1_COLOR_F = color(255, 255, 255, 0.10);
const CENTRO: D2D1_COLOR_F = color(16, 17, 21, 0.97);
const TEXTO: D2D1_COLOR_F = color(236, 238, 244, 1.0);
const TEXTO_TENUE: D2D1_COLOR_F = color(150, 154, 166, 1.0);

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
                textos: None,
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
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_GDI_COMPATIBLE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let t = unsafe { self.factory.CreateDCRenderTarget(&props)? };
        self.target = Some(t.clone());
        Ok(t)
    }

    fn formatos(&mut self, escala: f32) -> Result<(IDWriteTextFormat, IDWriteTextFormat)> {
        if let Some((e, a, b)) = &self.textos {
            if (*e - escala).abs() < 0.01 {
                return Ok((a.clone(), b.clone()));
            }
        }
        let crear = |tam: f32, peso: DWRITE_FONT_WEIGHT| -> Result<IDWriteTextFormat> {
            unsafe {
                let f = self.dwrite.CreateTextFormat(
                    w!("Segoe UI Variable Text"),
                    None,
                    peso,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    tam * escala,
                    w!("es-es"),
                )?;
                f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                f.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
                Ok(f)
            }
        };
        let sector = crear(13.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
        let centro = crear(12.0, DWRITE_FONT_WEIGHT_NORMAL)?;
        self.textos = Some((escala, sector.clone(), centro.clone()));
        Ok((sector, centro))
    }

    /// Pinta la escena en el DIB. Después, el DIB está listo para `UpdateLayeredWindow`.
    pub fn dibujar(&mut self, escena: &Escena) -> Result<()> {
        let resultado = self.dibujar_en(escena);
        if resultado.is_err() {
            // Dispositivo perdido o similar: el próximo fotograma recrea el render target.
            self.target = None;
        }
        resultado
    }

    fn dibujar_en(&mut self, e: &Escena) -> Result<()> {
        let target = self.target()?;
        let (fmt_sector, fmt_centro) = self.formatos(e.escala)?;
        let rect = RECT { left: 0, top: 0, right: self.ancho, bottom: self.alto };
        unsafe {
            target.BindDC(self.dc, &rect)?;
            target.BeginDraw();
            target.SetTransform(&Matrix3x2::identity());
            target.Clear(Some(&color(0, 0, 0, 0.0)));

            let pincel = target.CreateSolidColorBrush(&FONDO, None)?;
            let pantalla = D2D_RECT_F {
                left: 0.0,
                top: 0.0,
                right: self.ancho as f32,
                bottom: self.alto as f32,
            };
            target.FillRectangle(&pantalla, &pincel);

            // La rueda "florece": crece desde el 86 % alrededor de su centro.
            let s = 0.86 + 0.14 * e.progreso;
            let (cx, cy) = e.centro;
            let m = Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: cx * (1.0 - s), M32: cy * (1.0 - s) };
            target.SetTransform(&m);

            let ext = RADIO_EXTERIOR * e.escala;
            let int = RADIO_INTERIOR * e.escala;
            let n = e.nombres.len();

            for i in 0..n {
                let activo = e.seleccion == Some(i);
                pincel.SetColor(if activo { &SECTOR_ACTIVO } else { &SECTOR });
                if n == 1 {
                    // Un único acceso: el anillo completo, como trazo grueso.
                    let elipse = D2D1_ELLIPSE {
                        point: Vector2 { X: cx, Y: cy },
                        radiusX: (ext + int) / 2.0,
                        radiusY: (ext + int) / 2.0,
                    };
                    target.DrawEllipse(&elipse, &pincel, ext - int, None);
                } else {
                    let geo = self.sector(cx, cy, int, ext, i, n)?;
                    target.FillGeometry(&geo, &pincel, None);
                    pincel.SetColor(&BORDE);
                    target.DrawGeometry(&geo, &pincel, 1.0, None);
                }

                // Etiqueta en el radio medio del sector.
                let a = wheel::angulo_centro(i, n);
                let r = (ext + int) / 2.0;
                let (tx, ty) = (cx + r * a.cos(), cy + r * a.sin());
                let (mw, mh) = (54.0 * e.escala, 24.0 * e.escala);
                let caja = D2D_RECT_F { left: tx - mw, top: ty - mh, right: tx + mw, bottom: ty + mh };
                pincel.SetColor(&TEXTO);
                let texto: Vec<u16> = e.nombres[i].encode_utf16().collect();
                target.DrawText(
                    &texto,
                    &fmt_sector,
                    &caja,
                    &pincel,
                    D2D1_DRAW_TEXT_OPTIONS_CLIP,
                    DWRITE_MEASURING_MODE_NATURAL,
                );
            }

            // Centro: muestra el acceso elegido, o cómo cancelar.
            let centro = D2D1_ELLIPSE {
                point: Vector2 { X: cx, Y: cy },
                radiusX: int - 6.0 * e.escala,
                radiusY: int - 6.0 * e.escala,
            };
            pincel.SetColor(&CENTRO);
            target.FillEllipse(&centro, &pincel);
            pincel.SetColor(&BORDE);
            target.DrawEllipse(&centro, &pincel, 1.0, None);

            let (leyenda, tono) = match e.seleccion {
                Some(i) => (e.nombres[i].as_str(), &TEXTO),
                None if n == 0 => ("Sin accesos", &TEXTO_TENUE),
                None => ("Suelta aquí\npara cancelar", &TEXTO_TENUE),
            };
            let lado = int - 12.0 * e.escala;
            let caja = D2D_RECT_F { left: cx - lado, top: cy - lado, right: cx + lado, bottom: cy + lado };
            pincel.SetColor(tono);
            let texto: Vec<u16> = leyenda.encode_utf16().collect();
            target.DrawText(
                &texto,
                &fmt_centro,
                &caja,
                &pincel,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );

            target.EndDraw(None, None)?;
        }
        Ok(())
    }

    /// Un sector de corona circular: arco exterior en sentido horario, arco interior de vuelta.
    fn sector(&self, cx: f32, cy: f32, int: f32, ext: f32, i: usize, n: usize) -> Result<ID2D1PathGeometry> {
        let paso = TAU / n as f32;
        let medio = wheel::angulo_centro(i, n);
        let (a0, a1) = (medio - paso / 2.0 + HUECO / 2.0, medio + paso / 2.0 - HUECO / 2.0);
        let punto = |r: f32, a: f32| Vector2 { X: cx + r * a.cos(), Y: cy + r * a.sin() };
        let grande = if a1 - a0 > PI { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL };
        unsafe {
            let geo = self.factory.CreatePathGeometry()?;
            let sink = geo.Open()?;
            sink.BeginFigure(punto(ext, a0), D2D1_FIGURE_BEGIN_FILLED);
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: punto(ext, a1),
                size: D2D_SIZE_F { width: ext, height: ext },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: grande,
            });
            sink.AddLine(punto(int, a1));
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: punto(int, a0),
                size: D2D_SIZE_F { width: int, height: int },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE,
                arcSize: grande,
            });
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close()?;
            Ok(geo)
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
