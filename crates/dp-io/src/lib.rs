//! File formats. Phase 0 only writes PNG; the native format, PSD and ORA
//! arrive in later phases.

use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum IoError {
    #[error("pixel buffer has {actual} bytes, expected {expected} for {width}x{height} RGBA8")]
    BufferSize { width: u32, height: u32, expected: usize, actual: usize },
    #[error(transparent)]
    Image(#[from] image::ImageError),
}

/// Writes straight-alpha RGBA8 pixels (row-major, top to bottom) as a PNG.
pub fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), IoError> {
    let expected = width as usize * height as usize * 4;
    if rgba.len() != expected {
        return Err(IoError::BufferSize { width, height, expected, actual: rgba.len() });
    }
    image::save_buffer(path, rgba, width, height, image::ExtendedColorType::Rgba8)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_round_trip() {
        let path = std::env::temp_dir().join(format!("dp-io-test-{}.png", std::process::id()));
        let pixels = [255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 9, 8, 7, 6];
        write_png(&path, 2, 2, &pixels).unwrap();
        let back = image::open(&path).unwrap().into_rgba8();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(back.dimensions(), (2, 2));
        assert_eq!(back.as_raw().as_slice(), &pixels);
    }

    #[test]
    fn rejects_wrong_buffer_size() {
        let err = write_png(Path::new("unused.png"), 2, 2, &[0; 15]).unwrap_err();
        assert!(matches!(err, IoError::BufferSize { expected: 16, actual: 15, .. }));
    }
}
