//! The native document format: a ZIP archive holding `manifest.json`, one
//! raw premultiplied RGBA8 blob per layer, and a flattened `merged.png`
//! preview that other tools can read.

use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::Path;

use dp_core::{Document, Layer, LayerId};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::{check_buffer, IoError};

pub const DOCUMENT_EXTENSION: &str = "dpaint";

const FORMAT_NAME: &str = "drawpaint";
const FORMAT_VERSION: u32 = 1;
const MANIFEST_PATH: &str = "manifest.json";
const MERGED_PATH: &str = "merged.png";
/// A manifest is a few kilobytes; anything far beyond that is not ours.
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: String,
    version: u32,
    width: u32,
    height: u32,
    background: [f32; 3],
    active_layer: LayerId,
    /// Bottom to top.
    layers: Vec<Layer>,
}

/// A document read from disk with each layer's premultiplied RGBA8 pixels,
/// in the document's layer order.
#[derive(Debug)]
pub struct LoadedDocument {
    pub document: Document,
    pub layers: Vec<(LayerId, Vec<u8>)>,
}

fn layer_path(id: LayerId) -> String {
    format!("layers/{}.rgba", id.0)
}

/// Saves `document` to `path`.
///
/// `layer_pixels` supplies each layer's premultiplied RGBA8 pixels and
/// `merged` is the opaque flattened image. The file is written beside its
/// destination and renamed over it, so a failed save never destroys the
/// previous version.
pub fn save_document(
    path: &Path,
    document: &Document,
    mut layer_pixels: impl FnMut(LayerId) -> Vec<u8>,
    merged: &[u8],
) -> Result<(), IoError> {
    let (width, height) = (document.width(), document.height());
    check_buffer(width, height, merged)?;

    let mut temp_name = path.file_name().unwrap_or_default().to_os_string();
    temp_name.push(".tmp");
    let temp_path = path.with_file_name(temp_name);

    let mut write = || -> Result<(), IoError> {
        let mut zip = ZipWriter::new(BufWriter::new(File::create(&temp_path)?));
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated =
            SimpleFileOptions::default().compression_method(CompressionMethod::Deflated).large_file(true);

        let manifest = Manifest {
            format: FORMAT_NAME.to_owned(),
            version: FORMAT_VERSION,
            width,
            height,
            background: document.background,
            active_layer: document.active(),
            layers: document.layers().to_vec(),
        };
        zip.start_file(MANIFEST_PATH, deflated)?;
        serde_json::to_writer_pretty(&mut zip, &manifest)?;

        for layer in document.layers() {
            let pixels = layer_pixels(layer.id);
            check_buffer(width, height, &pixels)?;
            zip.start_file(layer_path(layer.id), deflated)?;
            zip.write_all(&pixels)?;
        }

        // PNG is already compressed.
        zip.start_file(MERGED_PATH, stored)?;
        let mut png = Vec::new();
        image::write_buffer_with_format(
            &mut std::io::Cursor::new(&mut png),
            merged,
            width,
            height,
            image::ExtendedColorType::Rgba8,
            image::ImageFormat::Png,
        )?;
        zip.write_all(&png)?;

        let file = zip.finish()?.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        Ok(())
    };

    match write() {
        Ok(()) => Ok(std::fs::rename(&temp_path, path)?),
        Err(error) => {
            let _ = std::fs::remove_file(&temp_path);
            Err(error)
        }
    }
}

/// Loads a document. Files are untrusted: every size is checked against the
/// manifest before memory is allocated for it, and canvases with a side
/// above `max_side` are refused.
pub fn load_document(path: &Path, max_side: u32) -> Result<LoadedDocument, IoError> {
    let mut zip = ZipArchive::new(File::open(path)?)
        .map_err(|error| IoError::NotADocument(format!("it is not a ZIP archive ({error})")))?;

    let manifest: Manifest = {
        let entry =
            zip.by_name(MANIFEST_PATH).map_err(|_| IoError::NotADocument("it has no manifest".to_owned()))?;
        let mut json = String::new();
        entry.take(MAX_MANIFEST_BYTES).read_to_string(&mut json)?;
        serde_json::from_str(&json)
            .map_err(|error| IoError::NotADocument(format!("its manifest is invalid ({error})")))?
    };
    if manifest.format != FORMAT_NAME {
        return Err(IoError::NotADocument(format!("its format is \"{}\"", manifest.format)));
    }
    if manifest.version > FORMAT_VERSION {
        return Err(IoError::NewerFormat { found: manifest.version, supported: FORMAT_VERSION });
    }
    let (width, height) = (manifest.width, manifest.height);
    if width > max_side || height > max_side {
        return Err(IoError::TooLarge { width, height, max: max_side });
    }

    let mut layers = manifest.layers;
    for layer in &mut layers {
        let opacity = layer.props.opacity;
        layer.props.opacity = if opacity.is_finite() { opacity.clamp(0.0, 1.0) } else { 1.0 };
    }
    let background = manifest.background.map(|c| if c.is_finite() { c.clamp(0.0, 1.0) } else { 1.0 });
    let document = Document::from_parts(width, height, background, layers, manifest.active_layer)?;

    let expected = width as usize * height as usize * 4;
    let mut pixels = Vec::with_capacity(document.layers().len());
    for layer in document.layers() {
        let name = layer_path(layer.id);
        let entry =
            zip.by_name(&name).map_err(|_| IoError::Corrupt(format!("layer data {name} is missing")))?;
        let mut data = Vec::new();
        // Read one byte past the expected size to detect oversized entries
        // without ever holding more than that in memory.
        entry.take(expected as u64 + 1).read_to_end(&mut data)?;
        if data.len() != expected {
            return Err(IoError::Corrupt(format!("layer data {name} has the wrong size")));
        }
        pixels.push((layer.id, data));
    }
    Ok(LoadedDocument { document, layers: pixels })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use dp_core::{BlendMode, Edit, History, LayerProps, PixelStore, Rect};

    use super::*;

    const W: u32 = 5;
    const H: u32 = 3;
    const BYTES: usize = (W * H * 4) as usize;

    struct NullStore;
    impl PixelStore for NullStore {
        fn read_rect(&mut self, _: LayerId, rect: Rect) -> Vec<u8> {
            vec![0; rect.byte_len()]
        }
        fn write_rect(&mut self, _: LayerId, _: Rect, _: &[u8]) {}
        fn create_layer(&mut self, _: LayerId) {}
        fn delete_layer(&mut self, _: LayerId) {}
    }

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("dp-io-{}-{name}.dpaint", std::process::id()))
    }

    /// A two-layer document with distinctive properties and pixels.
    fn sample() -> (Document, HashMap<LayerId, Vec<u8>>) {
        let mut document = Document::new(W, H).unwrap();
        let mut history = History::new(usize::MAX);
        let bottom = document.active();
        let top = document.new_layer();
        let top_id = top.id;
        history.perform(
            Edit::InsertLayer { index: 1, layer: top, pixels: None },
            &mut document,
            &mut NullStore,
        );
        let props = LayerProps {
            name: "Sombras ñ".to_owned(),
            visible: false,
            opacity: 0.25,
            blend: BlendMode::Multiply,
        };
        history.perform_props(top_id, props, &mut document);
        document.set_active(bottom);
        document.background = [0.5, 0.25, 1.0];
        let pixels = HashMap::from([
            (bottom, (0..BYTES).map(|i| i as u8).collect()),
            (top_id, (0..BYTES).map(|i| (i * 3) as u8).collect()),
        ]);
        (document, pixels)
    }

    fn save(path: &Path, document: &Document, pixels: &HashMap<LayerId, Vec<u8>>) -> Result<(), IoError> {
        save_document(path, document, |id| pixels[&id].clone(), &[255; BYTES])
    }

    #[test]
    fn round_trip_preserves_everything() {
        let path = temp("round");
        let (document, pixels) = sample();
        save(&path, &document, &pixels).unwrap();
        let loaded = load_document(&path, 4096).unwrap();
        std::fs::remove_file(&path).unwrap();

        assert_eq!(loaded.document, document);
        assert_eq!(loaded.layers.len(), 2);
        for (id, data) in &loaded.layers {
            assert_eq!(data, &pixels[id]);
        }
        // Layer pixels come back in document order.
        let order: Vec<LayerId> = loaded.layers.iter().map(|(id, _)| *id).collect();
        assert_eq!(order, document.layers().iter().map(|l| l.id).collect::<Vec<_>>());
    }

    #[test]
    fn merged_preview_is_a_readable_png() {
        let path = temp("merged");
        let (document, pixels) = sample();
        save(&path, &document, &pixels).unwrap();
        let mut zip = ZipArchive::new(File::open(&path).unwrap()).unwrap();
        let mut png = Vec::new();
        zip.by_name(MERGED_PATH).unwrap().read_to_end(&mut png).unwrap();
        std::fs::remove_file(&path).unwrap();
        let image = image::load_from_memory(&png).unwrap();
        assert_eq!((image.width(), image.height()), (W, H));
    }

    #[test]
    fn failed_save_keeps_the_previous_file() {
        let path = temp("keep");
        let (document, pixels) = sample();
        save(&path, &document, &pixels).unwrap();
        let original = std::fs::read(&path).unwrap();

        // A layer callback returning the wrong amount of data fails the save.
        let result = save_document(&path, &document, |_| vec![0; 3], &[255; BYTES]);
        assert!(matches!(result, Err(IoError::BufferSize { .. })));
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let mut temp_name = path.file_name().unwrap().to_os_string();
        temp_name.push(".tmp");
        assert!(!path.with_file_name(temp_name).exists(), "temporary file was left behind");
        std::fs::remove_file(&path).unwrap();
    }

    /// Writes a ZIP with the given entries, for malformed-file tests.
    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut zip = ZipWriter::new(File::create(path).unwrap());
        for (name, data) in entries {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    fn manifest_json(version: u32, width: u32) -> String {
        format!(
            r#"{{"format":"drawpaint","version":{version},"width":{width},"height":{H},
                "background":[1,1,1],"active_layer":1,
                "layers":[{{"id":1,"name":"a","visible":true,"opacity":7.5,"blend":"normal"}}]}}"#
        )
    }

    #[test]
    fn rejects_files_that_are_not_documents() {
        let path = temp("garbage");
        std::fs::write(&path, b"this is not a zip").unwrap();
        assert!(matches!(load_document(&path, 4096), Err(IoError::NotADocument(_))));

        write_zip(&path, &[("hello.txt", b"hi")]);
        assert!(matches!(load_document(&path, 4096), Err(IoError::NotADocument(_))));

        write_zip(&path, &[(MANIFEST_PATH, br#"{"format":"other"}"#)]);
        assert!(matches!(load_document(&path, 4096), Err(IoError::NotADocument(_))));
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn rejects_newer_versions_and_oversized_canvases() {
        let path = temp("limits");
        write_zip(&path, &[(MANIFEST_PATH, manifest_json(99, W).as_bytes())]);
        assert!(matches!(load_document(&path, 4096), Err(IoError::NewerFormat { found: 99, .. })));

        write_zip(&path, &[(MANIFEST_PATH, manifest_json(1, 10_000).as_bytes())]);
        assert!(matches!(load_document(&path, 4096), Err(IoError::TooLarge { width: 10_000, .. })));
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn rejects_missing_or_missized_layer_data() {
        let path = temp("layers");
        let manifest = manifest_json(1, W);
        write_zip(&path, &[(MANIFEST_PATH, manifest.as_bytes())]);
        assert!(matches!(load_document(&path, 4096), Err(IoError::Corrupt(_))));

        for wrong in [BYTES - 1, BYTES + 1] {
            write_zip(&path, &[(MANIFEST_PATH, manifest.as_bytes()), ("layers/1.rgba", &vec![0; wrong])]);
            assert!(matches!(load_document(&path, 4096), Err(IoError::Corrupt(_))), "size {wrong}");
        }

        // The right size loads, and the out-of-range opacity is clamped.
        write_zip(&path, &[(MANIFEST_PATH, manifest.as_bytes()), ("layers/1.rgba", &[0; BYTES])]);
        let loaded = load_document(&path, 4096).unwrap();
        assert_eq!(loaded.document.active_layer().props.opacity, 1.0);
        std::fs::remove_file(&path).unwrap();
    }
}
