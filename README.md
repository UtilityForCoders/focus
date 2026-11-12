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

Focus queda en el área de notificación. **Clic en su ícono** abre la configuración; el clic
derecho muestra el menú (configurar, abrir `config.json`, salir).

Para desarrollo:

```bash
cargo run -- --demo
```

Abre la rueda durante 4 segundos sin pulsar nada, para revisar el dibujo.

```bash
cargo run -- --ajustes
```

Abre la ventana de configuración al arrancar.

## Configurar los accesos

La ventana de configuración tiene dos columnas:

- **Aplicaciones instaladas**: todas las apps del menú Inicio, también las de la Microsoft
  Store, con su ícono. Escribe en el buscador y pulsa **Enter** (o haz doble clic, o usa
  **Agregar a la rueda**) para añadirla.
- **Accesos en la rueda**: en el orden en que aparecen, empezando arriba y en el sentido de
  las agujas del reloj. **Subir**, **Bajar** y **Quitar** cambian el orden. El formulario de
  abajo sirve para renombrar un acceso o crear uno propio (una carpeta, una URL, un comando);
  **Examinar…** elige un archivo o un acceso directo.

Cada cambio se guarda al momento y vale desde la siguiente pulsación. La rueda muestra el
ícono de cada acceso tal como lo da Windows; las URLs usan el del navegador predeterminado.

## config.json

`%APPDATA%\Focus\config.json`. Es lo que escribe la ventana de configuración, pero también se
puede editar a mano: se crea con ejemplos la primera vez y se relee en cada pulsación.

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
- `destino`: programa, carpeta, archivo, URL o URI. Acepta variables `%VAR%`. Las apps
  agregadas desde la lista usan `shell:AppsFolder\<id>`, que funciona igual para programas de
  escritorio y apps de la Store.

## Estructura

| Módulo | Qué hace |
|---|---|
| `main.rs` | Ventana de la rueda, bucle de mensajes, abrir/cerrar y animación |
| `hook.rs` | Ganchos globales `WH_MOUSE_LL` / `WH_KEYBOARD_LL` |
| `wheel.rs` | Geometría: qué sector cae bajo el puntero |
| `render.rs` | Dibujo con Direct2D y DirectWrite sobre una ventana layered |
| `settings.rs` | Ventana de configuración de accesos (controles nativos de Windows) |
| `apps.rs` | Lista de apps instaladas desde `shell:AppsFolder`, en segundo plano |
| `icons.rs` | Íconos del shell: píxeles para la rueda e índices para las listas |
| `config.rs` | Lectura, escritura y valores por defecto de `config.json` |
| `launch.rs` | Abrir accesos con `ShellExecuteW` |
| `tray.rs` | Ícono y menú del área de notificación |
