//! Geometría de la rueda: medidas y qué sector cae bajo el puntero. Sin Win32, para poder probarla.

use std::f32::consts::{PI, TAU};

/// Medidas en píxeles lógicos (a 96 ppp); se multiplican por la escala del monitor.
pub const RADIO_EXTERIOR: f32 = 180.0;
pub const RADIO_INTERIOR: f32 = 64.0;
/// Separación angular entre sectores, en radianes.
pub const HUECO: f32 = 0.035;

/// Ángulo (radianes) del centro del sector `i` de `n`. El sector 0 apunta hacia arriba y el
/// resto sigue en el sentido de las agujas del reloj, como en la pantalla (y hacia abajo).
pub fn angulo_centro(i: usize, n: usize) -> f32 {
    -PI / 2.0 + TAU * i as f32 / n as f32
}

/// Sector bajo el puntero, o `None` si está dentro del círculo central (cancelar) o no hay
/// sectores. Fuera del radio exterior sigue contando la dirección: apuntar es un gesto, no hace
/// falta acertar dentro del anillo.
pub fn sector_en(dx: f32, dy: f32, n: usize, radio_interior: f32) -> Option<usize> {
    if n == 0 || (dx * dx + dy * dy).sqrt() < radio_interior {
        return None;
    }
    // Ángulo medido desde "arriba", girando en sentido horario, en [0, TAU).
    let a = (dy.atan2(dx) + PI / 2.0).rem_euclid(TAU);
    let paso = TAU / n as f32;
    // Medio paso de desplazamiento: el sector 0 queda centrado arriba, no empezando arriba.
    Some((((a + paso / 2.0) / paso) as usize) % n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centro_cancela() {
        assert_eq!(sector_en(5.0, 5.0, 6, RADIO_INTERIOR), None);
    }

    #[test]
    fn direcciones_cardinales_con_cuatro_sectores() {
        assert_eq!(sector_en(0.0, -100.0, 4, RADIO_INTERIOR), Some(0)); // arriba
        assert_eq!(sector_en(100.0, 0.0, 4, RADIO_INTERIOR), Some(1)); // derecha
        assert_eq!(sector_en(0.0, 100.0, 4, RADIO_INTERIOR), Some(2)); // abajo
        assert_eq!(sector_en(-100.0, 0.0, 4, RADIO_INTERIOR), Some(3)); // izquierda
    }

    #[test]
    fn lejos_del_anillo_sigue_contando_la_direccion() {
        assert_eq!(sector_en(2000.0, 10.0, 4, RADIO_INTERIOR), Some(1));
    }

    #[test]
    fn un_solo_sector_ocupa_todo() {
        assert_eq!(sector_en(-80.0, 80.0, 1, RADIO_INTERIOR), Some(0));
    }

    #[test]
    fn sin_sectores_no_hay_seleccion() {
        assert_eq!(sector_en(100.0, 0.0, 0, RADIO_INTERIOR), None);
    }
}
