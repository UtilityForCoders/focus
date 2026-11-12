//! Ventana de configuración: elegir qué apps van en la rueda sin tocar `config.json` a mano.
//!
//! A la izquierda, todas las apps instaladas (las del menú Inicio, también las de la Store) con su
//! ícono y un buscador. A la derecha, los accesos de la rueda en orden, con un formulario para
//! renombrarlos o crear accesos propios (carpetas, URLs, comandos). Cada cambio se guarda al
//! momento y la rueda lo usa en la siguiente pulsación.
//!
//! Son controles nativos de Windows (ListView, Edit, Button) con el estilo moderno que activa el
//! manifiesto. Los íconos salen de la lista de imágenes del sistema, que ya está cargada.
//!
//! Sobre el estado: los controles notifican de forma síncrona (`WM_NOTIFY`, `EN_CHANGE`) mientras
//! el propio código les envía mensajes. El estado vive en un `RefCell` y el procedimiento de
//! ventana usa `try_borrow_mut`: si ya está prestado, la notificación la provocó el propio código
//! (llenar una lista, escribir en un campo) y se ignora, que es justo lo que se quiere.

use crate::apps::{self, AppInstalada};
use crate::config::{self, AbrirEn, Acceso, Config};
use crate::icons;
use std::cell::RefCell;
use std::sync::atomic::{AtomicIsize, Ordering};
use windows::core::{w, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemServices::{SS_CENTERIMAGE, SS_LEFT};
use windows::Win32::UI::Controls::Dialogs::*;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetFocus, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::*;

/// Avisa a la ventana principal de que la configuración cambió (para precargar íconos).
pub const WM_FOCUS_CONFIG: u32 = WM_APP + 20;
const WM_APPS_LISTA: u32 = WM_APP + 30;
const WM_APPS_ICONOS: u32 = WM_APP + 31;

const ID_BUSCAR: i32 = 101;
const ID_APPS: i32 = 102;
const ID_AGREGAR: i32 = 103;
const ID_RECARGAR: i32 = 104;
const ID_ACCESOS: i32 = 201;
const ID_SUBIR: i32 = 202;
const ID_BAJAR: i32 = 203;
const ID_QUITAR: i32 = 204;
const ID_NOMBRE: i32 = 210;
const ID_DESTINO: i32 = 211;
const ID_EXAMINAR: i32 = 212;
const ID_ARGS: i32 = 213;
const ID_APLICAR: i32 = 214;
const ID_NUEVO: i32 = 215;
const ID_ABRIR_EN: i32 = 220;
const ID_RETARDO: i32 = 221;

/// Más accesos que estos y los textos de la rueda empiezan a no caber.
const RECOMENDADO: usize = 10;

static VENTANA: AtomicIsize = AtomicIsize::new(0);

/// La ventana de configuración, si está abierta (para `IsDialogMessageW` en el bucle principal).
pub fn ventana() -> Option<HWND> {
    let v = VENTANA.load(Ordering::Relaxed);
    (v != 0).then_some(HWND(v as *mut _))
}

struct Controles {
    titulo_apps: HWND,
    titulo_accesos: HWND,
    buscar: HWND,
    recargar: HWND,
    apps: HWND,
    agregar: HWND,
    accesos: HWND,
    subir: HWND,
    bajar: HWND,
    quitar: HWND,
    contador: HWND,
    et_nombre: HWND,
    nombre: HWND,
    et_destino: HWND,
    destino: HWND,
    examinar: HWND,
    et_args: HWND,
    args: HWND,
    aplicar: HWND,
    nuevo: HWND,
    et_abrir_en: HWND,
    abrir_en: HWND,
    et_retardo: HWND,
    retardo: HWND,
}

struct Ajustes {
    hwnd: HWND,
    principal: HWND,
    cfg: Config,
    apps: Vec<AppInstalada>,
    cargando: bool,
    /// Fila de la lista de apps → posición en `apps`. Siempre en orden creciente.
    filtradas: Vec<usize>,
    c: Controles,
    fuente: HFONT,
    fuente_titulo: HFONT,
    icono_generico: i32,
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

fn escala(hwnd: HWND) -> f32 {
    unsafe { GetDpiForWindow(hwnd).max(96) as f32 / 96.0 }
}

// ─── Utilidades de controles ────────────────────────────────────────────────────────────────

fn enviar(h: HWND, msg: u32, wparam: usize, lparam: isize) -> isize {
    unsafe { SendMessageW(h, msg, Some(WPARAM(wparam)), Some(LPARAM(lparam))).0 }
}

fn leer(h: HWND) -> String {
    unsafe {
        let n = GetWindowTextLengthW(h);
        let mut buf = vec![0u16; n as usize + 1];
        let leidos = GetWindowTextW(h, &mut buf);
        String::from_utf16_lossy(&buf[..leidos as usize])
    }
}

fn escribir(h: HWND, texto: &str) {
    unsafe {
        let _ = SetWindowTextW(h, &HSTRING::from(texto));
    }
}

fn lv_vaciar(lv: HWND) {
    enviar(lv, LVM_DELETEALLITEMS, 0, 0);
}

fn lv_insertar(lv: HWND, fila: i32, texto: &str, imagen: i32) {
    let mut buf: Vec<u16> = texto.encode_utf16().chain(std::iter::once(0)).collect();
    let item = LVITEMW {
        mask: LVIF_TEXT | LVIF_IMAGE,
        iItem: fila,
        pszText: PWSTR(buf.as_mut_ptr()),
        iImage: imagen,
        ..Default::default()
    };
    enviar(lv, LVM_INSERTITEMW, 0, &item as *const _ as isize);
}

fn lv_imagen(lv: HWND, fila: i32, imagen: i32) {
    let item = LVITEMW { mask: LVIF_IMAGE, iItem: fila, iImage: imagen, ..Default::default() };
    enviar(lv, LVM_SETITEMW, 0, &item as *const _ as isize);
}

fn lv_seleccionadas(lv: HWND) -> Vec<usize> {
    let mut filas = Vec::new();
    let mut i = -1isize;
    loop {
        i = enviar(lv, LVM_GETNEXTITEM, i as usize, LVNI_SELECTED as isize);
        if i < 0 {
            break;
        }
        filas.push(i as usize);
    }
    filas
}

fn lv_seleccionar(lv: HWND, fila: Option<usize>) {
    let ninguna = LVITEMW { stateMask: LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0), ..Default::default() };
    enviar(lv, LVM_SETITEMSTATE, usize::MAX, &ninguna as *const _ as isize);
    if let Some(f) = fila {
        let item = LVITEMW {
            stateMask: LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0),
            state: LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0),
            ..Default::default()
        };
        enviar(lv, LVM_SETITEMSTATE, f, &item as *const _ as isize);
        enviar(lv, LVM_ENSUREVISIBLE, f, 0);
    }
}

/// La única columna ocupa el ancho útil de la lista. Se repite después de llenarla, porque la
/// barra vertical aparece (y le quita ancho) solo cuando hay más filas de las que caben; si no,
/// sale una barra horizontal que no hace falta.
fn ajustar_columna(lv: HWND) {
    let mut cr = RECT::default();
    unsafe {
        let _ = GetClientRect(lv, &mut cr);
    }
    enviar(lv, LVM_SETCOLUMNWIDTH, 0, cr.right as isize);
}

fn redibujo(h: HWND, activo: bool) {
    enviar(h, WM_SETREDRAW, activo as usize, 0);
    if activo {
        unsafe {
            let _ = InvalidateRect(Some(h), None, true);
        }
    }
}

// ─── Apertura ───────────────────────────────────────────────────────────────────────────────

/// Abre la ventana, o la trae al frente si ya estaba abierta.
pub fn abrir(principal: HWND) {
    if let Some(h) = ventana() {
        unsafe {
            let _ = ShowWindow(h, SW_RESTORE);
            let _ = SetForegroundWindow(h);
        }
        return;
    }
    if let Err(e) = crear(principal) {
        eprintln!("no se pudo abrir la configuración: {e}");
    }
}

fn crear(principal: HWND) -> windows::core::Result<()> {
    unsafe {
        let instancia: HINSTANCE = GetModuleHandleW(None)?.into();
        let clase = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instancia,
            lpszClassName: w!("FocusAjustes"),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hIcon: LoadIconW(None, IDI_APPLICATION)?,
            hbrBackground: GetSysColorBrush(COLOR_WINDOW),
            ..Default::default()
        };
        // Falla si ya estaba registrada (segunda apertura), y no pasa nada.
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
        let (an, al) = (((980.0 * s) as i32).min(t.right - t.left), ((680.0 * s) as i32).min(t.bottom - t.top));
        let (x, y) = (t.left + (t.right - t.left - an) / 2, t.top + (t.bottom - t.top - al) / 2);

        let hwnd = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            w!("FocusAjustes"),
            w!("Focus · Accesos de la rueda"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            x,
            y,
            an,
            al,
            None,
            None,
            Some(instancia),
            None,
        )?;

        let control = |clase: PCWSTR, texto: &str, estilo: u32, ex: WINDOW_EX_STYLE, id: i32| -> HWND {
            CreateWindowExW(
                ex,
                clase,
                &HSTRING::from(texto),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(estilo),
                0,
                0,
                0,
                0,
                Some(hwnd),
                Some(HMENU(id as isize as *mut _)),
                Some(instancia),
                None,
            )
            .unwrap_or_default()
        };
        let etiqueta = |texto: &str| control(w!("STATIC"), texto, SS_LEFT.0 | SS_CENTERIMAGE.0, WINDOW_EX_STYLE(0), -1);
        let boton = |texto: &str, id: i32| control(w!("BUTTON"), texto, WS_TABSTOP.0 | BS_PUSHBUTTON as u32, WINDOW_EX_STYLE(0), id);
        let campo = |id: i32, extra: u32| {
            control(w!("EDIT"), "", WS_TABSTOP.0 | ES_AUTOHSCROLL as u32 | extra, WS_EX_CLIENTEDGE, id)
        };
        let lista = |id: i32, extra: u32| {
            let lv = control(
                WC_LISTVIEWW,
                "",
                WS_TABSTOP.0 | LVS_REPORT | LVS_NOCOLUMNHEADER | LVS_SHOWSELALWAYS | LVS_SHAREIMAGELISTS | extra,
                WS_EX_CLIENTEDGE,
                id,
            );
            enviar(lv, LVM_SETEXTENDEDLISTVIEWSTYLE, 0, (LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER) as isize);
            let _ = SetWindowTheme(lv, w!("Explorer"), None);
            enviar(lv, LVM_SETIMAGELIST, LVSIL_SMALL as usize, icons::lista_sistema().0);
            let col = LVCOLUMNW { mask: LVCF_WIDTH, cx: 200, ..Default::default() };
            enviar(lv, LVM_INSERTCOLUMNW, 0, &col as *const _ as isize);
            lv
        };

        let c = Controles {
            titulo_apps: etiqueta("Aplicaciones instaladas"),
            titulo_accesos: etiqueta("Accesos en la rueda"),
            buscar: campo(ID_BUSCAR, 0),
            recargar: boton("Recargar", ID_RECARGAR),
            apps: lista(ID_APPS, 0),
            agregar: boton("Agregar a la rueda  \u{2192}", ID_AGREGAR),
            accesos: lista(ID_ACCESOS, LVS_SINGLESEL),
            subir: boton("Subir", ID_SUBIR),
            bajar: boton("Bajar", ID_BAJAR),
            quitar: boton("Quitar", ID_QUITAR),
            contador: etiqueta(""),
            et_nombre: etiqueta("Nombre"),
            nombre: campo(ID_NOMBRE, 0),
            et_destino: etiqueta("Destino"),
            destino: campo(ID_DESTINO, 0),
            examinar: boton("Examinar…", ID_EXAMINAR),
            et_args: etiqueta("Argumentos"),
            args: campo(ID_ARGS, 0),
            aplicar: boton("Guardar cambios", ID_APLICAR),
            nuevo: boton("Agregar como nuevo", ID_NUEVO),
            et_abrir_en: etiqueta("Abrir la rueda"),
            abrir_en: control(w!("COMBOBOX"), "", WS_TABSTOP.0 | CBS_DROPDOWNLIST as u32 | WS_VSCROLL.0, WINDOW_EX_STYLE(0), ID_ABRIR_EN),
            et_retardo: etiqueta("Retardo (ms)"),
            retardo: campo(ID_RETARDO, ES_NUMBER as u32),
        };

        enviar(c.buscar, EM_SETCUEBANNER, 1, w!("Buscar aplicación…").0 as isize);
        enviar(c.destino, EM_SETCUEBANNER, 1, w!("Programa, carpeta, archivo, URL o ms-settings:").0 as isize);
        enviar(c.args, EM_SETCUEBANNER, 1, w!("Opcional").0 as isize);
        enviar(c.abrir_en, CB_ADDSTRING, 0, w!("Bajo el puntero").0 as isize);
        enviar(c.abrir_en, CB_ADDSTRING, 0, w!("En el centro del monitor").0 as isize);

        let mut a = Ajustes {
            hwnd,
            principal,
            cfg: config::cargar(),
            apps: Vec::new(),
            cargando: true,
            filtradas: Vec::new(),
            c,
            fuente: HFONT::default(),
            fuente_titulo: HFONT::default(),
            icono_generico: icons::indice(""),
        };
        a.fuentes();
        a.distribuir();
        a.mostrar_opciones();
        a.llenar_accesos(None);

        match apps::en_cache() {
            Some(lista) => {
                a.apps = lista;
                a.cargando = false;
            }
            None => apps::cargar_en_segundo_plano(hwnd, WM_APPS_LISTA, WM_APPS_ICONOS),
        }
        a.filtrar();

        AJUSTES.with(|s| *s.borrow_mut() = Some(a));
        VENTANA.store(hwnd.0 as isize, Ordering::Relaxed);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        if let Some(b) = con_ajustes(|a| a.c.buscar) {
            let _ = SetFocus(Some(b));
        }
    }
    Ok(())
}

// ─── Estado y acciones ──────────────────────────────────────────────────────────────────────

impl Ajustes {
    fn todos(&self) -> Vec<HWND> {
        let c = &self.c;
        vec![
            c.titulo_apps, c.titulo_accesos, c.buscar, c.recargar, c.apps, c.agregar, c.accesos, c.subir,
            c.bajar, c.quitar, c.contador, c.et_nombre, c.nombre, c.et_destino, c.destino, c.examinar,
            c.et_args, c.args, c.aplicar, c.nuevo, c.et_abrir_en, c.abrir_en, c.et_retardo, c.retardo,
        ]
    }

    /// Fuente del sistema para mensajes (Segoe UI), al tamaño del DPI actual.
    fn fuentes(&mut self) {
        unsafe {
            let dpi = GetDpiForWindow(self.hwnd).max(96);
            let mut m = NONCLIENTMETRICSW { cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32, ..Default::default() };
            let _ = SystemParametersInfoForDpi(
                SPI_GETNONCLIENTMETRICS.0,
                m.cbSize,
                Some(&mut m as *mut _ as *mut _),
                0,
                dpi,
            );
            let normal = CreateFontIndirectW(&m.lfMessageFont);
            let mut lf = m.lfMessageFont;
            lf.lfWeight = 600;
            lf.lfHeight = (lf.lfHeight as f32 * 1.15) as i32;
            let titulo = CreateFontIndirectW(&lf);
            for h in self.todos() {
                enviar(h, WM_SETFONT, normal.0 as usize, 0);
            }
            enviar(self.c.titulo_apps, WM_SETFONT, titulo.0 as usize, 0);
            enviar(self.c.titulo_accesos, WM_SETFONT, titulo.0 as usize, 0);
            for viejo in [self.fuente, self.fuente_titulo] {
                if !viejo.is_invalid() {
                    let _ = DeleteObject(viejo.into());
                }
            }
            self.fuente = normal;
            self.fuente_titulo = titulo;
        }
    }

    /// Coloca los controles según el tamaño de la ventana. Dos columnas iguales.
    fn distribuir(&self) {
        let s = escala(self.hwnd);
        let px = |v: f32| (v * s).round() as i32;
        let mut r = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut r);
        }
        let (an, al) = (r.right, r.bottom);
        let (m, sep, fila) = (px(16.0), px(8.0), px(30.0));
        let paso = fila + sep;
        let col = (an - 3 * m) / 2;
        let (xi, xd) = (m, 2 * m + col);
        let y_titulo = m;
        let y_arriba = y_titulo + px(26.0) + sep;
        let et = px(96.0);

        let mover = |h: HWND, x: i32, y: i32, w: i32, alto: i32| unsafe {
            let _ = MoveWindow(h, x, y, w.max(0), alto.max(0), false);
        };
        let c = &self.c;

        // Columna izquierda: buscador, lista de apps y botón para agregar.
        mover(c.titulo_apps, xi, y_titulo, col, px(26.0));
        let w_recargar = px(96.0);
        mover(c.buscar, xi, y_arriba, col - w_recargar - sep, fila);
        mover(c.recargar, xi + col - w_recargar, y_arriba, w_recargar, fila);
        let y_lista = y_arriba + paso;
        mover(c.apps, xi, y_lista, col, al - m - paso - y_lista);
        mover(c.agregar, xi, al - m - fila, col, fila);

        // Columna derecha: accesos, botones de orden y el formulario.
        mover(c.titulo_accesos, xd, y_titulo, col, px(26.0));
        let y_form = al - m - 6 * paso + sep;
        mover(c.accesos, xd, y_arriba, col, y_form - sep - y_arriba);

        let wb = px(88.0);
        mover(c.subir, xd, y_form, wb, fila);
        mover(c.bajar, xd + wb + sep, y_form, wb, fila);
        mover(c.quitar, xd + 2 * (wb + sep), y_form, wb, fila);
        let x_cont = xd + 3 * (wb + sep);
        mover(c.contador, x_cont, y_form, xd + col - x_cont, fila);

        let x_campo = xd + et;
        let w_campo = col - et;
        let y1 = y_form + paso;
        mover(c.et_nombre, xd, y1, et, fila);
        mover(c.nombre, x_campo, y1, w_campo, fila);
        let y2 = y1 + paso;
        let w_examinar = px(100.0);
        mover(c.et_destino, xd, y2, et, fila);
        mover(c.destino, x_campo, y2, w_campo - w_examinar - sep, fila);
        mover(c.examinar, xd + col - w_examinar, y2, w_examinar, fila);
        let y3 = y2 + paso;
        mover(c.et_args, xd, y3, et, fila);
        mover(c.args, x_campo, y3, w_campo, fila);
        let y4 = y3 + paso;
        let w_accion = (w_campo - sep) / 2;
        mover(c.aplicar, x_campo, y4, w_accion, fila);
        mover(c.nuevo, x_campo + w_accion + sep, y4, w_accion, fila);
        let y5 = y4 + paso;
        mover(c.et_abrir_en, xd, y5, et, fila);
        let w_combo = px(200.0);
        // La altura del combo incluye la lista desplegable.
        mover(c.abrir_en, x_campo, y5, w_combo, px(200.0));
        let x_ret = x_campo + w_combo + 2 * sep;
        let w_et_ret = px(92.0);
        mover(c.et_retardo, x_ret, y5, w_et_ret, fila);
        mover(c.retardo, x_ret + w_et_ret, y5, (xd + col - x_ret - w_et_ret).min(px(80.0)), fila);

        ajustar_columna(c.apps);
        ajustar_columna(c.accesos);
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, true);
        }
    }

    fn mostrar_opciones(&self) {
        let i = if self.cfg.abrir_en == AbrirEn::Centro { 1 } else { 0 };
        enviar(self.c.abrir_en, CB_SETCURSEL, i, 0);
        escribir(self.c.retardo, &self.cfg.retardo_ms.to_string());
    }

    fn guardar(&self) {
        if let Err(e) = config::guardar(&self.cfg) {
            eprintln!("no se pudo guardar config.json: {e}");
        }
        unsafe {
            let _ = PostMessageW(Some(self.principal), WM_FOCUS_CONFIG, WPARAM(0), LPARAM(0));
        }
    }

    fn filtrar(&mut self) {
        let lv = self.c.apps;
        redibujo(lv, false);
        lv_vaciar(lv);
        if self.cargando {
            self.filtradas.clear();
            lv_insertar(lv, 0, "Cargando aplicaciones…", -1);
        } else {
            let q = leer(self.c.buscar);
            self.filtradas = (0..self.apps.len()).filter(|&i| apps::coincide(&self.apps[i], &q)).collect();
            for (fila, &i) in self.filtradas.iter().enumerate() {
                let app = &self.apps[i];
                let img = if app.icono >= 0 { app.icono } else { self.icono_generico };
                lv_insertar(lv, fila as i32, &app.nombre, img);
            }
            if !self.filtradas.is_empty() && !q.trim().is_empty() {
                lv_seleccionar(lv, Some(0));
            }
        }
        ajustar_columna(lv);
        redibujo(lv, true);
    }

    fn recibir_iconos(&mut self, tanda: Vec<(usize, i32)>) {
        for (i, icono) in tanda {
            if let Some(app) = self.apps.get_mut(i) {
                app.icono = icono;
            }
            if let Ok(fila) = self.filtradas.binary_search(&i) {
                lv_imagen(self.c.apps, fila as i32, if icono >= 0 { icono } else { self.icono_generico });
            }
        }
    }

    fn llenar_accesos(&mut self, seleccion: Option<usize>) {
        let lv = self.c.accesos;
        redibujo(lv, false);
        lv_vaciar(lv);
        for (fila, a) in self.cfg.accesos.iter().enumerate() {
            lv_insertar(lv, fila as i32, &a.nombre, icons::indice(&a.destino));
        }
        let seleccion = seleccion.filter(|&s| s < self.cfg.accesos.len());
        lv_seleccionar(lv, seleccion);
        ajustar_columna(lv);
        redibujo(lv, true);
        self.mostrar_formulario(seleccion);

        let n = self.cfg.accesos.len();
        let texto = match n {
            0 => "Sin accesos todavía".to_string(),
            1 => "1 acceso".to_string(),
            n if n > RECOMENDADO => format!("{n} accesos · se recomiendan hasta {RECOMENDADO}"),
            n => format!("{n} accesos"),
        };
        escribir(self.c.contador, &texto);
    }

    fn mostrar_formulario(&self, fila: Option<usize>) {
        let (nombre, destino, args) = match fila.and_then(|f| self.cfg.accesos.get(f)) {
            Some(a) => (a.nombre.as_str(), a.destino.as_str(), a.argumentos.as_deref().unwrap_or("")),
            None => ("", "", ""),
        };
        escribir(self.c.nombre, nombre);
        escribir(self.c.destino, destino);
        escribir(self.c.args, args);
    }

    fn seleccion_accesos(&self) -> Option<usize> {
        lv_seleccionadas(self.c.accesos).first().copied()
    }

    /// Agrega las apps seleccionadas a la izquierda (o la primera de la búsqueda, si no hay
    /// ninguna seleccionada). Las que ya están en la rueda no se repiten.
    fn agregar_apps(&mut self) {
        let mut filas = lv_seleccionadas(self.c.apps);
        if filas.is_empty() && !self.filtradas.is_empty() {
            filas.push(0);
        }
        let mut ultima = None;
        for fila in filas {
            let Some(app) = self.filtradas.get(fila).and_then(|&i| self.apps.get(i)) else { continue };
            match self.cfg.accesos.iter().position(|a| a.destino == app.destino) {
                Some(ya) => ultima = Some(ya),
                None => {
                    self.cfg.accesos.push(Acceso { nombre: app.nombre.clone(), destino: app.destino.clone(), argumentos: None });
                    ultima = Some(self.cfg.accesos.len() - 1);
                }
            }
        }
        if ultima.is_some() {
            self.guardar();
            self.llenar_accesos(ultima);
        } else {
            unsafe {
                let _ = MessageBeep(MB_OK);
            }
        }
    }

    fn formulario(&self) -> Option<Acceso> {
        let nombre = leer(self.c.nombre).trim().to_string();
        let destino = leer(self.c.destino).trim().to_string();
        let args = leer(self.c.args).trim().to_string();
        if nombre.is_empty() || destino.is_empty() {
            unsafe {
                let _ = MessageBeep(MB_ICONWARNING);
                let _ = SetFocus(Some(if nombre.is_empty() { self.c.nombre } else { self.c.destino }));
            }
            return None;
        }
        Some(Acceso { nombre, destino, argumentos: (!args.is_empty()).then_some(args) })
    }

    fn aplicar(&mut self) {
        let Some(fila) = self.seleccion_accesos() else {
            // Sin selección, "Guardar cambios" equivale a agregar uno nuevo.
            return self.nuevo();
        };
        let Some(a) = self.formulario() else { return };
        self.cfg.accesos[fila] = a;
        self.guardar();
        self.llenar_accesos(Some(fila));
    }

    fn nuevo(&mut self) {
        let Some(a) = self.formulario() else { return };
        self.cfg.accesos.push(a);
        self.guardar();
        self.llenar_accesos(Some(self.cfg.accesos.len() - 1));
    }

    fn mover(&mut self, delta: isize) {
        let Some(fila) = self.seleccion_accesos() else { return };
        let destino = fila as isize + delta;
        if destino < 0 || destino as usize >= self.cfg.accesos.len() {
            return;
        }
        self.cfg.accesos.swap(fila, destino as usize);
        self.guardar();
        self.llenar_accesos(Some(destino as usize));
    }

    fn quitar(&mut self) {
        let Some(fila) = self.seleccion_accesos() else { return };
        self.cfg.accesos.remove(fila);
        self.guardar();
        let n = self.cfg.accesos.len();
        self.llenar_accesos((n > 0).then(|| fila.min(n - 1)));
    }

    fn cambiar_opciones(&mut self) {
        let abrir_en = if enviar(self.c.abrir_en, CB_GETCURSEL, 0, 0) == 1 { AbrirEn::Centro } else { AbrirEn::Cursor };
        let retardo = leer(self.c.retardo).trim().parse::<u32>().ok().filter(|r| (50..=1000).contains(r));
        let mut cambio = abrir_en != self.cfg.abrir_en;
        self.cfg.abrir_en = abrir_en;
        if let Some(r) = retardo {
            cambio |= r != self.cfg.retardo_ms;
            self.cfg.retardo_ms = r;
        }
        if cambio {
            self.guardar();
        }
    }

    fn poner_archivo(&mut self, ruta: String) {
        if leer(self.c.nombre).trim().is_empty() {
            let nombre = std::path::Path::new(&ruta)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            escribir(self.c.nombre, &nombre);
        }
        escribir(self.c.destino, &ruta);
    }
}

/// Diálogo estándar para elegir un archivo o un acceso directo. Se ejecuta sin el estado
/// prestado: el diálogo tiene su propio bucle de mensajes y la ventana tiene que seguir viva.
fn examinar(dueño: HWND) -> Option<String> {
    let mut buf = vec![0u16; 1024];
    let filtro = w!("Programas y accesos directos\0*.exe;*.lnk;*.bat;*.cmd;*.ps1\0Todos los archivos\0*.*\0\0");
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: dueño,
        lpstrFilter: filtro,
        lpstrFile: PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        lpstrTitle: w!("Elegir un archivo para la rueda"),
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

// ─── Procedimiento de ventana ───────────────────────────────────────────────────────────────

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // Fondo blanco para las etiquetas, como el de la ventana. No necesita el estado.
        WM_CTLCOLORSTATIC => unsafe {
            let dc = HDC(wparam.0 as *mut _);
            SetBkMode(dc, TRANSPARENT);
            return LRESULT(GetSysColorBrush(COLOR_WINDOW).0 as isize);
        },
        WM_GETMINMAXINFO => unsafe {
            let s = escala(hwnd);
            let info = &mut *(lparam.0 as *mut MINMAXINFO);
            info.ptMinTrackSize = POINT { x: (780.0 * s) as i32, y: (560.0 * s) as i32 };
            return LRESULT(0);
        },
        WM_COMMAND if (wparam.0 & 0xFFFF) as i32 == ID_EXAMINAR => {
            if let Some(ruta) = examinar(hwnd) {
                con_ajustes(|a| a.poner_archivo(ruta));
            }
            return LRESULT(0);
        }
        WM_APPS_LISTA => {
            let lista = unsafe { Box::from_raw(lparam.0 as *mut Vec<AppInstalada>) };
            con_ajustes(|a| {
                a.apps = *lista;
                a.cargando = false;
                a.filtrar();
            });
            return LRESULT(0);
        }
        WM_APPS_ICONOS => {
            let tanda = unsafe { Box::from_raw(lparam.0 as *mut Vec<(usize, i32)>) };
            con_ajustes(|a| a.recibir_iconos(*tanda));
            return LRESULT(0);
        }
        WM_DESTROY => {
            VENTANA.store(0, Ordering::Relaxed);
            let fuentes = AJUSTES.with(|s| s.borrow_mut().take()).map(|a| (a.fuente, a.fuente_titulo));
            if let Some((f, t)) = fuentes {
                unsafe {
                    let _ = DeleteObject(f.into());
                    let _ = DeleteObject(t.into());
                }
            }
            return LRESULT(0);
        }
        _ => {}
    }

    let manejado = con_ajustes(|a| unsafe {
        match msg {
            WM_SIZE => {
                a.distribuir();
                true
            }
            WM_DPICHANGED => {
                let r = &*(lparam.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                a.fuentes();
                a.distribuir();
                true
            }
            WM_COMMAND => {
                let id = (wparam.0 & 0xFFFF) as i32;
                let codigo = ((wparam.0 >> 16) & 0xFFFF) as u32;
                match (id, codigo) {
                    (ID_BUSCAR, EN_CHANGE) => a.filtrar(),
                    (ID_RETARDO, EN_CHANGE) => a.cambiar_opciones(),
                    (ID_ABRIR_EN, CBN_SELCHANGE) => a.cambiar_opciones(),
                    (ID_AGREGAR, _) => a.agregar_apps(),
                    (ID_RECARGAR, _) => {
                        a.cargando = true;
                        a.filtrar();
                        apps::cargar_en_segundo_plano(hwnd, WM_APPS_LISTA, WM_APPS_ICONOS);
                    }
                    (ID_SUBIR, _) => a.mover(-1),
                    (ID_BAJAR, _) => a.mover(1),
                    (ID_QUITAR, _) => a.quitar(),
                    (ID_APLICAR, _) => a.aplicar(),
                    (ID_NUEVO, _) => a.nuevo(),
                    // Enter (IsDialogMessage lo convierte en IDOK): en el buscador o en la lista
                    // de apps agrega; en el formulario guarda.
                    (1, _) => {
                        let foco = GetFocus();
                        if foco == a.c.buscar || foco == a.c.apps {
                            a.agregar_apps();
                        } else if [a.c.nombre, a.c.destino, a.c.args].contains(&foco) {
                            a.aplicar();
                        }
                    }
                    // Escape cierra la ventana.
                    (2, _) => {
                        let _ = DestroyWindow(hwnd);
                    }
                    _ => return false,
                }
                true
            }
            WM_NOTIFY => {
                let cab = &*(lparam.0 as *const NMHDR);
                match (cab.idFrom as i32, cab.code) {
                    (ID_APPS, LVN_ITEMACTIVATE) => a.agregar_apps(),
                    (ID_ACCESOS, LVN_ITEMCHANGED) => {
                        let nm = &*(lparam.0 as *const NMLISTVIEW);
                        let seleccion = LVIS_SELECTED.0;
                        if nm.uChanged.0 & LVIF_STATE.0 != 0 && (nm.uNewState ^ nm.uOldState) & seleccion != 0 {
                            a.mostrar_formulario(a.seleccion_accesos());
                        }
                    }
                    _ => return false,
                }
                true
            }
            _ => false,
        }
    });
    if manejado == Some(true) {
        return LRESULT(0);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
