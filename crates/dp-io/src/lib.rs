//! File formats: the native `.dpaint` document and flat image import/export.

mod document;
mod image_file;

pub use document::{load_document, save_document, LoadedDocument, DOCUMENT_EXTENSION};
pub use image_file::{export_image, import_image, write_png, ImportedImage};

#[derive(Debug, thiserror::Error)]
pub enum IoError {
    #[error("pixel buffer has {actual} bytes, expected {expected} for {width}x{height} RGBA8")]
    BufferSize { width: u32, height: u32, expected: usize, actual: usize },
    #[error("unsupported file extension; use {supported}")]
    UnsupportedExtension { supported: &'static str },
    #[error("the image is {width}x{height} px, larger than the {max} px this device supports")]
    TooLarge { width: u32, height: u32, max: u32 },
    #[error("not a DrawPaint document: {0}")]
    NotADocument(String),
    #[error(
        "the document was saved by a newer version of DrawPaint (format {found}, supported {supported})"
    )]
    NewerFormat { found: u32, supported: u32 },
    #[error("the document is damaged: {0}")]
    Corrupt(String),
    #[error(transparent)]
    Document(#[from] dp_core::DocumentError),
    #[error(transparent)]
    Image(#[from] image::ImageError),
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

fn check_buffer(width: u32, height: u32, rgba: &[u8]) -> Result<(), IoError> {
    let expected = width as usize * height as usize * 4;
    if rgba.len() == expected {
        Ok(())
    } else {
        Err(IoError::BufferSize { width, height, expected, actual: rgba.len() })
    }
}
