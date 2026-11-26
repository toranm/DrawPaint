# Contribuir a DrawPaint

Gracias por tu interés. El proyecto está en una fase muy temprana; antes de empezar algo grande, abre un issue para comentarlo. El orden de trabajo previsto está en [docs/PLAN.md](docs/PLAN.md).

## Antes de enviar un cambio

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Los tests de `dp-render` dibujan en una GPU real o por software. Si la máquina no tiene ninguna, se saltan con un aviso; con `DP_REQUIRE_GPU=1` fallan en su lugar, que es lo que hace la CI. En Linux sin GPU basta con instalar `mesa-vulkan-drivers`.

## Convenciones

- Código, comentarios y mensajes de commit en inglés. Los textos de la interfaz están en español hasta que llegue la traducción.
- `dp-core` y `dp-brush` no dependen de `wgpu`, `winit` ni `egui`.
- `unsafe` solo en `dp-input`, con un comentario `SAFETY` en cada bloque.
- Todo cambio de comportamiento lleva su test.

## Licencia

Al contribuir aceptas que tu aportación se publique bajo [GPL-3.0-or-later](LICENSE).
