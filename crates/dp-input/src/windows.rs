//! Windows pen backend built on `WM_POINTER` messages.
//!
//! The window procedure is subclassed to observe pen messages before the
//! windowing library does. Messages are always forwarded afterwards, so UI
//! interaction through the windowing library keeps working.

use std::cell::RefCell;
use std::rc::Rc;

use glam::Vec2;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::System::Performance::QueryPerformanceFrequency;
use windows_sys::Win32::UI::Input::Pointer::{
    GetPointerDeviceRects, GetPointerPenInfo, GetPointerPenInfoHistory, GetPointerType,
    POINTER_FLAG_INCONTACT, POINTER_PEN_INFO,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    PEN_FLAG_ERASER, PEN_FLAG_INVERTED, PEN_MASK_PRESSURE, POINTER_INPUT_TYPE, PT_PEN, WM_NCDESTROY,
    WM_POINTERDOWN, WM_POINTERUP, WM_POINTERUPDATE,
};

use crate::pen::{digitizer_to_screen, ContactTracker, Edges};
use crate::{DeviceKind, PointerSample};

/// Identifies our subclass among any others installed on the window.
const SUBCLASS_ID: usize = 0x4450_5045; // "DPPE"
/// Pressure reported by Windows is normalized to this range.
const MAX_PRESSURE: f32 = 1024.0;
/// History entries read per message. A 300 Hz tablet at 30 fps produces 10.
const HISTORY_CAPACITY: usize = 64;
/// Largest plausible gap between the high-resolution position and the
/// integer pixel position. Beyond it the digitizer mapping is distrusted.
const MAX_SUBPIXEL_DEVIATION: f32 = 2.0;

struct State {
    samples: Vec<PointerSample>,
    contact: ContactTracker,
    seconds_per_tick: f64,
}

pub(crate) struct Hook {
    hwnd: HWND,
    state: Rc<RefCell<State>>,
}

impl Hook {
    pub(crate) fn install(window: &impl HasWindowHandle) -> Option<Self> {
        let RawWindowHandle::Win32(handle) = window.window_handle().ok()?.as_raw() else {
            return None;
        };
        let hwnd = handle.hwnd.get() as HWND;

        let mut frequency = 0i64;
        // SAFETY: `frequency` is a valid out pointer.
        unsafe { QueryPerformanceFrequency(&mut frequency) };
        let state = Rc::new(RefCell::new(State {
            samples: Vec::new(),
            contact: ContactTracker::default(),
            seconds_per_tick: 1.0 / frequency.max(1) as f64,
        }));

        // The subclass owns one strong reference, released in `release_ref`.
        let ref_data = Rc::into_raw(Rc::clone(&state)) as usize;
        // SAFETY: `hwnd` is a live window owned by this thread and `ref_data`
        // stays valid until the subclass is removed.
        let installed = unsafe { SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, ref_data) };
        if installed == 0 {
            // SAFETY: the subclass was not installed, so the reference is ours.
            unsafe { release_ref(ref_data) };
            return None;
        }
        Some(Self { hwnd, state })
    }

    pub(crate) fn drain(&self, out: &mut Vec<PointerSample>) {
        out.append(&mut self.state.borrow_mut().samples);
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        // SAFETY: removing a subclass is valid for any window handle; it
        // fails harmlessly if the window or the subclass is already gone.
        let removed = unsafe { RemoveWindowSubclass(self.hwnd, Some(subclass_proc), SUBCLASS_ID) };
        if removed != 0 {
            // SAFETY: the subclass was still installed, so its reference has
            // not been released by `WM_NCDESTROY`.
            unsafe { release_ref(Rc::as_ptr(&self.state) as usize) };
        }
    }
}

/// Releases the strong reference held by the subclass.
///
/// # Safety
/// `ref_data` must come from `Rc::into_raw` on an `Rc<RefCell<State>>` and
/// must be released exactly once.
unsafe fn release_ref(ref_data: usize) {
    drop(Rc::from_raw(ref_data as *const RefCell<State>));
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    ref_data: usize,
) -> LRESULT {
    match msg {
        WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP => {
            // SAFETY: `ref_data` is the pointer registered in `install`, alive
            // while the subclass is installed. Only shared access is taken.
            let state = &*(ref_data as *const RefCell<State>);
            // A re-entrant message while samples are being drained is skipped
            // rather than allowed to panic across the FFI boundary.
            if let Ok(mut state) = state.try_borrow_mut() {
                let pointer_id = (wparam & 0xFFFF) as u32;
                read_pen(hwnd, pointer_id, &mut state);
            }
        }
        WM_NCDESTROY => {
            RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID);
            release_ref(ref_data);
        }
        _ => {}
    }
    DefSubclassProc(hwnd, msg, wparam, lparam)
}

/// Appends every pen sample attached to the current message.
unsafe fn read_pen(hwnd: HWND, pointer_id: u32, state: &mut State) {
    let mut pointer_type: POINTER_INPUT_TYPE = 0;
    if GetPointerType(pointer_id, &mut pointer_type) == 0 || pointer_type != PT_PEN {
        return;
    }

    let mut infos: [POINTER_PEN_INFO; HISTORY_CAPACITY] = std::mem::zeroed();
    let mut count = HISTORY_CAPACITY as u32;
    if GetPointerPenInfoHistory(pointer_id, &mut count, infos.as_mut_ptr()) == 0 {
        if GetPointerPenInfo(pointer_id, infos.as_mut_ptr()) == 0 {
            return;
        }
        count = 1;
    }
    // `count` reports how many entries exist, which may exceed the buffer.
    let count = (count as usize).min(HISTORY_CAPACITY);
    if count == 0 {
        return;
    }

    let mut client_origin = POINT { x: 0, y: 0 };
    ClientToScreen(hwnd, &mut client_origin);
    let client_origin = Vec2::new(client_origin.x as f32, client_origin.y as f32);

    let empty = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let (mut device_rect, mut display_rect) = (empty, empty);
    let source = infos[0].pointerInfo.sourceDevice;
    let rects = (GetPointerDeviceRects(source, &mut device_rect, &mut display_rect) != 0)
        .then(|| (edges(device_rect), edges(display_rect)));

    // History is delivered newest first.
    for info in infos[..count].iter().rev() {
        let pointer = &info.pointerInfo;
        let pixel = Vec2::new(pointer.ptPixelLocationRaw.x as f32, pointer.ptPixelLocationRaw.y as f32);
        let himetric =
            Vec2::new(pointer.ptHimetricLocationRaw.x as f32, pointer.ptHimetricLocationRaw.y as f32);
        let screen = rects
            .and_then(|(device, display)| digitizer_to_screen(himetric, device, display))
            .filter(|precise| (*precise - pixel).abs().max_element() <= MAX_SUBPIXEL_DEVIATION)
            .unwrap_or(pixel);

        let contact = pointer.pointerFlags & POINTER_FLAG_INCONTACT != 0;
        let erasing = info.penFlags & (PEN_FLAG_ERASER | PEN_FLAG_INVERTED) != 0;
        let pressure = if !contact {
            0.0
        } else if info.penMask & PEN_MASK_PRESSURE != 0 {
            (info.pressure as f32 / MAX_PRESSURE).clamp(0.0, 1.0)
        } else {
            // Pens without a pressure sensor draw at full pressure.
            1.0
        };
        state.samples.push(PointerSample {
            device: if erasing { DeviceKind::Eraser } else { DeviceKind::Pen },
            phase: state.contact.update(contact),
            pos: screen - client_origin,
            pressure,
            tilt: Vec2::new(info.tiltX as f32, info.tiltY as f32),
            twist: info.rotation as f32,
            time: pointer.PerformanceCount as f64 * state.seconds_per_tick,
        });
    }
}

fn edges(rect: RECT) -> Edges {
    Edges {
        left: rect.left as f32,
        top: rect.top as f32,
        right: rect.right as f32,
        bottom: rect.bottom as f32,
    }
}
