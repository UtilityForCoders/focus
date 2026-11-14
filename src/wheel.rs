//! Geometría de la rueda: medidas y qué acceso cae bajo el puntero. Sin Win32, para poder
//! probarla.

use std::f32::consts::{PI, TAU};

/// Diámetro de cada botón orbital, en píxeles lógicos (a 96 ppp y tamaño 1.0).
pub const BOTON: f32 = 58.0;
/// Radio mínimo de la órbita en la que se reparten los botones.
pub const RADIO_MIN: f32 = 108.0;

/// Punto en un arco circular de radio `r` centrado en el origen, en ángulo `a`.
#[allow(dead_code)]
pub fn punto_arco(r: f32, a: f32) -> (f32, f32) {
    (r * a.cos(), r * a.sin())
}

/// Radio de la órbita: crece con la cantidad de accesos para que los botones no se pisen.
pub fn radio_anillo(n: usize) -> f32 {
    // Cada botón ocupa su diámetro más un 28 % de aire a lo largo de la circunferencia.
    RADIO_MIN.max(n as f32 * BOTON * 1.28 / TAU)
}

/// Distancia del centro al borde exterior de la rueda (para que no se salga del monitor).
pub fn alcance(n: usize) -> f32 {
    radio_anillo(n) + BOTON / 2.0 + 48.0
}

/// Ángulo (radianes) del acceso `i` de `n`. El 0 apunta hacia arriba y el resto sigue en el
/// sentido de las agujas del reloj, como en la pantalla (y hacia abajo).
#[allow(dead_code)]
pub fn angulo_centro(i: usize, n: usize) -> f32 {
    -PI / 2.0 + TAU * i as f32 / n as f32
}

/// Acceso bajo el puntero, o `None` si está dentro del círculo central (cancelar) o no hay
/// accesos. Cuenta la dirección, no la distancia: no hace falta acertar encima del botón.
pub fn sector_en(dx: f32, dy: f32, n: usize, radio_central: f32) -> Option<usize> {
    if n == 0 || (dx * dx + dy * dy).sqrt() < radio_central {
        return None;
    }
    // Ángulo medido desde "arriba", girando en sentido horario, en [0, TAU).
    let a = (dy.atan2(dx) + PI / 2.0).rem_euclid(TAU);
    let paso = TAU / n as f32;
    // Medio paso de desplazamiento: el acceso 0 queda centrado arriba, no empezando arriba.
    Some((((a + paso / 2.0) / paso) as usize) % n)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZONA: f32 = 30.0;

    #[test]
    fn centro_cancela() {
        assert_eq!(sector_en(5.0, 5.0, 6, ZONA), None);
    }

    #[test]
    fn direcciones_cardinales_con_cuatro_accesos() {
        assert_eq!(sector_en(0.0, -100.0, 4, ZONA), Some(0)); // arriba
        assert_eq!(sector_en(100.0, 0.0, 4, ZONA), Some(1)); // derecha
        assert_eq!(sector_en(0.0, 100.0, 4, ZONA), Some(2)); // abajo
        assert_eq!(sector_en(-100.0, 0.0, 4, ZONA), Some(3)); // izquierda
    }

    #[test]
    fn lejos_de_los_botones_sigue_contando_la_direccion() {
        assert_eq!(sector_en(2000.0, 10.0, 4, ZONA), Some(1));
    }

    #[test]
    fn un_solo_acceso_ocupa_todo() {
        assert_eq!(sector_en(-80.0, 80.0, 1, ZONA), Some(0));
    }

    #[test]
    fn sin_accesos_no_hay_seleccion() {
        assert_eq!(sector_en(100.0, 0.0, 0, ZONA), None);
    }

    #[test]
    fn el_anillo_crece_con_muchos_accesos() {
        assert_eq!(radio_anillo(6), RADIO_MIN);
        assert!(radio_anillo(20) > RADIO_MIN);
        // Con 20 accesos, el arco entre centros deja al menos el diámetro de un botón.
        assert!(TAU * radio_anillo(20) / 20.0 >= BOTON);
        let (x, y) = punto_arco(100.0, 0.0);
        assert!((x - 100.0).abs() < 1e-4);
        assert!(y.abs() < 1e-4);
    }

    #[test]
    fn el_alcance_deja_espacio_para_el_boton_y_su_etiqueta() {
        let n = 8;
        assert!(alcance(n) >= radio_anillo(n) + BOTON / 2.0 + 40.0);
    }

    #[test]
    fn distribucion_dinamica_de_2_a_8_accesos() {
        for n in 2..=8 {
            for i in 0..n {
                let ang_centro = angulo_centro(i, n);
                let (x, y) = punto_arco(100.0, ang_centro);
                let hit = sector_en(x, y, n, ZONA);
                assert_eq!(hit, Some(i), "Para n={n}, apuntar al centro del sector {i} debe seleccionarlo");
            }
        }
    }
}
