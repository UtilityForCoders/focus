//! Ventana de ajustes, dibujada entera con Direct2D sobre un fondo acrílico translúcido.
//!
//! A la izquierda, una barra con las secciones; a la derecha, filas con título, descripción y
//! un control (interruptor, selector segmentado o deslizador). En "Accesos", todas las apps
//! instaladas con su ícono y buscador, los grupos, y la lista elegida (la rueda principal o un
//! grupo) con su editor.
//!
//! **Cómo está hecha.** Es una interfaz de modo inmediato: cada `WM_PAINT` dibuja todo desde el
//! estado y, de paso, apunta en `zonas` qué rectángulo hace qué. El ratón se resuelve contra esas
//! zonas. No hay controles nativos: los de Windows no se pueden pintar sobre el fondo acrílico
//! (GDI escribe alfa 0 y el texto sale transparente), así que los campos de texto también son
//! propios, con cursor, borrar, flechas, Inicio/Fin, Ctrl+A y Ctrl+V.
//!
//! Las coordenadas son DIP (píxeles lógicos): el render target lleva el DPI de la ventana y el
//! ratón se divide por la escala al entrar.
//!
//! Sobre el estado: vive en un `RefCell`. Los diálogos modales (Examinar) y abrir archivos se
//! hacen con el estado devuelto, porque tienen su propio bucle de mensajes.

use crate::apps::{self, AppInstalada};
use crate::autostart;
use crate::config::{self, AbrirEn, Acceso, Boton, Config, Grupo, Modo};
use crate::icons::{self, Icono, IconoRef};
use crate::lucide;
use std::collections::HashMap;
use std::sync::Arc;
use crate::launch;
use crate::render::{self, Escena};
use crate::ui::{dentro, glifo, rect, rgba, tema, Alin, Color, Peso, Pintor, Rect};
use std::cell::RefCell;
use std::sync::atomic::{AtomicIsize, Ordering};
use windows::core::{w, Interface, BOOL, PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
use windows::Win32::UI::Controls::Dialogs::*;
use windows::Win32::UI::Controls::{MARGINS, WM_MOUSELEAVE};
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Avisa a la ventana principal de que la configuración cambió (para precargar íconos).
pub const WM_ORBYT_CONFIG: u32 = WM_APP + 20;
const WM_APPS_LISTA: u32 = WM_APP + 30;
const WM_APPS_ICONOS: u32 = WM_APP + 31;
const TIMER_CURSOR: usize = 1;

const ANCHO_LATERAL: f32 = 212.0;
const FILA_APP: f32 = 40.0;
const FILA_ACCESO: f32 = 44.0;
/// Más accesos que estos y la rueda empieza a quedar apretada.
const RECOMENDADO: usize = 10;

static VENTANA: AtomicIsize = AtomicIsize::new(0);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Seccion {
    General,
    Activacion,
    Apariencia,
    Accesos,
    Acerca,
}

const SECCIONES: [(Seccion, char, &str); 5] = [
    (Seccion::General, glifo::AJUSTES, "General"),
    (Seccion::Activacion, glifo::RATON, "Activación"),
    (Seccion::Apariencia, glifo::COLOR, "Apariencia"),
    (Seccion::Accesos, glifo::APPS, "Accesos"),
    (Seccion::Acerca, glifo::INFO, "Acerca de"),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CampoId {
    Buscar = 0,
    Nombre = 1,
    Destino = 2,
    Args = 3,
    BuscarIcono = 4,
    Archivo = 5,
    NombreGrupo = 6,
}

/// Pestañas del selector de íconos.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ModoSel {
    Lucide,
    Archivo,
}

/// Archivo que se abre por defecto en la pestaña "Archivo": tiene más de 300 íconos.
const ARCHIVO_ICONOS: &str = "%SystemRoot%\\System32\\shell32.dll";
const CELDA: f32 = 52.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Desliz {
    Retardo,
    Tamano,
    Zona,
    OpacidadFondo,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Accion {
    Seccion(Seccion),
    AbrirEn(AbrirEn),
    Boton(Boton),
    Modo(Modo),
    Deslizador(Desliz),
    InicioWindows,
    Oscurecer,
    MostrarNombres,
    Campo(CampoId),
    AgregarApp(usize),
    Elegir(usize),
    Subir(usize),
    Bajar(usize),
    Quitar(usize),
    Examinar,
    Guardar,
    Nuevo,
    Recargar,
    AbrirConfig,
    AbrirCarpeta,
    Salir,
    AbrirSelector,
    CerrarSelector,
    /// Zona que solo tapa lo de debajo (el panel del selector).
    Bloqueo,
    IconoAuto,
    ModoSelector(ModoSel),
    ElegirLucide(usize),
    ElegirArchivo(i32),
    ExaminarIconos,
    /// Qué lista se edita: la rueda principal (`None`) o un grupo.
    Contexto(Option<usize>),
    NuevoGrupo,
    GrupoIzquierda,
    GrupoDerecha,
    QuitarGrupo,
    IconoGrupo,
}

/// Lo que hay que hacer después de soltar el estado (diálogos con su propio bucle de mensajes).
enum Diferida {
    Examinar,
    ExaminarIconos,
    Abrir(Acceso),
}

// ─── Campo de texto ─────────────────────────────────────────────────────────────────────────

/// Texto editable de una línea. El cursor cuenta caracteres, no bytes.
#[derive(Default, Clone)]
struct Campo {
    texto: String,
    cursor: usize,
    /// Ctrl+A: lo próximo que se escriba reemplaza todo.
    todo: bool,
}

impl Campo {
    fn largo(&self) -> usize {
        self.texto.chars().count()
    }

    fn byte(&self, c: usize) -> usize {
        self.texto.char_indices().nth(c).map(|(i, _)| i).unwrap_or(self.texto.len())
    }

    /// Posición del cursor en unidades UTF-16 (lo que mide DirectWrite).
    fn cursor_utf16(&self) -> usize {
        self.texto.chars().take(self.cursor).map(char::len_utf16).sum()
    }

    fn poner(&mut self, s: &str) {
        self.texto = s.to_string();
        self.cursor = self.largo();
        self.todo = false;
    }

    fn quitar_seleccion(&mut self) -> bool {
        if self.todo {
            self.texto.clear();
            self.cursor = 0;
            self.todo = false;
            return true;
        }
        false
    }

    fn insertar(&mut self, s: &str) {
        self.quitar_seleccion();
        // Una sola línea: los saltos de un texto pegado se vuelven espacios.
        let limpio: String = s.chars().map(|c| if c == '\r' || c == '\n' || c == '\t' { ' ' } else { c }).collect();
        let b = self.byte(self.cursor);
        self.texto.insert_str(b, &limpio);
        self.cursor += limpio.chars().count();
    }

    fn borrar_atras(&mut self) {
        if self.quitar_seleccion() || self.cursor == 0 {
            return;
        }
        let b = self.byte(self.cursor - 1);
        self.texto.remove(b);
        self.cursor -= 1;
    }

    fn borrar_delante(&mut self) {
        if self.quitar_seleccion() || self.cursor >= self.largo() {
            return;
        }
        let b = self.byte(self.cursor);
        self.texto.remove(b);
    }

    fn mover(&mut self, delta: isize) {
        self.todo = false;
        self.cursor = (self.cursor as isize + delta).clamp(0, self.largo() as isize) as usize;
    }

    fn ir(&mut self, posicion: usize) {
        self.todo = false;
        self.cursor = posicion.min(self.largo());
    }
}

// ─── Estado ─────────────────────────────────────────────────────────────────────────────────

struct Zona {
    r: Rect,
    accion: Accion,
}

struct Ajustes {
    hwnd: HWND,
    principal: HWND,
    factory: ID2D1Factory,
    dwrite: IDWriteFactory,
    rt: Option<ID2D1HwndRenderTarget>,
    pintor: Option<Pintor>,
    /// El fondo acrílico está activo (Windows 11). Si no, se pinta un fondo opaco.
    vidrio: bool,
    escala: f32,

    seccion: Seccion,
    cfg: Config,
    inicio_windows: bool,

    apps: Vec<AppInstalada>,
    cargando: bool,
    /// Posiciones en `apps` que pasan el filtro de búsqueda, en orden.
    filtradas: Vec<usize>,

    campos: [Campo; 7],
    foco: Option<CampoId>,
    cursor_visible: bool,
    /// Primera mitad de un carácter fuera del plano básico (WM_CHAR llega en dos partes).
    sustituto: Option<u16>,
    seleccion: Option<usize>,
    /// Ícono elegido en el editor (`lucide:...` o `archivo:...`); `None` es el automático.
    icono_editor: Option<String>,
    /// Grupo cuya lista se edita; `None` es la rueda principal.
    grupo: Option<usize>,
    /// El selector de íconos está eligiendo el del grupo, no el del acceso.
    selector_grupo: bool,
    /// Se pulsó "quitar grupo" una vez: el segundo clic lo borra.
    confirmar_quitar: bool,

    selector: Option<ModoSel>,
    scroll_sel: f32,
    max_sel: f32,
    area_sel: Rect,
    lucide_filtrados: Vec<usize>,
    archivo_cantidad: u32,
    iconos_archivo: HashMap<(String, i32), Option<IconoRef>>,

    zonas: Vec<Zona>,
    raton: (f32, f32),
    hover: Option<Accion>,
    pulsada: Option<Accion>,
    arrastre: Option<Desliz>,
    pistas: Vec<(Desliz, Rect)>,
    campos_r: Vec<(CampoId, Rect, f32)>,

    scroll_apps: f32,
    scroll_accesos: f32,
    area_apps: Rect,
    area_accesos: Rect,
    max_apps: f32,
    max_accesos: f32,
}

thread_local! {
    static AJUSTES: RefCell<Option<Ajustes>> = const { RefCell::new(None) };
}

fn con_ajustes<R>(f: impl FnOnce(&mut Ajustes) -> R) -> Option<R> {
    AJUSTES.with(|a| match a.try_borrow_mut() {
        Ok(mut g) => g.as_mut().map(f),
        Err(_) => None,
    })
}

/// Abre la ventana, o la trae al frente si ya estaba abierta.
pub fn abrir(principal: HWND) {
    let v = VENTANA.load(Ordering::Relaxed);
    if v != 0 {
        unsafe {
            let h = HWND(v as *mut _);
            let _ = ShowWindow(h, SW_RESTORE);
            let _ = SetForegroundWindow(h);
        }
        return;
    }
    if let Err(e) = crear(principal) {
        eprintln!("no se pudo abrir los ajustes: {e}");
    }
}

/// Cierra la ventana si está abierta (al salir de Orbyt).
pub fn cerrar() {
    let v = VENTANA.load(Ordering::Relaxed);
    if v != 0 {
        unsafe {
            let _ = DestroyWindow(HWND(v as *mut _));
        }
    }
}

/// Repinta la ventana si está abierta (por ejemplo, porque llegó un favicon).
pub fn refrescar() {
    con_ajustes(|a| a.invalidar());
}

fn crear(principal: HWND) -> windows::core::Result<()> {
    unsafe {
        let instancia: HINSTANCE = GetModuleHandleW(None)?.into();
        let clase = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: instancia,
            lpszClassName: w!("OrbytAjustes"),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hIcon: LoadIconW(None, IDI_APPLICATION)?,
            ..Default::default()
        };
        RegisterClassW(&clase);

        // Tamaño inicial según el DPI del monitor donde está el puntero, centrado en él.
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        let monitor = MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let s = dx as f32 / 96.0;
        let t = info.rcWork;
        let (an, al) = (((1000.0 * s) as i32).min(t.right - t.left), ((680.0 * s) as i32).min(t.bottom - t.top));
        let (x, y) = (t.left + (t.right - t.left - an) / 2, t.top + (t.bottom - t.top - al) / 2);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("OrbytAjustes"),
            w!("Orbyt"),
            WS_OVERLAPPEDWINDOW,
            x,
            y,
            an,
            al,
            None,
            None,
            Some(instancia),
            None,
        )?;

        // Barra de título oscura y fondo acrílico. En Windows 10 el fondo falla y se usa uno opaco.
        let oscuro = BOOL(1);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &oscuro as *const _ as *const _, 4);
        let fondo = DWMSBT_TRANSIENTWINDOW;
        let vidrio = DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &fondo as *const _ as *const _, 4).is_ok()
            && DwmExtendFrameIntoClientArea(hwnd, &MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 }).is_ok();

        let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
        let mut a = Ajustes {
            hwnd,
            principal,
            factory,
            dwrite,
            rt: None,
            pintor: None,
            vidrio,
            escala: GetDpiForWindow(hwnd).max(96) as f32 / 96.0,
            seccion: Seccion::Accesos,
            cfg: config::cargar(),
            inicio_windows: autostart::activo(),
            apps: Vec::new(),
            cargando: true,
            filtradas: Vec::new(),
            campos: Default::default(),
            foco: None,
            cursor_visible: true,
            sustituto: None,
            seleccion: None,
            icono_editor: None,
            grupo: None,
            selector_grupo: false,
            confirmar_quitar: false,
            selector: None,
            scroll_sel: 0.0,
            max_sel: 0.0,
            area_sel: rect(0.0, 0.0, 0.0, 0.0),
            lucide_filtrados: lucide::buscar(""),
            archivo_cantidad: 0,
            iconos_archivo: HashMap::new(),
            zonas: Vec::new(),
            raton: (-1.0, -1.0),
            hover: None,
            pulsada: None,
            arrastre: None,
            pistas: Vec::new(),
            campos_r: Vec::new(),
            scroll_apps: 0.0,
            scroll_accesos: 0.0,
            area_apps: rect(0.0, 0.0, 0.0, 0.0),
            area_accesos: rect(0.0, 0.0, 0.0, 0.0),
            max_apps: 0.0,
            max_accesos: 0.0,
        };
        a.campo(CampoId::Archivo).poner(ARCHIVO_ICONOS);
        match apps::en_cache() {
            Some(lista) => {
                a.apps = lista;
                a.cargando = false;
            }
            None => apps::cargar_en_segundo_plano(hwnd, WM_APPS_LISTA, WM_APPS_ICONOS),
        }
        a.filtrar();
        a.ir_a((!a.cfg.grupos.is_empty()).then_some(0));

        AJUSTES.with(|s| *s.borrow_mut() = Some(a));
        VENTANA.store(hwnd.0 as isize, Ordering::Relaxed);
        SetTimer(Some(hwnd), TIMER_CURSOR, 530, None);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
    }
    Ok(())
}

// ─── Acciones ───────────────────────────────────────────────────────────────────────────────

impl Ajustes {
    fn invalidar(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn guardar(&self) {
        if let Err(e) = config::guardar(&self.cfg) {
            eprintln!("no se pudo guardar config.json: {e}");
        }
        unsafe {
            let _ = PostMessageW(Some(self.principal), WM_ORBYT_CONFIG, WPARAM(0), LPARAM(0));
        }
    }

    fn campo(&mut self, id: CampoId) -> &mut Campo {
        &mut self.campos[id as usize]
    }

    fn filtrar(&mut self) {
        let q = self.campos[CampoId::Buscar as usize].texto.clone();
        self.filtradas = (0..self.apps.len()).filter(|&i| apps::coincide(&self.apps[i], &q)).collect();
        self.scroll_apps = 0.0;
    }

    /// Accesos de la lista que se está editando (la rueda principal o un grupo).
    fn lista(&self) -> &Vec<Acceso> {
        match self.grupo.and_then(|g| self.cfg.grupos.get(g)) {
            Some(g) => &g.accesos,
            None => &self.cfg.accesos,
        }
    }

    fn lista_mut(&mut self) -> &mut Vec<Acceso> {
        match self.grupo {
            Some(g) if g < self.cfg.grupos.len() => &mut self.cfg.grupos[g].accesos,
            _ => &mut self.cfg.accesos,
        }
    }

    fn en_rueda(&self, destino: &str) -> bool {
        self.lista().iter().any(|a| a.destino == destino)
    }

    /// Cambia la lista que se edita.
    fn ir_a(&mut self, grupo: Option<usize>) {
        self.grupo = grupo.filter(|&g| g < self.cfg.grupos.len());
        self.scroll_accesos = 0.0;
        self.foco = None;
        let nombre = self.grupo.map(|g| self.cfg.grupos[g].nombre.clone()).unwrap_or_default();
        self.campo(CampoId::NombreGrupo).poner(&nombre);
        self.elegir(None);
    }

    /// Ícono que muestra marcado el selector: el del grupo o el del editor.
    fn icono_objetivo(&self) -> Option<String> {
        if self.selector_grupo {
            self.grupo.and_then(|g| self.cfg.grupos.get(g)).and_then(|g| g.icono.clone())
        } else {
            self.icono_editor.clone()
        }
    }

    fn elegir(&mut self, i: Option<usize>) {
        self.seleccion = i.filter(|&i| i < self.lista().len());
        let (n, d, a) = match self.seleccion.map(|i| &self.lista()[i]) {
            Some(x) => (x.nombre.clone(), x.destino.clone(), x.argumentos.clone().unwrap_or_default()),
            None => Default::default(),
        };
        self.icono_editor = self.seleccion.and_then(|i| self.lista()[i].icono.clone());
        self.campo(CampoId::Nombre).poner(&n);
        self.campo(CampoId::Destino).poner(&d);
        self.campo(CampoId::Args).poner(&a);
    }

    /// Cambia el ícono del editor; si se está editando un acceso, se guarda ya.
    fn poner_icono(&mut self, icono: Option<String>) {
        self.selector = None;
        self.foco = None;
        if self.selector_grupo {
            if let Some(g) = self.grupo.filter(|&g| g < self.cfg.grupos.len()) {
                self.cfg.grupos[g].icono = icono;
                self.guardar();
            }
            return;
        }
        self.icono_editor = icono;
        if let Some(i) = self.seleccion {
            self.lista_mut()[i].icono = self.icono_editor.clone();
            self.guardar();
        }
    }

    fn cargar_archivo(&mut self) {
        let ruta = self.campos[CampoId::Archivo as usize].texto.trim().to_string();
        self.archivo_cantidad = if ruta.is_empty() { 0 } else { icons::cantidad_en_archivo(&ruta).min(5000) };
        self.scroll_sel = 0.0;
    }

    fn abrir_selector(&mut self) {
        let modo = match self.icono_objetivo() {
            Some(i) if i.starts_with("archivo:") => {
                if let Some((ruta, _)) = icons::partir_archivo(&i["archivo:".len()..]) {
                    self.campo(CampoId::Archivo).poner(&ruta);
                }
                ModoSel::Archivo
            }
            _ => ModoSel::Lucide,
        };
        self.cambiar_modo(modo);
    }

    fn cambiar_modo(&mut self, modo: ModoSel) {
        self.selector = Some(modo);
        self.scroll_sel = 0.0;
        match modo {
            ModoSel::Lucide => self.foco = Some(CampoId::BuscarIcono),
            ModoSel::Archivo => {
                self.foco = Some(CampoId::Archivo);
                self.cargar_archivo();
            }
        }
    }

    /// Sin grupos no hay dónde poner un acceso: se crea uno.
    fn asegurar_grupo(&mut self) {
        if self.grupo.is_none() {
            self.cfg.grupos.push(Grupo { nombre: "Principal".into(), icono: None, accesos: Vec::new() });
            self.ir_a(Some(self.cfg.grupos.len() - 1));
        }
    }

    fn agregar_app(&mut self, i: usize) {
        self.asegurar_grupo();
        let Some(app) = self.apps.get(i) else { return };
        if let Some(ya) = self.lista().iter().position(|a| a.destino == app.destino) {
            self.elegir(Some(ya));
            return;
        }
        let nuevo = Acceso { nombre: app.nombre.clone(), destino: app.destino.clone(), argumentos: None, icono: None };
        self.lista_mut().push(nuevo);
        self.guardar();
        self.elegir(Some(self.lista().len() - 1));
        // Que el recién agregado quede a la vista.
        self.scroll_accesos = f32::MAX;
    }

    fn guardar_formulario(&mut self) {
        let nombre = self.campos[CampoId::Nombre as usize].texto.trim().to_string();
        let destino = self.campos[CampoId::Destino as usize].texto.trim().to_string();
        let args = self.campos[CampoId::Args as usize].texto.trim().to_string();
        if nombre.is_empty() || destino.is_empty() {
            unsafe {
                let _ = MessageBeep(MB_ICONWARNING);
            }
            self.foco = Some(if nombre.is_empty() { CampoId::Nombre } else { CampoId::Destino });
            return;
        }
        let acceso = Acceso { nombre, destino, argumentos: (!args.is_empty()).then_some(args), icono: self.icono_editor.clone() };
        if self.grupo.is_none() {
            // `ir_a` limpia el editor, pero el acceso ya está leído.
            self.asegurar_grupo();
        }
        match self.seleccion {
            Some(i) => self.lista_mut()[i] = acceso,
            None => {
                self.lista_mut().push(acceso);
                self.seleccion = Some(self.lista().len() - 1);
                self.scroll_accesos = f32::MAX;
            }
        }
        self.foco = None;
        self.guardar();
    }

    fn poner_deslizador(&mut self, d: Desliz, x: f32) {
        let Some((_, pista)) = self.pistas.iter().find(|(id, _)| *id == d) else { return };
        let v = ((x - pista.left) / (pista.right - pista.left)).clamp(0.0, 1.0);
        match d {
            Desliz::Retardo => {
                let ms = config::RETARDO_MIN as f32 + v * (config::RETARDO_MAX - config::RETARDO_MIN) as f32;
                self.cfg.retardo_ms = ((ms / 10.0).round() * 10.0) as u32;
            }
            Desliz::Tamano => {
                let t = config::TAMANO_MIN + v * (config::TAMANO_MAX - config::TAMANO_MIN);
                self.cfg.tamano = (t * 20.0).round() / 20.0;
            }
            Desliz::Zona => {
                self.cfg.zona_central = (config::ZONA_MIN + v * (config::ZONA_MAX - config::ZONA_MIN)).round();
            }
            Desliz::OpacidadFondo => {
                let op = config::OPACIDAD_MIN + v * (config::OPACIDAD_MAX - config::OPACIDAD_MIN);
                self.cfg.opacidad_fondo = (op * 100.0).round() / 100.0;
            }
        }
    }

    /// Ejecuta un clic. Lo que necesita un bucle de mensajes propio se devuelve para después.
    fn ejecutar(&mut self, accion: Accion) -> Option<Diferida> {
        if accion != Accion::QuitarGrupo {
            self.confirmar_quitar = false;
        }
        match accion {
            Accion::Seccion(s) => {
                self.seccion = s;
                self.foco = None;
                self.selector = None;
            }
            Accion::AbrirEn(v) => {
                self.cfg.abrir_en = v;
                self.guardar();
            }
            Accion::Modo(m) => {
                self.cfg.modo = m;
                self.guardar();
            }
            Accion::Boton(b) => {
                self.cfg.boton = b;
                self.guardar();
            }
            Accion::InicioWindows => {
                autostart::poner(!self.inicio_windows);
                self.inicio_windows = autostart::activo();
            }
            Accion::Oscurecer => {
                self.cfg.oscurecer = !self.cfg.oscurecer;
                self.guardar();
            }
            Accion::MostrarNombres => {
                self.cfg.mostrar_nombres = !self.cfg.mostrar_nombres;
                self.guardar();
            }
            Accion::AgregarApp(i) => self.agregar_app(i),
            Accion::Elegir(i) => self.elegir(Some(i)),
            Accion::Subir(i) if i > 0 => {
                self.lista_mut().swap(i, i - 1);
                self.guardar();
                self.elegir(Some(i - 1));
            }
            Accion::Bajar(i) if i + 1 < self.lista().len() => {
                self.lista_mut().swap(i, i + 1);
                self.guardar();
                self.elegir(Some(i + 1));
            }
            Accion::Quitar(i) if i < self.lista().len() => {
                self.lista_mut().remove(i);
                self.guardar();
                let n = self.lista().len();
                self.elegir((n > 0).then(|| i.min(n - 1)));
            }
            Accion::Contexto(g) => self.ir_a(g),
            Accion::NuevoGrupo => {
                self.cfg.grupos.push(Grupo { nombre: "Nuevo grupo".into(), icono: None, accesos: Vec::new() });
                self.guardar();
                self.ir_a(Some(self.cfg.grupos.len() - 1));
                // El nombre queda seleccionado: lo que se escriba lo reemplaza.
                self.foco = Some(CampoId::NombreGrupo);
                self.campo(CampoId::NombreGrupo).todo = true;
            }
            Accion::GrupoIzquierda => {
                if let Some(g) = self.grupo.filter(|&g| g > 0 && g < self.cfg.grupos.len()) {
                    self.cfg.grupos.swap(g, g - 1);
                    self.grupo = Some(g - 1);
                    self.guardar();
                }
            }
            Accion::GrupoDerecha => {
                if let Some(g) = self.grupo.filter(|&g| g + 1 < self.cfg.grupos.len()) {
                    self.cfg.grupos.swap(g, g + 1);
                    self.grupo = Some(g + 1);
                    self.guardar();
                }
            }
            Accion::QuitarGrupo => {
                if let Some(g) = self.grupo.filter(|&g| g < self.cfg.grupos.len()) {
                    // Un grupo puede tener muchos accesos: hace falta un segundo clic.
                    if !self.confirmar_quitar {
                        self.confirmar_quitar = true;
                    } else {
                        self.confirmar_quitar = false;
                        self.cfg.grupos.remove(g);
                        self.guardar();
                        let n = self.cfg.grupos.len();
                        self.ir_a((n > 0).then(|| g.min(n - 1)));
                    }
                }
            }
            Accion::IconoGrupo => {
                self.selector_grupo = true;
                self.abrir_selector();
            }
            Accion::Guardar => self.guardar_formulario(),
            Accion::Nuevo => {
                self.elegir(None);
                self.foco = Some(CampoId::Nombre);
            }
            Accion::Recargar => {
                self.cargando = true;
                self.apps.clear();
                self.filtrar();
                apps::cargar_en_segundo_plano(self.hwnd, WM_APPS_LISTA, WM_APPS_ICONOS);
            }
            Accion::Examinar => return Some(Diferida::Examinar),
            Accion::ExaminarIconos => return Some(Diferida::ExaminarIconos),
            Accion::AbrirSelector => {
                self.selector_grupo = false;
                self.abrir_selector();
            }
            Accion::CerrarSelector => {
                self.selector = None;
                self.foco = None;
            }
            Accion::ModoSelector(m) => self.cambiar_modo(m),
            Accion::IconoAuto => self.poner_icono(None),
            Accion::ElegirLucide(i) => self.poner_icono(Some(format!("lucide:{}", lucide::nombre_en(i)))),
            Accion::ElegirArchivo(i) => {
                let ruta = self.campos[CampoId::Archivo as usize].texto.trim().to_string();
                self.poner_icono(Some(format!("archivo:{ruta},{i}")));
            }
            Accion::AbrirConfig => {
                return Some(Diferida::Abrir(Acceso {
                    nombre: String::new(),
                    destino: "notepad.exe".into(),
                    argumentos: Some(format!("\"{}\"", config::ruta().display())),
                    icono: None,
                }))
            }
            Accion::Salir => unsafe {
                // La ventana principal cierra todo (ganchos, esta ventana) al destruirse.
                let _ = PostMessageW(Some(self.principal), WM_CLOSE, WPARAM(0), LPARAM(0));
            },
            Accion::AbrirCarpeta => {
                let dir = config::ruta().parent().map(|d| d.display().to_string()).unwrap_or_default();
                return Some(Diferida::Abrir(Acceso { nombre: String::new(), destino: dir, argumentos: None, icono: None }));
            }
            _ => {}
        }
        None
    }

    fn zona_en(&self, x: f32, y: f32) -> Option<Accion> {
        self.zonas.iter().rev().find(|z| dentro(&z.r, x, y)).map(|z| z.accion)
    }

    fn zona(&mut self, r: Rect, accion: Accion) {
        self.zonas.push(Zona { r, accion });
    }

    /// Coloca el cursor del campo donde se hizo clic.
    fn cursor_en(&mut self, id: CampoId, x: f32) {
        let Some(&(_, r, desplaz)) = self.campos_r.iter().find(|(c, _, _)| *c == id) else { return };
        let Some(p) = self.pintor.as_mut() else { return };
        let campo = &self.campos[id as usize];
        let rel = x - (r.left + 12.0) + desplaz;
        let (mut mejor, mut dist) = (0, f32::MAX);
        let mut u16s = 0;
        for (i, c) in campo.texto.chars().chain(std::iter::once(' ')).enumerate() {
            let px = p.medir_hasta(&campo.texto, 13.0, Peso::Normal, u16s);
            if (px - rel).abs() < dist {
                dist = (px - rel).abs();
                mejor = i;
            }
            u16s += c.len_utf16();
        }
        self.campos[id as usize].ir(mejor);
    }
}

// ─── Dibujo ─────────────────────────────────────────────────────────────────────────────────

impl Ajustes {
    fn asegurar_rt(&mut self) -> Option<ID2D1HwndRenderTarget> {
        if let Some(rt) = &self.rt {
            return Some(rt.clone());
        }
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
        let rt = unsafe { self.factory.CreateHwndRenderTarget(&props, &hprops).ok()? };
        self.pintor = Some(Pintor::nuevo(rt.cast().ok()?, self.dwrite.clone()).ok()?);
        self.rt = Some(rt.clone());
        Some(rt)
    }

    fn pintar(&mut self) {
        let Some(rt) = self.asegurar_rt() else { return };
        let Some(mut p) = self.pintor.take() else { return };
        let mut cr = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut cr);
        }
        let (w, h) = (cr.right as f32 / self.escala, cr.bottom as f32 / self.escala);
        self.zonas.clear();
        self.pistas.clear();
        self.campos_r.clear();
        unsafe {
            rt.BeginDraw();
            rt.SetTransform(&windows_numerics::Matrix3x2::identity());
            rt.Clear(Some(&tema::TRANSPARENTE));
        }
        // Sobre el acrílico, un velo oscuro que le da el tono; sin acrílico, un fondo opaco.
        let (fondo, lateral) = if self.vidrio {
            (rgba(14, 14, 16, 0.58), rgba(255, 255, 255, 0.025))
        } else {
            (rgba(24, 24, 27, 1.0), rgba(30, 30, 34, 1.0))
        };
        p.rellenar(rect(0.0, 0.0, w, h), 0.0, fondo);
        p.rellenar(rect(0.0, 0.0, ANCHO_LATERAL, h), 0.0, lateral);
        p.linea(ANCHO_LATERAL, 0.0, ANCHO_LATERAL, h, tema::SEPARADOR, 1.0);

        self.lateral(&mut p, h);
        let x = ANCHO_LATERAL + 32.0;
        let cw = (w - x - 32.0).max(200.0);
        match self.seccion {
            Seccion::General => self.general(&mut p, x, cw),
            Seccion::Activacion => self.activacion(&mut p, x, cw),
            Seccion::Apariencia => self.apariencia(&mut p, x, cw, h),
            Seccion::Accesos => self.accesos(&mut p, x, cw, h),
            Seccion::Acerca => self.acerca(&mut p, x, cw),
        }
        if self.seccion == Seccion::Accesos && self.selector.is_some() {
            self.selector_ui(&mut p, w, h);
        }

        let r = unsafe { rt.EndDraw(None, None) };
        self.pintor = Some(p);
        if r.is_err() {
            self.rt = None;
            self.pintor = None;
        }
    }

    fn lateral(&mut self, p: &mut Pintor, h: f32) {
        p.texto("Ajustes", rect(22.0, 20.0, 170.0, 30.0), 20.0, Peso::Semi, Alin::Izq, tema::TEXTO);
        let mut y = 70.0;
        for (s, g, nombre) in SECCIONES {
            let r = rect(10.0, y, ANCHO_LATERAL - 20.0, 36.0);
            let activa = self.seccion == s;
            let sobre = self.hover == Some(Accion::Seccion(s));
            if activa {
                p.rellenar(r, 7.0, tema::CONTROL_ACTIVO);
                p.rellenar(rect(r.left, r.top + 10.0, 3.0, 16.0), 1.5, tema::CLARO);
            } else if sobre {
                p.rellenar(r, 7.0, tema::CONTROL);
            }
            let c = if activa { tema::TEXTO } else { tema::TEXTO_2 };
            p.glifo(g, r.left + 22.0, r.top + 18.0, 14.0, c);
            p.texto(nombre, rect(r.left + 42.0, r.top, 140.0, 36.0), 13.0, if activa { Peso::Semi } else { Peso::Normal }, Alin::Izq, c);
            self.zona(r, Accion::Seccion(s));
            y += 40.0;
        }
        let version = format!("Orbyt {}", env!("CARGO_PKG_VERSION"));
        p.texto(&version, rect(22.0, h - 40.0, 170.0, 24.0), 11.5, Peso::Normal, Alin::Izq, tema::TEXTO_3);
    }

    fn titulo(&self, p: &mut Pintor, x: f32, cw: f32, texto: &str, sub: &str) -> f32 {
        p.texto(texto, rect(x, 22.0, cw, 34.0), 24.0, Peso::Semi, Alin::Izq, tema::TEXTO);
        if sub.is_empty() {
            return 76.0;
        }
        p.texto(sub, rect(x, 56.0, cw, 22.0), 12.5, Peso::Normal, Alin::Izq, tema::TEXTO_2);
        92.0
    }

    fn cabecera(&self, p: &mut Pintor, x: f32, y: f32, cw: f32, texto: &str) -> f32 {
        p.texto(texto, rect(x, y, cw, 22.0), 12.0, Peso::Semi, Alin::Izq, tema::TEXTO_2);
        y + 28.0
    }

    /// Fila con título y descripción a la izquierda; devuelve el centro vertical del control.
    fn fila(&self, p: &mut Pintor, x: f32, y: f32, cw: f32, titulo: &str, desc: &str) -> (f32, f32) {
        let alto = 66.0;
        p.rellenar(rect(x, y, cw, alto - 6.0), 10.0, tema::CONTROL);
        let ancho_txt = (cw - 360.0).max(220.0);
        p.texto(titulo, rect(x + 16.0, y + 9.0, ancho_txt, 22.0), 13.5, Peso::Semi, Alin::Izq, tema::TEXTO);
        p.texto(desc, rect(x + 16.0, y + 30.0, ancho_txt, 20.0), 12.0, Peso::Normal, Alin::Izq, tema::TEXTO_2);
        (y + (alto - 6.0) / 2.0, y + alto)
    }

    fn interruptor(&mut self, p: &mut Pintor, derecha: f32, cy: f32, activo: bool, accion: Accion) {
        let r = rect(derecha - 44.0, cy - 11.0, 44.0, 22.0);
        let sobre = self.hover == Some(accion);
        if activo {
            p.rellenar(r, 11.0, if sobre { rgba(255, 255, 255, 1.0) } else { tema::CLARO });
            p.circulo(r.right - 11.0, cy, 7.0, tema::TEXTO_OSCURO);
        } else {
            p.rellenar(r, 11.0, if sobre { tema::CONTROL_ACTIVO } else { tema::CONTROL_HOVER });
            p.bordear(r, 11.0, tema::BORDE_FUERTE, 1.0);
            p.circulo(r.left + 11.0, cy, 6.0, tema::TEXTO_2);
        }
        self.zona(rect(r.left - 4.0, r.top - 4.0, r.right - r.left + 8.0, 30.0), accion);
    }

    fn segmentado(&mut self, p: &mut Pintor, derecha: f32, cy: f32, opciones: &[(&str, Accion, bool)]) {
        let anchos: Vec<f32> = opciones.iter().map(|(t, _, _)| p.medir(t, 12.5, Peso::Medio) + 28.0).collect();
        let total: f32 = anchos.iter().sum::<f32>() + 6.0;
        let caja = rect(derecha - total, cy - 16.0, total, 32.0);
        p.rellenar(caja, 9.0, rgba(0, 0, 0, 0.22));
        p.bordear(caja, 9.0, tema::BORDE, 1.0);
        let mut x = caja.left + 3.0;
        for ((texto, accion, sel), ancho) in opciones.iter().zip(anchos) {
            let r = rect(x, caja.top + 3.0, ancho, 26.0);
            let sobre = self.hover == Some(*accion);
            if *sel {
                p.rellenar(r, 7.0, tema::CLARO);
            } else if sobre {
                p.rellenar(r, 7.0, tema::CONTROL_HOVER);
            }
            let c = if *sel { tema::TEXTO_OSCURO } else if sobre { tema::TEXTO } else { tema::TEXTO_2 };
            p.texto(texto, r, 12.5, Peso::Medio, Alin::Centro, c);
            self.zona(r, *accion);
            x += ancho;
        }
    }

    fn deslizador(&mut self, p: &mut Pintor, derecha: f32, cy: f32, id: Desliz, v: f32, valor: &str) {
        let ancho = 200.0;
        let x = derecha - ancho - 70.0;
        let pista = rect(x, cy - 2.0, ancho, 4.0);
        let v = v.clamp(0.0, 1.0);
        p.rellenar(pista, 2.0, tema::CONTROL_ACTIVO);
        p.rellenar(rect(x, cy - 2.0, ancho * v, 4.0), 2.0, rgba(240, 240, 244, 0.75));
        let activo = self.arrastre == Some(id) || self.hover == Some(Accion::Deslizador(id));
        let kx = x + ancho * v;
        p.circulo(kx, cy + 1.0, 9.0, rgba(0, 0, 0, 0.25));
        p.circulo(kx, cy, if activo { 9.0 } else { 8.0 }, tema::CLARO);
        p.texto(valor, rect(derecha - 62.0, cy - 12.0, 62.0, 24.0), 13.0, Peso::Semi, Alin::Der, tema::TEXTO);
        self.pistas.push((id, pista));
        self.zona(rect(x - 10.0, cy - 14.0, ancho + 20.0, 28.0), Accion::Deslizador(id));
    }

    fn boton(&mut self, p: &mut Pintor, r: Rect, texto: &str, g: Option<char>, primario: bool, accion: Accion) {
        let sobre = self.hover == Some(accion);
        let (fondo, color): (Color, Color) = match (primario, sobre) {
            (true, false) => (tema::CLARO, tema::TEXTO_OSCURO),
            (true, true) => (rgba(255, 255, 255, 1.0), tema::TEXTO_OSCURO),
            (false, false) => (tema::CONTROL_HOVER, tema::TEXTO),
            (false, true) => (tema::CONTROL_ACTIVO, tema::TEXTO),
        };
        p.rellenar(r, 8.0, fondo);
        if !primario {
            p.bordear(r, 8.0, tema::BORDE, 1.0);
        }
        match g {
            Some(g) if texto.is_empty() => p.glifo(g, (r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0, 13.0, color),
            Some(g) => {
                let ancho = p.medir(texto, 12.5, Peso::Semi) + 22.0;
                let x0 = (r.left + r.right - ancho) / 2.0;
                p.glifo(g, x0 + 7.0, (r.top + r.bottom) / 2.0, 12.0, color);
                p.texto(texto, rect(x0 + 22.0, r.top, ancho, r.bottom - r.top), 12.5, Peso::Semi, Alin::Izq, color);
            }
            None => p.texto(texto, r, 12.5, Peso::Semi, Alin::Centro, color),
        }
        self.zona(r, accion);
    }

    fn icono_boton(&mut self, p: &mut Pintor, cx: f32, cy: f32, g: char, accion: Accion, peligro: bool) {
        let r = rect(cx - 14.0, cy - 14.0, 28.0, 28.0);
        let sobre = self.hover == Some(accion);
        if sobre {
            p.rellenar(r, 7.0, tema::CONTROL_ACTIVO);
        }
        let c = if sobre && peligro { tema::PELIGRO } else if sobre { tema::TEXTO } else { tema::TEXTO_2 };
        p.glifo(g, cx, cy, 12.0, c);
        self.zona(r, accion);
    }

    fn campo_texto(&mut self, p: &mut Pintor, r: Rect, id: CampoId, marcador: &str, g: Option<char>) {
        let foco = self.foco == Some(id);
        let sobre = self.hover == Some(Accion::Campo(id));
        p.rellenar(r, 8.0, if foco { rgba(0, 0, 0, 0.30) } else { rgba(0, 0, 0, 0.20) });
        p.bordear(r, 8.0, if foco { rgba(255, 255, 255, 0.45) } else if sobre { tema::BORDE_FUERTE } else { tema::BORDE }, 1.0);
        let mut x0 = r.left + 12.0;
        if let Some(g) = g {
            p.glifo(g, r.left + 18.0, (r.top + r.bottom) / 2.0, 12.0, tema::TEXTO_3);
            x0 = r.left + 34.0;
        }
        let interior = rect(x0, r.top, r.right - 10.0 - x0, r.bottom - r.top);
        let campo = self.campos[id as usize].clone();
        let ancho_vis = interior.right - interior.left;
        let cursor_x = p.medir_hasta(&campo.texto, 13.0, Peso::Normal, campo.cursor_utf16());
        // Desplazamiento horizontal para que el cursor siempre se vea.
        let desplaz = (cursor_x - ancho_vis + 2.0).max(0.0);
        p.recortar(interior);
        if campo.texto.is_empty() {
            p.texto(marcador, interior, 13.0, Peso::Normal, Alin::Izq, tema::TEXTO_3);
        } else {
            if foco && campo.todo {
                let ancho = p.medir(&campo.texto, 13.0, Peso::Normal);
                p.rellenar(rect(interior.left - desplaz, r.top + 7.0, ancho, r.bottom - r.top - 14.0), 3.0, rgba(120, 150, 255, 0.45));
            }
            let t = rect(interior.left - desplaz, r.top, 10_000.0, r.bottom - r.top);
            p.texto(&campo.texto, t, 13.0, Peso::Normal, Alin::Izq, tema::TEXTO);
        }
        if foco && self.cursor_visible && !campo.todo {
            let cx = interior.left + cursor_x - desplaz;
            p.linea(cx, r.top + 8.0, cx, r.bottom - 8.0, tema::TEXTO, 1.2);
        }
        p.fin_recorte();
        self.campos_r.push((id, interior, desplaz));
        self.zona(r, Accion::Campo(id));
    }

    // ─── Secciones ──────────────────────────────────────────────────────────────────────────

    fn general(&mut self, p: &mut Pintor, x: f32, cw: f32) {
        let mut y = self.titulo(p, x, cw, "General", "");
        y = self.cabecera(p, x, y, cw, "Rueda");
        let (cy, sig) = self.fila(p, x, y, cw, "Posición", "Dónde aparece la rueda al mantener el botón.");
        let a = self.cfg.abrir_en;
        self.segmentado(p, x + cw - 16.0, cy, &[
            ("Bajo el puntero", Accion::AbrirEn(AbrirEn::Cursor), a == AbrirEn::Cursor),
            ("Centro de la pantalla", Accion::AbrirEn(AbrirEn::Centro), a == AbrirEn::Centro),
        ]);
        y = sig;
        let (cy, sig) = self.fila(p, x, y, cw, "Retardo", "Cuánto mantenerlo antes de que salga. Un clic corto funciona igual.");
        let v = (self.cfg.retardo_ms - config::RETARDO_MIN) as f32 / (config::RETARDO_MAX - config::RETARDO_MIN) as f32;
        let texto = format!("{} ms", self.cfg.retardo_ms);
        self.deslizador(p, x + cw - 16.0, cy, Desliz::Retardo, v, &texto);
        y = sig + 14.0;
        y = self.cabecera(p, x, y, cw, "Sistema");
        let (cy, _) = self.fila(p, x, y, cw, "Iniciar con Windows", "Orbyt se abre solo al iniciar sesión y queda en la bandeja.");
        let activo = self.inicio_windows;
        self.interruptor(p, x + cw - 16.0, cy, activo, Accion::InicioWindows);
    }

    fn activacion(&mut self, p: &mut Pintor, x: f32, cw: f32) {
        let mut y = self.titulo(p, x, cw, "Activación", "");
        y = self.cabecera(p, x, y, cw, "Ratón");
        let (cy, sig) = self.fila(p, x, y, cw, "Botón", "Qué botón abre la rueda. Los laterales suelen estar libres.");
        let b = self.cfg.boton;
        self.segmentado(p, x + cw - 16.0, cy, &[
            ("Central", Accion::Boton(Boton::Central), b == Boton::Central),
            ("Atrás", Accion::Boton(Boton::Atras), b == Boton::Atras),
            ("Adelante", Accion::Boton(Boton::Adelante), b == Boton::Adelante),
        ]);
        y = sig;
        let m = self.cfg.modo;
        let desc = match m {
            Modo::Soltar => "Apuntas y sueltas. Un grupo se abre tras apuntarlo de forma estable.",
            Modo::Clic => "El botón abre la rueda; eliges con clic izquierdo.",
        };
        let (cy, sig) = self.fila(p, x, y, cw, "Elegir con", desc);
        self.segmentado(p, x + cw - 16.0, cy, &[
            ("Soltar el botón", Accion::Modo(Modo::Soltar), m == Modo::Soltar),
            ("Clic", Accion::Modo(Modo::Clic), m == Modo::Clic),
        ]);
        y = sig;
        let (cy, sig) = self.fila(p, x, y, cw, "Zona central", "Círculo del centro: cancela o vuelve atrás.");
        let v = (self.cfg.zona_central - config::ZONA_MIN) / (config::ZONA_MAX - config::ZONA_MIN);
        let texto = format!("{} px", self.cfg.zona_central as i32);
        self.deslizador(p, x + cw - 16.0, cy, Desliz::Zona, v, &texto);
        y = sig + 14.0;
        y = self.cabecera(p, x, y, cw, "Teclado");
        let (cy, _) = self.fila(p, x, y, cw, "Escape", "Con la rueda abierta, Escape la cierra sin abrir nada.");
        p.texto("Siempre activo", rect(x + cw - 216.0, cy - 12.0, 200.0, 24.0), 12.5, Peso::Normal, Alin::Der, tema::TEXTO_3);
    }

    fn apariencia(&mut self, p: &mut Pintor, x: f32, cw: f32, h: f32) {
        let mut y = self.titulo(p, x, cw, "Apariencia", "");
        y = self.cabecera(p, x, y, cw, "Rueda");
        let (cy, sig) = self.fila(p, x, y, cw, "Tamaño", "Escala de los botones orbitales y del centro.");
        let v = (self.cfg.tamano - config::TAMANO_MIN) / (config::TAMANO_MAX - config::TAMANO_MIN);
        let texto = format!("{} %", (self.cfg.tamano * 100.0).round() as i32);
        self.deslizador(p, x + cw - 16.0, cy, Desliz::Tamano, v, &texto);
        y = sig;
        let (cy, sig) = self.fila(p, x, y, cw, "Oscurecer el fondo", "Atenúa un poco la pantalla mientras la rueda está abierta.");
        let o = self.cfg.oscurecer;
        self.interruptor(p, x + cw - 16.0, cy, o, Accion::Oscurecer);
        y = sig;
        let (cy, sig) = self.fila(p, x, y, cw, "Mostrar nombre seleccionado", "Muestra junto a la rueda el nombre del acceso al que apuntas.");
        let mn = self.cfg.mostrar_nombres;
        self.interruptor(p, x + cw - 16.0, cy, mn, Accion::MostrarNombres);
        y = sig;
        let (cy, sig) = self.fila(p, x, y, cw, "Opacidad de los botones", "Nivel de transparencia de los botones orbitales.");
        let v_op = (self.cfg.opacidad_fondo - config::OPACIDAD_MIN) / (config::OPACIDAD_MAX - config::OPACIDAD_MIN);
        let texto_op = format!("{} %", (self.cfg.opacidad_fondo * 100.0).round() as i32);
        self.deslizador(p, x + cw - 16.0, cy, Desliz::OpacidadFondo, v_op, &texto_op);
        y = sig + 14.0;

        // Vista previa en vivo, con los accesos y el tamaño actuales.
        y = self.cabecera(p, x, y, cw, "Vista previa");
        let caja = rect(x, y, cw, (h - y - 24.0).max(120.0));
        p.rellenar(caja, 12.0, rgba(0, 0, 0, 0.25));
        p.bordear(caja, 12.0, tema::BORDE, 1.0);
        let baldosas = render::baldosas_raiz(&self.cfg);
        // Cabe en la caja: si la rueda es más grande que el espacio, se reduce solo en la vista.
        let alcance = crate::wheel::alcance(baldosas.len()) * self.cfg.tamano;
        let lado = (caja.right - caja.left).min(caja.bottom - caja.top);
        let ajuste = (lado / 2.0 / alcance).min(1.0);
        let escena = Escena {
            centro: ((caja.left + caja.right) / 2.0, (caja.top + caja.bottom) / 2.0),
            escala: self.cfg.tamano * ajuste,
            baldosas: &baldosas,
            grupo: None,
            seleccion: (!baldosas.is_empty()).then_some(0),
            progreso: 1.0,
            progreso_grupo: 0.45,
            oscurecer: false,
            radio_central: self.cfg.zona_central * self.cfg.tamano * ajuste,
            mostrar_nombres: self.cfg.mostrar_nombres,
            color_hover: self.cfg.color_hover,
            opacidad_fondo: self.cfg.opacidad_fondo,
        };
        p.recortar(caja);
        render::pintar_rueda(p, &escena);
        p.fin_recorte();
    }

    fn accesos(&mut self, p: &mut Pintor, x: f32, cw: f32, h: f32) {
        let y = self.titulo(p, x, cw, "Accesos", "La rueda muestra tus grupos; cada grupo, sus apps. El orden sigue las agujas del reloj, desde arriba.");
        let sep = 24.0;
        let col = (cw - sep) / 2.0;
        let (xi, xd) = (x, x + col + sep);

        // ── Izquierda: apps instaladas ──
        p.texto("Aplicaciones instaladas", rect(xi, y, col - 40.0, 24.0), 13.5, Peso::Semi, Alin::Izq, tema::TEXTO);
        self.icono_boton(p, xi + col - 14.0, y + 12.0, glifo::RECARGAR, Accion::Recargar, false);
        self.campo_texto(p, rect(xi, y + 34.0, col, 36.0), CampoId::Buscar, "Buscar aplicación…", Some(glifo::BUSCAR));
        let area = rect(xi, y + 80.0, col, h - y - 80.0 - 24.0);
        self.area_apps = area;
        p.rellenar(area, 10.0, rgba(0, 0, 0, 0.18));
        p.bordear(area, 10.0, tema::BORDE, 1.0);
        let interior = rect(area.left + 4.0, area.top + 4.0, area.right - area.left - 8.0, area.bottom - area.top - 8.0);
        let alto_vis = interior.bottom - interior.top;
        self.max_apps = (self.filtradas.len() as f32 * FILA_APP - alto_vis).max(0.0);
        self.scroll_apps = self.scroll_apps.clamp(0.0, self.max_apps);
        p.recortar(interior);
        if self.cargando {
            p.texto("Cargando aplicaciones…", interior, 13.0, Peso::Normal, Alin::Centro, tema::TEXTO_2);
        } else if self.filtradas.is_empty() {
            p.texto("Ninguna aplicación coincide", interior, 13.0, Peso::Normal, Alin::Centro, tema::TEXTO_2);
        }
        let primera = (self.scroll_apps / FILA_APP) as usize;
        let ultima = ((self.scroll_apps + alto_vis) / FILA_APP).ceil() as usize + 1;
        for fila in primera..ultima.min(self.filtradas.len()) {
            let i = self.filtradas[fila];
            let ry = interior.top + fila as f32 * FILA_APP - self.scroll_apps;
            let r = rect(interior.left, ry, interior.right - interior.left, FILA_APP);
            let accion = Accion::AgregarApp(i);
            let sobre = self.hover == Some(accion);
            let ya = self.en_rueda(&self.apps[i].destino);
            if sobre {
                p.rellenar(r, 7.0, tema::CONTROL_HOVER);
            }
            if let Some(ic) = self.apps[i].icono.clone() {
                p.imagen(&ic, rect(r.left + 10.0, ry + 8.0, 24.0, 24.0), 1.0);
            }
            let nombre = self.apps[i].nombre.clone();
            p.texto(&nombre, rect(r.left + 44.0, ry, r.right - r.left - 84.0, FILA_APP), 13.0, Peso::Normal, Alin::Izq, tema::TEXTO);
            if ya {
                p.glifo(glifo::CHECK, r.right - 20.0, ry + FILA_APP / 2.0, 12.0, tema::TEXTO_3);
            } else if sobre {
                p.glifo(glifo::MAS, r.right - 20.0, ry + FILA_APP / 2.0, 12.0, tema::TEXTO);
            }
            // La zona se recorta al área visible de la lista.
            let rz = rect(r.left, ry.max(interior.top), r.right - r.left, (ry + FILA_APP).min(interior.bottom) - ry.max(interior.top));
            self.zona(rz, accion);
        }
        p.fin_recorte();
        self.barra(p, interior, self.scroll_apps, self.max_apps);

        // ── Derecha: grupos, la lista elegida y el editor ──
        let n = self.lista().len();
        let cuenta = match (self.grupo, n) {
            (None, _) => "Sin grupos".to_string(),
            (Some(_), 0) => "En el grupo".to_string(),
            (Some(_), n) if n > RECOMENDADO => format!("En el grupo · {n} (se recomiendan hasta {RECOMENDADO})"),
            (Some(_), n) => format!("En el grupo · {n}"),
        };
        p.texto(&cuenta, rect(xd, y, col, 24.0), 13.5, Peso::Semi, Alin::Izq, tema::TEXTO);
        let mut yd = self.pestanas_grupos(p, xd, y + 32.0, col);
        if self.grupo.is_some() {
            yd = self.barra_grupo(p, xd, yd, col);
        }
        let alto_editor = 278.0;
        let area = rect(xd, yd, col, (h - yd - 24.0 - alto_editor - 14.0).max(60.0));
        self.area_accesos = area;
        p.rellenar(area, 10.0, rgba(0, 0, 0, 0.18));
        p.bordear(area, 10.0, tema::BORDE, 1.0);
        let interior = rect(area.left + 4.0, area.top + 4.0, area.right - area.left - 8.0, area.bottom - area.top - 8.0);
        let alto_vis = interior.bottom - interior.top;
        self.max_accesos = (n as f32 * FILA_ACCESO - alto_vis).max(0.0);
        self.scroll_accesos = self.scroll_accesos.clamp(0.0, self.max_accesos);
        p.recortar(interior);
        if n == 0 {
            let aviso = if self.grupo.is_some() { "Grupo vacío: agrega apps desde la izquierda" } else { "Crea un grupo, o agrega una app y se crea uno" };
            p.texto(aviso, interior, 13.0, Peso::Normal, Alin::Centro, tema::TEXTO_2);
        }
        for i in 0..n {
            let ry = interior.top + i as f32 * FILA_ACCESO - self.scroll_accesos;
            if ry + FILA_ACCESO < interior.top || ry > interior.bottom {
                continue;
            }
            let r = rect(interior.left, ry, interior.right - interior.left, FILA_ACCESO);
            let elegida = self.seleccion == Some(i);
            let sobre = matches!(self.hover, Some(Accion::Elegir(j) | Accion::Subir(j) | Accion::Bajar(j) | Accion::Quitar(j)) if j == i);
            if elegida {
                p.rellenar(r, 7.0, tema::CONTROL_ACTIVO);
            } else if sobre {
                p.rellenar(r, 7.0, tema::CONTROL_HOVER);
            }
            let rz = rect(r.left, ry.max(interior.top), r.right - r.left, (ry + FILA_ACCESO).min(interior.bottom) - ry.max(interior.top));
            self.zona(rz, Accion::Elegir(i));
            let acceso = self.lista()[i].clone();
            p.texto(&format!("{}", i + 1), rect(r.left + 6.0, ry, 18.0, FILA_ACCESO), 11.5, Peso::Normal, Alin::Centro, tema::TEXTO_3);
            match icons::visual(&acceso) {
                Some(v) => p.visual(&v, rect(r.left + 30.0, ry + 9.0, 26.0, 26.0), tema::TEXTO),
                None => {
                    let ri = rect(r.left + 30.0, ry + 9.0, 26.0, 26.0);
                    p.rellenar(ri, 7.0, tema::CONTROL_ACTIVO);
                    let ini: String = acceso.nombre.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
                    p.texto(&ini, ri, 12.0, Peso::Semi, Alin::Centro, tema::TEXTO);
                }
            }
            let ancho_txt = r.right - r.left - 66.0 - if sobre || elegida { 100.0 } else { 0.0 };
            p.texto(&acceso.nombre, rect(r.left + 66.0, ry, ancho_txt, FILA_ACCESO), 13.0, Peso::Medio, Alin::Izq, tema::TEXTO);
            if sobre || elegida {
                let cy = ry + FILA_ACCESO / 2.0;
                self.icono_boton(p, r.right - 86.0, cy, glifo::ARRIBA, Accion::Subir(i), false);
                self.icono_boton(p, r.right - 54.0, cy, glifo::ABAJO, Accion::Bajar(i), false);
                self.icono_boton(p, r.right - 20.0, cy, glifo::BORRAR, Accion::Quitar(i), true);
            }
        }
        p.fin_recorte();
        self.barra(p, interior, self.scroll_accesos, self.max_accesos);

        // Editor del acceso elegido (o de uno nuevo).
        let ed = rect(xd, area.bottom + 14.0, col, alto_editor);
        p.rellenar(ed, 10.0, tema::CONTROL);
        p.bordear(ed, 10.0, tema::BORDE, 1.0);
        let titulo = if self.seleccion.is_some() { "Editar acceso" } else { "Nuevo acceso personalizado" };
        p.texto(titulo, rect(ed.left + 16.0, ed.top + 10.0, col - 32.0, 24.0), 13.0, Peso::Semi, Alin::Izq, tema::TEXTO);
        let et = 86.0;
        let xc = ed.left + 16.0 + et;
        let wc = col - 32.0 - et;
        let mut fy = ed.top + 42.0;
        for (id, etiqueta, marcador) in [
            (CampoId::Nombre, "Nombre", "Cómo se ve en la rueda"),
            (CampoId::Destino, "Destino", "Programa, carpeta, archivo, URL o ms-settings:"),
            (CampoId::Args, "Argumentos", "Opcional"),
        ] {
            p.texto(etiqueta, rect(ed.left + 16.0, fy, et, 34.0), 12.5, Peso::Normal, Alin::Izq, tema::TEXTO_2);
            let ancho = if id == CampoId::Destino { wc - 42.0 } else { wc };
            self.campo_texto(p, rect(xc, fy, ancho, 34.0), id, marcador, None);
            if id == CampoId::Destino {
                self.boton(p, rect(xc + wc - 34.0, fy, 34.0, 34.0), "", Some(glifo::CARPETA), false, Accion::Examinar);
            }
            fy += 42.0;
        }
        // Ícono: vista previa, elegir uno propio o volver al automático.
        p.texto("Ícono", rect(ed.left + 16.0, fy, et, 34.0), 12.5, Peso::Normal, Alin::Izq, tema::TEXTO_2);
        let caja = rect(xc, fy, 34.0, 34.0);
        p.rellenar(caja, 8.0, rgba(0, 0, 0, 0.25));
        p.bordear(caja, 8.0, tema::BORDE, 1.0);
        let destino = self.campos[CampoId::Destino as usize].texto.clone();
        let vista = match &self.icono_editor {
            Some(i) => icons::visual_propio(i),
            None => icons::automatico(&destino),
        };
        if let Some(v) = vista {
            p.visual(&v, rect(xc + 6.0, fy + 6.0, 22.0, 22.0), tema::TEXTO);
        }
        self.boton(p, rect(xc + 44.0, fy, 110.0, 34.0), "Elegir…", None, false, Accion::AbrirSelector);
        // Con un ícono propio, un botón para volver al automático.
        if self.icono_editor.is_some() {
            self.boton(p, rect(xc + 162.0, fy, (wc - 162.0).min(116.0), 34.0), "Automático", None, false, Accion::IconoAuto);
        }
        fy += 42.0;
        let wb = (wc - 10.0) / 2.0;
        self.boton(p, rect(xc, fy + 4.0, wb, 34.0), "Guardar", Some(glifo::CHECK), true, Accion::Guardar);
        self.boton(p, rect(xc + wb + 10.0, fy + 4.0, wb, 34.0), "Nuevo", Some(glifo::MAS), false, Accion::Nuevo);
    }

    /// Pastillas para elegir qué grupo se edita, y "+ Grupo".
    /// Si no caben en una fila, siguen en la de abajo. Devuelve dónde sigue el contenido.
    fn pestanas_grupos(&mut self, p: &mut Pintor, x: f32, y: f32, ancho: f32) -> f32 {
        let alto = 28.0;
        let mut items: Vec<(String, Accion, bool)> = Vec::new();
        for (i, g) in self.cfg.grupos.iter().enumerate() {
            let nombre = if g.nombre.trim().is_empty() { "Sin nombre".to_string() } else { g.nombre.clone() };
            items.push((nombre, Accion::Contexto(Some(i)), self.grupo == Some(i)));
        }
        let (mut cx, mut cy) = (x, y);
        for (texto, accion, sel) in items {
            let w = (p.medir(&texto, 12.5, Peso::Medio) + 26.0).min(ancho);
            if cx > x && cx + w > x + ancho {
                cx = x;
                cy += alto + 6.0;
            }
            let r = rect(cx, cy, w, alto);
            let sobre = self.hover == Some(accion);
            if sel {
                p.rellenar(r, alto / 2.0, tema::CLARO);
            } else {
                p.rellenar(r, alto / 2.0, if sobre { tema::CONTROL_ACTIVO } else { tema::CONTROL_HOVER });
                p.bordear(r, alto / 2.0, tema::BORDE, 1.0);
            }
            let c = if sel { tema::TEXTO_OSCURO } else if sobre { tema::TEXTO } else { tema::TEXTO_2 };
            p.texto(&texto, rect(r.left + 13.0, r.top, w - 26.0, alto), 12.5, Peso::Medio, Alin::Centro, c);
            self.zona(r, accion);
            cx += w + 6.0;
        }
        // "+ Grupo"
        let w = 84.0;
        if cx > x && cx + w > x + ancho {
            cx = x;
            cy += alto + 6.0;
        }
        let r = rect(cx, cy, w, alto);
        let sobre = self.hover == Some(Accion::NuevoGrupo);
        p.rellenar(r, alto / 2.0, if sobre { tema::CONTROL_ACTIVO } else { rgba(0, 0, 0, 0.0) });
        p.bordear(r, alto / 2.0, if sobre { tema::BORDE_FUERTE } else { tema::BORDE }, 1.0);
        let c = if sobre { tema::TEXTO } else { tema::TEXTO_2 };
        p.glifo(glifo::MAS, r.left + 18.0, cy + alto / 2.0, 10.0, c);
        p.texto("Grupo", rect(r.left + 30.0, cy, w - 38.0, alto), 12.5, Peso::Medio, Alin::Izq, c);
        self.zona(r, Accion::NuevoGrupo);
        cy + alto + 12.0
    }

    /// Ícono, nombre, orden y borrar del grupo que se está editando.
    fn barra_grupo(&mut self, p: &mut Pintor, x: f32, y: f32, ancho: f32) -> f32 {
        let Some(g) = self.grupo.filter(|&g| g < self.cfg.grupos.len()) else { return y };
        let alto = 34.0;
        // Ícono: clic para elegir otro.
        let caja = rect(x, y, alto, alto);
        let sobre = self.hover == Some(Accion::IconoGrupo);
        p.rellenar(caja, 8.0, if sobre { tema::CONTROL_ACTIVO } else { rgba(0, 0, 0, 0.25) });
        p.bordear(caja, 8.0, if sobre { tema::BORDE_FUERTE } else { tema::BORDE }, 1.0);
        let visual = self.cfg.grupos[g].icono.as_deref().and_then(icons::visual_propio);
        match visual {
            Some(v) => p.visual(&v, rect(x + 7.0, y + 7.0, 20.0, 20.0), tema::TEXTO),
            None => {
                if let Some(n) = lucide::nombre("folder") {
                    p.lucide(n, rect(x + 8.0, y + 8.0, 18.0, 18.0), tema::TEXTO_2);
                }
            }
        }
        self.zona(caja, Accion::IconoGrupo);

        let confirmar = self.confirmar_quitar;
        let ancho_botones = if confirmar { 64.0 + 112.0 } else { 100.0 };
        self.campo_texto(p, rect(x + alto + 8.0, y, ancho - alto - 8.0 - ancho_botones, alto), CampoId::NombreGrupo, "Nombre del grupo", None);
        let cy = y + alto / 2.0;
        let derecha = x + ancho;
        if confirmar {
            self.icono_boton(p, derecha - 112.0 - 46.0, cy, glifo::IZQUIERDA, Accion::GrupoIzquierda, false);
            self.icono_boton(p, derecha - 112.0 - 14.0, cy, glifo::DERECHA, Accion::GrupoDerecha, false);
            let r = rect(derecha - 104.0, y + 2.0, 104.0, alto - 4.0);
            let sobre = self.hover == Some(Accion::QuitarGrupo);
            p.rellenar(r, 8.0, if sobre { tema::PELIGRO } else { rgba(0, 0, 0, 0.25) });
            p.bordear(r, 8.0, tema::PELIGRO, 1.0);
            p.texto("¿Quitar grupo?", r, 12.0, Peso::Semi, Alin::Centro, if sobre { tema::TEXTO_OSCURO } else { tema::PELIGRO });
            self.zona(r, Accion::QuitarGrupo);
        } else {
            self.icono_boton(p, derecha - 82.0, cy, glifo::IZQUIERDA, Accion::GrupoIzquierda, false);
            self.icono_boton(p, derecha - 50.0, cy, glifo::DERECHA, Accion::GrupoDerecha, false);
            self.icono_boton(p, derecha - 16.0, cy, glifo::BORRAR, Accion::QuitarGrupo, true);
        }
        y + alto + 12.0
    }

    /// Panel para elegir un ícono: los de Lucide o los que trae un archivo.
    fn selector_ui(&mut self, p: &mut Pintor, w: f32, h: f32) {
        let Some(modo) = self.selector else { return };
        // Velo sobre todo: un clic fuera del panel lo cierra.
        p.rellenar(rect(0.0, 0.0, w, h), 0.0, rgba(0, 0, 0, 0.55));
        self.zona(rect(0.0, 0.0, w, h), Accion::CerrarSelector);
        let ancho = (w - ANCHO_LATERAL - 80.0).min(760.0);
        let panel = rect(ANCHO_LATERAL + (w - ANCHO_LATERAL - ancho) / 2.0, 40.0, ancho, h - 80.0);
        p.sombra(panel, 14.0, 2.0);
        p.rellenar(panel, 14.0, rgba(30, 30, 34, 0.98));
        p.bordear(panel, 14.0, tema::BORDE_FUERTE, 1.0);
        self.zona(panel, Accion::Bloqueo);

        let (x0, x1) = (panel.left + 20.0, panel.right - 20.0);
        p.texto("Elegir ícono", rect(x0, panel.top + 16.0, 200.0, 30.0), 17.0, Peso::Semi, Alin::Izq, tema::TEXTO);
        self.icono_boton(p, x1 - 14.0, panel.top + 31.0, glifo::CERRAR, Accion::CerrarSelector, false);
        self.segmentado(p, x1 - 40.0, panel.top + 31.0, &[
            ("Lucide", Accion::ModoSelector(ModoSel::Lucide), modo == ModoSel::Lucide),
            ("Archivo", Accion::ModoSelector(ModoSel::Archivo), modo == ModoSel::Archivo),
        ]);

        let actual = self.icono_objetivo();
        if actual.is_some() {
            self.boton(p, rect(x0 + 150.0, panel.top + 15.0, 110.0, 32.0), "Automático", None, false, Accion::IconoAuto);
        }
        let y = panel.top + 62.0;
        let total = match modo {
            ModoSel::Lucide => {
                self.campo_texto(p, rect(x0, y, x1 - x0, 36.0), CampoId::BuscarIcono, "Buscar ícono (en inglés: home, mail, music…)", Some(glifo::BUSCAR));
                self.lucide_filtrados.len()
            }
            ModoSel::Archivo => {
                self.campo_texto(p, rect(x0, y, x1 - x0 - 44.0, 36.0), CampoId::Archivo, "Ruta de un .dll, .exe o .ico", None);
                self.boton(p, rect(x1 - 36.0, y, 36.0, 36.0), "", Some(glifo::CARPETA), false, Accion::ExaminarIconos);
                self.archivo_cantidad as usize
            }
        };

        let area = rect(x0, y + 48.0, x1 - x0, panel.bottom - 40.0 - (y + 48.0));
        self.area_sel = area;
        p.rellenar(area, 10.0, rgba(0, 0, 0, 0.22));
        let interior = rect(area.left + 6.0, area.top + 6.0, area.right - area.left - 12.0, area.bottom - area.top - 12.0);
        let cols = ((interior.right - interior.left) / CELDA).floor().max(1.0) as usize;
        let filas = total.div_ceil(cols);
        let alto_vis = interior.bottom - interior.top;
        self.max_sel = (filas as f32 * CELDA - alto_vis).max(0.0);
        self.scroll_sel = self.scroll_sel.clamp(0.0, self.max_sel);
        let margen = ((interior.right - interior.left) - cols as f32 * CELDA) / 2.0;

        let mut nombre_hover = None;
        p.recortar(interior);
        if total == 0 {
            let aviso = match modo {
                ModoSel::Lucide => "Ningún ícono coincide",
                ModoSel::Archivo => "Este archivo no tiene íconos (o no existe)",
            };
            p.texto(aviso, interior, 13.0, Peso::Normal, Alin::Centro, tema::TEXTO_2);
        }
        let ruta = self.campos[CampoId::Archivo as usize].texto.trim().to_string();
        let primera = (self.scroll_sel / CELDA) as usize;
        let ultima = ((self.scroll_sel + alto_vis) / CELDA).ceil() as usize + 1;
        for fila in primera..ultima.min(filas) {
            for col in 0..cols {
                let k = fila * cols + col;
                if k >= total {
                    break;
                }
                let cx = interior.left + margen + col as f32 * CELDA;
                let cy = interior.top + fila as f32 * CELDA - self.scroll_sel;
                let celda = rect(cx + 3.0, cy + 3.0, CELDA - 6.0, CELDA - 6.0);
                let (accion, elegido) = match modo {
                    ModoSel::Lucide => {
                        let i = self.lucide_filtrados[k];
                        let n = lucide::nombre_en(i);
                        (Accion::ElegirLucide(i), actual.as_deref() == Some(format!("lucide:{n}").as_str()))
                    }
                    ModoSel::Archivo => {
                        (Accion::ElegirArchivo(k as i32), actual.as_deref() == Some(format!("archivo:{ruta},{k}").as_str()))
                    }
                };
                let sobre = self.hover == Some(accion);
                if elegido {
                    p.rellenar(celda, 8.0, tema::CONTROL_ACTIVO);
                    p.bordear(celda, 8.0, rgba(255, 255, 255, 0.45), 1.0);
                } else if sobre {
                    p.rellenar(celda, 8.0, tema::CONTROL_HOVER);
                }
                let (mx, my) = (cx + CELDA / 2.0, cy + CELDA / 2.0);
                match modo {
                    ModoSel::Lucide => {
                        let n = lucide::nombre_en(self.lucide_filtrados[k]);
                        let c = if sobre || elegido { tema::TEXTO } else { tema::TEXTO_2 };
                        p.lucide(n, rect(mx - 12.0, my - 12.0, 24.0, 24.0), c);
                        if sobre {
                            nombre_hover = Some(n.to_string());
                        }
                    }
                    ModoSel::Archivo => {
                        let ic = self
                            .iconos_archivo
                            .entry((ruta.clone(), k as i32))
                            .or_insert_with(|| {
                                icons::de_archivo(&ruta, k as i32, 64)
                                    .map(|(an, al, px)| Arc::new(Icono::nuevo(format!("sel:{ruta},{k}"), an, al, px)))
                            })
                            .clone();
                        if let Some(ic) = ic {
                            p.imagen(&ic, rect(mx - 16.0, my - 16.0, 32.0, 32.0), 1.0);
                        }
                        if sobre {
                            nombre_hover = Some(format!("Ícono {k}"));
                        }
                    }
                }
                let arriba = celda.top.max(interior.top);
                let abajo = celda.bottom.min(interior.bottom);
                if abajo > arriba {
                    self.zona(rect(celda.left, arriba, CELDA - 6.0, abajo - arriba), accion);
                }
            }
        }
        p.fin_recorte();
        self.barra(p, interior, self.scroll_sel, self.max_sel);

        let pie = match (nombre_hover, modo) {
            (Some(n), _) => n,
            (None, ModoSel::Lucide) => format!("{total} íconos de Lucide (lucide.dev, licencia ISC)"),
            (None, ModoSel::Archivo) => format!("{total} íconos en el archivo"),
        };
        p.texto(&pie, rect(x0, panel.bottom - 34.0, x1 - x0, 24.0), 12.0, Peso::Normal, Alin::Izq, tema::TEXTO_2);
    }

    fn acerca(&mut self, p: &mut Pintor, x: f32, cw: f32) {
        let mut y = self.titulo(p, x, cw, "Acerca de", "");
        let (cy, sig) = self.fila(p, x, y, cw, "Versión", "Lanzador radial para Windows, en Rust nativo.");
        p.texto(env!("CARGO_PKG_VERSION"), rect(x + cw - 216.0, cy - 12.0, 200.0, 24.0), 13.0, Peso::Semi, Alin::Der, tema::TEXTO);
        y = sig;
        y += 14.0;
        y = self.cabecera(p, x, y, cw, "Archivos");
        let ruta = config::ruta().display().to_string();
        let (cy, sig) = self.fila(p, x, y, cw, "config.json", &ruta);
        self.boton(p, rect(x + cw - 16.0 - 220.0, cy - 16.0, 100.0, 32.0), "Abrir", None, false, Accion::AbrirConfig);
        self.boton(p, rect(x + cw - 16.0 - 110.0, cy - 16.0, 110.0, 32.0), "Carpeta", Some(glifo::CARPETA), false, Accion::AbrirCarpeta);
        y = sig;
        let (cy, sig) = self.fila(p, x, y, cw, "Salir de Orbyt", "Quita el ícono de la bandeja y deja de escuchar el botón.");
        self.boton(p, rect(x + cw - 16.0 - 110.0, cy - 16.0, 110.0, 32.0), "Salir", Some(glifo::CERRAR), false, Accion::Salir);
        y = sig + 14.0;
        y = self.cabecera(p, x, y, cw, "Cómo se usa");
        let texto = match self.cfg.modo {
            Modo::Soltar => "Mantén pulsado el botón configurado, apunta a un grupo hasta que se abra, apunta a una app y suelta. \
                             En el centro se vuelve atrás o se cancela; Escape también cancela. Un clic corto funciona como siempre.",
            Modo::Clic => "Mantén pulsado el botón configurado hasta que salga la rueda y suéltalo. Con clic izquierdo abres un \
                           grupo y luego una app; el clic derecho o el centro vuelven atrás o cierran. Escape también cierra.",
        };
        p.parrafo(texto, rect(x, y, cw.min(640.0), 80.0), 13.0, tema::TEXTO_2);
    }

    /// Barra de desplazamiento fina, solo si hace falta.
    fn barra(&self, p: &mut Pintor, area: Rect, scroll: f32, max: f32) {
        if max <= 0.0 {
            return;
        }
        let alto = area.bottom - area.top;
        let total = alto + max;
        let largo = (alto * alto / total).max(28.0);
        let y = area.top + (alto - largo) * (scroll / max);
        p.rellenar(rect(area.right - 5.0, y, 3.0, largo), 1.5, rgba(255, 255, 255, 0.22));
    }
}

// ─── Teclado y portapapeles ─────────────────────────────────────────────────────────────────

fn portapapeles() -> Option<String> {
    unsafe {
        OpenClipboard(None).ok()?;
        let texto = (|| {
            let h = GetClipboardData(13 /* CF_UNICODETEXT */).ok()?;
            let global = windows::Win32::Foundation::HGLOBAL(h.0);
            let p = GlobalLock(global) as *const u16;
            if p.is_null() {
                return None;
            }
            let mut n = 0;
            while *p.add(n) != 0 {
                n += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
            let _ = GlobalUnlock(global);
            Some(s)
        })();
        let _ = CloseClipboard();
        texto
    }
}

impl Ajustes {
    fn caracter(&mut self, c: u16) -> Option<Diferida> {
        if c == 0x1B && self.selector.is_some() {
            self.selector = None;
            self.foco = None;
            return None;
        }
        let Some(id) = self.foco else {
            if c == 0x1B {
                unsafe {
                    let _ = DestroyWindow(self.hwnd);
                }
            }
            return None;
        };
        self.cursor_visible = true;
        match c {
            0x08 => self.campo(id).borrar_atras(),
            0x01 => self.campo(id).todo = true,
            0x16 => {
                if let Some(t) = portapapeles() {
                    self.campo(id).insertar(&t);
                }
            }
            0x0D => match id {
                CampoId::Buscar => {
                    if let Some(&i) = self.filtradas.first() {
                        self.agregar_app(i);
                    }
                }
                CampoId::BuscarIcono => {
                    if let Some(&i) = self.lucide_filtrados.first() {
                        self.poner_icono(Some(format!("lucide:{}", lucide::nombre_en(i))));
                    }
                }
                CampoId::Archivo => self.cargar_archivo(),
                CampoId::NombreGrupo => self.foco = None,
                _ => self.guardar_formulario(),
            },
            0x1B => self.foco = None,
            0x09 if self.selector.is_some() => {}
            0x09 => {
                let orden = [CampoId::Buscar, CampoId::Nombre, CampoId::Destino, CampoId::Args];
                let i = orden.iter().position(|x| *x == id).unwrap_or(0);
                self.foco = Some(orden[(i + 1) % orden.len()]);
            }
            0xD800..=0xDBFF => self.sustituto = Some(c),
            0xDC00..=0xDFFF => {
                if let Some(alto) = self.sustituto.take() {
                    let s = String::from_utf16_lossy(&[alto, c]);
                    self.campo(id).insertar(&s);
                }
            }
            c if c >= 0x20 && c != 0x7F => {
                let s = String::from_utf16_lossy(&[c]);
                self.campo(id).insertar(&s);
            }
            _ => {}
        }
        if c != 0x0D && c != 0x09 && c != 0x1B {
            self.al_cambiar(id);
        }
        None
    }

    /// Lo que depende del texto de un campo (filtros de búsqueda, archivo de íconos).
    fn al_cambiar(&mut self, id: CampoId) {
        match id {
            CampoId::Buscar => self.filtrar(),
            CampoId::BuscarIcono => {
                self.lucide_filtrados = lucide::buscar(&self.campos[id as usize].texto);
                self.scroll_sel = 0.0;
            }
            CampoId::Archivo => self.cargar_archivo(),
            CampoId::NombreGrupo => {
                // El nombre del grupo se guarda mientras se escribe.
                let nombre = self.campos[id as usize].texto.trim().to_string();
                if let Some(g) = self.grupo.filter(|&g| g < self.cfg.grupos.len()) {
                    self.cfg.grupos[g].nombre = nombre;
                    self.guardar();
                }
            }
            _ => {}
        }
    }

    fn tecla(&mut self, vk: u16) -> bool {
        let Some(id) = self.foco else { return false };
        self.cursor_visible = true;
        let campo = self.campo(id);
        match VIRTUAL_KEY(vk) {
            VK_LEFT => campo.mover(-1),
            VK_RIGHT => campo.mover(1),
            VK_HOME => campo.ir(0),
            VK_END => campo.ir(usize::MAX),
            VK_DELETE => {
                campo.borrar_delante();
                self.al_cambiar(id);
            }
            _ => return false,
        }
        true
    }
}

fn examinar(dueño: HWND, filtro: PCWSTR, titulo: PCWSTR) -> Option<String> {
    let mut buf = vec![0u16; 1024];
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: dueño,
        lpstrFilter: filtro,
        lpstrFile: PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        lpstrTitle: titulo,
        // Sin seguir los .lnk: el acceso directo conserva sus argumentos y su ícono.
        Flags: OFN_FILEMUSTEXIST | OFN_NOCHANGEDIR | OFN_NODEREFERENCELINKS | OFN_EXPLORER,
        ..Default::default()
    };
    unsafe {
        if !GetOpenFileNameW(&mut ofn).as_bool() {
            return None;
        }
    }
    let fin = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..fin]))
}

fn hacer(d: Diferida, hwnd: HWND) {
    match d {
        Diferida::ExaminarIconos => {
            let filtro = w!("Archivos con íconos\0*.dll;*.exe;*.ico;*.cpl;*.mun\0Todos los archivos\0*.*\0\0");
            if let Some(ruta) = examinar(hwnd, filtro, w!("Elegir un archivo con íconos")) {
                con_ajustes(|a| {
                    a.campo(CampoId::Archivo).poner(&ruta);
                    a.cargar_archivo();
                    a.invalidar();
                });
            }
        }
        Diferida::Examinar => {
            let filtro = w!("Programas y accesos directos\0*.exe;*.lnk;*.bat;*.cmd;*.ps1\0Todos los archivos\0*.*\0\0");
            if let Some(ruta) = examinar(hwnd, filtro, w!("Elegir un archivo para la rueda")) {
                con_ajustes(|a| {
                    if a.campos[CampoId::Nombre as usize].texto.trim().is_empty() {
                        let nombre = std::path::Path::new(&ruta).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                        a.campo(CampoId::Nombre).poner(&nombre);
                    }
                    a.campo(CampoId::Destino).poner(&ruta);
                    a.invalidar();
                });
            }
        }
        Diferida::Abrir(acceso) => launch::abrir(&acceso),
    }
}

// ─── Procedimiento de ventana ───────────────────────────────────────────────────────────────

fn punto(lparam: LPARAM) -> (i32, i32) {
    ((lparam.0 & 0xFFFF) as i16 as i32, ((lparam.0 >> 16) & 0xFFFF) as i16 as i32)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => return LRESULT(1),
        WM_GETMINMAXINFO => unsafe {
            let s = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
            let info = &mut *(lparam.0 as *mut MINMAXINFO);
            info.ptMinTrackSize = POINT { x: (860.0 * s) as i32, y: (600.0 * s) as i32 };
            return LRESULT(0);
        },
        WM_APPS_LISTA => {
            let lista = unsafe { Box::from_raw(lparam.0 as *mut Vec<AppInstalada>) };
            con_ajustes(|a| {
                a.apps = *lista;
                a.cargando = false;
                a.filtrar();
                a.invalidar();
            });
            return LRESULT(0);
        }
        WM_APPS_ICONOS => {
            let tanda = unsafe { Box::from_raw(lparam.0 as *mut Vec<(usize, Option<icons::IconoRef>)>) };
            con_ajustes(|a| {
                for (i, ic) in *tanda {
                    if let Some(app) = a.apps.get_mut(i) {
                        app.icono = ic;
                    }
                }
                a.invalidar();
            });
            return LRESULT(0);
        }
        WM_DESTROY => {
            VENTANA.store(0, Ordering::Relaxed);
            AJUSTES.with(|s| s.borrow_mut().take());
            return LRESULT(0);
        }
        _ => {}
    }

    let mut diferida = None;
    let manejado = con_ajustes(|a| unsafe {
        match msg {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                BeginPaint(hwnd, &mut ps);
                a.pintar();
                let _ = EndPaint(hwnd, &ps);
            }
            WM_SIZE => {
                if let Some(rt) = &a.rt {
                    let (w, h) = ((lparam.0 & 0xFFFF) as u32, ((lparam.0 >> 16) & 0xFFFF) as u32);
                    let _ = rt.Resize(&D2D_SIZE_U { width: w.max(1), height: h.max(1) });
                }
                a.invalidar();
            }
            WM_DPICHANGED => {
                a.escala = ((wparam.0 & 0xFFFF) as f32 / 96.0).max(1.0);
                a.rt = None;
                a.pintor = None;
                let r = &*(lparam.0 as *const RECT);
                let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                a.invalidar();
            }
            WM_TIMER if wparam.0 == TIMER_CURSOR => {
                if a.foco.is_some() {
                    a.cursor_visible = !a.cursor_visible;
                    a.invalidar();
                }
            }
            WM_MOUSEMOVE => {
                let (x, y) = punto(lparam);
                a.raton = (x as f32 / a.escala, y as f32 / a.escala);
                if let Some(d) = a.arrastre {
                    a.poner_deslizador(d, a.raton.0);
                    a.invalidar();
                }
                let nuevo = a.zona_en(a.raton.0, a.raton.1);
                if nuevo != a.hover {
                    a.hover = nuevo;
                    a.invalidar();
                }
                let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                let _ = TrackMouseEvent(&mut tme);
            }
            WM_MOUSELEAVE => {
                a.hover = None;
                a.invalidar();
            }
            WM_SETCURSOR if (lparam.0 & 0xFFFF) as u32 == HTCLIENT => {
                let cursor = match a.hover {
                    Some(Accion::Campo(_)) => IDC_IBEAM,
                    Some(_) => IDC_HAND,
                    None => IDC_ARROW,
                };
                if let Ok(c) = LoadCursorW(None, cursor) {
                    SetCursor(Some(c));
                }
            }
            WM_LBUTTONDOWN => {
                let (x, y) = punto(lparam);
                a.raton = (x as f32 / a.escala, y as f32 / a.escala);
                SetCapture(hwnd);
                let accion = a.zona_en(a.raton.0, a.raton.1);
                a.pulsada = accion;
                match accion {
                    Some(Accion::Deslizador(d)) => {
                        a.arrastre = Some(d);
                        a.poner_deslizador(d, a.raton.0);
                    }
                    Some(Accion::Campo(id)) => {
                        a.foco = Some(id);
                        a.cursor_visible = true;
                        a.cursor_en(id, a.raton.0);
                    }
                    _ => a.foco = None,
                }
                a.invalidar();
            }
            WM_LBUTTONUP => {
                let _ = ReleaseCapture();
                if a.arrastre.take().is_some() {
                    a.guardar();
                } else {
                    let accion = a.zona_en(a.raton.0, a.raton.1);
                    if let Some(acc) = accion.filter(|x| Some(*x) == a.pulsada) {
                        if !matches!(acc, Accion::Campo(_)) {
                            diferida = a.ejecutar(acc);
                        }
                    }
                }
                a.pulsada = None;
                a.hover = a.zona_en(a.raton.0, a.raton.1);
                a.invalidar();
            }
            WM_MOUSEWHEEL => {
                let delta = ((wparam.0 >> 16) & 0xFFFF) as i16 as f32 / 120.0;
                let mut pt = POINT { x: punto(lparam).0, y: punto(lparam).1 };
                let _ = ScreenToClient(hwnd, &mut pt);
                let (x, y) = (pt.x as f32 / a.escala, pt.y as f32 / a.escala);
                if a.selector.is_some() {
                    if dentro(&a.area_sel, x, y) {
                        a.scroll_sel = (a.scroll_sel - delta * CELDA * 2.0).clamp(0.0, a.max_sel);
                    }
                } else if dentro(&a.area_apps, x, y) && a.seccion == Seccion::Accesos {
                    a.scroll_apps = (a.scroll_apps - delta * FILA_APP * 3.0).clamp(0.0, a.max_apps);
                } else if dentro(&a.area_accesos, x, y) && a.seccion == Seccion::Accesos {
                    a.scroll_accesos = (a.scroll_accesos - delta * FILA_ACCESO * 2.0).clamp(0.0, a.max_accesos);
                }
                a.hover = a.zona_en(a.raton.0, a.raton.1);
                a.invalidar();
            }
            WM_CHAR => {
                diferida = a.caracter(wparam.0 as u16);
                a.invalidar();
            }
            WM_KEYDOWN => {
                if !a.tecla(wparam.0 as u16) {
                    return false;
                }
                a.invalidar();
            }
            _ => return false,
        }
        true
    });
    if let Some(d) = diferida.take() {
        hacer(d, hwnd);
    }
    if manejado == Some(true) {
        // WM_SETCURSOR tiene que decir que ya puso el cursor.
        return LRESULT(if msg == WM_SETCURSOR { 1 } else { 0 });
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn campo_inserta_y_borra_con_acentos() {
        let mut c = Campo::default();
        c.insertar("canción");
        assert_eq!(c.cursor, 7);
        c.mover(-1);
        c.borrar_atras();
        assert_eq!(c.texto, "cancin");
        assert_eq!(c.cursor, 5);
    }

    #[test]
    fn campo_cursor_y_seleccion() {
        let mut c = Campo::default();
        c.poner("hola");
        c.ir(0);
        c.insertar("¡");
        assert_eq!(c.texto, "¡hola");
        assert_eq!(c.cursor_utf16(), 1);
        c.todo = true;
        c.insertar("chau");
        assert_eq!(c.texto, "chau");
        c.ir(usize::MAX);
        c.borrar_delante();
        assert_eq!(c.texto, "chau");
        c.borrar_atras();
        assert_eq!(c.texto, "cha");
    }

    #[test]
    fn campo_pegar_quita_saltos_de_linea() {
        let mut c = Campo::default();
        c.insertar("a\r\nb");
        assert_eq!(c.texto, "a  b");
    }

    #[test]
    fn cursor_utf16_cuenta_pares_sustitutos() {
        let mut c = Campo::default();
        // U+10348 (letra gótica): fuera del plano básico, ocupa dos unidades UTF-16.
        c.poner("a\u{10348}b");
        assert_eq!(c.cursor_utf16(), 4);
    }
}
