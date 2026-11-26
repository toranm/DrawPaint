# DrawPaint

App de dibujo y pintura digital open source, escrita en Rust, para Windows y Linux. Su flujo de trabajo está inspirado en las apps de ilustración para tableta: pinceles sensibles a la presión, capas, reglas, estabilizador y timelapse.

> **Estado: fase 0 (cimientos).** Todavía no es una herramienta de dibujo utilizable: hay un lienzo navegable con un pincel sensible a la presión, sin capas, deshacer ni guardado. El camino hasta la 1.0 está en [docs/PLAN.md](docs/PLAN.md).

## Qué hay ahora

- Ventana con lienzo de 2048 × 2048 px: zoom, rotación y desplazamiento.
- Pincel redondo en GPU con tamaño, flujo y dureza, sensible a la presión.
- Entrada de lápiz nativa en Windows (Windows Ink): presión, inclinación, borrador y todos los puntos intermedios que envía la tableta.
- Panel de diagnóstico del puntero (dispositivo, presión, inclinación, muestras por segundo).

## Compilar y ejecutar

Hace falta [Rust](https://rustup.rs); la versión exacta se instala sola a partir de `rust-toolchain.toml`. En Windows se necesitan además las *Build Tools* de Visual Studio con el componente de C++.

```sh
cargo run --release -p drawpaint
```

## Controles

| Acción | Control |
|---|---|
| Pintar | Lápiz o botón izquierdo del ratón |
| Zoom | Rueda |
| Rotar la vista | Mayús + rueda |
| Mover la vista | Botón central, o Espacio + arrastrar |
| Ajustar a la ventana | Ctrl+0 |
| Zoom al 100 % | Ctrl+1 |

## Estructura

| Crate | Contenido |
|---|---|
| `dp-core` | Modelo de documento y matemáticas de la vista. Sin GPU. |
| `dp-brush` | Motor de pincel: de muestras del puntero a *dabs*. Sin GPU. |
| `dp-render` | Render con wgpu: lienzo, pincel y presentación. |
| `dp-input` | Entrada de puntero unificada y backends nativos de lápiz. |
| `dp-io` | Formatos de archivo. |
| `dp-ui` | Interfaz con egui. |
| `drawpaint` | El ejecutable. |

## Contribuir

Consulta [CONTRIBUTING.md](CONTRIBUTING.md).

## Licencia

[GPL-3.0-or-later](LICENSE).
