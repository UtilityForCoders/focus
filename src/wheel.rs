//! Geometría de la rueda: medidas y qué acceso cae bajo el puntero. Sin Win32, para poder
//! probarla.

use std::f32::consts::{PI, TAU};

/// Lado de cada baldosa, en píxeles lógicos (a 96 ppp y tamaño 1.0).
pub const BALDOSA: f32 = 54.0;
/// Radio mínimo del círculo en el que se reparten las baldosas.
pub const RADIO_MIN: f32 = 112.0;

pub const RADIO_INTERIOR: f32 = 38.0;   // radio del hueco central
pub const RADIO_EXTERIOR: f32 = 108.0;  // radio exterior del segmento
pub const GAP_ANGULO: f32 = 0.055;      // separación angular en radianes entre segmentos
pub const REDONDEO_ESQUINA: f32 = 8.0;  // px lógicos para esquinas redondeadas

/// Calcula el radio del anillo exterior escalado según cantidad de accesos.
/// Para n pequeño se mantiene en RADIO_EXTERIOR; para n grande puede crecer un poco.
pub fn radio_ext(n: usize) -> f32 {
    // Para n <= 4 el pétalo es más grande para no verse vacío
    if n <= 4 { RADIO_EXTERIOR * 1.15 } else { RADIO_EXTERIOR }
}

pub fn radio_int(_n: usize) -> f32 {
    // El hueco central se ajusta para que el centro no quede demasiado grande
    RADIO_INTERIOR.max(10.0)
}

/// Ángulo inicial y final del sector i de n (en radianes).
/// El gap se reparte mitad en cada lado del sector.
/// El sector 0 está centrado en -PI/2 (arriba).
pub fn angulos_sector(i: usize, n: usize) -> (f32, f32) {
    let paso = TAU / n as f32;
    let centro = -PI / 2.0 + paso * i as f32;
    let mitad = paso / 2.0 - GAP_ANGULO / 2.0;
    (centro - mitad, centro + mitad)
}

/// Punto en un arco circular de radio `r` centrado en el origen, en ángulo `a`.
#[allow(dead_code)]
pub fn punto_arco(r: f32, a: f32) -> (f32, f32) {
    (r * a.cos(), r * a.sin())
}

/// Radio del círculo de baldosas: crece con la cantidad de accesos para que no se pisen.
pub fn radio_anillo(n: usize) -> f32 {
    // Cada baldosa ocupa su lado más un 35 % de aire a lo largo de la circunferencia.
    RADIO_MIN.max(n as f32 * BALDOSA * 1.35 / TAU)
}

/// Distancia del centro al borde exterior de la rueda (para que no se salga del monitor).
pub fn alcance(n: usize) -> f32 {
    radio_anillo(n) + BALDOSA * 0.7 + 40.0
}

/// Ángulo (radianes) del acceso `i` de `n`. El 0 apunta hacia arriba y el resto sigue en el
/// sentido de las agujas del reloj, como en la pantalla (y hacia abajo).
#[allow(dead_code)]
pub fn angulo_centro(i: usize, n: usize) -> f32 {
    -PI / 2.0 + TAU * i as f32 / n as f32
}

/// Acceso bajo el puntero, o `None` si está dentro del círculo central (cancelar) o no hay
/// accesos. Cuenta la dirección, no la distancia: no hace falta acertar encima de la baldosa.
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
    fn lejos_de_las_baldosas_sigue_contando_la_direccion() {
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
        // Con 20 accesos, el arco entre centros deja al menos el lado de una baldosa.
        assert!(TAU * radio_anillo(20) / 20.0 >= BALDOSA);
    }

    #[test]
    fn angulos_sector_y_punto_arco() {
        let (ini, fin) = angulos_sector(0, 4);
        assert!(ini < fin);
        let (x, y) = punto_arco(100.0, 0.0);
        assert!((x - 100.0).abs() < 1e-4);
        assert!(y.abs() < 1e-4);
        assert!(radio_ext(4) > radio_ext(6));
        assert_eq!(radio_int(4), RADIO_INTERIOR);
    }

    #[test]
    fn distribucion_dinamica_de_2_a_8_accesos() {
        for n in 2..=8 {
            let mut prev_fin = -PI - 1.0;
            for i in 0..n {
                let (ini, fin) = angulos_sector(i, n);
                assert!(ini < fin, "Para n={n}, sector {i}: ini ({ini}) debe ser menor que fin ({fin})");
                let ang_centro = (ini + fin) / 2.0;
                let (x, y) = punto_arco(100.0, ang_centro);
                let hit = sector_en(x, y, n, ZONA);
                assert_eq!(hit, Some(i), "Para n={n}, apuntar al centro del sector {i} debe seleccionarlo");
                assert!(ini >= prev_fin || i == 0, "No debe haber superposición entre sectores contiguos");
                prev_fin = fin;
            }
        }
    }
}
