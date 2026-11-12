//! Configuración en `%APPDATA%\Focus\config.json`.
//!
//! Si el archivo no existe se crea con unos accesos de ejemplo, para que la rueda tenga algo que
//! mostrar la primera vez. Un JSON mal escrito no tumba la app: se avisa por consola (en debug) y
//! se usan los valores por defecto, sin sobrescribir el archivo del usuario.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AbrirEn {
    /// La rueda aparece centrada bajo el puntero.
    Cursor,
    /// La rueda aparece en el centro del monitor donde está el puntero.
    Centro,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Acceso {
    /// Texto que se ve en el sector.
    pub nombre: String,
    /// Programa, carpeta, archivo, URL o URI (`ms-settings:`). Acepta variables `%VAR%`.
    pub destino: String,
    /// Argumentos opcionales, por ejemplo `/k echo hola` para `cmd.exe`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argumentos: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "abrir_en_defecto")]
    pub abrir_en: AbrirEn,
    /// Cuánto hay que mantener el botón central antes de que salga la rueda. Una pulsación más
    /// corta se reenvía como clic central normal (abrir enlaces en otra pestaña, etc.).
    #[serde(default = "retardo_defecto")]
    pub retardo_ms: u32,
    #[serde(default)]
    pub accesos: Vec<Acceso>,
}

fn abrir_en_defecto() -> AbrirEn {
    AbrirEn::Cursor
}

fn retardo_defecto() -> u32 {
    160
}

impl Default for Config {
    fn default() -> Self {
        let acceso = |nombre: &str, destino: &str| Acceso {
            nombre: nombre.into(),
            destino: destino.into(),
            argumentos: None,
        };
        Config {
            abrir_en: abrir_en_defecto(),
            retardo_ms: retardo_defecto(),
            accesos: vec![
                acceso("Explorador", "explorer.exe"),
                acceso("Navegador", "https://www.google.com"),
                acceso("Bloc de notas", "notepad.exe"),
                acceso("Calculadora", "calc.exe"),
                acceso("Terminal", "cmd.exe"),
                acceso("Descargas", "%USERPROFILE%\\Downloads"),
                acceso("Ajustes", "ms-settings:"),
            ],
        }
    }
}

pub fn ruta() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("Focus").join("config.json")
}

/// Lee la configuración. Si no hay archivo, lo crea con los valores por defecto.
pub fn cargar() -> Config {
    let ruta = ruta();
    match std::fs::read_to_string(&ruta) {
        Ok(texto) => match serde_json::from_str::<Config>(&texto) {
            Ok(mut cfg) => {
                cfg.retardo_ms = cfg.retardo_ms.clamp(50, 1000);
                cfg
            }
            Err(e) => {
                eprintln!("config.json inválido ({e}); se usan los valores por defecto");
                Config::default()
            }
        },
        Err(_) => {
            let cfg = Config::default();
            if let Some(dir) = ruta.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(json) = serde_json::to_string_pretty(&cfg) {
                let _ = std::fs::write(&ruta, json);
            }
            cfg
        }
    }
}

/// Expande `%VAR%` con las variables de entorno. Una variable desconocida se deja tal cual.
pub fn expandir(texto: &str) -> String {
    let mut salida = String::with_capacity(texto.len());
    let mut resto = texto;
    while let Some(ini) = resto.find('%') {
        salida.push_str(&resto[..ini]);
        let tras = &resto[ini + 1..];
        match tras.find('%') {
            Some(fin) if fin > 0 => {
                let nombre = &tras[..fin];
                match std::env::var(nombre) {
                    Ok(valor) => salida.push_str(&valor),
                    Err(_) => {
                        salida.push('%');
                        salida.push_str(nombre);
                        salida.push('%');
                    }
                }
                resto = &tras[fin + 1..];
            }
            _ => {
                salida.push('%');
                resto = tras;
            }
        }
    }
    salida.push_str(resto);
    salida
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expande_variables_conocidas_y_respeta_las_demas() {
        std::env::set_var("FOCUS_PRUEBA", "C:\\x");
        assert_eq!(expandir("%FOCUS_PRUEBA%\\y"), "C:\\x\\y");
        assert_eq!(expandir("%NO_EXISTE_FOCUS%"), "%NO_EXISTE_FOCUS%");
        assert_eq!(expandir("100%"), "100%");
        assert_eq!(expandir("sin variables"), "sin variables");
    }

    #[test]
    fn json_parcial_usa_valores_por_defecto() {
        let cfg: Config = serde_json::from_str(r#"{"accesos":[]}"#).unwrap();
        assert_eq!(cfg.abrir_en, AbrirEn::Cursor);
        assert_eq!(cfg.retardo_ms, 160);
    }
}
