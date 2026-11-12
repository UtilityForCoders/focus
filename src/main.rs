//! Focus: lanzador radial para Windows.
//!
//! Mantén pulsado el botón central del ratón en cualquier sitio, apunta hacia un acceso y suelta.
//! Un clic central corto se reenvía tal cual, así que abrir enlaces en otra pestaña sigue
//! funcionando.
//!
//! Todo ocurre en un solo hilo: los ganchos de entrada, la ventana de la rueda y el ícono de la
//! bandeja comparten el bucle de mensajes del hilo principal.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod hook;
mod launch;
mod render;
mod tray;
mod wheel;

use config::{AbrirEn, Config};
use hook::*;
use render::{Escena, Renderer};
use std::cell::RefCell;
use std::sync::atomic::Ordering;
use std::time::Instant;
use windows::core::{w, Result};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const TIMER_MANTENER: usize = 1;
const TIMER_FOTOGRAMA: usize = 2;
const TIMER_DEMO: usize = 3;
/// Duración de la animación de apertura.
const APERTURA_MS: f32 = 140.0;

struct Rueda {
    /// Esquina superior izquierda del monitor, en coordenadas de pantalla.
    origen: POINT,
    ancho: i32,
    alto: i32,
    /// Centro de la rueda relativo a la ventana.
    centro: (f32, f32),
    escala: f32,
    seleccion: Option<usize>,
    inicio: Instant,
    progreso: f32,
}

struct App {
    hwnd: HWND,
    cfg: Config,
    nombres: Vec<String>,
    renderer: Renderer,
    pulsado_en: POINT,
    rueda: Option<Rueda>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn con_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.borrow_mut().as_mut().map(f))
}

fn main() -> Result<()> {
    unsafe {
        // Una sola instancia: dos ganchos peleándose por el mismo botón no sirven para nada.
        let _mutex = CreateMutexW(None, true, w!("Local\\FocusLanzadorRadial"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(());
        }
        // Coordenadas físicas en todos los monitores: el gancho y la ventana hablan el mismo idioma.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let instancia: HINSTANCE = GetModuleHandleW(None)?.into();
        let clase = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instancia,
            lpszClassName: w!("FocusRueda"),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        RegisterClassW(&clase);

        // La rueda nunca toma el foco ni recibe clics: la ventana en la que estabas sigue siendo
        // la activa y el teclado llega por el gancho.
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("FocusRueda"),
            w!("Focus"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instancia),
            None,
        )?;

        let cfg = config::cargar();
        APP.with(|a| {
            *a.borrow_mut() = Some(App {
                hwnd,
                nombres: cfg.accesos.iter().map(|x| x.nombre.clone()).collect(),
                cfg,
                renderer: Renderer::nuevo().expect("no se pudo iniciar Direct2D"),
                pulsado_en: POINT::default(),
                rueda: None,
            })
        });

        tray::agregar(hwnd);
        hook::instalar(hwnd)?;

        // `focus --demo`: abre la rueda bajo el puntero durante unos segundos sin pulsar nada.
        // Sirve para revisar el dibujo (el gancho ignora los clics simulados con SendInput).
        if std::env::args().any(|a| a == "--demo") {
            ESTADO.store(ABIERTA, Ordering::SeqCst);
            con_app(|app| {
                let _ = GetCursorPos(&mut app.pulsado_en);
                abrir(app);
            });
            SetTimer(Some(hwnd), TIMER_DEMO, 4000, None);
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        tray::quitar(hwnd);
    }
    Ok(())
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_FOCUS_DOWN => {
            let (x, y) = desempaquetar(lparam.0);
            con_app(|app| {
                // Se relee en cada pulsación: los cambios en config.json valen sin reiniciar.
                app.cfg = config::cargar();
                app.nombres = app.cfg.accesos.iter().map(|x| x.nombre.clone()).collect();
                app.pulsado_en = POINT { x, y };
                unsafe { SetTimer(Some(hwnd), TIMER_MANTENER, app.cfg.retardo_ms, None) };
            });
        }
        WM_FOCUS_TAP => {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_MANTENER);
            }
            reenviar_clic_central();
        }
        WM_FOCUS_RELEASE => {
            let elegido = con_app(|app| {
                actualizar(app);
                let i = app.rueda.as_ref().and_then(|r| r.seleccion);
                cerrar(app);
                i.and_then(|i| app.cfg.accesos.get(i).cloned())
            })
            .flatten();
            if let Some(acceso) = elegido {
                launch::abrir(&acceso);
            }
        }
        WM_FOCUS_CANCEL => {
            con_app(cerrar);
        }
        WM_TIMER => match wparam.0 {
            TIMER_MANTENER => {
                unsafe {
                    let _ = KillTimer(Some(hwnd), TIMER_MANTENER);
                }
                // Si el botón ya se soltó, el gancho dejó el estado en INACTIVO y esto no hace nada.
                if ESTADO.compare_exchange(PENDIENTE, ABIERTA, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                    con_app(abrir);
                }
            }
            TIMER_FOTOGRAMA => {
                con_app(actualizar);
            }
            TIMER_DEMO => unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_DEMO);
                if ESTADO.compare_exchange(ABIERTA, INACTIVO, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                    con_app(cerrar);
                }
            },
            _ => {}
        },
        tray::WM_FOCUS_TRAY => {
            let evento = (lparam.0 as u32) & 0xFFFF;
            if evento == WM_RBUTTONUP || evento == WM_LBUTTONUP {
                match tray::menu(hwnd) {
                    tray::CMD_EDITAR => {
                        let ruta = config::ruta();
                        config::cargar(); // crea el archivo si aún no existe
                        launch::abrir(&config::Acceso {
                            nombre: String::new(),
                            destino: "notepad.exe".into(),
                            argumentos: Some(format!("\"{}\"", ruta.display())),
                        });
                    }
                    tray::CMD_CARPETA => {
                        if let Some(dir) = config::ruta().parent() {
                            launch::abrir(&config::Acceso {
                                nombre: String::new(),
                                destino: dir.display().to_string(),
                                argumentos: None,
                            });
                        }
                    }
                    tray::CMD_SALIR => unsafe {
                        let _ = DestroyWindow(hwnd);
                    },
                    _ => {}
                }
            }
        }
        WM_DESTROY => unsafe { PostQuitMessage(0) },
        _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
    LRESULT(0)
}

/// Un clic central corto no era para Focus: se devuelve al sistema como si nada.
fn reenviar_clic_central() {
    let evento = |flags| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dwFlags: flags, ..Default::default() },
        },
    };
    let entradas = [evento(MOUSEEVENTF_MIDDLEDOWN), evento(MOUSEEVENTF_MIDDLEUP)];
    unsafe {
        SendInput(&entradas, std::mem::size_of::<INPUT>() as i32);
    }
}

fn abrir(app: &mut App) {
    let p = app.pulsado_en;
    unsafe {
        let monitor = MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
        let escala = dpi_x as f32 / 96.0;

        let m = info.rcMonitor;
        let (ancho, alto) = (m.right - m.left, m.bottom - m.top);
        let centro = match app.cfg.abrir_en {
            AbrirEn::Centro => (ancho as f32 / 2.0, alto as f32 / 2.0),
            AbrirEn::Cursor => {
                // Bajo el puntero, pero sin que la rueda se salga del monitor.
                let margen = (wheel::RADIO_EXTERIOR + 12.0) * escala;
                let limitar = |v: f32, max: f32| if max <= 2.0 * margen { max / 2.0 } else { v.clamp(margen, max - margen) };
                (limitar((p.x - m.left) as f32, ancho as f32), limitar((p.y - m.top) as f32, alto as f32))
            }
        };

        app.rueda = Some(Rueda {
            origen: POINT { x: m.left, y: m.top },
            ancho,
            alto,
            centro,
            escala,
            seleccion: None,
            inicio: Instant::now(),
            progreso: 0.0,
        });
        // El primer fotograma se pinta antes de mostrar la ventana, para que nunca se vea vacía.
        pintar(app);
        let _ = ShowWindow(app.hwnd, SW_SHOWNOACTIVATE);
        let _ = SetWindowPos(app.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        SetTimer(Some(app.hwnd), TIMER_FOTOGRAMA, 15, None);
    }
}

fn cerrar(app: &mut App) {
    unsafe {
        let _ = KillTimer(Some(app.hwnd), TIMER_FOTOGRAMA);
        let _ = ShowWindow(app.hwnd, SW_HIDE);
    }
    app.rueda = None;
}

/// Recalcula la selección y el avance de la animación; solo repinta si algo cambió.
fn actualizar(app: &mut App) {
    let n = app.nombres.len();
    let Some(r) = app.rueda.as_mut() else { return };
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    let dx = (p.x - r.origen.x) as f32 - r.centro.0;
    let dy = (p.y - r.origen.y) as f32 - r.centro.1;
    let seleccion = wheel::sector_en(dx, dy, n, wheel::RADIO_INTERIOR * r.escala);

    let t = (r.inicio.elapsed().as_secs_f32() * 1000.0 / APERTURA_MS).min(1.0);
    let progreso = 1.0 - (1.0 - t).powi(3); // ease-out cúbico

    if seleccion != r.seleccion || progreso != r.progreso {
        r.seleccion = seleccion;
        r.progreso = progreso;
        pintar(app);
    }
}

fn pintar(app: &mut App) {
    let Some(r) = app.rueda.as_ref() else { return };
    if let Err(e) = app.renderer.preparar(r.ancho, r.alto) {
        eprintln!("preparar: {e}");
        return;
    }
    let escena = Escena {
        centro: r.centro,
        escala: r.escala,
        nombres: &app.nombres,
        seleccion: r.seleccion,
        progreso: r.progreso,
    };
    if let Err(e) = app.renderer.dibujar(&escena) {
        eprintln!("dibujar: {e}");
        return;
    }
    let tam = SIZE { cx: r.ancho, cy: r.alto };
    let origen_src = POINT::default();
    let mezcla = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: (255.0 * r.progreso.max(0.15)) as u8,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    unsafe {
        if let Err(e) = UpdateLayeredWindow(
            app.hwnd,
            None,
            Some(&r.origen),
            Some(&tam),
            Some(app.renderer.dc()),
            Some(&origen_src),
            COLORREF(0),
            Some(&mezcla),
            ULW_ALPHA,
        ) {
            eprintln!("UpdateLayeredWindow: {e}");
        }
    }
}
