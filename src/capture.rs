use std::collections::BTreeMap;
use std::io;
use std::os::fd::RawFd;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use v4l::buffer::Type;
use v4l::capability::Flags;
use v4l::frameinterval::FrameIntervalEnum;
use v4l::framesize::FrameSizeEnum;
use v4l::v4l_sys::*;
use v4l::v4l2::{self, vidioc::*};
use v4l::video::{Capture, capture::Parameters};
use v4l::{Device, FourCC, Fraction, Memory};
use winit::event_loop::EventLoopProxy;

use crate::decode::{Color, Layout, PixelFormat, color, decode_jpeg};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Mode {
    pub pixel: PixelFormat,
    pub width: u32,
    pub height: u32,
    #[serde(with = "ModeInterval")]
    pub interval: Fraction,
}

#[derive(Serialize, Deserialize)]
#[serde(remote = "Fraction")]
struct ModeInterval {
    numerator: u32,
    denominator: u32,
}

impl PartialEq for Mode {
    fn eq(&self, other: &Self) -> bool {
        (self.pixel, self.width, self.height) == (other.pixel, other.width, other.height)
            && self.interval.numerator == other.interval.numerator
            && self.interval.denominator == other.interval.denominator
    }
}

impl Mode {
    pub fn fps(self) -> f64 {
        f64::from(self.interval.denominator) / f64::from(self.interval.numerator)
    }

    fn valid_interval(self) -> bool {
        self.interval.numerator != 0 && self.interval.denominator != 0
    }
}

pub struct Frame {
    pub mode: Mode,
    pub layout: Layout,
    pub color: Color,
    pub bytes: Vec<u8>,
    /// When the device captured the frame, on the monotonic clock.
    pub timestamp: Duration,
    /// When the capture thread dequeued the buffer.
    pub dequeued: Duration,
    /// When the frame was copied out, decoded, and ready for the renderer.
    pub ready: Duration,
}

#[derive(Default)]
pub struct Inbox {
    pub latest: Option<Frame>,
    pub spare: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Source {
    pub path: PathBuf,
    pub name: String,
}

/// The capture devices and the modes of the current device, if any.
#[derive(Clone, Default, PartialEq)]
pub struct Status {
    pub devices: Vec<Source>,
    pub device: Option<Source>,
    pub modes: Vec<Mode>,
    pub mode: Option<Mode>,
}

impl Status {
    fn publish(&self, proxy: &EventLoopProxy<Event>) -> bool {
        proxy.send_event(Event::Status(self.clone())).is_ok()
    }
}

pub enum Event {
    Frame,
    Status(Status),
    Stopped,
    ModeFailed(String, Mode),
    Encoded(Vec<u8>),
}

pub enum Command {
    Device(Source),
    Mode(Source, Mode),
    Sources(Vec<Source>),
}

fn devices() -> Vec<Source> {
    let mut nodes = v4l::context::enum_devices();
    nodes.retain(|node| {
        node.path()
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("video"))
            .is_some_and(|index| index.parse::<usize>().is_ok())
    });
    nodes.sort_unstable_by_key(v4l::context::Node::index);
    nodes
        .into_iter()
        .filter_map(|node| {
            let caps = Device::with_path(node.path()).ok()?.query_caps().ok()?;
            (caps.driver == "uvcvideo" && caps.capabilities.contains(Flags::VIDEO_CAPTURE)).then(
                || Source {
                    path: node.path().to_owned(),
                    name: caps.card,
                },
            )
        })
        .collect()
}

pub fn scan_on_request(requests: Receiver<()>, commands: Sender<Command>) {
    while requests.recv().is_ok() {
        if commands.send(Command::Sources(devices())).is_err() {
            break;
        }
    }
}

fn modes(device: &Device) -> io::Result<Vec<Mode>> {
    let mut modes = Vec::new();
    for format in device.enum_formats()? {
        let Some(pixel) = PixelFormat::from_fourcc(format.fourcc) else {
            continue;
        };
        for size in device.enum_framesizes(format.fourcc)? {
            let FrameSizeEnum::Discrete(size) = size.size else {
                continue;
            };
            for interval in device.enum_frameintervals(format.fourcc, size.width, size.height)? {
                modes.push(Mode {
                    pixel,
                    width: size.width,
                    height: size.height,
                    interval: match interval.interval {
                        FrameIntervalEnum::Discrete(interval) => interval,
                        FrameIntervalEnum::Stepwise(range) => range.min,
                    },
                });
            }
        }
    }
    Ok(modes)
}

fn choose_mode(modes: impl IntoIterator<Item = Mode>) -> Option<Mode> {
    modes
        .into_iter()
        .filter(|mode| mode.valid_interval())
        .max_by(|a, b| {
            let rate_a = u64::from(a.interval.denominator) * u64::from(b.interval.numerator);
            let rate_b = u64::from(b.interval.denominator) * u64::from(a.interval.numerator);
            let area = |mode: &Mode| u64::from(mode.width) * u64::from(mode.height);
            rate_a
                .cmp(&rate_b)
                .then_with(|| area(a).cmp(&area(b)))
                .then_with(|| (a.pixel != PixelFormat::Mjpeg).cmp(&(b.pixel != PixelFormat::Mjpeg)))
        })
}

fn candidates(devices: &[Source], preferred: Option<&Source>, failed: &[Source]) -> Vec<Source> {
    let mut candidates = devices.to_vec();
    candidates.retain(|source| !failed.contains(source));
    candidates.sort_by_key(|source| Some(source) != preferred);
    candidates
}

fn requested_mode(listed: &[Mode], saved: Option<Mode>) -> Option<Mode> {
    saved
        .filter(|mode| mode.valid_interval() && listed.contains(mode))
        .or_else(|| choose_mode(listed.iter().copied()))
}

pub fn run(
    inbox: Arc<Mutex<Inbox>>,
    proxy: EventLoopProxy<Event>,
    commands: Receiver<Command>,
    last_source: Option<String>,
    mut picked: BTreeMap<String, Mode>,
) {
    let mut status = Status {
        devices: devices(),
        ..Default::default()
    };
    let mut preferred = status
        .devices
        .iter()
        .find(|source| Some(source.name.as_str()) == last_source.as_deref())
        .cloned();
    let mut failed = Vec::new();
    let mut idle = None;
    loop {
        if let Some(source) = candidates(&status.devices, preferred.as_ref(), &failed)
            .into_iter()
            .next()
        {
            let mut manual = None;
            let result = stream(
                &source,
                &mut status,
                &mut picked,
                &inbox,
                &proxy,
                &commands,
                &mut manual,
            );
            if status.device.take().is_some() {
                idle = None;
            }
            let was_running = status.mode.take().is_some();
            if was_running {
                if proxy.send_event(Event::Stopped).is_err() {
                    return;
                }
                let mut inbox = inbox.lock().unwrap();
                if let Some(frame) = inbox.latest.take() {
                    inbox.spare = frame.bytes;
                }
            }
            match result {
                Ok(Some(source)) => {
                    preferred = Some(source);
                    failed.clear();
                }
                Ok(None) => return,
                Err(error) => {
                    eprintln!("Capture failed for {}: {error}", source.name);
                    status.devices = devices();
                    if was_running {
                        failed.clear();
                        preferred = Some(source);
                        continue;
                    }
                    // A source that is no longer listed was unplugged, which
                    // says nothing about its mode.
                    if let Some(mode) = manual
                        && picked.get(&source.name) == Some(&mode)
                        && status.devices.contains(&source)
                    {
                        picked.remove(&source.name);
                        if proxy
                            .send_event(Event::ModeFailed(source.name.clone(), mode))
                            .is_err()
                        {
                            return;
                        }
                        if choose_mode(status.modes.iter().copied())
                            .is_some_and(|automatic| automatic != mode)
                        {
                            preferred = Some(source);
                            continue;
                        }
                    }
                    preferred = None;
                    failed.push(source);
                }
            }
            continue;
        }
        status.modes.clear();
        if idle.as_ref() != Some(&status) {
            if !status.publish(&proxy) {
                return;
            }
            idle = Some(status.clone());
        }
        failed.clear();
        match commands.recv_timeout(Duration::from_secs(1)) {
            Ok(Command::Sources(sources)) => status.devices = sources,
            Ok(Command::Device(source)) => preferred = Some(source),
            Ok(Command::Mode(source, mode)) => {
                picked.insert(source.name, mode);
            }
            Err(RecvTimeoutError::Timeout) => status.devices = devices(),
            Err(RecvTimeoutError::Disconnected) => return,
        };
    }
}

fn stream(
    source: &Source,
    status: &mut Status,
    picked: &mut BTreeMap<String, Mode>,
    inbox: &Mutex<Inbox>,
    proxy: &EventLoopProxy<Event>,
    commands: &Receiver<Command>,
    manual: &mut Option<Mode>,
) -> io::Result<Option<Source>> {
    let device = Device::with_path(&source.path)?;
    status.modes = modes(&device)?;
    let saved = picked.get(&source.name).copied();
    let requested = requested_mode(&status.modes, saved)
        .ok_or_else(|| io::Error::other("Device has no usable modes"))?;
    *manual = saved.filter(|&saved| saved == requested);
    let handle = device.handle();
    let fd = handle.fd();
    let mut format = v4l2_format {
        type_: Type::VideoCapture as u32,
        ..unsafe { std::mem::zeroed() }
    };
    format.fmt.pix = v4l2_pix_format {
        width: requested.width,
        height: requested.height,
        pixelformat: requested.pixel.fourcc().into(),
        ..unsafe { std::mem::zeroed() }
    };
    // SAFETY: VIDIOC_S_FMT takes a v4l2_format.
    unsafe { ioctl(fd, VIDIOC_S_FMT, &mut format) }?;
    // The stream type selects the single-planar member of the union.
    let format = unsafe { format.fmt.pix };
    let pixel = PixelFormat::from_fourcc(FourCC::from(format.pixelformat))
        .ok_or_else(|| io::Error::other("Unsupported capture format"))?;
    let interval = device
        .set_params(&Parameters::new(requested.interval))?
        .interval;
    let mode = Mode {
        pixel,
        width: format.width,
        height: format.height,
        interval,
    };
    let color = color(format, pixel);
    let raw_layout = Layout::raw(pixel, mode.width, mode.height, format.bytesperline);
    let mut decoder = (pixel == PixelFormat::Mjpeg)
        .then(turbojpeg::Decompressor::new)
        .transpose()
        .map_err(io::Error::other)?;

    // The driver drops a frame that arrives while it has no queued buffer, so
    // it needs a second one to fill while the first is copied out.
    let mut buffers = v4l2_requestbuffers {
        count: 2,
        type_: Type::VideoCapture as u32,
        memory: Memory::Mmap as u32,
        ..unsafe { std::mem::zeroed() }
    };
    // SAFETY: VIDIOC_REQBUFS takes a v4l2_requestbuffers.
    unsafe { ioctl(fd, VIDIOC_REQBUFS, &mut buffers) }?;
    if buffers.count == 0 {
        return Err(io::Error::other("Driver allocated no capture buffers"));
    }
    let mapped = (0..buffers.count)
        .map(|index| Mapping::new(fd, index))
        .collect::<io::Result<Vec<_>>>()?;
    let mut kind = Type::VideoCapture as u32;
    // SAFETY: VIDIOC_STREAMON takes a 32-bit buffer type.
    unsafe { ioctl(fd, VIDIOC_STREAMON, &mut kind) }?;
    status.device = Some(source.clone());

    let result = (|| {
        if !status.publish(proxy) {
            return Ok(None);
        }
        let mut jpeg = Vec::new();
        let mut bytes = Vec::new();
        loop {
            match commands.try_recv() {
                Ok(Command::Device(next))
                    if status.device.as_ref() != Some(&next) && status.devices.contains(&next) =>
                {
                    return Ok(Some(next));
                }
                Ok(Command::Mode(source, mode)) => {
                    let selected = status.device.as_ref() == Some(&source);
                    picked.insert(source.name, mode);
                    if selected && status.mode != Some(mode) {
                        return Ok(status.device.clone());
                    }
                }
                Ok(Command::Sources(sources)) if sources != status.devices => {
                    status.devices = sources;
                    if !status.publish(proxy) {
                        return Ok(None);
                    }
                }
                Err(TryRecvError::Disconnected) => return Ok(None),
                _ => {}
            }
            // The timeout keeps commands responsive when the device stops sending frames.
            match handle.poll(libc::POLLIN, 100) {
                Ok(0) => continue,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => result?,
            };

            let mut newest = None;
            for _ in 0..buffers.count {
                let mut ready = buffer(0);
                // SAFETY: VIDIOC_DQBUF takes a v4l2_buffer.
                match unsafe { ioctl(fd, VIDIOC_DQBUF, &mut ready) } {
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    result => result?,
                }
                let dequeued = monotonic_now();
                let used = ready.bytesused as usize;
                if ready.flags & V4L2_BUF_FLAG_ERROR != 0
                    || used == 0
                    || used > mapped[ready.index as usize].len
                    || raw_layout.is_some_and(|layout| !layout.fits(used))
                {
                    // SAFETY: VIDIOC_QBUF takes a v4l2_buffer.
                    unsafe { ioctl(fd, VIDIOC_QBUF, &mut buffer(ready.index)) }?;
                    continue;
                }
                if let Some((older, _)) = newest.replace((ready, dequeued)) {
                    // SAFETY: VIDIOC_QBUF takes a v4l2_buffer.
                    unsafe { ioctl(fd, VIDIOC_QBUF, &mut buffer(older.index)) }?;
                }
            }
            let Some((ready, dequeued)) = newest else {
                continue;
            };
            let data = &mapped[ready.index as usize];
            let used = ready.bytesused as usize;
            if bytes.capacity() == 0 {
                bytes = std::mem::take(&mut inbox.lock().unwrap().spare);
            }
            let destination = if decoder.is_some() {
                &mut jpeg
            } else {
                &mut bytes
            };
            destination.clear();
            // DQBUF gives userspace ownership until QBUF returns the mapping to the driver.
            destination
                .extend_from_slice(unsafe { std::slice::from_raw_parts(data.ptr.cast(), used) });
            // SAFETY: VIDIOC_QBUF takes a v4l2_buffer.
            unsafe { ioctl(fd, VIDIOC_QBUF, &mut buffer(ready.index)) }?;
            let timestamp = Duration::new(
                ready.timestamp.tv_sec as u64,
                ready.timestamp.tv_usec as u32 * 1_000,
            );

            let layout = match &mut decoder {
                Some(decoder) => decode_jpeg(decoder, &jpeg, &mut bytes, mode.width, mode.height),
                None => raw_layout,
            };
            let Some(layout) = layout else { continue };
            let frame = Frame {
                mode,
                layout,
                color,
                bytes,
                timestamp,
                dequeued,
                ready: monotonic_now(),
            };
            if status.mode.is_none() {
                status.mode = Some(mode);
                if !status.publish(proxy) {
                    return Ok(None);
                }
            }
            let previous = inbox.lock().unwrap().latest.replace(frame);
            let wake = previous.is_none();
            bytes = previous.map_or_else(Vec::new, |frame| frame.bytes);
            if wake && proxy.send_event(Event::Frame).is_err() {
                return Ok(None);
            }
        }
    })();
    // SAFETY: VIDIOC_STREAMOFF takes a 32-bit buffer type.
    let stop = unsafe { ioctl(fd, VIDIOC_STREAMOFF, &mut kind) };
    result.and_then(|command| stop.map(|()| command))
}

fn buffer(index: u32) -> v4l2_buffer {
    v4l2_buffer {
        index,
        type_: Type::VideoCapture as u32,
        memory: Memory::Mmap as u32,
        ..unsafe { std::mem::zeroed() }
    }
}

struct Mapping {
    ptr: *mut libc::c_void,
    len: usize,
}

impl Mapping {
    fn new(fd: RawFd, index: u32) -> io::Result<Self> {
        let mut query = buffer(index);
        // SAFETY: VIDIOC_QUERYBUF takes a v4l2_buffer.
        unsafe { ioctl(fd, VIDIOC_QUERYBUF, &mut query) }?;
        let len = query.length as usize;
        let ptr = unsafe {
            v4l2::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd,
                query.m.offset.into(),
            )?
        };
        let mapping = Self { ptr, len };
        // SAFETY: VIDIOC_QBUF takes a v4l2_buffer.
        unsafe { ioctl(fd, VIDIOC_QBUF, &mut buffer(index)) }?;
        Ok(mapping)
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // Each successful mmap owns the mapping until this value drops.
        let _ = unsafe { v4l2::munmap(self.ptr, self.len) };
    }
}

// SAFETY: The request must accept a pointer to T and value must satisfy its requirements.
unsafe fn ioctl<T>(fd: RawFd, request: _IOC_TYPE, value: &mut T) -> io::Result<()> {
    unsafe { v4l2::ioctl(fd, request, std::ptr::from_mut(value).cast()) }
}

pub fn monotonic_now() -> Duration {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
    Duration::new(now.tv_sec as u64, now.tv_nsec as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(pixel: PixelFormat, width: u32, height: u32, num: u32, den: u32) -> Mode {
        Mode {
            pixel,
            width,
            height,
            interval: Fraction::new(num, den),
        }
    }

    #[test]
    fn rate_precedes_area_and_format() {
        let fast = mode(PixelFormat::Mjpeg, 1280, 720, 1, 60);
        let slow = mode(PixelFormat::Nv12, 3840, 2160, 1001, 60_000);
        assert_eq!(choose_mode([fast, slow]).unwrap().pixel, fast.pixel);
    }

    #[test]
    fn area_precedes_format_at_equal_rate() {
        let small = mode(PixelFormat::Nv12, 1280, 720, 2, 120);
        let large = mode(PixelFormat::Mjpeg, 1920, 1080, 1, 60);
        assert_eq!(choose_mode([large, small]).unwrap().width, large.width);
    }

    #[test]
    fn uncompressed_wins_at_equal_rate_and_area() {
        let jpeg = mode(PixelFormat::Mjpeg, 1920, 1080, 1, 60);
        let raw = mode(PixelFormat::Yuyv, 1920, 1080, 2, 120);
        assert_eq!(choose_mode([raw, jpeg]).unwrap().pixel, raw.pixel);
    }

    #[test]
    fn discards_invalid_intervals() {
        let invalid = mode(PixelFormat::Nv12, 1920, 1080, 0, 60);
        assert!(choose_mode([invalid]).is_none());
    }

    #[test]
    fn recovery_candidates_follow_scan_order_after_any_retry() {
        let source = |name: &str| Source {
            path: PathBuf::from(name),
            name: name.to_owned(),
        };
        let a = source("A");
        let b = source("B");
        let c = source("C");
        let listed = [a.clone(), b.clone(), c.clone()];
        let remaining = [a.clone(), c.clone()];
        assert_eq!(candidates(&listed, Some(&b), &[]), [b.clone(), a, c]);
        assert_eq!(candidates(&remaining, Some(&b), &[]), remaining);
        assert_eq!(candidates(&listed, None, &[b]), remaining);
        assert!(candidates(&listed, None, &listed).is_empty());
        assert!(candidates(&[], None, &[]).is_empty());
        assert_eq!(candidates(&listed, None, &[]), listed);
    }

    #[test]
    fn saved_mode_wins_only_if_still_listed() {
        let automatic = mode(PixelFormat::Nv12, 1920, 1080, 1, 60);
        let manual = mode(PixelFormat::Mjpeg, 1280, 720, 1, 30);
        assert_eq!(
            requested_mode(&[automatic, manual], Some(manual)),
            Some(manual)
        );
        assert_eq!(requested_mode(&[automatic], Some(manual)), Some(automatic));
    }
}
