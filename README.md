# Orbyt

Lanzador radial para Windows 10 y 11, escrito en Rust puro (Win32 + Direct2D).

Mantén pulsado el **botón central del ratón** (o uno lateral, según los ajustes) en cualquier
sitio: aparece una rueda con tus grupos de apps. Abre un grupo y elige una app, soltando el
botón o con clic, según el modo (ver [Grupos](#grupos)). **Escape** cancela. Un clic central corto
se reenvía tal cual, así que abrir enlaces en otra pestaña sigue funcionando.

## Uso

```bash
cargo run --release
```

Orbyt queda en el área de notificación. **Clic en su ícono** abre la configuración; el clic
derecho muestra un menú propio, oscuro como el resto de la app, con **Configuración** y
**Salir de Orbyt**. Abrir `config.json` y su carpeta están en Configuración → **Acerca de**, donde
también se puede salir.

Para desarrollo:

```bash
cargo run -- --demo
```

Abre la rueda durante 4 segundos sin pulsar nada, para revisar el dibujo.

```bash
cargo run -- --ajustes
```

Abre la ventana de configuración al arrancar.

## Ajustes

Una ventana oscura con fondo acrílico (Windows 11), dibujada con Direct2D, con estas secciones:

- **General**: dónde aparece la rueda (bajo el puntero o en el centro), el retardo y si Orbyt
  se inicia con Windows.
- **Activación**: qué botón abre la rueda (central, lateral atrás o lateral adelante), si se
  elige soltando el botón o con clic, y el tamaño de la zona central.
- **Apariencia**: tamaño de la rueda y oscurecer el fondo, con una vista previa en vivo.
- **Accesos**: los grupos y sus apps (ver abajo).
- **Acerca de**: versión, abrir `config.json` o su carpeta, y salir de Orbyt.

### Accesos

La sección tiene dos columnas:

- **Aplicaciones instaladas**: todas las apps del menú Inicio, también las de la Microsoft
  Store, con su ícono. Haz clic en una para añadirla, o escribe en el buscador y pulsa
  **Enter** para añadir la primera que coincida.
- **En el grupo**: las apps del grupo elegido, en el orden en que aparecen, empezando arriba y
  en el sentido de las agujas del reloj. Al pasar el ratón por una fila aparecen los botones para subir, bajar o quitar. El
  editor de abajo sirve para renombrar un acceso o crear uno propio (una carpeta, una URL, un
  comando); el botón de carpeta elige un archivo o un acceso directo.

Encima de la lista hay una pastilla por grupo; las apps que agregues desde la izquierda van al
grupo elegido (si no hay ninguno, se crea uno). **+ Grupo** crea uno nuevo. Con un grupo elegido
aparece una barra para cambiarle el ícono, el nombre y el orden, o quitarlo (pide un segundo
clic). Todos los grupos se editan igual, también "Principal".

Cada cambio se guarda al momento y vale desde la siguiente pulsación.

### Grupos

La rueda principal solo muestra grupos (por ejemplo *Multimedia* con Twitch, YouTube y Kick, o
*Agentes de IA* con Claude y ChatGPT), como botones distribuidos sobre una órbita. Sin ícono propio, un grupo
muestra en miniatura los de sus primeros cuatro accesos. Al abrir uno, la rueda se vuelve a
desplegar con sus apps y su nombre bajo el botón central.

En **Activación → Elegir con** hay dos modos:

| | Soltar el botón | Clic |
|---|---|---|
| Abrir la rueda | Mantén el botón | Mantén el botón y suéltalo: la rueda queda abierta |
| Abrir un grupo | Mantén la dirección estable durante 650 ms | Clic izquierdo sobre él |
| Abrir una app | Apúntala y suelta | Clic izquierdo sobre ella |
| Volver | Quédate un instante en el centro (muestra una flecha) | Clic derecho, o clic en el centro |
| Cerrar | Suelta en el centro o pulsa **Escape** | Clic derecho o en el centro (en la rueda principal), **Escape** o el botón otra vez |

Como en todo Orbyt, cuenta la dirección del puntero, no hace falta acertar encima del botón.
En modo clic, los clics con la rueda abierta son para Orbyt y no llegan a la ventana de debajo.

### Íconos

Cada acceso usa su ícono automático, salvo que elijas otro con **Elegir…** en el editor:

- **Apps, programas, carpetas y archivos**: el ícono que da Windows, pedido a 256 px. Se prueban
  dos variantes y se queda la que no tenga halo blanco (el borde claro que deja un ícono
  recortado sobre fondo blanco).
- **Webs**: el favicon del sitio. Se descarga una vez en segundo plano (solo se envía el nombre
  del host al servicio de favicons de Google, o al de DuckDuckGo si falla) y se guarda en
  `%APPDATA%\Orbyt\favicons`. Mientras no llega, se usa el ícono del navegador.
- **Lucide**: 1.891 íconos de línea, con buscador (las palabras clave están en inglés).
- **Archivo**: cualquier ícono de un `.dll`, `.exe` o `.ico`; por defecto se abre
  `shell32.dll`, con más de 300.

En `config.json` el ícono elegido se guarda como `"icono": "lucide:music"` o
`"icono": "archivo:%SystemRoot%\\System32\\shell32.dll,31"` (las barras van dobles dentro del
JSON).

## config.json

`%APPDATA%\Orbyt\config.json`. Es lo que escribe la ventana de configuración, pero también se
puede editar a mano: se crea con ejemplos la primera vez y se relee en cada pulsación.

```json
{
  "abrir_en": "cursor",
  "retardo_ms": 160,
  "modo": "soltar",
  "grupos": [
    {
      "nombre": "Sistema",
      "accesos": [
        { "nombre": "Explorador", "destino": "explorer.exe" },
        { "nombre": "Descargas", "destino": "%USERPROFILE%\\Downloads" },
        { "nombre": "Ajustes", "destino": "ms-settings:" },
        { "nombre": "Terminal", "destino": "cmd.exe", "argumentos": "/k echo hola" }
      ]
    },
    {
      "nombre": "Agentes de IA",
      "icono": "lucide:bot",
      "accesos": [
        { "nombre": "Claude", "destino": "https://claude.ai" },
        { "nombre": "ChatGPT", "destino": "https://chatgpt.com" }
      ]
    }
  ]
}
```

- `abrir_en`: `cursor` (bajo el puntero) o `centro` (centro del monitor).
- `retardo_ms`: cuánto hay que mantener el botón para que salga la rueda (80 a 600).
- `boton`: `central`, `atras` o `adelante`.
- `modo`: `soltar` (apuntar y soltar el botón) o `clic` (el botón abre la rueda y se elige con clic).
- `tamano`: escala de la rueda (0.8 a 1.4).
- `zona_central`: radio del círculo central (cancelar o volver), en píxeles (20 a 60).
- `oscurecer`: `true` o `false`.
- `grupos`: cada uno con `nombre`, `icono` (opcional) y sus `accesos`. Un `accesos` suelto en la
  raíz, de versiones anteriores, se convierte al cargar en un grupo "Principal".
- `destino`: programa, carpeta, archivo, URL o URI. Acepta variables `%VAR%`. Las apps
  agregadas desde la lista usan `shell:AppsFolder\<id>`, que funciona igual para programas de
  escritorio y apps de la Store.

## Estructura

| Módulo | Qué hace |
|---|---|
| `main.rs` | Ventana de la rueda, bucle de mensajes, abrir/cerrar, grupos y animación |
| `hook.rs` | Ganchos globales `WH_MOUSE_LL` / `WH_KEYBOARD_LL`, botón configurable |
| `wheel.rs` | Geometría: qué sector cae bajo el puntero |
| `render.rs` | La rueda de botones orbitales, sobre una ventana layered (y en la vista previa) |
| `settings.rs` | Ventana de ajustes: barra lateral, secciones, listas y campos de texto propios |
| `ui.rs` | Kit de dibujo compartido: tema oscuro, formas, texto, glifos de Segoe Fluent Icons |
| `autostart.rs` | Iniciar con Windows (clave `Run` del usuario) |
| `apps.rs` | Lista de apps instaladas desde `shell:AppsFolder`, en segundo plano |
| `icons.rs` | Íconos: shell a 256 px sin halo, favicons, íconos de archivos, WIC y caché |
| `web.rs` | Descarga de favicons con WinHTTP y caché en disco |
| `lucide.rs` | Búsqueda de íconos Lucide y lectura de sus trazos SVG |
| `lucide_datos.rs` | Datos de Lucide, generados por `scripts/generar_lucide.py` |
| `config.rs` | Lectura, escritura y valores por defecto de `config.json` |
| `launch.rs` | Abrir accesos con `ShellExecuteW` |
| `tray.rs` | Ícono del área de notificación |
| `menu.rs` | Menú propio del clic derecho en la bandeja, dibujado con Direct2D |

## Licencias de terceros

Los íconos de [Lucide](https://lucide.dev) se distribuyen bajo licencia ISC; el texto completo
está en [licencias/LUCIDE.txt](licencias/LUCIDE.txt). Para actualizarlos:

```bash
npm pack lucide-static@<versión>
```

Extrae el `.tgz` y ejecuta:

```bash
python -I scripts/generar_lucide.py package/icon-nodes.json package/tags.json src/lucide_datos.rs
```
