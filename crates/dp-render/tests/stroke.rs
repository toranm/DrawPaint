//! Renders a stroke on a real (or software) GPU and checks the pixels.

use dp_brush::{BrushParams, StrokeBuilder};
use dp_render::{Canvas, DabRenderer};
use glam::vec2;

const SIZE: u32 = 256;

/// Returns `None` when the machine has no usable graphics adapter.
///
/// Set `DP_REQUIRE_GPU=1` (as CI does) to turn that into a failure instead of
/// a silently skipped test.
fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        force_fallback_adapter: std::env::var_os("DP_FORCE_SOFTWARE_GPU").is_some(),
        ..Default::default()
    }));
    let adapter = match adapter {
        Ok(adapter) => adapter,
        Err(error) => {
            assert!(std::env::var_os("DP_REQUIRE_GPU").is_none(), "no graphics adapter: {error}");
            eprintln!("skipping: no graphics adapter ({error})");
            return None;
        }
    };
    eprintln!("adapter: {:?}", adapter.get_info());
    Some(pollster::block_on(adapter.request_device(&Default::default())).expect("create device"))
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE + x) * 4) as usize;
    pixels[i..i + 4].try_into().unwrap()
}

#[test]
fn pressure_stroke_reaches_the_canvas() {
    let Some((device, queue)) = device() else { return };
    let canvas = Canvas::new(&device, SIZE, SIZE);
    let mut renderer = DabRenderer::new(&device);

    // A horizontal stroke whose pressure rises from 0 to 1.
    let params = BrushParams { size: 40.0, min_size_ratio: 0.05, hardness: 0.9, ..Default::default() };
    let mut dabs = Vec::new();
    let mut stroke = StrokeBuilder::begin(params, vec2(28.0, 128.0), 0.0, &mut dabs);
    for i in 1..=100 {
        let t = i as f32 / 100.0;
        stroke.extend(vec2(28.0 + 200.0 * t, 128.0), t, &mut dabs);
    }
    renderer.queue(&dabs, [0.0, 0.0, 1.0]);

    let mut encoder = device.create_command_encoder(&Default::default());
    canvas.clear(&mut encoder, [1.0; 4]);
    renderer.flush(&device, &queue, &mut encoder, &canvas);
    queue.submit([encoder.finish()]);
    assert!(!renderer.has_pending());

    let pixels = canvas.read_pixels(&device, &queue);
    assert_eq!(pixels.len(), (SIZE * SIZE * 4) as usize);
    if let Some(dir) = std::env::var_os("DP_TEST_OUTPUT") {
        let path = std::path::Path::new(&dir).join("stroke.png");
        dp_io::write_png(&path, SIZE, SIZE, &pixels).unwrap();
    }

    let white = [255, 255, 255, 255];
    let blue = [0, 0, 255, 255];
    // Untouched paper.
    assert_eq!(pixel(&pixels, 5, 5), white);
    assert_eq!(pixel(&pixels, 128, 20), white);
    // The stroke centre line is fully covered at the thick end.
    assert_eq!(pixel(&pixels, 200, 128), blue);
    // Pressure made the stroke thicker towards the right: 12 px off the
    // centre line is paper at the thin end and ink at the thick end.
    assert_eq!(pixel(&pixels, 60, 140), white);
    assert_eq!(pixel(&pixels, 220, 140), blue);
    // The edge is antialiased: somewhere across it there is a blend.
    let column: Vec<[u8; 4]> = (128..160).map(|y| pixel(&pixels, 200, y)).collect();
    assert!(column.iter().any(|p| *p != white && *p != blue), "no antialiased edge: {column:?}");
}
