//! Focus: lanzador radial para Windows.
//!
//! Mantén pulsado el botón central del ratón (o uno lateral, según los ajustes) en cualquier
//! sitio, apunta hacia un acceso y suelta. Un clic corto se reenvía tal cual, así que abrir
//! enlaces en otra pestaña sigue funcionando.
//!
//! Todo ocurre en un solo hilo: los ganchos de entrada, la ventana de la rueda y el ícono de la
//! bandeja comparten el bucle de mensajes del hilo principal.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
// Un brazo de `match` que nombra una constante que no está importada no es un error: Rust lo lee
// como una variable nueva que atrapa cualquier valor y deja muertos los brazos de debajo. En un
// procedimiento de ventana eso es una ventana que pinta bien y no responde a nada (pasó con
// `WM_MOUSELEAVE`, que vive en `UI::Controls`). Así deja de compilar en lugar de ser un aviso.
#![deny(unreachable_patterns)]

mod apps;
mod autostart;
mod config;
mod hook;
mod icons;
mod launch;
mod lucide;
mod lucide_datos;
mod menu;
mod render;
mod ritmo;
mod settings;
mod tray;
mod ui;
mod web;
mod wheel;

use config::{AbrirEn, Boton, Config, Modo};
use hook::*;
use render::{Baldosa, Escena, Renderer};
use std::cell::RefCell;
use std::sync::atomic::Ordering;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
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
const TIMER_DEMO: usize = 3;
/// Duración de la animación de apertura.
const APERTURA_MS: f32 = 140.0;
/// Cuánto hay que apuntar a un grupo para que se abra.
const ESPERA_GRUPO_MS: u128 = 280;
/// Cuánto hay que quedarse en el centro, dentro de un grupo, para volver a la rueda principal.
const ESPERA_VOLVER_MS: u128 = 300;

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
    /// Grupo abierto; `None` en la rueda principal.
    nivel: Option<usize>,
    /// Grupo al que se está apuntando en la raíz, y desde cuándo (para abrirlo al rato).
    apuntado: Option<(usize, Instant)>,
    /// Dentro de un grupo, desde cuándo el puntero está en el círculo central (para volver).
    en_centro: Option<Instant>,
}

struct App {
    hwnd: HWND,
    cfg: Config,
    /// Baldosas de la rueda principal: grupos primero y después accesos sueltos.
    raiz: Vec<Baldosa>,
    /// Baldosas de cada grupo, en el mismo orden que `cfg.grupos`.
    grupos: Vec<Vec<Baldosa>>,
    renderer: Renderer,
    pulsado_en: POINT,
    rueda: Option<Rueda>,
}

impl App {
    /// Relee `config.json` y prepara los íconos. Los ya extraídos salen de la caché, así que
    /// después de la primera vez esto es casi gratis.
    fn recargar(&mut self) {
        self.cfg = config::cargar();
        self.raiz = render::baldosas_raiz(&self.cfg);
        self.grupos = self.cfg.grupos.iter().map(render::baldosas_grupo).collect();
        hook::poner_boton(self.cfg.boton);
        hook::poner_modo_clic(self.cfg.modo == Modo::Clic);
    }

    /// Baldosas del nivel que muestra la rueda.
    fn baldosas(&self, nivel: Option<usize>) -> &[Baldosa] {
        match nivel.and_then(|g| self.grupos.get(g)) {
            Some(b) => b,
            None => &self.raiz,
        }
    }

    /// Acceso que abre la baldosa `i` del nivel; `None` si es un grupo.
    fn acceso(&self, nivel: Option<usize>, i: usize) -> Option<config::Acceso> {
        match nivel {
            Some(g) => self.cfg.grupos.get(g)?.accesos.get(i).cloned(),
            None => self.cfg.accesos.get(i.checked_sub(self.cfg.grupos.len())?).cloned(),
        }
    }
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Si el estado ya está prestado (un mensaje que llega mientras se atiende otro), no se hace
/// nada en lugar de entrar en pánico.
fn con_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| match a.try_borrow_mut() {
        Ok(mut g) => g.as_mut().map(f),
        Err(_) => None,
    })
}

fn main() -> Result<()> {
    unsafe {
        // Una sola instancia: dos ganchos peleándose por el mismo botón no sirven para nada.
        let _mutex = CreateMutexW(None, true, w!("Local\\FocusLanzadorRadial"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(());
        }
        // Coordenadas físicas en todos los monitores: el gancho y la ventana hablan el mismo idioma.
        // (El manifiesto ya lo pide; esto cubre ejecutar sin él.)
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // COM de un solo hilo: lo piden el shell (íconos, ShellExecute) y los diálogos.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

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

        let mut app = App {
            hwnd,
            cfg: Config::default(),
            raiz: Vec::new(),
            grupos: Vec::new(),
            renderer: Renderer::nuevo().expect("no se pudo iniciar Direct2D"),
            pulsado_en: POINT::default(),
            rueda: None,
        };
        app.recargar();
        APP.with(|a| *a.borrow_mut() = Some(app));

        tray::agregar(hwnd);
        hook::instalar(hwnd)?;
        web::poner_aviso(hwnd);

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
        // `focus --ajustes`: abre la ventana de configuración al arrancar.
        if std::env::args().any(|a| a == "--ajustes") {
            settings::abrir(hwnd);
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        tray::quitar(hwnd);
        hook::quitar();
        // El estado (Direct2D, íconos) se libera aquí y no en los destructores de fin de proceso.
        APP.with(|a| a.borrow_mut().take());
    }
    Ok(())
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_FOCUS_DOWN => {
            let (x, y) = desempaquetar(lparam.0);
            con_app(|app| {
                app.pulsado_en = POINT { x, y };
                // El temporizador arranca antes de releer: el tiempo de leer el archivo (y algún
                // ícono nuevo) corre a la vez que la espera, no después.
                unsafe { SetTimer(Some(hwnd), TIMER_MANTENER, app.cfg.retardo_ms, None) };
                // Se relee en cada pulsación: los cambios en config.json valen sin reiniciar.
                app.recargar();
            });
        }
        web::WM_FOCUS_FAVICON => {
            // Llegó un favicon: se releen los íconos web en la rueda y en los ajustes.
            icons::olvidar_webs();
            con_app(App::recargar);
            settings::refrescar();
        }
        settings::WM_FOCUS_CONFIG => {
            // La ventana de configuración guardó: se precargan los íconos nuevos ya, para que
            // la próxima rueda no tenga que esperar por ellos.
            con_app(App::recargar);
        }
        WM_FOCUS_TAP => {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_MANTENER);
            }
            reenviar_clic(hook::boton());
        }
        WM_FOCUS_RELEASE => {
            let elegido = con_app(|app| {
                actualizar(app);
                // Soltar sobre un grupo que aún no se abrió no abre nada.
                let elegido = app.rueda.as_ref().and_then(|r| app.acceso(r.nivel, r.seleccion?));
                cerrar(app);
                elegido
            })
            .flatten();
            if let Some(acceso) = elegido {
                launch::abrir(&acceso);
            }
        }
        WM_FOCUS_CANCEL => {
            con_app(cerrar);
        }
        ritmo::WM_FOCUS_FOTOGRAMA => {
            ritmo::atendido();
            con_app(actualizar);
        }
        WM_FOCUS_CLIC => {
            let elegido = con_app(|app| clic(app, lparam.0 == 1)).flatten();
            if let Some(acceso) = elegido {
                launch::abrir(&acceso);
            }
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
            if evento == WM_LBUTTONUP {
                settings::abrir(hwnd);
            } else if evento == WM_RBUTTONUP {
                menu::mostrar(hwnd);
            }
        }
        menu::WM_FOCUS_MENU => match wparam.0 {
            menu::CMD_CONFIGURAR => settings::abrir(hwnd),
            menu::CMD_SALIR => unsafe {
                let _ = DestroyWindow(hwnd);
            },
            _ => {}
        },
        WM_DESTROY => unsafe {
            // Lo primero, soltar el ratón y el teclado del sistema; después, cerrar los ajustes
            // con el proceso aún vivo (dejar que se destruyan al terminar el proceso tardaba
            // varios segundos, y con los ganchos puestos el puntero se congelaba).
            hook::quitar();
            settings::cerrar();
            PostQuitMessage(0)
        },
        _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
    LRESULT(0)
}

/// Un clic corto no era para Focus: se devuelve al sistema como si nada (el gancho deja pasar
/// los eventos inyectados).
fn reenviar_clic(boton: Boton) {
    let evento = |flags, datos: u16| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dwFlags: flags, mouseData: datos as u32, ..Default::default() },
        },
    };
    let entradas = match boton {
        Boton::Central => [evento(MOUSEEVENTF_MIDDLEDOWN, 0), evento(MOUSEEVENTF_MIDDLEUP, 0)],
        Boton::Atras => [evento(MOUSEEVENTF_XDOWN, XBUTTON1), evento(MOUSEEVENTF_XUP, XBUTTON1)],
        Boton::Adelante => [evento(MOUSEEVENTF_XDOWN, XBUTTON2), evento(MOUSEEVENTF_XUP, XBUTTON2)],
    };
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
        // El tamaño elegido en los ajustes se aplica como una escala más.
        let escala = dpi_x as f32 / 96.0 * app.cfg.tamano;

        let m = info.rcMonitor;
        let (ancho, alto) = (m.right - m.left, m.bottom - m.top);
        let centro = match app.cfg.abrir_en {
            AbrirEn::Centro => (ancho as f32 / 2.0, alto as f32 / 2.0),
            AbrirEn::Cursor => {
                // Bajo el puntero, pero sin que la rueda se salga del monitor.
                // El margen cuenta el nivel más grande: al abrir un grupo la rueda no se mueve.
                let mayor = app.grupos.iter().map(Vec::len).chain([app.raiz.len()]).max().unwrap_or(0);
                let margen = wheel::alcance(mayor) * escala;
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
            nivel: None,
            apuntado: None,
            en_centro: None,
        });
        // El primer fotograma se pinta antes de mostrar la ventana, para que nunca se vea vacía.
        pintar(app);
        let _ = ShowWindow(app.hwnd, SW_SHOWNOACTIVATE);
        let _ = SetWindowPos(app.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        ritmo::empezar(app.hwnd);
    }
}

fn cerrar(app: &mut App) {
    unsafe {
        ritmo::parar();
        let _ = ShowWindow(app.hwnd, SW_HIDE);
    }
    app.rueda = None;
}

/// Muestra otro nivel: un grupo o, con `None`, la rueda principal.
fn navegar(app: &mut App, nivel: Option<usize>) {
    let Some(r) = app.rueda.as_mut() else { return };
    r.nivel = nivel;
    r.apuntado = None;
    r.en_centro = None;
    r.seleccion = None;
    // La animación de apertura se repite: las baldosas del nuevo nivel salen del centro.
    r.inicio = Instant::now();
    r.progreso = 0.0;
    pintar(app);
}

/// Modo clic: el izquierdo abre el grupo o la app apuntada; el derecho, o un clic en el centro,
/// vuelve a la rueda principal o, si ya está en ella, la cierra. Devuelve el acceso a abrir.
fn clic(app: &mut App, derecho: bool) -> Option<config::Acceso> {
    actualizar(app);
    let r = app.rueda.as_ref()?;
    let (nivel, seleccion) = (r.nivel, r.seleccion);
    let Some(i) = seleccion.filter(|_| !derecho) else {
        match nivel {
            Some(_) => navegar(app, None),
            None => {
                cerrar(app);
                hook::terminar();
            }
        }
        return None;
    };
    if nivel.is_none() && app.raiz.get(i).is_some_and(Baldosa::es_grupo) {
        navegar(app, Some(i));
        return None;
    }
    let acceso = app.acceso(nivel, i);
    cerrar(app);
    hook::terminar();
    acceso
}

/// Recalcula la selección y el avance de la animación; solo repinta si algo cambió.
fn actualizar(app: &mut App) {
    let Some(nivel) = app.rueda.as_ref().map(|r| r.nivel) else { return };
    let baldosas = app.baldosas(nivel);
    let n = baldosas.len();
    let es_grupo: Vec<bool> = baldosas.iter().map(Baldosa::es_grupo).collect();
    let zona = app.cfg.zona_central;
    // En modo clic se navega con clics, no esperando.
    let por_espera = app.cfg.modo == Modo::Soltar;
    let Some(r) = app.rueda.as_mut() else { return };
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    let dx = (p.x - r.origen.x) as f32 - r.centro.0;
    let dy = (p.y - r.origen.y) as f32 - r.centro.1;
    let radio_central = zona * r.escala;
    let seleccion = wheel::sector_en(dx, dy, n, radio_central);
    let distancia = (dx * dx + dy * dy).sqrt();
    let ahora = Instant::now();

    // Navegación entre niveles.
    let mut cambio: Option<Option<usize>> = None;
    match r.nivel {
        _ if !por_espera => {}
        None => match seleccion.filter(|&i| es_grupo[i]) {
            Some(i) => {
                let desde = match r.apuntado {
                    Some((g, t)) if g == i => t,
                    _ => {
                        r.apuntado = Some((i, ahora));
                        ahora
                    }
                };
                // Se abre al quedarse apuntando un momento, o al pasar de largo la baldosa.
                let lejos = distancia > (wheel::radio_anillo(n) + wheel::BALDOSA * 0.75) * r.escala;
                if lejos || ahora.duration_since(desde).as_millis() >= ESPERA_GRUPO_MS {
                    cambio = Some(Some(i));
                }
            }
            None => r.apuntado = None,
        },
        Some(_) => {
            // Volver: quedarse un momento en el círculo central (soltar ahí sigue cancelando).
            if distancia < radio_central {
                let desde = *r.en_centro.get_or_insert(ahora);
                if ahora.duration_since(desde).as_millis() >= ESPERA_VOLVER_MS {
                    cambio = Some(None);
                }
            } else {
                r.en_centro = None;
            }
        }
    }
    if let Some(nuevo) = cambio {
        navegar(app, nuevo);
        return;
    }

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
    let grupo = r.nivel.and_then(|g| app.cfg.grupos.get(g)).map(|g| g.nombre.as_str());
    // Por campos y no con `app.baldosas()`: el renderer se presta mutable a la vez.
    let baldosas: &[Baldosa] = match r.nivel.and_then(|g| app.grupos.get(g)) {
        Some(b) => b,
        None => &app.raiz,
    };
    let escena = Escena {
        centro: r.centro,
        escala: r.escala,
        baldosas,
        grupo,
        seleccion: r.seleccion,
        progreso: r.progreso,
        oscurecer: app.cfg.oscurecer,
        radio_central: app.cfg.zona_central * r.escala,
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
