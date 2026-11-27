use crate::BrushParams;

/// A named starting point for the brush settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushPreset {
    /// Display name. Spanish until translations arrive.
    pub name: &'static str,
    pub params: BrushParams,
}

/// The built-in brushes.
pub fn presets() -> &'static [BrushPreset] {
    const BASE: BrushParams = BrushParams {
        size: 12.0,
        min_size_ratio: 0.1,
        spacing: 0.1,
        flow: 1.0,
        opacity: 1.0,
        hardness: 0.8,
        pressure_affects_size: true,
        pressure_affects_flow: false,
        pressure_gamma: 1.0,
        stabilizer: 0.0,
        erase: false,
    };
    const PRESETS: [BrushPreset; 5] = [
        // Crisp inking line whose width follows pressure.
        BrushPreset {
            name: "Pluma",
            params: BrushParams { size: 10.0, min_size_ratio: 0.05, hardness: 0.95, stabilizer: 0.4, ..BASE },
        },
        // Thin, slightly soft line that darkens with pressure.
        BrushPreset {
            name: "L\u{e1}piz",
            params: BrushParams {
                size: 5.0,
                min_size_ratio: 0.6,
                flow: 0.7,
                hardness: 0.5,
                pressure_affects_flow: true,
                ..BASE
            },
        },
        // Large, fully soft tip that builds up slowly.
        BrushPreset {
            name: "Aer\u{f3}grafo",
            params: BrushParams {
                size: 120.0,
                min_size_ratio: 1.0,
                spacing: 0.05,
                flow: 0.08,
                hardness: 0.0,
                pressure_affects_size: false,
                pressure_affects_flow: true,
                ..BASE
            },
        },
        // Flat, translucent stroke that does not darken where it overlaps itself.
        BrushPreset {
            name: "Rotulador",
            params: BrushParams {
                size: 30.0,
                min_size_ratio: 1.0,
                opacity: 0.5,
                hardness: 0.9,
                pressure_affects_size: false,
                ..BASE
            },
        },
        BrushPreset {
            name: "Borrador",
            params: BrushParams { size: 40.0, min_size_ratio: 0.3, hardness: 0.9, erase: true, ..BASE },
        },
    ];
    &PRESETS
}
