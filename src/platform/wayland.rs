use std::ffi::c_void;
use std::thread;

use crossbeam_channel::{Receiver, Sender};
use egui::Context;
use wayland_client::{
    globals::{registry_queue_init, GlobalListContents},
    protocol::{wl_pointer, wl_registry, wl_seat},
    Connection, Dispatch, QueueHandle,
};
use wayland_protocols::wp::pointer_gestures::zv1::client::{
    zwp_pointer_gesture_pinch_v1::{self, ZwpPointerGesturePinchV1},
    zwp_pointer_gestures_v1::{self, ZwpPointerGesturesV1},
};

pub struct PinchGestureState {
    pub tx: Sender<f64>,
    pub ctx: Context,
    pub last_scale: f64,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for PinchGestureState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for PinchGestureState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_seat::WlSeat,
        _event: wl_seat::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for PinchGestureState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_pointer::WlPointer,
        _event: wl_pointer::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpPointerGesturesV1, ()> for PinchGestureState {
    fn event(
        _state: &mut Self,
        _proxy: &ZwpPointerGesturesV1,
        _event: zwp_pointer_gestures_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpPointerGesturePinchV1, ()> for PinchGestureState {
    fn event(
        state: &mut Self,
        _proxy: &ZwpPointerGesturePinchV1,
        event: zwp_pointer_gesture_pinch_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            zwp_pointer_gesture_pinch_v1::Event::Begin { .. } => {
                state.last_scale = 1.0;
            }
            zwp_pointer_gesture_pinch_v1::Event::Update { scale, .. } => {
                let current_scale = scale;
                if state.last_scale > 0.0 && current_scale > 0.0 {
                    let delta = current_scale / state.last_scale;
                    state.last_scale = current_scale;
                    let _ = state.tx.send(delta);
                    state.ctx.request_repaint();
                }
            }
            zwp_pointer_gesture_pinch_v1::Event::End { .. } => {
                state.last_scale = 1.0;
            }
            _ => {}
        }
    }
}

/// Initializes the background Wayland touchpad pinch gesture listener.
/// Returns a receiver for relative continuous pinch magnification factors.
pub fn init_wayland_pinch_listener(
    display_ptr: *mut c_void,
    ctx: Context,
) -> Option<Receiver<f64>> {
    if display_ptr.is_null() {
        return None;
    }

    let (tx, rx) = crossbeam_channel::unbounded();
    let display_addr = display_ptr as usize;

    // Isolate unsafe foreign display backend initialization (Rule 6)
    // display_ptr comes directly from winit's RawDisplayHandle::Wayland.
    let spawn_res = thread::Builder::new()
        .name("pdfview-wayland-gestures".to_string())
        .spawn(move || {
            let backend = unsafe {
                wayland_client::backend::Backend::from_foreign_display(display_addr as *mut _)
            };
            let conn = Connection::from_backend(backend);

            let (globals, mut queue) = match registry_queue_init::<PinchGestureState>(&conn) {
                Ok(res) => res,
                Err(_) => return,
            };

            let qh = queue.handle();
            let gestures: ZwpPointerGesturesV1 = match globals.bind(&qh, 1..=3, ()) {
                Ok(g) => g,
                Err(_) => return,
            };

            let seat: wl_seat::WlSeat = match globals.bind(&qh, 1..=8, ()) {
                Ok(s) => s,
                Err(_) => return,
            };

            let pointer = seat.get_pointer(&qh, ());
            let _pinch = gestures.get_pinch_gesture(&pointer, &qh, ());

            let mut state = PinchGestureState {
                tx,
                ctx,
                last_scale: 1.0,
            };

            loop {
                if queue.blocking_dispatch(&mut state).is_err() {
                    break;
                }
            }
        });

    if spawn_res.is_ok() {
        Some(rx)
    } else {
        None
    }
}
