//! Undo and redo.
//!
//! Every change to a document is an [`Edit`]. Applying an edit performs it
//! and returns the edit that reverses it, so undo, redo and the first
//! execution all go through [`Edit::apply`].

use std::collections::VecDeque;

use crate::{Document, Layer, LayerId, LayerProps, Rect};

/// Where layer pixels live. Pixels are RGBA8 with premultiplied alpha, in
/// tightly packed rows.
pub trait PixelStore {
    fn read_rect(&mut self, layer: LayerId, rect: Rect) -> Vec<u8>;
    fn write_rect(&mut self, layer: LayerId, rect: Rect, pixels: &[u8]);
    /// Creates a fully transparent layer.
    fn create_layer(&mut self, layer: LayerId);
    fn delete_layer(&mut self, layer: LayerId);
}

/// Pixels of one rectangle, compressed in memory.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    rect: Rect,
    compressed: Vec<u8>,
}

impl Snapshot {
    /// # Panics
    /// If `pixels` does not hold exactly `rect` as RGBA8.
    pub fn new(rect: Rect, pixels: &[u8]) -> Self {
        assert_eq!(pixels.len(), rect.byte_len(), "snapshot size does not match its rectangle");
        Self { rect, compressed: lz4_flex::compress(pixels) }
    }

    pub fn rect(&self) -> Rect {
        self.rect
    }

    fn pixels(&self) -> Vec<u8> {
        lz4_flex::decompress(&self.compressed, self.rect.byte_len())
            .expect("snapshots are only created by Snapshot::new")
    }

    fn bytes(&self) -> usize {
        self.compressed.len()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    /// Replaces a rectangle of a layer's pixels.
    Pixels {
        layer: LayerId,
        snapshot: Snapshot,
    },
    /// Inserts a layer at `index`, with `pixels` or transparent.
    InsertLayer {
        index: usize,
        layer: Layer,
        pixels: Option<Snapshot>,
    },
    RemoveLayer {
        id: LayerId,
    },
    MoveLayer {
        id: LayerId,
        to: usize,
    },
    SetProps {
        id: LayerId,
        props: LayerProps,
    },
}

impl Edit {
    /// Performs the edit and returns its inverse, or `None` if the edit does
    /// not apply to the document (for example, its layer no longer exists).
    pub fn apply(self, doc: &mut Document, store: &mut dyn PixelStore) -> Option<Edit> {
        match self {
            Edit::Pixels { layer, snapshot } => {
                doc.layer(layer)?;
                let rect = snapshot.rect();
                let current = Snapshot::new(rect, &store.read_rect(layer, rect));
                store.write_rect(layer, rect, &snapshot.pixels());
                Some(Edit::Pixels { layer, snapshot: current })
            }
            Edit::InsertLayer { index, layer, pixels } => {
                if doc.layer(layer.id).is_some() || !doc.can_add_layer() {
                    return None;
                }
                let id = layer.id;
                store.create_layer(id);
                if let Some(snapshot) = pixels {
                    store.write_rect(id, snapshot.rect(), &snapshot.pixels());
                }
                doc.insert(index, layer);
                Some(Edit::RemoveLayer { id })
            }
            Edit::RemoveLayer { id } => {
                // Check first: the pixels must be saved before anything is removed.
                if doc.layers().len() <= 1 || doc.layer(id).is_none() {
                    return None;
                }
                let full = Rect::from_size(doc.width(), doc.height());
                let pixels = Snapshot::new(full, &store.read_rect(id, full));
                let (index, layer) = doc.remove(id)?;
                store.delete_layer(id);
                Some(Edit::InsertLayer { index, layer, pixels: Some(pixels) })
            }
            Edit::MoveLayer { id, to } => {
                let from = doc.move_layer(id, to)?;
                Some(Edit::MoveLayer { id, to: from })
            }
            Edit::SetProps { id, props } => {
                let current = doc.props_mut(id)?;
                let previous = std::mem::replace(current, props);
                Some(Edit::SetProps { id, props: previous })
            }
        }
    }

    /// Approximate heap memory held by this edit.
    fn bytes(&self) -> usize {
        match self {
            Edit::Pixels { snapshot, .. } => snapshot.bytes(),
            Edit::InsertLayer { pixels, .. } => pixels.as_ref().map_or(0, Snapshot::bytes),
            _ => 0,
        }
    }
}

/// Linear undo/redo history bounded by memory rather than step count.
#[derive(Debug)]
pub struct History {
    /// Inverses of performed edits, oldest first.
    undo: VecDeque<Edit>,
    /// Inverses of undone edits; the next redo is last.
    redo: Vec<Edit>,
    budget_bytes: usize,
    /// Layer whose property change may still absorb further changes.
    open_props: Option<LayerId>,
}

impl History {
    pub fn new(budget_bytes: usize) -> Self {
        Self { undo: VecDeque::new(), redo: Vec::new(), budget_bytes, open_props: None }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Performs `edit` as a new user action. Returns false if it did not apply.
    pub fn perform(&mut self, edit: Edit, doc: &mut Document, store: &mut dyn PixelStore) -> bool {
        match edit.apply(doc, store) {
            Some(inverse) => {
                self.record(inverse);
                true
            }
            None => false,
        }
    }

    /// Changes layer properties. Consecutive calls for the same layer merge
    /// into one undo step until [`History::close_props`] is called, so
    /// dragging a slider is undone at once.
    pub fn perform_props(&mut self, id: LayerId, props: LayerProps, doc: &mut Document) -> bool {
        let Some(current) = doc.props_mut(id) else { return false };
        let previous = std::mem::replace(current, props);
        if self.open_props != Some(id) {
            self.record(Edit::SetProps { id, props: previous });
            self.open_props = Some(id);
        }
        true
    }

    /// Ends the current run of merged property changes.
    pub fn close_props(&mut self) {
        self.open_props = None;
    }

    /// Records the inverse of an action that was already performed elsewhere,
    /// such as a brush stroke composited on the GPU.
    pub fn record(&mut self, inverse: Edit) {
        self.open_props = None;
        self.redo.clear();
        self.undo.push_back(inverse);
        self.trim();
    }

    pub fn undo(&mut self, doc: &mut Document, store: &mut dyn PixelStore) -> bool {
        self.open_props = None;
        let Some(edit) = self.undo.pop_back() else { return false };
        if let Some(inverse) = edit.apply(doc, store) {
            self.redo.push(inverse);
        }
        true
    }

    pub fn redo(&mut self, doc: &mut Document, store: &mut dyn PixelStore) -> bool {
        self.open_props = None;
        let Some(edit) = self.redo.pop() else { return false };
        if let Some(inverse) = edit.apply(doc, store) {
            self.undo.push_back(inverse);
            self.trim();
        }
        true
    }

    fn trim(&mut self) {
        let mut total: usize = self.undo.iter().map(Edit::bytes).sum();
        // The most recent step is always kept, even if it alone exceeds the budget.
        while total > self.budget_bytes && self.undo.len() > 1 {
            total -= self.undo.pop_front().map_or(0, |e| e.bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::BlendMode;

    const W: u32 = 4;
    const H: u32 = 3;

    /// CPU pixel store for tests.
    #[derive(Default)]
    struct MemoryStore {
        layers: HashMap<LayerId, Vec<u8>>,
    }

    impl PixelStore for MemoryStore {
        fn read_rect(&mut self, layer: LayerId, rect: Rect) -> Vec<u8> {
            let data = &self.layers[&layer];
            let mut out = Vec::new();
            for y in rect.y..rect.y + rect.height {
                let start = ((y * W + rect.x) * 4) as usize;
                out.extend_from_slice(&data[start..start + rect.width as usize * 4]);
            }
            out
        }

        fn write_rect(&mut self, layer: LayerId, rect: Rect, pixels: &[u8]) {
            let data = self.layers.get_mut(&layer).unwrap();
            for (row, y) in (rect.y..rect.y + rect.height).enumerate() {
                let start = ((y * W + rect.x) * 4) as usize;
                let len = rect.width as usize * 4;
                data[start..start + len].copy_from_slice(&pixels[row * len..(row + 1) * len]);
            }
        }

        fn create_layer(&mut self, layer: LayerId) {
            self.layers.insert(layer, vec![0; (W * H * 4) as usize]);
        }

        fn delete_layer(&mut self, layer: LayerId) {
            self.layers.remove(&layer);
        }
    }

    fn setup() -> (Document, MemoryStore, History) {
        let doc = Document::new(W, H).unwrap();
        let mut store = MemoryStore::default();
        store.create_layer(doc.active());
        (doc, store, History::new(usize::MAX))
    }

    /// Paints `value` into `rect` the way a stroke does: the pixels change
    /// first, then the inverse is recorded.
    fn paint(doc: &Document, store: &mut MemoryStore, history: &mut History, rect: Rect, value: u8) {
        let layer = doc.active();
        let before = Snapshot::new(rect, &store.read_rect(layer, rect));
        store.write_rect(layer, rect, &vec![value; rect.byte_len()]);
        history.record(Edit::Pixels { layer, snapshot: before });
    }

    #[test]
    fn stroke_undo_redo() {
        let (mut doc, mut store, mut history) = setup();
        let layer = doc.active();
        let blank = store.layers[&layer].clone();
        paint(&doc, &mut store, &mut history, Rect::new(1, 1, 2, 2), 200);
        let painted = store.layers[&layer].clone();
        assert_ne!(painted, blank);

        assert!(history.undo(&mut doc, &mut store));
        assert_eq!(store.layers[&layer], blank);
        assert!(history.redo(&mut doc, &mut store));
        assert_eq!(store.layers[&layer], painted);
        assert!(!history.redo(&mut doc, &mut store));
    }

    #[test]
    fn new_action_clears_redo() {
        let (mut doc, mut store, mut history) = setup();
        paint(&doc, &mut store, &mut history, Rect::new(0, 0, 1, 1), 10);
        history.undo(&mut doc, &mut store);
        assert!(history.can_redo());
        paint(&doc, &mut store, &mut history, Rect::new(0, 0, 1, 1), 20);
        assert!(!history.can_redo());
    }

    #[test]
    fn removing_a_layer_keeps_its_pixels_for_undo() {
        let (mut doc, mut store, mut history) = setup();
        let bottom = doc.active();
        let top = doc.new_layer();
        let top_id = top.id;
        assert!(history.perform(
            Edit::InsertLayer { index: 1, layer: top, pixels: None },
            &mut doc,
            &mut store
        ));
        paint(&doc, &mut store, &mut history, Rect::new(0, 0, W, H), 77);
        let painted = store.layers[&top_id].clone();

        assert!(history.perform(Edit::RemoveLayer { id: top_id }, &mut doc, &mut store));
        assert!(!store.layers.contains_key(&top_id));
        assert_eq!(doc.active(), bottom);

        history.undo(&mut doc, &mut store);
        assert_eq!(doc.index_of(top_id), Some(1));
        assert_eq!(doc.active(), top_id);
        assert_eq!(store.layers[&top_id], painted);

        // Undo everything: paint, then the insertion.
        history.undo(&mut doc, &mut store);
        history.undo(&mut doc, &mut store);
        assert_eq!(doc.layers().len(), 1);
        assert!(!store.layers.contains_key(&top_id));
        // And redo it all.
        while history.can_redo() {
            history.redo(&mut doc, &mut store);
        }
        // Insert, paint and remove were replayed: the layer is gone again.
        assert_eq!(doc.layers().len(), 1);
        assert!(doc.layer(top_id).is_none());
        assert!(!store.layers.contains_key(&top_id));
    }

    #[test]
    fn last_layer_removal_is_refused_without_touching_pixels() {
        let (mut doc, mut store, mut history) = setup();
        let only = doc.active();
        assert!(!history.perform(Edit::RemoveLayer { id: only }, &mut doc, &mut store));
        assert!(store.layers.contains_key(&only));
        assert!(!history.can_undo());
    }

    #[test]
    fn move_layer_round_trip() {
        let (mut doc, mut store, mut history) = setup();
        let a = doc.active();
        let b = doc.new_layer();
        let b_id = b.id;
        history.perform(Edit::InsertLayer { index: 1, layer: b, pixels: None }, &mut doc, &mut store);
        history.perform(Edit::MoveLayer { id: a, to: 1 }, &mut doc, &mut store);
        assert_eq!(doc.layers().iter().map(|l| l.id).collect::<Vec<_>>(), vec![b_id, a]);
        history.undo(&mut doc, &mut store);
        assert_eq!(doc.layers().iter().map(|l| l.id).collect::<Vec<_>>(), vec![a, b_id]);
    }

    #[test]
    fn slider_drag_is_one_undo_step() {
        let (mut doc, mut store, mut history) = setup();
        let id = doc.active();
        let original = doc.active_layer().props.clone();
        for opacity in [0.9, 0.7, 0.5] {
            let props = LayerProps { opacity, ..original.clone() };
            assert!(history.perform_props(id, props, &mut doc));
        }
        history.close_props();
        let props = LayerProps { blend: BlendMode::Multiply, ..doc.active_layer().props.clone() };
        history.perform_props(id, props, &mut doc);

        history.undo(&mut doc, &mut store);
        assert_eq!(doc.active_layer().props.blend, BlendMode::Normal);
        assert_eq!(doc.active_layer().props.opacity, 0.5);
        history.undo(&mut doc, &mut store);
        assert_eq!(doc.active_layer().props, original);
        history.redo(&mut doc, &mut store);
        assert_eq!(doc.active_layer().props.opacity, 0.5);
    }

    #[test]
    fn budget_drops_oldest_steps_but_keeps_the_latest() {
        let (mut doc, mut store, _) = setup();
        let mut history = History::new(1);
        for value in [1, 2, 3] {
            paint(&doc, &mut store, &mut history, Rect::new(0, 0, W, H), value);
        }
        assert!(history.undo(&mut doc, &mut store));
        assert_eq!(store.layers[&doc.active()][0], 2);
        assert!(!history.undo(&mut doc, &mut store));
    }

    #[test]
    fn edits_on_a_missing_layer_are_skipped() {
        let (mut doc, mut store, _) = setup();
        let ghost = LayerId(999);
        let snapshot = Snapshot::new(Rect::new(0, 0, 1, 1), &[0; 4]);
        assert!(Edit::Pixels { layer: ghost, snapshot }.apply(&mut doc, &mut store).is_none());
        assert!(Edit::MoveLayer { id: ghost, to: 0 }.apply(&mut doc, &mut store).is_none());
    }
}
