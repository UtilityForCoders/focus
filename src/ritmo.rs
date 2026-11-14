//! Pulso de fotogramas al ritmo de la pantalla.
//!
//! Mientras la rueda está abierta, un hilo aparte espera a cada composición del escritorio
//! (`DwmFlush`, que vuelve una vez por refresco) y avisa a la ventana con un mensaje. Así la
//! animación va a 60, 120, 144 Hz o lo que dé el monitor, en lugar de al paso fijo de ~15,6 ms
//! de `SetTimer`. La espera no puede hacerse en el hilo principal: ahí viven los ganchos de
//! entrada, que tienen que responder enseguida.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::OnceLock;
use std::thread::{self, Thread};
use std::time::Duration;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Toca pintar un fotograma.
pub const WM_FOCUS_FOTOGRAMA: u32 = WM_APP + 6;

static ACTIVO: AtomicBool = AtomicBool::new(false);
/// Hay un aviso en la cola sin atender: no se manda otro hasta que se atienda, para que un hilo
/// principal ocupado no acumule fotogramas atrasados.
static EN_COLA: AtomicBool = AtomicBool::new(false);
static VENTANA: AtomicIsize = AtomicIsize::new(0);
static HILO: OnceLock<Thread> = OnceLock::new();

/// Empieza a mandar `WM_FOCUS_FOTOGRAMA` a `hwnd` en cada refresco de pantalla.
pub fn empezar(hwnd: HWND) {
    VENTANA.store(hwnd.0 as isize, Ordering::SeqCst);
    EN_COLA.store(false, Ordering::SeqCst);
    ACTIVO.store(true, Ordering::SeqCst);
    // Un único hilo para toda la vida del programa: se duerme cuando la rueda se cierra.
    HILO.get_or_init(|| thread::Builder::new().name("ritmo".into()).spawn(bucle).expect("hilo de fotogramas").thread().clone())
        .unpark();
}

pub fn parar() {
    ACTIVO.store(false, Ordering::SeqCst);
}

/// Llamar al atender `WM_FOCUS_FOTOGRAMA`, antes de pintar.
pub fn atendido() {
    EN_COLA.store(false, Ordering::SeqCst);
}

fn bucle() {
    loop {
        while !ACTIVO.load(Ordering::SeqCst) {
            thread::park();
        }
        // Sin composición (no debería pasar en Windows 10/11) se cae a ~60 Hz.
        if unsafe { DwmFlush() }.is_err() {
            thread::sleep(Duration::from_millis(16));
        }
        if ACTIVO.load(Ordering::SeqCst) && !EN_COLA.swap(true, Ordering::SeqCst) {
            let hwnd = HWND(VENTANA.load(Ordering::SeqCst) as *mut _);
            if unsafe { PostMessageW(Some(hwnd), WM_FOCUS_FOTOGRAMA, WPARAM(0), LPARAM(0)) }.is_err() {
                EN_COLA.store(false, Ordering::SeqCst);
            }
        }
    }
}
