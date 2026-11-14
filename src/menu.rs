//! Menú del clic derecho en el ícono de la bandeja, dibujado con Direct2D con el mismo tema
//! oscuro que la rueda y los ajustes (el menú nativo de Windows sale claro y no se puede
//! tematizar de forma soportada).
//!
//! Es una ventana emergente propia: se abre junto al puntero, resalta la opción bajo el ratón,
//! se maneja también con el teclado (flechas, Enter, Escape) y se cierra sola al perder el foco o
//! al hacer clic fuera, como un menú normal. Al elegir una opción avisa a la ventana principal
//! con `WM_FOCUS_MENU`.

use crate::ui::{glifo, rect, rgba, tema, Alin, Peso, Pintor};
use std::cell::RefCell;
use windows::core::{w, Interface, BOOL};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Se eligió una opción; `wparam` es el comando.
pub const WM_FOCUS_MENU: u32 = WM_APP + 50;
pub const CMD_CONFIGURAR: usize = 1;
pub const CMD_SALIR: usize = 2;

/// Opciones del menú: comando, glifo y texto.
const OPCIONES: [(usize, char, &str); 2] = [
    (CMD_CONFIGURAR, glifo::AJUSTES, "Configuración"),
    (CMD_SALIR, glifo::APAGAR, "Salir de Focus"),
];
/// Antes de esta opción va una línea separadora.
const SEPARAR_ANTES: usize = 1;

const ANCHO: f32 = 210.0;
const FILA: f32 = 36.0;
const MARGEN: f32 = 5.0;
const SEPARADOR: f32 = 9.0;
/// Comprueba si se hizo clic fuera del menú (por si no consiguió el foco).
const TIMER_FUERA: usize = 1;

/// Borde superior de la opción `i`, en DIP.
fn arriba(i: usize) -> f32 {
    MARGEN + i as f32 * FILA + if i >= SEPARAR_ANTES { SEPARADOR } else { 0.0 }
}

fn alto_total() -> f32 {
    arriba(OPCIONES.len() - 1) + FILA + MARGEN
}
const FONDO: crate::ui::Color = rgba(32, 32, 36, 1.0);

struct Menu {
    hwnd: HWND,
    principal: HWND,
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    rt: Option<ID2D1HwndRenderTarget>,
    pintor: Option<Pintor>,
    escala: f32,
    /// Opción resaltada (con el ratón o las flechas).
    sobre: Option<usize>,
}

thread_local! {
    static MENU: RefCell<Option<Menu>> = const { RefCell::new(None) };
}

fn con_menu<R>(f: impl FnOnce(&mut Menu) -> R) -> Option<R> {
    MENU.with(|m| match m.try_borrow_mut() {
        Ok(mut g) => g.as_mut().map(f),
        Err(_) => None,
    })
}

/// Abre el menú junto al puntero. Si ya estaba abierto, lo cierra (un segundo clic derecho en
/// el ícono hace lo mismo que en un menú nativo).
pub fn mostrar(principal: HWND) {
    if let Some(h) = con_menu(|m| m.hwnd) {
        unsafe {
            let _ = DestroyWindow(h);
        }
        return;
    }
    if let Err(e) = crear(principal) {
        eprintln!("no se pudo abrir el menú: {e}");
    }
}

fn crear(principal: HWND) -> windows::core::Result<()> {
    unsafe {
        let instancia: HINSTANCE = GetModuleHandleW(None)?.into();
        let clase = WNDCLASSW {
            // La sombra del sistema, como la de los menús de Windows.
            style: CS_DROPSHADOW,
            lpfnWndProc: Some(wndproc),
            hInstance: instancia,
            lpszClassName: w!("FocusMenu"),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        RegisterClassW(&clase);

        // Tamaño según el DPI del monitor del puntero; se abre hacia arriba (la bandeja suele
        // estar abajo) y sin salirse del área de trabajo.
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        let monitor = MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let s = dx as f32 / 96.0;
        let an = (ANCHO * s).round() as i32;
        let al = (alto_total() * s).round() as i32;
        let t = info.rcWork;
        let x = p.x.min(t.right - an).max(t.left);
        let y = if p.y - al >= t.top { p.y - al } else { p.y.min(t.bottom - al) };

        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            w!("FocusMenu"),
            w!("Focus"),
            WS_POPUP,
            x,
            y,
            an,
            al,
            None,
            None,
            Some(instancia),
            None,
        )?;

        // Esquinas redondeadas pequeñas, como los menús de Windows 11 (en Windows 10 falla y
        // quedan rectas).
        let esquinas = DWMWCP_ROUNDSMALL;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &esquinas as *const _ as *const _, 4);
        let oscuro = BOOL(1);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &oscuro as *const _ as *const _, 4);

        let menu = Menu {
            hwnd,
            principal,
            factory: D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?,
            dwrite: DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?,
            rt: None,
            pintor: None,
            escala: s,
            sobre: None,
        };
        MENU.with(|m| *m.borrow_mut() = Some(menu));
        let _ = ShowWindow(hwnd, SW_SHOW);
        // Con el foco, el menú se entera de que tiene que cerrarse al hacer clic fuera y recibe
        // el teclado. Windows no siempre lo concede: por eso también está `TIMER_FUERA`.
        let _ = SetForegroundWindow(hwnd);
        SetTimer(Some(hwnd), TIMER_FUERA, 50, None);
    }
    Ok(())
}

impl Menu {
    fn invalidar(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    /// Opción bajo un punto en DIP, si hay alguna.
    fn opcion_en(&self, x: f32, y: f32) -> Option<usize> {
        if x < MARGEN || x >= ANCHO - MARGEN {
            return None;
        }
        (0..OPCIONES.len()).find(|&i| y >= arriba(i) && y < arriba(i) + FILA)
    }

    fn pintar(&mut self) {
        if self.rt.is_none() {
            let mut cr = RECT::default();
            unsafe {
                let _ = GetClientRect(self.hwnd, &mut cr);
            }
            let dpi = self.escala * 96.0;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: dpi,
                dpiY: dpi,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let hprops = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd: self.hwnd,
                pixelSize: D2D_SIZE_U { width: cr.right.max(1) as u32, height: cr.bottom.max(1) as u32 },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            let Ok(rt) = (unsafe { self.factory.CreateHwndRenderTarget(&props, &hprops) }) else { return };
            let Ok(base) = rt.cast() else { return };
            let Ok(pintor) = Pintor::nuevo(base, self.dwrite.clone()) else { return };
            self.pintor = Some(pintor);
            self.rt = Some(rt);
        }
        let (Some(rt), Some(p)) = (self.rt.clone(), self.pintor.as_mut()) else { return };
        let alto = alto_total();
        unsafe {
            rt.BeginDraw();
            rt.SetTransform(&windows_numerics::Matrix3x2::identity());
            rt.Clear(Some(&FONDO));
        }
        // Borde fino (en Windows 11 el sistema dibuja el suyo encima de las esquinas).
        p.bordear(rect(0.5, 0.5, ANCHO - 1.0, alto - 1.0), 0.0, tema::BORDE_FUERTE, 1.0);
        for (i, (_, g, texto)) in OPCIONES.iter().enumerate() {
            if i == SEPARAR_ANTES {
                let y = arriba(i) - SEPARADOR / 2.0;
                p.linea(MARGEN + 8.0, y, ANCHO - MARGEN - 8.0, y, tema::BORDE_FUERTE, 1.0);
            }
            let r = rect(MARGEN, arriba(i), ANCHO - 2.0 * MARGEN, FILA);
            if self.sobre == Some(i) {
                p.rellenar(r, 5.0, tema::CONTROL_ACTIVO);
            }
            let cy = r.top + FILA / 2.0;
            p.glifo(*g, r.left + 18.0, cy, 14.0, tema::TEXTO);
            p.texto(texto, rect(r.left + 40.0, r.top, r.right - r.left - 48.0, FILA), 13.0, Peso::Normal, Alin::Izq, tema::TEXTO);
        }
        if unsafe { rt.EndDraw(None, None) }.is_err() {
            self.rt = None;
            self.pintor = None;
        }
    }

    fn resaltar(&mut self, nuevo: Option<usize>) {
        if nuevo != self.sobre {
            self.sobre = nuevo;
            self.invalidar();
        }
    }
}

/// Elige una opción: avisa a la ventana principal y cierra el menú.
fn elegir(hwnd: HWND, principal: HWND, i: usize) {
    unsafe {
        let _ = PostMessageW(Some(principal), WM_FOCUS_MENU, WPARAM(OPCIONES[i].0), LPARAM(0));
        let _ = DestroyWindow(hwnd);
    }
}

fn punto(lparam: LPARAM) -> (i32, i32) {
    ((lparam.0 & 0xFFFF) as i16 as i32, ((lparam.0 >> 16) & 0xFFFF) as i16 as i32)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            unsafe { BeginPaint(hwnd, &mut ps) };
            con_menu(Menu::pintar);
            let _ = unsafe { EndPaint(hwnd, &ps) };
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            con_menu(|m| {
                let (x, y) = punto(lparam);
                let i = m.opcion_en(x as f32 / m.escala, y as f32 / m.escala);
                m.resaltar(i);
                let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                let _ = unsafe { TrackMouseEvent(&mut tme) };
            });
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            con_menu(|m| m.resaltar(None));
            return LRESULT(0);
        }
        // Como en un menú nativo, vale tanto el clic izquierdo como el derecho.
        WM_LBUTTONUP | WM_RBUTTONUP => {
            let elegida = con_menu(|m| {
                let (x, y) = punto(lparam);
                m.opcion_en(x as f32 / m.escala, y as f32 / m.escala).map(|i| (m.principal, i))
            })
            .flatten();
            if let Some((principal, i)) = elegida {
                elegir(hwnd, principal, i);
            }
            return LRESULT(0);
        }
        WM_KEYDOWN => {
            let n = OPCIONES.len();
            match VIRTUAL_KEY(wparam.0 as u16) {
                VK_ESCAPE => unsafe {
                    let _ = DestroyWindow(hwnd);
                },
                VK_DOWN => {
                    con_menu(|m| m.resaltar(Some(m.sobre.map_or(0, |i| (i + 1) % n))));
                }
                VK_UP => {
                    con_menu(|m| m.resaltar(Some(m.sobre.map_or(n - 1, |i| (i + n - 1) % n))));
                }
                VK_RETURN | VK_SPACE => {
                    if let Some((principal, i)) = con_menu(|m| m.sobre.map(|i| (m.principal, i))).flatten() {
                        elegir(hwnd, principal, i);
                    }
                }
                _ => {}
            }
            return LRESULT(0);
        }
        // Respaldo por si el menú no tiene el foco (y no le llega WM_ACTIVATE): un clic de
        // cualquier botón fuera de él lo cierra.
        WM_TIMER if wparam.0 == TIMER_FUERA => unsafe {
            let pulsado = [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON].iter().any(|&k| GetAsyncKeyState(k.0 as i32) as u16 & 0x8000 != 0);
            if pulsado {
                let mut p = POINT::default();
                let mut r = RECT::default();
                let _ = GetCursorPos(&mut p);
                let _ = GetWindowRect(hwnd, &mut r);
                let fuera = p.x < r.left || p.x >= r.right || p.y < r.top || p.y >= r.bottom;
                if fuera {
                    let _ = DestroyWindow(hwnd);
                }
            }
            return LRESULT(0);
        },
        // Clic fuera (u otra ventana al frente): se cierra, como un menú normal.
        WM_ACTIVATE if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE => unsafe {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        },
        WM_DESTROY => {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_FUERA);
            }
            MENU.with(|m| m.borrow_mut().take());
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
