//! Incrusta el manifiesto (`app.rc` → `app.manifest`) en el .exe. Sin él, las listas y botones de
//! la ventana de configuración se ven con el estilo clásico de Windows 95. Usa el compilador de
//! recursos del Windows SDK (rc.exe), que viene con las Build Tools de Visual Studio.

fn main() {
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=app.rc");
    embed_resource::compile("app.rc", embed_resource::NONE)
        .manifest_optional()
        .expect("no se pudo compilar app.rc (¿falta el Windows SDK?)");
}
