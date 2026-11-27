use std::path::Path;

use crate::{check_buffer, IoError};

/// A decoded image, ready to become a layer.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedImage {
    pub width: u32,
    pub height: u32,
    /// RGBA8 with premultiplied alpha.
    pub pixels: Vec<u8>,
}

/// Writes straight-alpha RGBA8 pixels (row-major, top to bottom) as a PNG.
pub fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), IoError> {
    check_buffer(width, height, rgba)?;
    image::save_buffer(path, rgba, width, height, image::ExtendedColorType::Rgba8)?;
    Ok(())
}

/// Writes an opaque flattened image as PNG or JPEG, chosen by extension.
pub fn export_image(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), IoError> {
    check_buffer(width, height, rgba)?;
    let extension = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("png") => write_png(path, width, height, rgba),
        Some("jpg" | "jpeg") => {
            // JPEG has no alpha channel.
            let rgb: Vec<u8> = rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
            image::save_buffer(path, &rgb, width, height, image::ExtendedColorType::Rgb8)?;
            Ok(())
        }
        _ => Err(IoError::UnsupportedExtension { supported: ".png, .jpg" }),
    }
}

/// Decodes a PNG or JPEG. Images with a side above `max_side` are refused
/// before their pixels are decoded.
pub fn import_image(path: &Path, max_side: u32) -> Result<ImportedImage, IoError> {
    let reader = image::ImageReader::open(path)?.with_guessed_format()?;
    let (width, height) = reader.into_dimensions()?;
    if width == 0 || height == 0 || width > max_side || height > max_side {
        return Err(IoError::TooLarge { width, height, max: max_side });
    }
    let mut pixels = image::ImageReader::open(path)?.with_guessed_format()?.decode()?.into_rgba8().into_raw();
    premultiply(&mut pixels);
    Ok(ImportedImage { width, height, pixels })
}

fn premultiply(rgba: &mut [u8]) {
    for pixel in rgba.chunks_exact_mut(4) {
        let alpha = u32::from(pixel[3]);
        if alpha < 255 {
            for channel in &mut pixel[..3] {
                // Rounded division by 255.
                *channel = ((u32::from(*channel) * alpha + 127) / 255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("dp-io-{}-{name}", std::process::id()))
    }

    #[test]
    fn png_round_trip_premultiplies() {
        let path = temp("round.png");
        let straight = [255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 200, 100, 50, 51];
        write_png(&path, 2, 2, &straight).unwrap();
        let image = import_image(&path, 4096).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(image.pixels, [255, 0, 0, 255, 0, 128, 0, 128, 0, 0, 0, 0, 40, 20, 10, 51]);
    }

    #[test]
    fn jpeg_export_is_readable() {
        let path = temp("flat.jpg");
        let gray = [128u8, 128, 128, 255].repeat(16 * 16);
        export_image(&path, 16, 16, &gray).unwrap();
        let image = import_image(&path, 4096).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!((image.width, image.height), (16, 16));
        assert!(image.pixels.chunks_exact(4).all(|p| p[0].abs_diff(128) <= 2 && p[3] == 255));
    }

    #[test]
    fn rejects_wrong_buffer_size_and_extension() {
        let err = write_png(Path::new("unused.png"), 2, 2, &[0; 15]).unwrap_err();
        assert!(matches!(err, IoError::BufferSize { expected: 16, actual: 15, .. }));
        let err = export_image(Path::new("unused.bmp"), 1, 1, &[0; 4]).unwrap_err();
        assert!(matches!(err, IoError::UnsupportedExtension { .. }));
    }

    #[test]
    fn oversized_images_are_refused() {
        let path = temp("big.png");
        write_png(&path, 8, 2, &[0; 64]).unwrap();
        let err = import_image(&path, 4).unwrap_err();
        std::fs::remove_file(&path).unwrap();
        assert!(matches!(err, IoError::TooLarge { width: 8, height: 2, max: 4 }));
    }
}
