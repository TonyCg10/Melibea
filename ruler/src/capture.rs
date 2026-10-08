//! Capturing every output once, through `wlr-screencopy`.
//!
//! This is the only code that speaks the capture protocol. It runs before anything is shown,
//! so nothing of the ruler's own is ever in a capture.

use smithay_client_toolkit::{
    delegate_output, delegate_registry, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shm::{Shm, ShmHandler, raw::RawPool},
};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, WEnum,
    globals::registry_queue_init,
    protocol::{
        wl_buffer::{self, WlBuffer},
        wl_output::{self, WlOutput},
        wl_shm,
    },
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

use crate::image::{Image, Transform};
use crate::units;

pub struct Capture {
    pub name: String,
    pub logical: (i32, i32),
    /// In the output's logical orientation (the output's transform already undone), at the output's scale.
    pub image: Image,
}

impl Capture {
    pub fn scale(&self) -> f64 {
        units::scale(self.image.width(), self.logical.0)
    }
}

struct Frame {
    name: String,
    logical: (i32, i32),
    transform: Transform,
    proxy: ZwlrScreencopyFrameV1,
    /// width, height, stride, format of the shm buffer the compositor asked for.
    shm: Option<(i32, i32, i32, wl_shm::Format)>,
    /// Every buffer option has been described and a copy can be requested.
    described: bool,
    buffer: Option<(RawPool, WlBuffer)>,
    y_invert: bool,
    result: Option<Result<(), String>>,
}

struct CaptureState {
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,
    frames: Vec<Frame>,
}

pub fn capture_all(conn: &Connection) -> Result<Vec<Capture>, String> {
    let (globals, mut queue) = registry_queue_init(conn)
        .map_err(|e| format!("cannot read the compositor's globals: {e}"))?;
    let qh = queue.handle();
    let manager: ZwlrScreencopyManagerV1 = globals
        .bind(&qh, 1..=3, ())
        .map_err(|_| "the compositor does not offer wlr-screencopy".to_owned())?;
    let shm =
        Shm::bind(&globals, &qh).map_err(|_| "the compositor does not offer wl_shm".to_owned())?;

    let mut state = CaptureState {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        shm,
        frames: Vec::new(),
    };
    let wayland = |e: wayland_client::DispatchError| format!("Wayland error while capturing: {e}");
    // The first round trip announces the outputs, the second their names and sizes.
    queue.roundtrip(&mut state).map_err(wayland)?;
    queue.roundtrip(&mut state).map_err(wayland)?;

    let outputs: Vec<(WlOutput, String, (i32, i32), Transform)> = state
        .output_state
        .outputs()
        .filter_map(|output| {
            let info = state.output_state.info(&output)?;
            Some((output, info.name?, info.logical_size?, transform_of(info.transform)))
        })
        .collect();
    if outputs.is_empty() {
        return Err("there are no outputs to capture".to_owned());
    }

    for (index, (output, name, logical, transform)) in outputs.into_iter().enumerate() {
        // 0: without the cursor.
        let frame = manager.capture_output(0, &output, &qh, index);
        state.frames.push(Frame {
            name,
            logical,
            transform,
            proxy: frame,
            shm: None,
            described: false,
            buffer: None,
            y_invert: false,
            result: None,
        });
    }

    while state.frames.iter().any(|frame| frame.result.is_none()) {
        queue.blocking_dispatch(&mut state).map_err(wayland)?;
        for frame in &mut state.frames {
            if frame.result.is_some() || frame.buffer.is_some() || !frame.described {
                continue;
            }
            let Some((width, height, stride, format)) = frame.shm else {
                frame.result = Some(Err(
                    "the compositor offered no buffer format the ruler reads".to_owned(),
                ));
                continue;
            };
            let len = usize::try_from(stride).unwrap_or(0) * usize::try_from(height).unwrap_or(0);
            let mut pool = RawPool::new(len, &state.shm)
                .map_err(|e| format!("cannot create a shm pool: {e}"))?;
            let buffer = pool.create_buffer(0, width, height, stride, format, (), &qh);
            frame.proxy.copy(&buffer);
            frame.buffer = Some((pool, buffer));
        }
    }

    let mut captures = Vec::new();
    for frame in state.frames {
        frame
            .result
            .unwrap_or(Ok(()))
            .map_err(|e| format!("capturing {}: {e}", frame.name))?;
        let (width, height, stride, _) = frame.shm.ok_or("a capture finished without a buffer")?;
        let (mut pool, buffer) = frame.buffer.ok_or("a capture finished without a buffer")?;
        let pixels = pool.mmap().to_vec();
        buffer.destroy();
        drop(pool);
        let image = Image::new(
            u32::try_from(width).map_err(|_| "negative capture width")?,
            u32::try_from(height).map_err(|_| "negative capture height")?,
            u32::try_from(stride).map_err(|_| "negative capture stride")?,
            pixels,
        );
        // Done with the compositor's side of this frame.
        frame.proxy.destroy();
        captures.push(Capture {
            name: frame.name,
            logical: frame.logical,
            image: image.oriented(frame.transform, frame.y_invert),
        });
    }
    Ok(captures)
}

fn transform_of(transform: wl_output::Transform) -> Transform {
    use wl_output::Transform as Wl;
    match transform {
        Wl::_90 => Transform::Rotated90,
        Wl::_180 => Transform::Rotated180,
        Wl::_270 => Transform::Rotated270,
        Wl::Flipped => Transform::Flipped,
        Wl::Flipped90 => Transform::Flipped90,
        Wl::Flipped180 => Transform::Flipped180,
        Wl::Flipped270 => Transform::Flipped270,
        _ => Transform::Normal,
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for CaptureState {
    fn event(
        _: &mut Self,
        _: &ZwlrScreencopyManagerV1,
        _: <ZwlrScreencopyManagerV1 as Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlBuffer, ()> for CaptureState {
    fn event(
        _: &mut Self,
        _: &WlBuffer,
        _: wl_buffer::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, usize> for CaptureState {
    fn event(
        state: &mut Self,
        proxy: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        index: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_screencopy_frame_v1::Event;
        let frame = &mut state.frames[*index];
        match event {
            Event::Buffer {
                format,
                width,
                height,
                stride,
            } => {
                if let WEnum::Value(format @ (wl_shm::Format::Xrgb8888 | wl_shm::Format::Argb8888)) =
                    format
                    && let (Ok(w), Ok(h), Ok(s)) = (
                        i32::try_from(width),
                        i32::try_from(height),
                        i32::try_from(stride),
                    )
                {
                    frame.shm.get_or_insert((w, h, s, format));
                }
                // Before version 3 there is no buffer_done: the one buffer event is all there is.
                if proxy.version() < 3 {
                    frame.described = true;
                }
            }
            Event::BufferDone => frame.described = true,
            Event::Flags {
                flags: WEnum::Value(flags),
            } => frame.y_invert = flags.contains(zwlr_screencopy_frame_v1::Flags::YInvert),
            Event::Ready { .. } => frame.result = Some(Ok(())),
            Event::Failed => {
                frame.result = Some(Err("the compositor refused the capture".to_owned()));
            }
            _ => {}
        }
    }
}

impl OutputHandler for CaptureState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}
}

impl ShmHandler for CaptureState {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for CaptureState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

delegate_output!(CaptureState);
delegate_shm!(CaptureState);
delegate_registry!(CaptureState);
