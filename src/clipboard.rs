use std::collections::HashMap;
use std::error::Error;
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use smithay_client_toolkit::data_device_manager::WritePipe;
use smithay_client_toolkit::reexports::calloop::{EventLoop, LoopHandle, PostAction, channel};
use smithay_client_toolkit::reexports::client::backend::{Backend, ObjectId};
use smithay_client_toolkit::reexports::client::protocol::{
    wl_callback,
    wl_data_device::{self, WlDataDevice},
    wl_data_device_manager::WlDataDeviceManager,
    wl_data_offer,
    wl_data_source::{self, WlDataSource},
    wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_touch,
};
use smithay_client_toolkit::reexports::client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop, event_created_child,
};
use wgpu::{MapMode, PollType};
use winit::event_loop::{EventLoopProxy, OwnedDisplayHandle};
use winit::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};

use crate::capture::Event;
use crate::render::Snapshot;

const PNG: &str = "image/png";

pub struct Clipboard {
    state: State,
    queue: EventQueue<State>,
    conn: Connection,
    _display: OwnedDisplayHandle,
}

struct State {
    manager: Option<WlDataDeviceManager>,
    seats: HashMap<u32, Seat>,
    serial: Option<(u32, u32)>,
    sources: HashMap<ObjectId, WlDataSource>,
    jobs: channel::Sender<Job>,
    stopping: Arc<AtomicBool>,
    copied: bool,
}

struct Seat {
    proxy: wl_seat::WlSeat,
    device: Option<WlDataDevice>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    touch: Option<wl_touch::WlTouch>,
    used_serial: Option<u32>,
}

impl Clipboard {
    pub fn start(
        display: OwnedDisplayHandle,
        proxy: EventLoopProxy<Event>,
    ) -> Result<(Self, JoinHandle<()>), Box<dyn Error>> {
        let RawDisplayHandle::Wayland(handle) = display.display_handle()?.as_raw() else {
            return Err("a Wayland display is required".into());
        };
        // The owned handle keeps the display alive until the guest queue is gone.
        let backend = unsafe { Backend::from_foreign_display(handle.display.as_ptr().cast()) };
        let conn = Connection::from_backend(backend);
        let queue = conn.new_event_queue();
        conn.display().get_registry(&queue.handle(), ());
        let (jobs, requests) = channel::channel();
        let stopping = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&stopping);
        let worker = thread::Builder::new()
            .name("clipboard".into())
            .spawn(move || {
                if let Err(error) = work(requests, &proxy, &stop) {
                    eprintln!("Clipboard worker stopped: {error}");
                }
            })?;
        let state = State {
            manager: None,
            seats: HashMap::new(),
            serial: None,
            sources: HashMap::new(),
            jobs,
            stopping,
            copied: false,
        };
        let clipboard = Self {
            state,
            queue,
            conn,
            _display: display,
        };
        Ok((clipboard, worker))
    }

    pub fn copy(&self, snapshot: Snapshot) {
        if self.state.jobs.send(Job::Copy(snapshot)).is_err() {
            eprintln!("Cannot copy the frame: clipboard worker is unavailable");
        }
    }

    pub fn publish(&mut self, png: Vec<u8>) -> Result<(), Box<dyn Error>> {
        let state = &mut self.state;
        // The input and the encoded frame arrive through separate queues.
        self.queue.dispatch_pending(state)?;
        let (seat, serial) = state.serial.ok_or("no Wayland input serial is available")?;
        let seat = state.seats.get_mut(&seat).ok_or("input seat is gone")?;
        // Two encodes can complete after the same input event.
        if seat.used_serial == Some(serial) {
            return Ok(());
        }
        let manager = state
            .manager
            .as_ref()
            .ok_or("no Wayland data device manager")?;
        let device = seat.device.as_ref().ok_or("no Wayland data device")?;
        let qh = self.queue.handle();
        let source = manager.create_data_source(&qh, Arc::new(png));
        source.offer(PNG.into());
        device.set_selection(Some(&source), serial);
        seat.used_serial = Some(serial);
        let id = source.id();
        state.sources.insert(id.clone(), source);
        self.conn.display().sync(&qh, id);
        Ok(())
    }

    pub fn dispatch(&mut self) -> bool {
        if let Err(error) = self.queue.dispatch_pending(&mut self.state) {
            eprintln!("Cannot dispatch Wayland clipboard events: {error}");
        }
        std::mem::take(&mut self.state.copied)
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        self.state.stopping.store(true, Ordering::Relaxed);
        for source in self.state.sources.values() {
            source.destroy();
        }
        self.state.seats.clear();
        let _ = self.conn.flush();
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            if let wl_registry::Event::GlobalRemove { name } = event {
                state.seats.remove(&name);
            }
            return;
        };
        match interface.as_str() {
            "wl_data_device_manager" => {
                let manager: WlDataDeviceManager = registry.bind(name, version.min(3), qh, ());
                for seat in state.seats.values_mut() {
                    seat.device = Some(manager.get_data_device(&seat.proxy, qh, ()));
                }
                state.manager = Some(manager);
            }
            "wl_seat" => {
                let proxy = registry.bind(name, version.min(7), qh, name);
                let device = state
                    .manager
                    .as_ref()
                    .map(|manager| manager.get_data_device(&proxy, qh, ()));
                state.seats.insert(
                    name,
                    Seat {
                        proxy,
                        device,
                        keyboard: None,
                        pointer: None,
                        touch: None,
                        used_serial: None,
                    },
                );
            }
            _ => {}
        }
    }
}

fn update_input<P: Proxy>(
    device: &mut Option<P>,
    enabled: bool,
    create: impl FnOnce() -> P,
    release: fn(&P),
) {
    if enabled {
        device.get_or_insert_with(create);
    } else {
        release_input(device.take(), release);
    }
}

fn release_input<P: Proxy>(device: Option<P>, release: fn(&P)) {
    if let Some(device) = device
        && device.version() >= 3
    {
        release(&device);
    }
}

impl Drop for Seat {
    fn drop(&mut self) {
        release_input(self.keyboard.take(), wl_keyboard::WlKeyboard::release);
        release_input(self.pointer.take(), wl_pointer::WlPointer::release);
        release_input(self.touch.take(), wl_touch::WlTouch::release);
        if let Some(device) = &self.device
            && device.version() >= 2
        {
            device.release();
        }
        if self.proxy.version() >= 5 {
            self.proxy.release();
        }
    }
}

impl Dispatch<wl_seat::WlSeat, u32> for State {
    fn event(
        state: &mut Self,
        proxy: &wl_seat::WlSeat,
        event: wl_seat::Event,
        &name: &u32,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use wl_seat::Capability;
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
            && let Some(seat) = state.seats.get_mut(&name)
        {
            update_input(
                &mut seat.keyboard,
                caps.contains(Capability::Keyboard),
                || proxy.get_keyboard(qh, name),
                wl_keyboard::WlKeyboard::release,
            );
            update_input(
                &mut seat.pointer,
                caps.contains(Capability::Pointer),
                || proxy.get_pointer(qh, name),
                wl_pointer::WlPointer::release,
            );
            update_input(
                &mut seat.touch,
                caps.contains(Capability::Touch),
                || proxy.get_touch(qh, name),
                wl_touch::WlTouch::release,
            );
        }
    }
}

macro_rules! track_serial {
    ($module:ident::$proxy:ident, $serial:ident in $pattern:pat) => {
        impl Dispatch<$module::$proxy, u32> for State {
            fn event(
                state: &mut Self,
                _: &$module::$proxy,
                event: $module::Event,
                &seat: &u32,
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                if let $pattern = event {
                    state.serial = Some((seat, $serial));
                }
            }
        }
    };
}

track_serial!(wl_keyboard::WlKeyboard, serial in wl_keyboard::Event::Key {
    serial,
    state: WEnum::Value(wl_keyboard::KeyState::Pressed),
    ..
});
track_serial!(wl_pointer::WlPointer, serial in wl_pointer::Event::Button { serial, .. });
track_serial!(wl_touch::WlTouch, serial in wl_touch::Event::Down { serial, .. });

delegate_noop!(State: ignore WlDataDeviceManager);
delegate_noop!(State: ignore wl_data_offer::WlDataOffer);

impl Dispatch<WlDataDevice, ()> for State {
    event_created_child!(State, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, ())
    ]);

    fn event(
        _: &mut Self,
        _: &WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // This device advertises selections. Incoming offers are unused.
        if let wl_data_device::Event::Selection { id: Some(offer) }
        | wl_data_device::Event::Enter {
            id: Some(offer), ..
        } = event
        {
            offer.destroy();
        }
    }
}

impl Dispatch<WlDataSource, Arc<Vec<u8>>> for State {
    fn event(
        state: &mut Self,
        source: &WlDataSource,
        event: wl_data_source::Event,
        png: &Arc<Vec<u8>>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Send { mime_type, fd } if mime_type == PNG => {
                let _ = state.jobs.send(Job::Paste(fd.into(), Arc::clone(png)));
            }
            wl_data_source::Event::Cancelled => {
                state.sources.remove(&source.id());
                source.destroy();
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_callback::WlCallback, ObjectId> for State {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        source: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if state.sources.contains_key(source) {
            state.copied = true;
        } else {
            eprintln!("Cannot copy the frame: Wayland cancelled the selection");
        }
    }
}

enum Job {
    Copy(Snapshot),
    Paste(WritePipe, Arc<Vec<u8>>),
}

fn work(
    requests: channel::Channel<Job>,
    proxy: &EventLoopProxy<Event>,
    stop: &AtomicBool,
) -> Result<(), Box<dyn Error>> {
    let mut event_loop = EventLoop::try_new()?;
    let handle = event_loop.handle();
    let pipes = handle.clone();
    handle.insert_source(requests, move |event, _, &mut ()| {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        match event {
            channel::Event::Msg(Job::Copy(snapshot)) => match encode(snapshot, stop) {
                Ok(png) => {
                    let _ = proxy.send_event(Event::Encoded(png));
                }
                Err(error) if !stop.load(Ordering::Relaxed) => {
                    eprintln!("Cannot copy the frame: {error}")
                }
                Err(_) => {}
            },
            channel::Event::Msg(Job::Paste(pipe, png)) => write_pipe(&pipes, pipe, png),
            channel::Event::Closed => {}
        }
    })?;
    while !stop.load(Ordering::Relaxed) {
        event_loop.dispatch(None, &mut ())?;
    }
    Ok(())
}

fn write_pipe(handle: &LoopHandle<'_, ()>, pipe: WritePipe, png: Arc<Vec<u8>>) {
    let fd = pipe.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return;
    }
    let mut written = 0;
    let _ = handle.insert_source(pipe, move |_, file, _| {
        match (&**file).write(&png[written..]) {
            Ok(n) if n > 0 && written + n < png.len() => {
                written += n;
                PostAction::Continue
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                PostAction::Continue
            }
            _ => PostAction::Remove,
        }
    });
}

fn encode(snapshot: Snapshot, stop: &AtomicBool) -> Result<Vec<u8>, Box<dyn Error>> {
    let slice = snapshot.buffer.slice(..);
    let (mapped, map_result) = mpsc::channel();
    slice.map_async(MapMode::Read, move |result| {
        let _ = mapped.send(result);
    });
    // Polls without waiting, since a waiting poll holds the device lock that
    // presenting a frame needs.
    loop {
        if stop.load(Ordering::Relaxed) {
            return Err("clipboard is closed".into());
        }
        snapshot.device.poll(PollType::Poll)?;
        if let Ok(result) = map_result.try_recv() {
            result?;
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    let row = snapshot.width as usize * 4;
    let mut bytes = Vec::with_capacity(row * snapshot.height as usize);
    for line in slice.get_mapped_range()?.chunks(snapshot.stride as usize) {
        bytes.extend_from_slice(&line[..row]);
    }
    if snapshot.bgra {
        for pixel in bytes.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
    }
    let mut png = Vec::new();
    let mut encoder = png::Encoder::new(&mut png, snapshot.width, snapshot.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fastest);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&bytes)?;
    writer.finish()?;
    Ok(png)
}
