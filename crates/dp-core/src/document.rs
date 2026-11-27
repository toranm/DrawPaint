use glam::Vec2;
use serde::{Deserialize, Serialize};

/// Stable identity of a layer within one document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LayerId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Add,
}

impl BlendMode {
    pub const ALL: [BlendMode; 5] =
        [BlendMode::Normal, BlendMode::Multiply, BlendMode::Screen, BlendMode::Overlay, BlendMode::Add];
}

/// The user-editable properties of a layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayerProps {
    pub name: String,
    pub visible: bool,
    /// 0..=1.
    pub opacity: f32,
    pub blend: BlendMode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: LayerId,
    #[serde(flatten)]
    pub props: LayerProps,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentError {
    /// Width or height is zero or above [`Document::MAX_SIDE`].
    InvalidSize {
        width: u32,
        height: u32,
    },
    NoLayers,
    TooManyLayers(usize),
    DuplicateLayerId(LayerId),
    UnknownActiveLayer(LayerId),
}

impl std::fmt::Display for DocumentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSize { width, height } => write!(f, "invalid canvas size {width}x{height}"),
            Self::NoLayers => write!(f, "document has no layers"),
            Self::TooManyLayers(count) => write!(f, "document has {count} layers"),
            Self::DuplicateLayerId(id) => write!(f, "layer id {} appears twice", id.0),
            Self::UnknownActiveLayer(id) => write!(f, "active layer {} does not exist", id.0),
        }
    }
}

impl std::error::Error for DocumentError {}

/// A painting document: canvas size and the layer stack. Pixels live in a
/// [`crate::PixelStore`], keyed by [`LayerId`].
///
/// A document always has at least one layer, and its active layer always
/// exists.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    width: u32,
    height: u32,
    /// Opaque paper colour under all layers, sRGB-encoded.
    pub background: [f32; 3],
    /// Bottom to top.
    layers: Vec<Layer>,
    active: LayerId,
    next_id: u64,
}

impl Document {
    pub const MAX_SIDE: u32 = 16384;
    pub const MAX_LAYERS: usize = 256;

    /// Creates a document with a single empty layer.
    pub fn new(width: u32, height: u32) -> Result<Self, DocumentError> {
        Self::check_size(width, height)?;
        let mut doc =
            Self { width, height, background: [1.0; 3], layers: Vec::new(), active: LayerId(0), next_id: 1 };
        let layer = doc.new_layer();
        doc.active = layer.id;
        doc.layers.push(layer);
        Ok(doc)
    }

    /// Rebuilds a document from stored parts, validating its invariants.
    pub fn from_parts(
        width: u32,
        height: u32,
        background: [f32; 3],
        layers: Vec<Layer>,
        active: LayerId,
    ) -> Result<Self, DocumentError> {
        Self::check_size(width, height)?;
        if layers.is_empty() {
            return Err(DocumentError::NoLayers);
        }
        if layers.len() > Self::MAX_LAYERS {
            return Err(DocumentError::TooManyLayers(layers.len()));
        }
        let mut ids: Vec<LayerId> = layers.iter().map(|l| l.id).collect();
        ids.sort();
        if let Some(pair) = ids.windows(2).find(|pair| pair[0] == pair[1]) {
            return Err(DocumentError::DuplicateLayerId(pair[0]));
        }
        if !ids.contains(&active) {
            return Err(DocumentError::UnknownActiveLayer(active));
        }
        let next_id = ids.last().map_or(1, |id| id.0.saturating_add(1));
        Ok(Self { width, height, background, layers, active, next_id })
    }

    fn check_size(width: u32, height: u32) -> Result<(), DocumentError> {
        let valid = |side| (1..=Self::MAX_SIDE).contains(&side);
        if valid(width) && valid(height) {
            Ok(())
        } else {
            Err(DocumentError::InvalidSize { width, height })
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn size(&self) -> Vec2 {
        Vec2::new(self.width as f32, self.height as f32)
    }

    /// Layers from bottom to top.
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.layers.iter().find(|l| l.id == id)
    }

    pub fn index_of(&self, id: LayerId) -> Option<usize> {
        self.layers.iter().position(|l| l.id == id)
    }

    pub fn active(&self) -> LayerId {
        self.active
    }

    pub fn active_layer(&self) -> &Layer {
        self.layer(self.active).expect("the active layer always exists")
    }

    /// Makes `id` the active layer. Returns false if it does not exist.
    pub fn set_active(&mut self, id: LayerId) -> bool {
        let exists = self.layer(id).is_some();
        if exists {
            self.active = id;
        }
        exists
    }

    pub fn can_add_layer(&self) -> bool {
        self.layers.len() < Self::MAX_LAYERS
    }

    /// Allocates a fresh layer with default properties. It is not part of
    /// the document until inserted through an [`crate::Edit`].
    pub fn new_layer(&mut self) -> Layer {
        let id = LayerId(self.next_id);
        self.next_id += 1;
        Layer {
            id,
            props: LayerProps {
                name: format!("Capa {}", id.0),
                visible: true,
                opacity: 1.0,
                blend: BlendMode::Normal,
            },
        }
    }

    pub(crate) fn insert(&mut self, index: usize, layer: Layer) {
        self.active = layer.id;
        self.layers.insert(index.min(self.layers.len()), layer);
    }

    /// Removes a layer, moving the active layer to a neighbour if needed.
    /// The last remaining layer cannot be removed.
    pub(crate) fn remove(&mut self, id: LayerId) -> Option<(usize, Layer)> {
        if self.layers.len() <= 1 {
            return None;
        }
        let index = self.index_of(id)?;
        let layer = self.layers.remove(index);
        if self.active == id {
            self.active = self.layers[index.saturating_sub(1)].id;
        }
        Some((index, layer))
    }

    /// Moves a layer to `to`, returning its previous index.
    pub(crate) fn move_layer(&mut self, id: LayerId, to: usize) -> Option<usize> {
        let from = self.index_of(id)?;
        let layer = self.layers.remove(from);
        self.layers.insert(to.min(self.layers.len()), layer);
        Some(from)
    }

    pub(crate) fn props_mut(&mut self, id: LayerId) -> Option<&mut LayerProps> {
        self.layers.iter_mut().find(|l| l.id == id).map(|l| &mut l.props)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_document_has_one_active_layer() {
        let doc = Document::new(100, 50).unwrap();
        assert_eq!(doc.layers().len(), 1);
        assert_eq!(doc.active_layer().id, doc.layers()[0].id);
    }

    #[test]
    fn rejects_bad_sizes() {
        assert!(Document::new(0, 10).is_err());
        assert!(Document::new(10, Document::MAX_SIDE + 1).is_err());
    }

    #[test]
    fn from_parts_validates() {
        let mut doc = Document::new(10, 10).unwrap();
        let a = doc.layers()[0].clone();
        let b = doc.new_layer();
        let ok = Document::from_parts(10, 10, [1.0; 3], vec![a.clone(), b.clone()], b.id).unwrap();
        // Fresh ids never collide with loaded ones.
        assert!(ok.clone().new_layer().id > b.id);
        assert_eq!(
            Document::from_parts(10, 10, [1.0; 3], vec![a.clone(), a.clone()], a.id),
            Err(DocumentError::DuplicateLayerId(a.id))
        );
        assert_eq!(
            Document::from_parts(10, 10, [1.0; 3], vec![a.clone()], b.id),
            Err(DocumentError::UnknownActiveLayer(b.id))
        );
        assert_eq!(Document::from_parts(10, 10, [1.0; 3], vec![], a.id), Err(DocumentError::NoLayers));
    }

    #[test]
    fn last_layer_cannot_be_removed() {
        let mut doc = Document::new(10, 10).unwrap();
        let only = doc.active();
        assert!(doc.remove(only).is_none());
        assert_eq!(doc.layers().len(), 1);
    }

    #[test]
    fn removing_active_layer_activates_neighbour() {
        let mut doc = Document::new(10, 10).unwrap();
        let bottom = doc.active();
        let top = doc.new_layer();
        doc.insert(1, top.clone());
        assert_eq!(doc.active(), top.id);
        doc.remove(top.id).unwrap();
        assert_eq!(doc.active(), bottom);
    }
}
