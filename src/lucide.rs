//! Íconos de Lucide (https://lucide.dev, licencia ISC): búsqueda y lectura de sus trazos.
//!
//! Los datos están en `lucide_datos.rs`, generado por `scripts/generar_lucide.py`: cada ícono es
//! una sola cadena de `path` SVG en una caja de 24x24, pensada para dibujarse con trazo de 2 y
//! extremos redondeados. Aquí se convierte esa cadena en comandos absolutos (`Cmd`), que el pintor
//! pasa a una geometría de Direct2D. La lectura no toca Win32, así que se puede probar.

use crate::lucide_datos::ICONOS;

/// Datos de trazo de un ícono por nombre (`"globe"`, `"terminal"`...).
pub fn trazo(nombre: &str) -> Option<&'static str> {
    ICONOS.binary_search_by(|(n, _, _)| n.cmp(&nombre)).ok().map(|i| ICONOS[i].2)
}

/// Nombre estático (para guardar referencias sin copiar).
pub fn nombre(nombre: &str) -> Option<&'static str> {
    ICONOS.binary_search_by(|(n, _, _)| n.cmp(&nombre)).ok().map(|i| ICONOS[i].0)
}

#[cfg(test)]
pub fn total() -> usize {
    ICONOS.len()
}

pub fn nombre_en(i: usize) -> &'static str {
    ICONOS[i].0
}

/// Posiciones de los íconos que coinciden con la búsqueda: cada palabra tiene que aparecer en el
/// nombre o en las palabras clave. Primero los que coinciden por nombre.
pub fn buscar(q: &str) -> Vec<usize> {
    let palabras: Vec<String> = q.to_lowercase().split_whitespace().map(String::from).collect();
    if palabras.is_empty() {
        return (0..ICONOS.len()).collect();
    }
    let (mut por_nombre, mut por_clave) = (Vec::new(), Vec::new());
    for (i, (n, claves, _)) in ICONOS.iter().enumerate() {
        if palabras.iter().all(|p| n.contains(p.as_str())) {
            por_nombre.push(i);
        } else if palabras.iter().all(|p| n.contains(p.as_str()) || claves.contains(p.as_str())) {
            por_clave.push(i);
        }
    }
    por_nombre.extend(por_clave);
    por_nombre
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cmd {
    Mover(f32, f32),
    Linea(f32, f32),
    Cubica(f32, f32, f32, f32, f32, f32),
    Cuadratica(f32, f32, f32, f32),
    Arco { rx: f32, ry: f32, rot: f32, grande: bool, horario: bool, x: f32, y: f32 },
    Cerrar,
}

struct Lector<'a> {
    s: &'a [u8],
    i: usize,
}

impl Lector<'_> {
    fn saltar(&mut self) {
        while self.i < self.s.len() && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',') {
            self.i += 1;
        }
    }

    fn numero(&mut self) -> Option<f32> {
        self.saltar();
        let ini = self.i;
        let s = self.s;
        if self.i < s.len() && (s[self.i] == b'-' || s[self.i] == b'+') {
            self.i += 1;
        }
        let mut punto = false;
        while self.i < s.len() {
            match s[self.i] {
                b'0'..=b'9' => self.i += 1,
                // "0.5.5" son dos números: un segundo punto empieza el siguiente.
                b'.' if !punto => {
                    punto = true;
                    self.i += 1;
                }
                b'e' | b'E' => {
                    self.i += 1;
                    if self.i < s.len() && (s[self.i] == b'-' || s[self.i] == b'+') {
                        self.i += 1;
                    }
                }
                _ => break,
            }
        }
        std::str::from_utf8(&s[ini..self.i]).ok()?.parse().ok()
    }

    /// Las banderas de un arco pueden venir pegadas ("011"): son un solo dígito cada una.
    fn bandera(&mut self) -> Option<bool> {
        self.saltar();
        let c = *self.s.get(self.i)?;
        self.i += 1;
        match c {
            b'0' => Some(false),
            b'1' => Some(true),
            _ => None,
        }
    }
}

/// Convierte datos de `path` SVG en comandos absolutos. Ante algo que no entiende, devuelve lo
/// leído hasta ahí: un ícono a medias es mejor que ninguno.
pub fn leer(d: &str) -> Vec<Cmd> {
    let mut l = Lector { s: d.as_bytes(), i: 0 };
    let mut cmds = Vec::new();
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let (mut sx, mut sy) = (0.0f32, 0.0f32);
    // Último punto de control, para S y T (que lo reflejan).
    let mut ctrl_c: Option<(f32, f32)> = None;
    let mut ctrl_q: Option<(f32, f32)> = None;
    let mut letra: Option<u8> = None;
    loop {
        l.saltar();
        if l.i >= l.s.len() {
            break;
        }
        let c = l.s[l.i];
        if c.is_ascii_alphabetic() {
            letra = Some(c);
            l.i += 1;
        } else {
            // Un número sin letra repite el comando anterior; tras M, los pares son líneas.
            letra = match letra {
                Some(b'M') => Some(b'L'),
                Some(b'm') => Some(b'l'),
                Some(b'Z' | b'z') | None => return cmds,
                otra => otra,
            };
        }
        let Some(le) = letra else { return cmds };
        let rel = le.is_ascii_lowercase();
        let (bx, by) = if rel { (x, y) } else { (0.0, 0.0) };
        macro_rules! num {
            () => {
                match l.numero() {
                    Some(v) => v,
                    None => return cmds,
                }
            };
        }
        let (mut nc, mut nq) = (None, None);
        match le.to_ascii_uppercase() {
            b'M' => {
                x = bx + num!();
                y = by + num!();
                sx = x;
                sy = y;
                cmds.push(Cmd::Mover(x, y));
            }
            b'L' => {
                x = bx + num!();
                y = by + num!();
                cmds.push(Cmd::Linea(x, y));
            }
            b'H' => {
                x = bx + num!();
                cmds.push(Cmd::Linea(x, y));
            }
            b'V' => {
                y = by + num!();
                cmds.push(Cmd::Linea(x, y));
            }
            b'C' => {
                let (x1, y1, x2, y2) = (bx + num!(), by + num!(), bx + num!(), by + num!());
                let (ex, ey) = (bx + num!(), by + num!());
                cmds.push(Cmd::Cubica(x1, y1, x2, y2, ex, ey));
                nc = Some((x2, y2));
                (x, y) = (ex, ey);
            }
            b'S' => {
                let (x1, y1) = ctrl_c.map_or((x, y), |(cx, cy)| (2.0 * x - cx, 2.0 * y - cy));
                let (x2, y2) = (bx + num!(), by + num!());
                let (ex, ey) = (bx + num!(), by + num!());
                cmds.push(Cmd::Cubica(x1, y1, x2, y2, ex, ey));
                nc = Some((x2, y2));
                (x, y) = (ex, ey);
            }
            b'Q' => {
                let (x1, y1) = (bx + num!(), by + num!());
                let (ex, ey) = (bx + num!(), by + num!());
                cmds.push(Cmd::Cuadratica(x1, y1, ex, ey));
                nq = Some((x1, y1));
                (x, y) = (ex, ey);
            }
            b'T' => {
                let (x1, y1) = ctrl_q.map_or((x, y), |(cx, cy)| (2.0 * x - cx, 2.0 * y - cy));
                let (ex, ey) = (bx + num!(), by + num!());
                cmds.push(Cmd::Cuadratica(x1, y1, ex, ey));
                nq = Some((x1, y1));
                (x, y) = (ex, ey);
            }
            b'A' => {
                let (rx, ry, rot) = (num!(), num!(), num!());
                let (Some(grande), Some(horario)) = (l.bandera(), l.bandera()) else { return cmds };
                let (ex, ey) = (bx + num!(), by + num!());
                cmds.push(Cmd::Arco { rx: rx.abs(), ry: ry.abs(), rot, grande, horario, x: ex, y: ey });
                (x, y) = (ex, ey);
            }
            b'Z' => {
                cmds.push(Cmd::Cerrar);
                (x, y) = (sx, sy);
            }
            _ => return cmds,
        }
        ctrl_c = nc;
        ctrl_q = nq;
    }
    cmds
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn los_datos_estan_ordenados_para_buscar() {
        assert!(ICONOS.windows(2).all(|w| w[0].0 < w[1].0));
        assert!(trazo("globe").is_some());
        assert!(trazo("no-existe-este-icono").is_none());
    }

    #[test]
    fn busca_por_nombre_y_por_palabra_clave() {
        let r = buscar("globe");
        assert_eq!(nombre_en(r[0]), "globe");
        // "world" es palabra clave de globe, no parte del nombre.
        assert!(buscar("world").iter().any(|&i| nombre_en(i) == "globe"));
        assert_eq!(buscar("").len(), total());
    }

    #[test]
    fn lee_comandos_absolutos_y_relativos() {
        let c = leer("M2 12h20");
        assert_eq!(c, vec![Cmd::Mover(2.0, 12.0), Cmd::Linea(22.0, 12.0)]);
        let c = leer("M10 10l5 5 5-5");
        assert_eq!(c, vec![Cmd::Mover(10.0, 10.0), Cmd::Linea(15.0, 15.0), Cmd::Linea(20.0, 10.0)]);
        let c = leer("M1 1V5H3Z");
        assert_eq!(c, vec![Cmd::Mover(1.0, 1.0), Cmd::Linea(1.0, 5.0), Cmd::Linea(3.0, 5.0), Cmd::Cerrar]);
    }

    #[test]
    fn lee_arcos_con_banderas_pegadas_y_numeros_con_dos_puntos() {
        let c = leer("M12 2a14.5 14.5 0 0 0 0 20");
        assert_eq!(c[1], Cmd::Arco { rx: 14.5, ry: 14.5, rot: 0.0, grande: false, horario: false, x: 12.0, y: 22.0 });
        let c = leer("M0 0a1 1 0 011 1");
        assert_eq!(c[1], Cmd::Arco { rx: 1.0, ry: 1.0, rot: 0.0, grande: false, horario: true, x: 1.0, y: 1.0 });
        let c = leer("M0 0l.5.5");
        assert_eq!(c[1], Cmd::Linea(0.5, 0.5));
    }

    #[test]
    fn curvas_reflejan_el_control_anterior() {
        let c = leer("M0 0C1 1 2 1 3 0S5 -1 6 0");
        assert_eq!(c[2], Cmd::Cubica(4.0, -1.0, 5.0, -1.0, 6.0, 0.0));
    }

    #[test]
    fn cada_forma_empieza_en_su_sitio_aunque_su_m_fuera_relativo() {
        // shuffle tiene paths que empiezan con "m": unidos, deben seguir siendo absolutos.
        let cmds = leer(trazo("shuffle").unwrap());
        assert!(cmds.contains(&Cmd::Mover(18.0, 14.0)));
        assert!(cmds.contains(&Cmd::Mover(18.0, 2.0)));
        // Ningún punto se sale de la caja de 24x24 (con margen por las curvas).
        for c in cmds {
            if let Cmd::Mover(x, y) | Cmd::Linea(x, y) = c {
                assert!((-1.0..=25.0).contains(&x) && (-1.0..=25.0).contains(&y), "{c:?}");
            }
        }
    }

    #[test]
    fn todos_los_iconos_se_leen_sin_perder_comandos() {
        for (nombre, _, d) in ICONOS {
            let cmds = leer(d);
            assert!(!cmds.is_empty(), "{nombre} no produjo comandos");
            assert!(matches!(cmds[0], Cmd::Mover(..)), "{nombre} no empieza con M");
        }
    }
}
