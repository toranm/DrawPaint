//! Renders on a real (or software) GPU and checks the resulting pixels.

use dp_brush::{BrushParams, Dab, StrokeBuilder};
use dp_core::{BlendMode, Document, Edit, History, LayerId, LayerProps, Rect, Snapshot};
use dp_render::{CanvasRenderer, StrokeStyle, CANVAS_FORMAT};
use glam::vec2;

const SIZE: u32 = 256;
const BLUE: StrokeStyle = StrokeStyle { color: [0.0, 0.0, 1.0], opacity: 1.0, erase: false };

/// Returns `None` when the machine has no usable graphics adapter.
///
/// Set `DP_REQUIRE_GPU=1` (as CI does) to turn that into a failure instead of
/// a silently skipped test.
fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()));
    let adapter = match adapter {
        Ok(adapter) => adapter,
        Err(error) => {
            assert!(std::env::var_os("DP_REQUIRE_GPU").is_none(), "no graphics adapter: {error}");
            eprintln!("skipping: no graphics adapter ({error})");
            return None;
        }
    };
    Some(pollster::block_on(adapter.request_device(&Default::default())).expect("create device"))
}

fn setup() -> Option<(Document, CanvasRenderer)> {
    let (device, queue) = device()?;
    let document = Document::new(SIZE, SIZE).unwrap();
    let renderer = CanvasRenderer::new(&device, &queue, CANVAS_FORMAT, &document);
    Some((document, renderer))
}

fn pixel(pixels: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * SIZE + x) * 4) as usize;
    pixels[i..i + 4].try_into().unwrap()
}

fn close(actual: [u8; 4], expected: [u8; 4]) -> bool {
    actual.iter().zip(expected).all(|(a, e)| a.abs_diff(e) <= 2)
}

fn full() -> Rect {
    Rect::from_size(SIZE, SIZE)
}

fn bounds(dabs: &[Dab]) -> Rect {
    dabs.iter()
        .fold(Rect::EMPTY, |rect, dab| rect.union(Rect::around_circle(dab.center, dab.radius + 1.0, full())))
}

fn save(name: &str, pixels: &[u8]) {
    if let Some(dir) = std::env::var_os("DP_TEST_OUTPUT") {
        dp_io::write_png(&std::path::Path::new(&dir).join(name), SIZE, SIZE, pixels).unwrap();
    }
}

fn solid(color: [u8; 4]) -> Vec<u8> {
    color.repeat((SIZE * SIZE) as usize)
}

/// Draws a straight stroke and commits it to `layer`.
fn stroke(renderer: &mut CanvasRenderer, layer: LayerId, params: BrushParams, points: &[(f32, f32, f32)]) {
    let mut dabs = Vec::new();
    let (x, y, pressure) = points[0];
    let mut builder = StrokeBuilder::begin(params, vec2(x, y), pressure, &mut dabs);
    for window in points.windows(2) {
        let ((x0, y0, p0), (x1, y1, p1)) = (window[0], window[1]);
        for i in 1..=50 {
            let t = i as f32 / 50.0;
            builder.extend(vec2(x0 + (x1 - x0) * t, y0 + (y1 - y0) * t), p0 + (p1 - p0) * t, &mut dabs);
        }
    }
    builder.finish(&mut dabs);
    renderer.begin_stroke();
    renderer.queue_dabs(&dabs);
    let style = StrokeStyle { opacity: params.opacity, erase: params.erase, ..BLUE };
    renderer.commit_stroke(layer, bounds(&dabs), style);
}

#[test]
fn pressure_stroke_reaches_the_layer_and_the_composite() {
    let Some((document, mut renderer)) = setup() else { return };
    let layer = document.active();
    let params = BrushParams { size: 40.0, min_size_ratio: 0.05, hardness: 0.9, ..Default::default() };
    stroke(&mut renderer, layer, params, &[(28.0, 128.0, 0.0), (228.0, 128.0, 1.0)]);

    // The layer itself is transparent except for the stroke.
    let pixels = renderer.pixels().read_rect(layer, full());
    assert_eq!(pixel(&pixels, 5, 5), [0, 0, 0, 0]);
    assert_eq!(pixel(&pixels, 200, 128), [0, 0, 255, 255]);

    let flat = renderer.flatten(&document);
    save("stroke.png", &flat);
    let white = [255, 255, 255, 255];
    let blue = [0, 0, 255, 255];
    assert_eq!(pixel(&flat, 5, 5), white);
    assert_eq!(pixel(&flat, 200, 128), blue);
    // Pressure made the stroke thicker towards the right: 12 px off the
    // centre line is paper at the thin end and ink at the thick end.
    assert_eq!(pixel(&flat, 60, 140), white);
    assert_eq!(pixel(&flat, 220, 140), blue);
    // The edge is antialiased: somewhere across it there is a blend.
    let column: Vec<[u8; 4]> = (128..160).map(|y| pixel(&flat, 200, y)).collect();
    assert!(column.iter().any(|p| *p != white && *p != blue), "no antialiased edge: {column:?}");
}

#[test]
fn stroke_opacity_caps_self_overlap() {
    let Some((document, mut renderer)) = setup() else { return };
    let layer = document.active();
    let params = BrushParams {
        size: 30.0,
        opacity: 0.5,
        hardness: 1.0,
        pressure_affects_size: false,
        ..Default::default()
    };
    // An X: the two arms cross at the centre within a single stroke.
    stroke(
        &mut renderer,
        layer,
        params,
        &[(40.0, 40.0, 1.0), (216.0, 216.0, 1.0), (216.0, 40.0, 1.0), (40.0, 216.0, 1.0)],
    );
    let pixels = renderer.pixels().read_rect(layer, full());
    save("overlap.png", &renderer.flatten(&document));
    let half = [0, 0, 128, 128];
    assert!(close(pixel(&pixels, 80, 80), half), "arm: {:?}", pixel(&pixels, 80, 80));
    assert!(close(pixel(&pixels, 128, 128), half), "crossing: {:?}", pixel(&pixels, 128, 128));
}

#[test]
fn low_flow_builds_up_to_full_coverage() {
    let Some((document, mut renderer)) = setup() else { return };
    let layer = document.active();
    let params = BrushParams {
        size: 40.0,
        flow: 0.02,
        spacing: 0.01,
        hardness: 1.0,
        pressure_affects_size: false,
        ..Default::default()
    };
    stroke(&mut renderer, layer, params, &[(60.0, 128.0, 1.0), (200.0, 128.0, 1.0)]);
    let pixels = renderer.pixels().read_rect(layer, full());
    // 100 overlapping dabs at 2 % reach 1 - 0.98^100 = 87 % in the middle.
    let alpha = pixel(&pixels, 128, 128)[3];
    assert!((216..=226).contains(&alpha), "alpha {alpha}");
}

#[test]
fn eraser_removes_paint() {
    let Some((document, mut renderer)) = setup() else { return };
    let layer = document.active();
    renderer.pixels().write_rect(layer, full(), &solid([255, 0, 0, 255]));
    let params = BrushParams {
        size: 30.0,
        hardness: 1.0,
        pressure_affects_size: false,
        erase: true,
        ..Default::default()
    };
    stroke(&mut renderer, layer, params, &[(40.0, 128.0, 1.0), (216.0, 128.0, 1.0)]);
    let pixels = renderer.pixels().read_rect(layer, full());
    assert_eq!(pixel(&pixels, 128, 128), [0, 0, 0, 0]);
    assert_eq!(pixel(&pixels, 128, 60), [255, 0, 0, 255]);
    // The paper shows through the erased band.
    let flat = renderer.flatten(&document);
    save("eraser.png", &flat);
    assert_eq!(pixel(&flat, 128, 128), [255, 255, 255, 255]);
}

#[test]
fn stroke_undo_and_redo_restore_pixels() {
    let Some((mut document, mut renderer)) = setup() else { return };
    let layer = document.active();
    let mut history = History::new(usize::MAX);

    let mut dabs = Vec::new();
    let params = BrushParams { size: 20.0, ..Default::default() };
    let mut builder = StrokeBuilder::begin(params, vec2(50.0, 50.0), 1.0, &mut dabs);
    builder.extend(vec2(150.0, 90.0), 1.0, &mut dabs);
    let rect = bounds(&dabs);
    renderer.begin_stroke();
    renderer.queue_dabs(&dabs);
    let before = renderer.commit_stroke(layer, rect, BLUE);
    history.record(Edit::Pixels { layer, snapshot: Snapshot::new(rect, &before) });

    let painted = renderer.pixels().read_rect(layer, full());
    assert_eq!(pixel(&painted, 100, 70), [0, 0, 255, 255]);

    assert!(history.undo(&mut document, renderer.pixels()));
    let undone = renderer.pixels().read_rect(layer, full());
    assert!(undone.iter().all(|&byte| byte == 0), "undo left paint behind");

    assert!(history.redo(&mut document, renderer.pixels()));
    assert_eq!(renderer.pixels().read_rect(layer, full()), painted);
}

#[test]
fn sub_rectangle_round_trip() {
    let Some((document, mut renderer)) = setup() else { return };
    let layer = document.active();
    // An odd width exercises the row padding of GPU copies.
    let rect = Rect::new(17, 33, 7, 5);
    let data: Vec<u8> = (0..rect.byte_len()).map(|i| (i * 7 % 256) as u8).collect();
    renderer.pixels().write_rect(layer, rect, &data);
    assert_eq!(renderer.pixels().read_rect(layer, rect), data);
    let all = renderer.pixels().read_rect(layer, full());
    assert_eq!(pixel(&all, 16, 33), [0, 0, 0, 0]);
    assert_eq!(pixel(&all, 17, 33), [data[0], data[1], data[2], data[3]]);
}

#[test]
fn blend_modes_and_layer_properties() {
    let Some((mut document, mut renderer)) = setup() else { return };
    let mut history = History::new(usize::MAX);
    let bottom = document.active();
    let top = document.new_layer();
    let top_id = top.id;
    assert!(history.perform(
        Edit::InsertLayer { index: 1, layer: top, pixels: None },
        &mut document,
        renderer.pixels()
    ));
    // Opaque mid-tones, premultiplied.
    renderer.pixels().write_rect(bottom, full(), &solid([204, 102, 51, 255]));
    renderer.pixels().write_rect(top_id, full(), &solid([128, 128, 255, 255]));

    let mut composite = |blend: BlendMode, opacity: f32, visible: bool| {
        let props = LayerProps { blend, opacity, visible, ..document.layer(top_id).unwrap().props.clone() };
        assert!(history.perform_props(top_id, props, &mut document));
        pixel(&renderer.flatten(&document), 10, 10)
    };
    let cases = [
        (BlendMode::Normal, [128, 128, 255, 255]),
        (BlendMode::Multiply, [102, 51, 51, 255]),
        (BlendMode::Screen, [230, 179, 255, 255]),
        // Bottom red is light (screen-like), green and blue are dark (multiply-like).
        (BlendMode::Overlay, [204, 102, 102, 255]),
        (BlendMode::Add, [255, 230, 255, 255]),
    ];
    for (blend, expected) in cases {
        let actual = composite(blend, 1.0, true);
        assert!(close(actual, expected), "{blend:?}: {actual:?} != {expected:?}");
    }
    // Half opacity mixes halfway; a hidden layer shows only what is below.
    let half = composite(BlendMode::Normal, 0.5, true);
    assert!(close(half, [166, 115, 153, 255]), "half opacity: {half:?}");
    assert_eq!(composite(BlendMode::Normal, 1.0, false), [204, 102, 51, 255]);
}

#[test]
fn translucent_layers_composite_over_paper() {
    let Some((document, mut renderer)) = setup() else { return };
    let layer = document.active();
    // 50 % red, premultiplied.
    renderer.pixels().write_rect(layer, full(), &solid([128, 0, 0, 128]));
    let flat = renderer.flatten(&document);
    assert!(close(pixel(&flat, 3, 3), [255, 127, 127, 255]), "{:?}", pixel(&flat, 3, 3));
}
