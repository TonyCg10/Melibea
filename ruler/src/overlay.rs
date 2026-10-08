//! The interactive ruler: one full-screen layer surface per output showing its frozen capture,
//! with the guides for the measurement under the pointer drawn on top.

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, delegate_shm, delegate_simple,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState, SimpleGlobal},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers},
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use wayland_client::{
    Connection, Dispatch, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
};
use wayland_protocols::wp::viewporter::client::{
    wp_viewport::{self, WpViewport},
    wp_viewporter::WpViewporter,
};

use crate::capture::Capture;
use crate::cli::Mode;
use crate::draw::{self, Canvas, Guides, Rect};
use crate::edges::{self, Bounds, Region};
use crate::image::Image;
use crate::text::{self, Label, Text};
use crate::units;

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const TOLERANCE_STEP: u8 = 5;

pub enum Outcome {
    Copy(String),
    Cancel,
    Fail(String),
}

struct Surface {
    capture: Capture,
    layer: LayerSurface,
    viewport: WpViewport,
    configured: bool,
    /// A buffer has been committed: until then, the first one is damaged whole.
    shown: bool,
    frame: Frame,
    /// What the last frame drew over the picture: damaged again on the next one.
    drawn: Option<Rect>,
}

/// Frame-callback pacing: a new buffer is committed only once the last one has been shown.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Frame {
    Idle,
    Pending,
    /// Pending, and something changed meanwhile: redraw when it is done.
    PendingDirty,
}

impl Drop for Surface {
    fn drop(&mut self) {
        // Before `layer`, which destroys the `wl_surface` when it drops.
        self.viewport.destroy();
    }
}

struct Overlay {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    compositor: CompositorState,
    shm: Shm,
    viewporter: SimpleGlobal<WpViewporter, 1>,
    pool: SlotPool,
    surfaces: Vec<Surface>,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    /// The surface under the pointer and the physical pixel it points at.
    hover: Option<(usize, (u32, u32))>,
    mode: Mode,
    tolerance: u8,
    /// The last filled region, kept while the pointer stays inside it.
    cached: Option<(usize, u8, Region)>,
    text: Text,
    outcome: Option<Outcome>,
    /// Set once the surfaces exist: from then on, any output change ends the ruler.
    ready: bool,
}

pub fn run(conn: &Connection, captures: Vec<Capture>, tolerance: u8, text: Text) -> Outcome {
    match setup_and_run(conn, captures, tolerance, text) {
        Ok(outcome) => outcome,
        Err(message) => Outcome::Fail(message),
    }
}

fn setup_and_run(
    conn: &Connection,
    captures: Vec<Capture>,
    tolerance: u8,
    text: Text,
) -> Result<Outcome, String> {
    let (globals, mut queue) = registry_queue_init(conn)
        .map_err(|e| format!("cannot read the compositor's globals: {e}"))?;
    let qh = queue.handle();
    let wayland = |e: wayland_client::DispatchError| format!("Wayland error: {e}");

    let compositor = CompositorState::bind(&globals, &qh)
        .map_err(|_| "the compositor does not offer wl_compositor")?;
    let layer_shell =
        LayerShell::bind(&globals, &qh).map_err(|_| "the compositor does not offer layer shell")?;
    let shm = Shm::bind(&globals, &qh).map_err(|_| "the compositor does not offer wl_shm")?;
    let viewporter = SimpleGlobal::<WpViewporter, 1>::bind(&globals, &qh)
        .map_err(|_| "the compositor does not offer wp_viewporter")?;
    let largest = captures
        .iter()
        .map(|c| c.image.stride() as usize * c.image.height() as usize)
        .max()
        .unwrap_or(4096);
    // Room for the frame on screen and the next one; the pool grows if more is needed.
    let pool =
        SlotPool::new(largest * 2, &shm).map_err(|e| format!("cannot create a shm pool: {e}"))?;

    let mut state = Overlay {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        shm,
        viewporter,
        pool,
        surfaces: Vec::new(),
        pointer: None,
        keyboard: None,
        hover: None,
        mode: Mode::Box,
        tolerance,
        cached: None,
        text,
        outcome: None,
        ready: false,
    };
    // The first round trip announces the outputs and seats, the second their details.
    queue.roundtrip(&mut state).map_err(wayland)?;
    queue.roundtrip(&mut state).map_err(wayland)?;

    for capture in captures {
        let output = state
            .output_state
            .outputs()
            .find(|o| {
                state.output_state.info(o).and_then(|i| i.name).as_deref()
                    == Some(capture.name.as_str())
            })
            .ok_or_else(|| {
                format!(
                    "output {} disappeared before the ruler opened",
                    capture.name
                )
            })?;
        let surface = state.compositor.create_surface(&qh);
        let viewport = state
            .viewporter
            .get()
            .map_err(|_| "the compositor does not offer wp_viewporter")?
            .get_viewport(&surface, &qh, ());
        let layer = layer_shell.create_layer_surface(
            &qh,
            surface,
            Layer::Overlay,
            Some("melibea-ruler"),
            Some(&output),
        );
        layer.set_anchor(Anchor::all());
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer.set_size(0, 0);
        layer.commit();
        state.surfaces.push(Surface {
            capture,
            layer,
            viewport,
            configured: false,
            shown: false,
            frame: Frame::Idle,
            drawn: None,
        });
    }
    state.ready = true;

    while state.outcome.is_none() {
        queue.blocking_dispatch(&mut state).map_err(wayland)?;
    }
    let outcome = state.outcome.take().unwrap_or(Outcome::Cancel);
    // Take the surfaces down now, before the result is copied, rather than at exit.
    drop(state);
    let _ = conn.flush();
    Ok(outcome)
}

impl Overlay {
    fn surface_index(&self, surface: &wl_surface::WlSurface) -> Option<usize> {
        self.surfaces
            .iter()
            .position(|s| s.layer.wl_surface() == surface)
    }

    fn physical(&self, index: usize, position: (f64, f64)) -> (u32, u32) {
        let capture = &self.surfaces[index].capture;
        let scale = capture.scale();
        (
            units::to_physical(position.0, scale, capture.image.width()),
            units::to_physical(position.1, scale, capture.image.height()),
        )
    }

    fn bounds(&mut self, index: usize, at: (u32, u32)) -> Bounds {
        let image = &self.surfaces[index].capture.image;
        match self.mode {
            Mode::Spacing => edges::spacing(image, at.0, at.1, self.tolerance),
            Mode::Box => {
                if let Some((cached_index, cached_tolerance, region)) = &self.cached
                    && *cached_index == index
                    && *cached_tolerance == self.tolerance
                    && region.contains(at.0, at.1)
                {
                    return region.bounds();
                }
                let region = edges::region(image, at.0, at.1, self.tolerance);
                let bounds = region.bounds();
                self.cached = Some((index, self.tolerance, region));
                bounds
            }
        }
    }

    /// The text to copy for what is under the pointer now.
    fn current_result(&mut self) -> Option<String> {
        let (index, at) = self.hover?;
        let bounds = self.bounds(index, at);
        let scale = self.surfaces[index].capture.scale();
        Some(units::clipboard(
            units::to_logical(bounds.width(), scale),
            units::to_logical(bounds.height(), scale),
        ))
    }

    /// Moves the pointer's pixel, redrawing the surface it left and the one it is on.
    fn set_hover(&mut self, hover: Option<(usize, (u32, u32))>, qh: &QueueHandle<Self>) {
        let previous = std::mem::replace(&mut self.hover, hover);
        if previous == hover {
            return;
        }
        if let Some((old, _)) = previous
            && hover.is_none_or(|(new, _)| new != old)
        {
            self.redraw(old, qh);
        }
        if let Some((new, _)) = hover {
            self.redraw(new, qh);
        }
    }

    /// Shows the surface's current state, or marks it to be shown once its frame is done.
    fn redraw(&mut self, index: usize, qh: &QueueHandle<Self>) {
        {
            let surface = &mut self.surfaces[index];
            if !surface.configured {
                return;
            }
            if surface.frame != Frame::Idle {
                surface.frame = Frame::PendingDirty;
                return;
            }
        }
        let hover_here = self.hover.filter(|(i, _)| *i == index).map(|(_, at)| at);
        // Before `paint` borrows the pool and the surface, which `bounds` would conflict with.
        let measured = hover_here.map(|at| (at, self.bounds(index, at)));
        if let Err(message) = self.paint(index, measured, qh) {
            self.finish(Outcome::Fail(message));
        }
    }

    #[allow(clippy::cast_possible_truncation, reason = "scales are small")]
    #[allow(
        clippy::cast_possible_wrap,
        reason = "output sizes are far below i32::MAX"
    )]
    fn paint(
        &mut self,
        index: usize,
        measured: Option<((u32, u32), Bounds)>,
        qh: &QueueHandle<Self>,
    ) -> Result<(), String> {
        let Self {
            pool,
            surfaces,
            text,
            mode,
            tolerance,
            ..
        } = self;
        let (mode, tolerance) = (*mode, *tolerance);
        let surface = &mut surfaces[index];
        let image = &surface.capture.image;
        let (width, height, stride) = (
            image.width() as i32,
            image.height() as i32,
            image.stride() as i32,
        );
        let (buffer, canvas) = pool
            .create_buffer(width, height, stride, wl_shm::Format::Xrgb8888)
            .map_err(|e| format!("cannot allocate a frame for {}: {e}", surface.capture.name))?;
        copy_frame(canvas, image);

        let mut drawn = None;
        if let Some((at, bounds)) = measured {
            let scale = surface.capture.scale();
            let px = scale.round().max(1.0) as i32;
            let mut c = Canvas::new(canvas, image.width(), image.height(), image.stride());
            let at_i = (at.0 as i32, at.1 as i32);
            let mut guides = Guides::new(px);
            match mode {
                Mode::Box => guides.boxed(bounds),
                Mode::Spacing => guides.spacing(bounds, at_i),
            }
            guides.crosshair(at_i);
            let lines = guides.paint(&mut c);
            let main = units::label(
                units::to_logical(bounds.width(), scale),
                units::to_logical(bounds.height(), scale),
            );
            let detail = format!(
                "{} · tol {tolerance} · {}",
                mode.name(),
                surface.capture.name
            );
            let label = Label {
                main: &main,
                detail: &detail,
            };
            let size = text::label_size(text, &label, scale);
            let preferred = match mode {
                Mode::Box => (
                    bounds.x0 as i32 - px,
                    bounds.y0 as i32 - px - size.1 - 4 * px,
                ),
                Mode::Spacing => (at_i.0 + 12 * px, at_i.1 + 12 * px),
            };
            let place = draw::place_label(size, preferred, c.size());
            let label_rect = text::draw_label(text, &mut c, &label, place, scale);
            drawn = Some(lines.map_or(label_rect, |l| l.union(label_rect)));
        }

        let wl_surface = surface.layer.wl_surface();
        buffer
            .attach_to(wl_surface)
            .map_err(|e| format!("cannot show the frame for {}: {e}", surface.capture.name))?;
        // Every buffer is the whole picture; only what changed since the last one is damaged.
        let damage = match (surface.drawn, drawn) {
            (Some(old), Some(new)) => Some(old.union(new)),
            (old, new) => old.or(new),
        };
        match damage.filter(|_| surface.shown) {
            Some(d) => wl_surface.damage_buffer(d.x, d.y, d.w, d.h),
            None => wl_surface.damage_buffer(0, 0, width, height),
        }
        wl_surface.frame(qh, wl_surface.clone());
        surface.layer.commit();
        surface.drawn = drawn;
        surface.shown = true;
        surface.frame = Frame::Pending;
        Ok(())
    }

    fn finish(&mut self, outcome: Outcome) {
        if self.outcome.is_none() {
            self.outcome = Some(outcome);
        }
    }
}

/// Copies the frozen frame into the start of a buffer's canvas.
///
/// The canvas is the whole `SlotPool` slot, which is the frame's `stride × height` rounded up
/// to a multiple of 64 bytes, so it is often longer than the frame. Exactly the frame's bytes
/// are copied; the tail past them is never shown.
fn copy_frame(canvas: &mut [u8], image: &Image) {
    let len = image.stride() as usize * image.height() as usize;
    canvas[..len].copy_from_slice(&image.data()[..len]);
}

impl LayerShellHandler for Overlay {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.finish(Outcome::Cancel);
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        _: LayerSurfaceConfigure,
        _: u32,
    ) {
        let Some(index) = self.surfaces.iter().position(|s| &s.layer == layer) else {
            return;
        };
        let surface = &mut self.surfaces[index];
        // The buffer is the capture at its physical size; the viewport presents it at the
        // output's logical size, so a scaled output shows its real pixels one to one.
        surface
            .viewport
            .set_destination(surface.capture.logical.0, surface.capture.logical.1);
        surface.configured = true;
        self.redraw(index, qh);
    }
}

impl CompositorHandler for Overlay {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn frame(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        let Some(index) = self.surface_index(surface) else {
            return;
        };
        let dirty = std::mem::replace(&mut self.surfaces[index].frame, Frame::Idle);
        if dirty == Frame::PendingDirty {
            self.redraw(index, qh);
        }
    }
}

impl PointerHandler for Overlay {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            let Some(index) = self.surface_index(&event.surface) else {
                continue;
            };
            match event.kind {
                PointerEventKind::Enter { serial } => {
                    // The ruler draws its own crosshair on the measured pixel.
                    pointer.set_cursor(serial, None, 0, 0);
                    self.set_hover(Some((index, self.physical(index, event.position))), qh);
                }
                PointerEventKind::Leave { .. } => {
                    if self.hover.is_some_and(|(i, _)| i == index) {
                        self.set_hover(None, qh);
                    }
                }
                PointerEventKind::Motion { .. } => {
                    self.set_hover(Some((index, self.physical(index, event.position))), qh);
                }
                PointerEventKind::Press {
                    button: BTN_LEFT, ..
                } => {
                    let outcome = self.current_result().map_or(Outcome::Cancel, Outcome::Copy);
                    self.finish(outcome);
                }
                PointerEventKind::Press {
                    button: BTN_RIGHT, ..
                } => self.finish(Outcome::Cancel),
                PointerEventKind::Axis { vertical, .. } if vertical.discrete != 0 => {
                    // One step per notch: a high-resolution wheel sends several axis events
                    // per notch, but only one carries a discrete step. Wheel up widens the
                    // tolerance, wheel down narrows it.
                    let tolerance = if vertical.discrete < 0 {
                        self.tolerance.saturating_add(TOLERANCE_STEP)
                    } else {
                        self.tolerance.saturating_sub(TOLERANCE_STEP)
                    };
                    if tolerance != self.tolerance {
                        self.tolerance = tolerance;
                        self.redraw(index, qh);
                    }
                }
                _ => {}
            }
        }
    }
}

impl KeyboardHandler for Overlay {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
    }

    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
    }

    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: Modifiers,
        _: u32,
    ) {
    }

    fn press_key(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        match event.keysym {
            Keysym::Escape => self.finish(Outcome::Cancel),
            Keysym::space => {
                self.mode = self.mode.toggled();
                if let Some((index, _)) = self.hover {
                    self.redraw(index, qh);
                }
            }
            _ => {}
        }
    }
}

impl SeatHandler for Overlay {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            self.keyboard = self.seat_state.get_keyboard(qh, &seat, None).ok();
        }
        if capability == Capability::Pointer && self.pointer.is_none() {
            self.pointer = self.seat_state.get_pointer(qh, &seat).ok();
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard
            && let Some(keyboard) = self.keyboard.take()
        {
            keyboard.release();
        }
        if capability == Capability::Pointer
            && let Some(pointer) = self.pointer.take()
        {
            pointer.release();
        }
    }
}

impl OutputHandler for Overlay {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    // Measuring a capture of a layout that no longer exists gives false numbers.
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        if self.ready {
            eprintln!("melibea-ruler: an output was connected; closing");
            self.finish(Outcome::Cancel);
        }
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        if self.ready {
            eprintln!("melibea-ruler: an output was disconnected; closing");
            self.finish(Outcome::Cancel);
        }
    }
}

impl ShmHandler for Overlay {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl AsMut<SimpleGlobal<WpViewporter, 1>> for Overlay {
    fn as_mut(&mut self) -> &mut SimpleGlobal<WpViewporter, 1> {
        &mut self.viewporter
    }
}

impl Dispatch<WpViewport, ()> for Overlay {
    fn event(
        _: &mut Self,
        _: &WpViewport,
        _: wp_viewport::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl ProvidesRegistryState for Overlay {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(Overlay);
delegate_output!(Overlay);
delegate_shm!(Overlay);
delegate_seat!(Overlay);
delegate_keyboard!(Overlay);
delegate_pointer!(Overlay);
delegate_layer!(Overlay);
delegate_registry!(Overlay);
delegate_simple!(Overlay, WpViewporter, 1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frame_is_copied_into_a_longer_canvas() {
        // 942 × 4 = 3768 bytes a row, as in the nest: 3 rows are 11304 bytes, and the slot
        // rounds that up to 11328.
        let image = Image::from_rgb(942, 3, |x, y| [u8::try_from((x + y) % 256).unwrap(), 7, 9]);
        let len = image.data().len();
        let mut canvas = vec![0xAB; len.next_multiple_of(64)];
        assert!(canvas.len() > len);
        copy_frame(&mut canvas, &image);
        assert_eq!(&canvas[..len], image.data());
        assert!(
            canvas[len..].iter().all(|&b| b == 0xAB),
            "the tail is left alone"
        );
    }

    #[test]
    fn only_the_frame_is_copied_from_longer_image_data() {
        let mut data = vec![1u8; 2 * 4 * 2];
        data.extend([2u8; 16]);
        let image = Image::new(2, 2, 8, data);
        let mut canvas = vec![0u8; 64];
        copy_frame(&mut canvas, &image);
        assert!(canvas[..16].iter().all(|&b| b == 1));
        assert!(canvas[16..].iter().all(|&b| b == 0));
    }
}
