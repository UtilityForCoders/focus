# Focus

Lanzador radial para Windows 10 y 11, escrito en Rust puro (Win32 + Direct2D).

Mantén pulsado el **botón central del ratón** en cualquier sitio: aparece una rueda con tus
accesos. Apunta hacia uno y suelta para abrirlo. Si sueltas en el círculo central o pulsas
**Escape**, se cancela. Un clic central corto se reenvía tal cual, así que abrir enlaces en otra
pestaña sigue funcionando.

## Uso

```bash
cargo run --release
```

Focus queda en el área de notificación. Su menú permite editar los accesos o salir.

Para ver la rueda sin pulsar nada (útil al cambiar el dibujo):

```bash
cargo run -- --demo
```

## Configuración

`%APPDATA%\Focus\config.json`. Se crea con ejemplos la primera vez y se relee en cada
pulsación, así que los cambios valen sin reiniciar.

```json
{
  "abrir_en": "cursor",
  "retardo_ms": 160,
  "accesos": [
    { "nombre": "Explorador", "destino": "explorer.exe" },
    { "nombre": "Descargas", "destino": "%USERPROFILE%\\Downloads" },
    { "nombre": "Navegador", "destino": "https://www.google.com" },
    { "nombre": "Ajustes", "destino": "ms-settings:" },
    { "nombre": "Terminal", "destino": "cmd.exe", "argumentos": "/k echo hola" }
  ]
}
```

- `abrir_en`: `cursor` (bajo el puntero) o `centro` (centro del monitor).
- `retardo_ms`: cuánto hay que mantener el botón para que salga la rueda (50 a 1000).
- `destino`: programa, carpeta, archivo, URL o URI. Acepta variables `%VAR%`.

## Estructura

| Módulo | Qué hace |
|---|---|
| `main.rs` | Ventana de la rueda, bucle de mensajes, abrir/cerrar y animación |
| `hook.rs` | Ganchos globales `WH_MOUSE_LL` / `WH_KEYBOARD_LL` |
| `wheel.rs` | Geometría: qué sector cae bajo el puntero |
| `render.rs` | Dibujo con Direct2D y DirectWrite sobre una ventana layered |
| `config.rs` | Lectura y valores por defecto de `config.json` |
| `launch.rs` | Abrir accesos con `ShellExecuteW` |
| `tray.rs` | Ícono y menú del área de notificación |
