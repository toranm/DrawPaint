# DrawPaint — Plan de proyecto

App de dibujo y pintura digital open source, escrita en Rust, fuertemente inspirada en el flujo de trabajo de ibis Paint. Objetivo: Windows y Linux. **Foco inicial: Windows.**

> Alcance de "inspirada en": replicamos el *tipo* de herramientas y el flujo de trabajo (pinceles, capas, reglas, estabilizador, timelapse, herramientas de manga). No copiamos nombre, iconos, pinceles, materiales, formato `.ipv` ni el diseño visual de ibis Paint. Todo el arte, los presets y la UI son propios.

---

## 1. Objetivos y no-objetivos

**Objetivos**

- Trazo con lápiz de baja latencia (meta: < 20 ms del movimiento al píxel a 60 Hz; sin saltos con tabletas de 200+ Hz).
- Lienzos de hasta 8192 × 8192 px, uso cómodo en 4096 × 4096 con 30+ capas en una GPU integrada moderna.
- Funcionar bien con tableta gráfica (Wacom, Huion, XP-Pen), pantalla táctil con lápiz (Surface) y ratón.
- Un único binario sin dependencias de runtime; instalador y versión portable.
- Código portable desde el día 1: nada específico de Windows fuera de la capa de entrada y el empaquetado.

**No-objetivos (por ahora)**

- Android/iOS/macOS/web.
- Funciones en la nube, cuentas, tienda de materiales, IA.
- Gestión de color completa (ICC/CMYK) y 16/32 bits por canal: se deja la puerta abierta pero no entra en la v1.
- Compatibilidad con el formato `.ipv`.

---

## 2. Stack técnico

| Área | Elección | Motivo |
|---|---|---|
| Lenguaje | Rust estable, workspace de Cargo | — |
| Ventana y eventos | `winit` | Estándar, Windows + X11 + Wayland |
| GPU | `wgpu` (DX12 en Windows, Vulkan en Linux, GL como respaldo) | Una sola API, shaders WGSL |
| UI | `egui` (`egui-wgpu` + `egui-winit`) | Iteración rápida, se integra en el mismo render pass que el lienzo, fácil de tematizar |
| Entrada de lápiz | Capa propia `dp-input`: backend WM_POINTER con el crate `windows`; en Linux, `octotablet` o backends propios (Wayland tablet-v2 / XInput2) | `winit` no expone inclinación ni el historial de puntos de alta frecuencia; ver riesgo R1 |
| Imágenes | `image` (PNG/JPEG/WebP), `psd` (lectura), escritor PSD propio | — |
| Compresión | `zstd` (archivo), `lz4_flex` (deshacer en memoria) | — |
| Serialización | `serde` + JSON (manifiesto, presets), binario para píxeles | — |
| Texto | `cosmic-text` | Shaping, fuentes del sistema, bidi |
| Vectores (reglas, marcos, selección) | `lyon` (teselado) / `kurbo` (geometría) | — |
| Paralelismo | `rayon` | Filtros y E/S en CPU |
| i18n | `fluent` | Español e inglés desde el principio |
| Diálogos nativos | `rfd` | Abrir/guardar |
| Errores / logs | `thiserror`, `anyhow` (solo en el binario), `tracing` | — |

**Alternativas descartadas para la UI:** `iced` (más difícil mezclar con un lienzo wgpu a medida), `Slint` (licencia y control del render), Tauri/webview (latencia y memoria). Si egui se queda corto en estética, el lienzo y el núcleo no dependen de él.

---

## 3. Arquitectura

### 3.1 Crates del workspace

```
crates/
  dp-core      Modelo de documento, capas, comandos, historial, selección, geometría. Sin GPU ni ventana.
  dp-brush     Motor de pincel: muestreo de entrada → dabs. Dinámicas, estabilizador, presets. Sin GPU.
  dp-render    Todo lo wgpu: texturas de capa, rasterizado de dabs, composición, filtros GPU, readback.
  dp-io        Formato nativo, PNG/JPEG/WebP, PSD, ORA, exportación de timelapse.
  dp-input     Abstracción de puntero (lápiz/táctil/ratón) + backends por plataforma.
  dp-ui        Paneles y widgets egui, temas, atajos, i18n.
  drawpaint    Binario: bucle de eventos, pegamento entre crates, configuración.
```

Regla de dependencias: `dp-core` y `dp-brush` no conocen `wgpu`, `winit` ni `egui`. Así se pueden probar sin GPU y reutilizar en otros frontends.

### 3.2 Modelo de documento (`dp-core`)

- `Document { size, dpi, layers: árbol, selection, guides, metadata }`.
- Nodo de capa: `Raster`, `Group` (carpeta), `Text`, `Frame` (viñetas), más adelante `Adjustment`.
- Propiedades por capa: nombre, visible, opacidad, modo de fusión, recorte a la capa inferior (clipping), bloqueo de alfa, bloqueo total, marca "capa de referencia", máscara opcional.
- Los píxeles no viven en `dp-core`: cada capa raster tiene un `LayerId` y el almacenamiento lo implementa `dp-render` tras el trait `PixelStore`.
- **Toda modificación es un `Command`** (`AddLayer`, `Stroke`, `Fill`, `Transform`, `SetBlendMode`...). Un comando sabe aplicarse y deshacerse. Esto da deshacer/rehacer, autoguardado incremental y la base del timelapse.

### 3.3 Píxeles y color

- Formato de trabajo: **RGBA8, alfa premultiplicado, mezcla en espacio sRGB** (gamma), que es el comportamiento que los usuarios de ibis/SAI/CSP esperan.
- El tipo de píxel es un parámetro del `PixelStore` para poder añadir RGBA16F más adelante sin reescribir.
- **Fuente de verdad: texturas GPU.** Una textura por capa en el MVP (ver decisión D1), con cuota de VRAM: las capas no visibles o más antiguas se descargan a RAM comprimidas (lz4) y se recargan bajo demanda.
- Límite de capas calculado según tamaño del lienzo y memoria disponible (igual que hace ibis), mostrado al crear el lienzo.

**D1 — Textura completa por capa vs. teselas dispersas.** El MVP usa textura completa por capa: mucho más simple y suficiente hasta ~4K con decenas de capas gracias a la descarga a RAM. La interfaz `PixelStore` trabaja con rectángulos (`read_rect`, `write_rect`, `dirty_rect`), de modo que migrar a un atlas de teselas de 256 px (fase 6) no afecta al resto del código.

### 3.4 Motor de pincel

Pipeline de un trazo:

```
eventos de puntero (posición, presión, inclinación, tiempo)
  → estabilizador (media móvil ponderada / "cuerda" con retardo configurable)
  → interpolación (Catmull-Rom) y remuestreo por espaciado
  → dinámicas (curvas presión→tamaño/opacidad, velocidad, inclinación, jitter, entrada/salida forzada)
  → lista de dabs {pos, tamaño, ángulo, opacidad, flujo, color}          [dp-brush, CPU]
  → rasterizado instanciado de dabs en un búfer de trazo                  [dp-render, GPU]
  → composición búfer de trazo → capa con la opacidad y modo del pincel
```

- **Búfer de trazo separado**: la opacidad del pincel limita el trazo completo (no se acumula al cruzarse consigo mismo), y permite previsualizar sin tocar la capa hasta soltar el lápiz.
- Puntas: círculo paramétrico (dureza) y puntas de textura; textura de papel opcional.
- Tipos de pincel: normal, borrador (mismo motor, modo destination-out), difuminar/dedo (lee del destino con ping-pong), desenfocar, acuarela simple (mezcla con el color existente).
- Respeta: selección activa, bloqueo de alfa, recorte, simetría y regla activa (la regla transforma las posiciones antes de generar dabs).
- Presets en JSON (`.dpbrush`), importables/exportables.
- Predicción de trazo opcional para ocultar latencia (los puntos predichos se dibujan y se descartan en el siguiente frame).

### 3.5 Render y composición

- Composición por capas en shaders WGSL; modos de fusión: normal, multiplicar, trama, superponer, sumar, oscurecer, aclarar, sobreexponer/subexponer color, luz suave/fuerte, diferencia, tono, saturación, color, luminosidad, etc. Los modos no separables y los que no admite el blending fijo se hacen con ping-pong entre dos texturas.
- **Caché de composición**: se guardan dos texturas, "todo lo de debajo de la capa activa" y "todo lo de encima". Mientras se pinta, cada frame solo compone *debajo + capa activa (+ búfer de trazo) + encima*, sea cual sea el número de capas.
- Solo se recompone el rectángulo sucio; solo se redibuja la ventana cuando algo cambia (sin bucle continuo, para no gastar batería).
- Vista: desplazamiento, zoom, rotación y volteo del lienzo como matriz de vista; filtrado nearest al ampliar > 100 %, mipmaps/lineal al reducir; rejilla de píxeles a zoom alto.
- Presentación de baja latencia: `desired_maximum_frame_latency = 1`, probar `Mailbox`/`Immediate` frente a vsync.
- Readback asíncrono GPU→CPU para deshacer, guardar, relleno y varita mágica.

### 3.6 Deshacer / rehacer

- Cada comando de píxeles guarda el rectángulo afectado **antes** y **después**, comprimido con lz4, en RAM con presupuesto configurable; lo que excede va a un archivo temporal en disco.
- Los comandos de estructura (añadir/mover capa, propiedades) guardan solo el delta lógico.
- Historial lineal con límite por memoria, no por número de pasos.

### 3.7 Formato de archivo y persistencia

- **Nativo `.dpaint`**: contenedor ZIP con `manifest.json` (versión de formato, árbol de capas, propiedades, reglas, texto editable), un blob por capa (píxeles comprimidos con zstd, recortados a su caja delimitadora), `thumbnail.png`, `merged.png` y, opcionalmente, `timelapse/`.
- Versionado del formato y migraciones desde el primer día.
- **Autoguardado y recuperación**: guardado periódico en segundo plano a un archivo paralelo; al arrancar tras un cierre inesperado se ofrece recuperar.
- **Interoperabilidad**: PNG (con transparencia), JPEG, WebP; PSD lectura y escritura (capas, grupos, modos, opacidad, recorte); OpenRaster `.ora`.
- Galería de obras al inicio (miniaturas de una carpeta de proyectos), como en ibis.

### 3.8 Timelapse

- MVP: al terminar cada trazo/comando se captura un fotograma reducido del lienzo compuesto (p. ej. lado largo 1080 px) y se añade a `timelapse/` dentro del proyecto. Simple, robusto y no exige que el motor sea determinista.
- Exportación: secuencia de PNG y GIF/APNG/WebP animado en Rust puro; MP4 mediante `ffmpeg` externo si está instalado (evita empaquetar códecs con licencias problemáticas).
- Futuro: reproducción real por re-ejecución del registro de comandos.

### 3.9 Entrada (`dp-input`)

Evento unificado:

```rust
struct PointerSample {
    device: DeviceKind,      // Pen, Eraser, Touch, Mouse
    pos: Vec2,               // píxeles físicos, subpíxel
    pressure: f32,           // 0..1
    tilt: Vec2, twist: f32,
    buttons: Buttons,        // punta, botón lateral, borrador
    time: Duration,          // marca de tiempo del dispositivo
}
```

- **Windows**: mensajes `WM_POINTER*` sobre el HWND de winit (subclassing), con `GetPointerPenInfoHistory` para recuperar todos los puntos intermedios. Cubre Windows Ink (Surface y la mayoría de tabletas actuales). Wintab como backend opcional posterior para controladores antiguos.
- **Linux (después)**: Wayland `tablet-v2` y X11 XInput2, evaluando `octotablet`.
- Rechazo de palma: si hay lápiz en proximidad, el táctil no dibuja.
- Gestos táctiles: dos dedos para mover/zoom/rotar, toque con dos dedos = deshacer, tres = rehacer (configurable).
- Curva de presión global configurable y por pincel.

### 3.10 UI

- Disposición pensada para lápiz y táctil, con atajos de teclado completos para escritorio: barra de herramientas, controles deslizantes rápidos de tamaño y opacidad junto al lienzo, panel de capas, selector de color (rueda HSV + cuadrado, paleta, historial de colores), panel de propiedades del pincel con vista previa del trazo.
- Paneles acoplables/plegables; modo pantalla completa sin UI (Tab).
- Tema oscuro y claro; escalado HiDPI por monitor.
- Atajos reconfigurables; modificadores temporales (Espacio = mano, Alt = cuentagotas, Ctrl = mover, R = rotar vista).
- Ventana de referencia (imagen flotante de la que se puede tomar color).

---

## 4. Funcionalidades por áreas

| Área | Contenido |
|---|---|
| Pinceles | Pluma, lápiz, rotulador, aerógrafo, acuarela, difumino, desenfoque, borrador; editor de pinceles; importación de presets |
| Capas | Raster, carpetas, recorte, bloqueo de alfa, modos de fusión, opacidad, fusionar, duplicar, capa de referencia, máscaras |
| Selección | Rectángulo, elipse, lazo, polígono, varita mágica, invertir, expandir/contraer, difuminar; capa de selección pintable |
| Relleno | Cubo con tolerancia, cierre de huecos, referencia a capa actual / capa de referencia / lienzo; degradados |
| Transformación | Mover, escalar, rotar, libre, perspectiva, malla; interpolación bilineal/bicúbica |
| Reglas | Recta, circular, elíptica, radial, paralela, simetría (espejo, caleidoscopio), perspectiva de 1–3 puntos |
| Estabilizador | Nivel ajustable, modo "cuerda", afinado forzado de inicio y fin |
| Filtros | Brillo/contraste, tono/saturación, curvas, niveles, desenfoque gaussiano/movimiento, enfoque, mosaico, ruido, extracción de línea, mapa de degradado, licuar |
| Manga | División de viñetas, tramas (como patrón de capa), líneas de velocidad/concentración, bocadillos |
| Texto | Capa de texto editable, fuentes del sistema, vertical/horizontal, contorno |
| Formas | Línea, rectángulo, elipse, polígono, curva Bézier; dibujadas con el pincel activo |
| Timelapse | Captura automática y exportación |
| Animación | Línea de tiempo por fotogramas, papel cebolla, exportar GIF/APNG (fase tardía) |

---

## 5. Fases

Cada fase termina con una versión ejecutable en Windows y etiquetada.

### Fase 0 — Cimientos y pruebas de concepto
- Workspace, licencia, README, CONTRIBUTING, CI (fmt, clippy, tests); ver 6.1.
- Ventana winit + wgpu + egui mostrando una textura con mover/zoom/rotar.
- **Spike R1**: backend WM_POINTER con presión, inclinación, borrador e historial de puntos. Probar con al menos una tableta Wacom/Huion y un dispositivo con pantalla y lápiz.
- **Spike R2**: medir latencia de presentación (vsync vs. mailbox) y coste de rasterizar dabs grandes.
- *Criterio de salida*: se dibuja una línea sensible a la presión en una textura, sin puntos perdidos.

### Fase 1 — MVP de pintura (v0.1)
- Documento con capas raster: añadir, borrar, reordenar, visibilidad, opacidad, modo normal/multiplicar/trama/superponer/sumar.
- Motor de pincel con búfer de trazo, 5 presets (pluma, lápiz, aerógrafo, rotulador, borrador), tamaño/opacidad, curva de presión.
- Estabilizador básico, cuentagotas, selector de color HSV.
- Deshacer/rehacer con rectángulos sucios.
- Guardar/abrir `.dpaint`, exportar/importar PNG y JPEG.
- Atajos esenciales.
- *Criterio de salida*: se puede hacer un boceto completo de principio a fin y reabrirlo sin pérdida.

### Fase 2 — Herramientas esenciales (v0.2)
- Selección (rectángulo, lazo, varita), relleno con cubo (tolerancia + capa de referencia), degradado.
- Transformación (mover/escalar/rotar/libre) de capa y de selección.
- Recorte, bloqueo de alfa, carpetas, fusionar capas, resto de modos de fusión con caché de composición.
- Difumino y desenfoque como pinceles.
- Gestos táctiles y rechazo de palma.
- Autoguardado y recuperación; galería de inicio.
- Timelapse (captura) y exportación a secuencia/GIF.

### Fase 3 — Reglas, formas y editor de pinceles (v0.3)
- Todas las reglas y simetrías; herramientas de forma.
- Editor de pinceles completo, puntas de textura, textura de papel, importación/exportación de presets.
- Filtros de ajuste y desenfoque (GPU) con vista previa en vivo.
- Ventana de referencia, paletas de color.
- Instalador MSI + zip portable; publicación automática de releases.

### Fase 4 — Interoperabilidad y texto (v0.4)
- PSD lectura/escritura, ORA.
- Herramienta de texto.
- Transformación de perspectiva y malla; licuar.
- Máscaras de capa; cierre de huecos en el relleno.
- i18n completa (es/en), atajos reconfigurables, preferencias.

### Fase 5 — Manga y Linux (v0.5)
- Viñetas, tramas, líneas de efecto, bocadillos.
- **Port a Linux**: backends de lápiz Wayland/X11, pruebas en GNOME/KDE, AppImage y Flatpak.
- Exportación MP4 del timelapse vía ffmpeg.

### Fase 6 — Rendimiento y 1.0
- Almacenamiento por teselas dispersas en GPU, lienzos de 8K+.
- Perfilado (Tracy), pruebas en GPU integradas antiguas, respaldo GL.
- Animación por fotogramas.
- Documentación de usuario, web del proyecto, 1.0.

---

## 6. Calidad

- **Tests unitarios** en `dp-core` y `dp-brush` (sin GPU): comandos y su inverso, generación de dabs determinista, estabilizador, geometría de reglas.
- **Tests de imagen dorada** en `dp-render`: se renderiza offscreen con un adaptador software (WARP en Windows, lavapipe en Linux) y se compara con tolerancia. Cubre modos de fusión, pinceles y filtros.
- **Tests de ida y vuelta** en `dp-io`: guardar → abrir → comparar; fuzzing de los lectores (`cargo-fuzz`) porque abren archivos no confiables.
- **Reproducción de trazos grabados**: capturas reales de `PointerSample` como fixtures para detectar regresiones de tacto y rendimiento.
- Benchmarks con `criterion` (dabs/s, tiempo de composición, tiempo de guardado).
- CI: `cargo fmt --check`, `clippy -D warnings`, tests, `cargo-deny` (licencias y avisos de seguridad).
- `unsafe` solo en `dp-input` (FFI de Windows) y revisado.

### 6.1 CI

El repositorio es público, así que la CI usa los runners alojados por GitHub (`windows-latest` y `ubuntu-latest`), que no consumen créditos en repos públicos. En cada push y pull request:

- **Windows**: formato, clippy, tests y compilación en release; el `drawpaint.exe` resultante se publica como artefacto de la ejecución.
- **Linux**: clippy y tests, con Vulkan por software (`mesa-vulkan-drivers`) para los tests de render.

Los tests de render exigen un adaptador gráfico en CI (`DP_REQUIRE_GPU=1`): WARP en Windows, lavapipe en Linux.

**Runner self-hosted (opcional).** Solo compensa si hace falta una GPU real en CI o si el repo vuelve a ser privado. En ese caso: runner de Windows como servicio bajo un usuario local sin permisos de administrador (idealmente en una VM), aprobación manual de workflows para colaboradores externos, ningún secreto en jobs de PR y nunca `pull_request_target`. En un repo público, un PR de un fork puede ejecutar código arbitrario en la máquina del runner.

---

## 7. Distribución

- Windows 10 1809+ y Windows 11, x86_64 (ARM64 más adelante).
- Releases en GitHub generadas por la CI al empujar una etiqueta: instalador MSI (`cargo-wix` o `cargo-dist`) y zip portable; después `winget` y Scoop.
- Firma de código: sin certificado al principio (aviso de SmartScreen); evaluar SignPath para proyectos open source.
- Configuración en `%APPDATA%\DrawPaint`, proyectos en `Documentos\DrawPaint`; modo portable si existe `portable.txt` junto al ejecutable.
- Linux: AppImage y Flatpak (fase 5).

---

## 8. Riesgos

| # | Riesgo | Mitigación |
|---|---|---|
| R1 | La entrada de lápiz en Windows es fragmentada (Windows Ink vs. Wintab, controladores que no informan bien) | Spike en fase 0; capa `dp-input` propia con backends intercambiables; fixtures de trazos reales |
| R2 | Latencia percibida por vsync y cola de presentación | Medir pronto; latencia de frame 1, predicción de trazo |
| R3 | VRAM insuficiente con muchas capas | Cuota + descarga a RAM en MVP; teselas dispersas en fase 6; límite de capas visible para el usuario |
| R4 | Los modos de fusión y pinceles que leen del destino requieren ping-pong y complican el render | Diseñar el compositor con ping-pong desde el inicio; caché de composición |
| R5 | El readback GPU→CPU es lento y asíncrono (afecta a deshacer, relleno, guardar) | Leer solo rectángulos sucios; hacerlo fuera del camino crítico del trazo |
| R6 | egui se queda corto en aspecto o accesibilidad | `dp-ui` aislado; núcleo independiente de la UI |
| R7 | Alcance enorme (ibis tiene años de funciones) | Fases con criterio de salida; cada versión es usable por sí sola |
| R8 | Diferencias entre GPU/drivers | Tests dorados con adaptador software; respaldo GL; opción de elegir backend |

---

## 9. Decisiones pendientes

1. ~~Licencia~~ — decidido: `GPL-3.0-or-later`.
2. **Nombre definitivo.** "DrawPaint" es el nombre de trabajo; conviene comprobar que no choca con marcas existentes.
3. **Hardware de prueba.** Qué tableta(s) y dispositivos hay disponibles para el spike de entrada.
4. **Idioma del código y la documentación.** Recomendación: código y comentarios en inglés, documentación de usuario en español e inglés.

---

## 10. Estado

**Fase 0 — en curso.**

Hecho:

- Workspace con los siete crates, licencia, README, CONTRIBUTING y CI.
- Ventana winit + wgpu + egui con lienzo navegable (zoom, rotación, desplazamiento, ajustar).
- Pincel redondo en GPU sensible a la presión, verificado con un test que comprueba los píxeles.
- Backend de lápiz `WM_POINTER` para Windows: presión, inclinación, borrador, historial de puntos y posición subpíxel.

Pendiente para cerrar la fase (requiere hardware real con Windows):

- **Spike R1**: probar el backend de lápiz con una tableta y con una pantalla con lápiz. El código compila para Windows pero aún no se ha ejecutado en un dispositivo. El panel "Puntero" muestra los datos necesarios para validarlo.
- **Spike R2**: medir la latencia de presentación (vsync frente a mailbox) y el coste de los dabs grandes.

Simplificaciones conscientes de la fase 0, que se resuelven en la fase 1: se pinta directamente sobre el lienzo (sin búfer de trazo), interpolación lineal entre muestras (sin estabilizador), el borrador pinta con el color del papel, y los bordes del lienzo rotado no están suavizados.
