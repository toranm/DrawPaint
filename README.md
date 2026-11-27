# DrawPaint

App de dibujo y pintura digital open source, escrita en Rust, para Windows y Linux. Su flujo de trabajo está inspirado en las apps de ilustración para tableta: pinceles sensibles a la presión, capas, reglas, estabilizador y timelapse.

> **Estado: v0.1 (fase 1 del plan).** Ya se puede hacer un boceto completo, guardarlo y reabrirlo, pero faltan muchas herramientas (selección, relleno, transformación, reglas, texto…). El camino hasta la 1.0 está en [docs/PLAN.md](docs/PLAN.md).

## Qué hay ahora

- **Pinceles**: pluma, lápiz, aerógrafo, rotulador y borrador, con tamaño, opacidad, flujo, dureza, estabilizador y curva de presión ajustables.
- **Capas**: crear, borrar, reordenar, ocultar, renombrar, opacidad y modos de fusión (normal, multiplicar, trama, superponer, sumar).
- **Deshacer y rehacer** de trazos y de operaciones de capa, limitado por memoria y no por número de pasos.
- **Archivos**: formato propio `.dpaint` (conserva las capas), abrir PNG y JPEG, exportar a PNG y JPEG.
- **Lápiz en Windows** (Windows Ink): presión, inclinación, borrador y todos los puntos intermedios que envía la tableta. Probado con una XP-Pen Deco 01 V3.
- Lienzo con zoom, rotación y desplazamiento; cuentagotas; aviso antes de perder cambios sin guardar.

## Compilar y ejecutar

Hace falta [Rust](https://rustup.rs); la versión exacta se instala sola a partir de `rust-toolchain.toml`. En Windows se necesitan además las *Build Tools* de Visual Studio con el componente de C++.

```sh
cargo run --release -p drawpaint
```

## Controles

| Acción | Control |
|---|---|
| Pintar | Lápiz o botón izquierdo del ratón |
| Tomar color del lienzo | Alt + clic |
| Tamaño del pincel | `[` y `]` |
| Pincel / borrador | B / E |
| Deshacer / rehacer | Ctrl+Z / Ctrl+Y |
| Zoom | Rueda |
| Rotar la vista | Mayús + rueda |
| Mover la vista | Botón central, o Espacio + arrastrar |
| Ajustar a la ventana / zoom al 100 % | Ctrl+0 / Ctrl+1 |
| Nuevo / abrir / guardar / guardar como | Ctrl+N / Ctrl+O / Ctrl+S / Ctrl+Mayús+S |
| Exportar imagen | Ctrl+E |

También se puede abrir un archivo pasándolo como argumento: `drawpaint dibujo.dpaint`.

## Estructura

| Crate | Contenido |
|---|---|
| `dp-core` | Documento, capas, historial de deshacer y matemáticas de la vista. Sin GPU. |
| `dp-brush` | Motor de pincel: de muestras del puntero a *dabs*. Sin GPU. |
| `dp-render` | Render con wgpu: capas, trazo, fusión y presentación. |
| `dp-input` | Entrada de puntero unificada y backends nativos de lápiz. |
| `dp-io` | Formato `.dpaint`, PNG y JPEG. |
| `dp-ui` | Interfaz con egui. |
| `drawpaint` | El ejecutable. |

## Contribuir

Consulta [CONTRIBUTING.md](CONTRIBUTING.md).

## Licencia

[GPL-3.0-or-later](LICENSE).
