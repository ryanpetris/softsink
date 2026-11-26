use std::error::Error;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{
    AtomicBool, AtomicU32, AtomicUsize,
    Ordering::{Acquire, Relaxed, Release},
};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use audioadapter_buffers::direct::InterleavedSlice;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, Device, ErrorKind, Stream, StreamConfig, SupportedBufferSize};
use rtrb::RingBuffer;
use rubato::{
    Adjustable, Async, FixedAsync, Resampler, Resizable, SincInterpolationParameters,
    WindowFunction,
};

const BUFFER_FRAMES: usize = 256;
// A larger cushion tolerates more scheduling jitter at the cost of audio latency.
const CUSHION_MS: usize = 10;
const MAX_DRIFT: f64 = 0.005;
// Long enough for an output that has to wake up, such as a suspended
// Bluetooth sink, before a start counts as failed.
const START_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Session {
    sound_card: AtomicBool,
    available: AtomicBool,
    unavailable: AtomicBool,
    started: AtomicUsize,
    cancelled: AtomicBool,
    failed: AtomicBool,
    depth_ms: AtomicU32,
    underruns: AtomicU32,
}

impl Session {
    fn stopped(&self) -> bool {
        self.cancelled.load(Relaxed) || self.failed.load(Relaxed)
    }
}

#[derive(Default)]
pub struct Readings {
    /// The source has a sound card on its USB device.
    pub sound_card: bool,
    pub available: bool,
    /// The source has a sound card, but audio could not start or has stopped.
    pub unavailable: bool,
    pub figures: Option<Figures>,
}

#[derive(Clone, Copy)]
pub struct Figures {
    pub stream_id: usize,
    pub depth_ms: f32,
    pub underruns: u32,
}

type Request = Option<(PathBuf, Arc<Session>)>;

enum Message {
    /// The selected source, with the sender its streams report failures on.
    Select(Option<(PathBuf, Arc<Session>, Sender<Message>)>),
    /// A stream of the selected source failed.
    Failed,
}

pub struct Audio {
    gain: Arc<AtomicU32>,
    current: Request,
    requests: Sender<Message>,
}

impl Audio {
    pub fn new(volume: f32, muted: bool) -> io::Result<Self> {
        let (requests, receiver) = mpsc::channel();
        let audio = Self {
            gain: Arc::new(AtomicU32::new(0)),
            current: None,
            requests,
        };
        audio.set_gain(volume, muted);
        let gain = Arc::clone(&audio.gain);
        std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || run(receiver, gain))?;
        Ok(audio)
    }

    pub fn set_gain(&self, volume: f32, muted: bool) {
        let gain = if muted || !volume.is_finite() {
            0.0
        } else {
            volume.clamp(0.0, 1.0)
        };
        self.gain.store(gain.to_bits(), Relaxed);
    }

    pub fn readings(&self) -> Readings {
        let Some((_, session)) = &self.current else {
            return Readings::default();
        };
        let stream_id = session.started.load(Acquire);
        Readings {
            sound_card: session.sound_card.load(Relaxed),
            available: session.available.load(Relaxed),
            unavailable: session.unavailable.load(Relaxed),
            figures: (stream_id != 0 && !session.stopped()).then(|| Figures {
                stream_id,
                depth_ms: f32::from_bits(session.depth_ms.load(Relaxed)),
                underruns: session.underruns.load(Relaxed),
            }),
        }
    }

    pub fn select(&mut self, path: Option<&Path>) {
        if self.current.as_ref().map(|(path, _)| path.as_path()) == path {
            return;
        }
        if let Some((_, session)) = self.current.take() {
            session.cancelled.store(true, Relaxed);
        }
        // The lookup runs here rather than on the worker, so readings show
        // whether the source has a sound card as soon as it is selected.
        self.current = path.map(|path| {
            let session = Session {
                sound_card: usb_audio_parent(path).is_some().into(),
                ..Session::default()
            };
            (path.to_owned(), Arc::new(session))
        });
        let selected = self.current.clone();
        let _ = self.requests.send(Message::Select(
            selected.map(|(path, session)| (path, session, self.requests.clone())),
        ));
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        self.select(None);
    }
}

fn run(requests: Receiver<Message>, gain: Arc<AtomicU32>) {
    let mut current = None;
    // The open streams, and when they opened until they first play.
    let mut playing: Option<((Stream, Stream), Option<Instant>)> = None;
    let mut stream_id = 0;
    // Set once a failure is printed, and cleared when audio plays or the
    // source changes, so each outage prints one line.
    let reported = Arc::new(AtomicBool::new(false));
    loop {
        // Streams that play need no checking: a failure arrives as a message.
        let message = match &playing {
            Some((_, Some(_))) => requests.recv_timeout(Duration::from_millis(50)),
            None if current.is_some() => requests.recv_timeout(Duration::from_secs(1)),
            _ => requests.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match message {
            Ok(Message::Select(selected)) => {
                playing = None;
                current = selected;
                reported.store(false, Relaxed);
            }
            // Both streams can report one outage. The second report must not
            // cut short the wait before the next attempt.
            Ok(Message::Failed) if playing.is_none() => continue,
            Ok(Message::Failed) | Err(RecvTimeoutError::Timeout) => {
                if let Some((_, opened)) = &mut playing
                    && let Some((_, session, _)) = &current
                {
                    if session.stopped() {
                        playing = None;
                        if session.failed.load(Relaxed) {
                            session.unavailable.store(true, Relaxed);
                        }
                    } else if session.started.load(Relaxed) != 0 {
                        *opened = None;
                        reported.store(false, Relaxed);
                        session.unavailable.store(false, Relaxed);
                    } else if opened.is_some_and(|at| at.elapsed() >= START_TIMEOUT) {
                        session.failed.store(true, Relaxed);
                        start_failed(session, &reported, "Audio playback timed out");
                        playing = None;
                    }
                    continue;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
        let Some((path, session, wake)) = &current else {
            continue;
        };
        if session.cancelled.load(Relaxed) {
            continue;
        }
        // A sound card can register after its video node, so a source
        // without one is looked up again on the next pass.
        let Some(video) = usb_audio_parent(path) else {
            continue;
        };
        session.sound_card.store(true, Relaxed);
        session.started.store(0, Relaxed);
        session.failed.store(false, Relaxed);
        session.underruns.store(0, Relaxed);
        stream_id += 1;
        match open(video, session, &gain, &reported, wake, stream_id) {
            Ok(Some(streams)) => playing = Some((streams, Some(Instant::now()))),
            Ok(None) => session.unavailable.store(true, Relaxed),
            Err(error) => start_failed(session, &reported, error),
        }
    }
}

fn start_failed(session: &Session, reported: &AtomicBool, error: impl std::fmt::Display) {
    session.unavailable.store(true, Relaxed);
    if !reported.swap(true, Relaxed) {
        eprintln!("Could not start audio: {error}");
    }
}

fn stream_config(config: cpal::SupportedStreamConfig) -> StreamConfig {
    let mut stream = config.config();
    if let SupportedBufferSize::Range { min, max } = *config.buffer_size() {
        let frames = (u64::from(stream.sample_rate) * BUFFER_FRAMES as u64 / 48_000) as u32;
        stream.buffer_size = BufferSize::Fixed(frames.clamp(min, max));
    }
    stream
}

fn stream_error(
    session: Arc<Session>,
    reported: Arc<AtomicBool>,
    wake: Sender<Message>,
) -> impl FnMut(cpal::Error) {
    move |error| {
        if !session.stopped()
            && matches!(
                error.kind(),
                ErrorKind::DeviceNotAvailable
                    | ErrorKind::HostUnavailable
                    | ErrorKind::StreamInvalidated
                    | ErrorKind::UnsupportedConfig
            )
        {
            session.failed.store(true, Relaxed);
            if !reported.swap(true, Relaxed) {
                eprintln!("Audio stopped: {error}");
            }
            let _ = wake.send(Message::Failed);
        }
    }
}

fn open(
    video: PathBuf,
    session: &Arc<Session>,
    gain: &Arc<AtomicU32>,
    reported: &Arc<AtomicBool>,
    wake: &Sender<Message>,
    stream_id: usize,
) -> Result<Option<(Stream, Stream)>, Box<dyn Error>> {
    let Some((input, host)) = paired_input(video)? else {
        return Err("No matching PipeWire audio input".into());
    };
    session.available.store(true, Relaxed);
    let output = host
        .default_output_device()
        .ok_or("No default audio output")?;
    let input_config = stream_config(input.default_input_config()?);
    let output_config = stream_config(output.default_output_config()?);
    let input_channels = usize::from(input_config.channels);
    let output_channels = usize::from(output_config.channels);
    let input_rate = input_config.sample_rate as usize;
    let base_ratio = f64::from(output_config.sample_rate) / input_rate as f64;
    let target = (input_rate * CUSHION_MS / 1000).max(1);
    // One second holds the reserve for the largest block PipeWire delivers.
    let (mut producer, mut consumer) = RingBuffer::new(input_rate);
    let filter = SincInterpolationParameters::new(96, WindowFunction::BlackmanHarris2);
    let mut resampler = Async::<f32>::new_sinc(
        base_ratio,
        1.0 / (1.0 - MAX_DRIFT),
        &filter,
        BUFFER_FRAMES,
        2,
        FixedAsync::Output,
    )?;
    let mut input_frames = vec![[0.0; 2]; resampler.input_frames_max()];
    let mut output_frames = vec![[0.0; 2]; resampler.output_frames_max()];

    let capture_block = Arc::new(AtomicUsize::new(0));
    let input_block = Arc::clone(&capture_block);
    let input_session = Arc::clone(session);
    let capture = input.build_input_stream(
        input_config,
        move |data: &[f32], _| {
            if input_session.stopped() {
                return;
            }
            let frames = data.chunks_exact(input_channels);
            input_block.store(frames.len(), Relaxed);
            let count = frames.len().min(producer.slots());
            if let Ok(chunk) = producer.write_chunk_uninit(count) {
                chunk.fill_from_iter(
                    frames.map(|frame| [frame[0], frame.get(1).copied().unwrap_or(frame[0])]),
                );
            }
        },
        stream_error(Arc::clone(session), Arc::clone(reported), wake.clone()),
        None,
    )?;

    let gain_bits = Arc::clone(gain);
    let mut previous_gain = f32::from_bits(gain_bits.load(Relaxed));
    let mut priming = true;
    let output_session = Arc::clone(session);
    let playback = output.build_output_stream(
        output_config,
        move |data: &mut [f32], _| {
            data.fill(0.0);
            let frames = data.len() / output_channels;
            if frames == 0 || output_session.stopped() {
                return;
            }

            let mut fill = consumer.slots();
            let expected = (frames as f64 / base_ratio).ceil() as usize;
            // Leave enough queued frames to play until the next capture block.
            let reserve = target.max(capture_block.load(Relaxed));
            let desired = reserve + expected;
            if fill > desired + reserve {
                let skip = fill - desired;
                consumer
                    .read_chunk(skip)
                    .expect("queued frames")
                    .commit(skip);
                fill = desired;
                resampler.reset();
            }

            let gain = f32::from_bits(gain_bits.load(Relaxed));
            let mut played = false;
            if !priming || fill >= desired {
                priming = false;
                let adjustment = ratio_adjustment(fill.saturating_sub(expected), reserve);
                let gain_step = (gain - previous_gain) / frames as f32;
                for chunk in
                    data[..frames * output_channels].chunks_mut(BUFFER_FRAMES * output_channels)
                {
                    let count = chunk.len() / output_channels;
                    resampler.set_chunk_size(count).expect("output block size");
                    resampler
                        .set_resample_ratio_relative(1.0 + adjustment, true)
                        .expect("bounded drift");
                    let needed = resampler.input_frames_next();
                    if consumer.slots() < needed {
                        output_session.underruns.fetch_add(1, Relaxed);
                        resampler.reset();
                        priming = true;
                        break;
                    }

                    let (_, _) = consumer.pop_partial_slice(&mut input_frames[..needed]);
                    let input = InterleavedSlice::new(input_frames.as_flattened(), 2, needed)
                        .expect("input block size");
                    let mut output =
                        InterleavedSlice::new_mut(output_frames.as_flattened_mut(), 2, count)
                            .expect("output block size");
                    let (_, written) = resampler
                        .process_into_buffer(&input, &mut output, None)
                        .expect("resampler buffer sizes");

                    for (frame, &[left, right]) in chunk
                        .chunks_exact_mut(output_channels)
                        .zip(&output_frames[..written])
                    {
                        previous_gain += gain_step;
                        if let [mono] = frame {
                            *mono = (left + right) * 0.5 * previous_gain;
                        } else {
                            frame[0] = left * previous_gain;
                            frame[1] = right * previous_gain;
                        }
                    }
                    played |= written != 0;
                }
            }
            previous_gain = gain;
            let depth_ms = consumer.slots() as f32 * 1000.0 / input_rate as f32;
            output_session.depth_ms.store(depth_ms.to_bits(), Relaxed);
            if played {
                output_session.started.store(stream_id, Release);
            }
        },
        stream_error(Arc::clone(session), Arc::clone(reported), wake.clone()),
        None,
    )?;

    if session.stopped() {
        return Ok(None);
    }
    capture.play()?;
    playback.play()?;
    Ok(Some((capture, playback)))
}

fn ratio_adjustment(fill: usize, target: usize) -> f64 {
    ((1.0 - fill as f64 / target as f64) * 0.002).clamp(-MAX_DRIFT, MAX_DRIFT)
}

#[cfg(target_os = "linux")]
fn usb_parent(path: &Path) -> Option<&Path> {
    path.ancestors()
        .find(|parent| parent.join("idVendor").is_file())
}

#[cfg(target_os = "linux")]
fn usb_audio_parent(path: &Path) -> Option<PathBuf> {
    use std::fs;

    let video =
        fs::canonicalize(Path::new("/sys/class/video4linux").join(path.file_name()?)).ok()?;
    let video = usb_parent(&video)?;
    fs::read_dir("/sys/class/sound")
        .ok()?
        .flatten()
        .filter(|entry| entry.file_name().as_encoded_bytes().starts_with(b"card"))
        .filter_map(|entry| fs::canonicalize(entry.path()).ok())
        .any(|card| usb_parent(&card) == Some(video))
        .then(|| video.to_owned())
}

#[cfg(target_os = "linux")]
fn paired_input(video: PathBuf) -> Result<Option<(Device, cpal::Host)>, Box<dyn Error>> {
    use std::cell::{Cell, RefCell};
    use std::fs;
    use std::rc::Rc;

    use pipewire as pw;
    use pw::types::ObjectType;

    let host = cpal::host_from_id(cpal::HostId::PipeWire)?;
    let names = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let mainloop = pw::main_loop::MainLoopRc::new(None)?;
        let context = pw::context::ContextRc::new(&mainloop, None)?;
        let core = context.connect_rc(None)?;
        let registry = core.get_registry_rc()?;
        let _registry_listener = registry
            .add_listener_local()
            .global({
                let registry = registry.clone();
                let bindings = RefCell::new(Vec::new());
                let names = Rc::clone(&names);
                move |global| {
                    if global.type_ != ObjectType::Node
                        || !global.props.is_some_and(|props| {
                            matches!(
                                props.get("media.class"),
                                Some("Audio/Source" | "Audio/Duplex")
                            )
                        })
                    {
                        return;
                    }
                    let Ok(node) = registry.bind::<pw::node::Node, _>(global) else {
                        return;
                    };
                    let video = video.clone();
                    let names = Rc::clone(&names);
                    let listener = node
                        .add_listener_local()
                        .info(move |info| {
                            let Some(props) = info.props() else { return };
                            let Some(card) = props
                                .get("api.alsa.pcm.card")
                                .and_then(|card| card.parse::<u32>().ok())
                            else {
                                return;
                            };
                            let path =
                                fs::canonicalize(format!("/sys/class/sound/card{card}")).ok();
                            if path.as_deref().and_then(usb_parent) == Some(video.as_path())
                                && let Some(name) = props.get("node.name")
                            {
                                names.borrow_mut().push(name.to_owned());
                            }
                        })
                        .register();
                    bindings.borrow_mut().push((listener, node));
                }
            })
            .register();

        // The registry lists nodes first. Their ALSA properties arrive after binding.
        for _ in 0..2 {
            let pending = core.sync(0)?;
            let done = Rc::new(Cell::new(false));
            let _listener = core
                .add_listener_local()
                .done({
                    let done = Rc::clone(&done);
                    move |id, seq| {
                        if id == pw::core::PW_ID_CORE && seq == pending {
                            done.set(true);
                        }
                    }
                })
                .register();
            while !done.get() {
                if mainloop
                    .loop_()
                    .iterate(pw::loop_::Timeout::Finite(Duration::from_secs(1)))
                    <= 0
                {
                    return Err("PipeWire device lookup timed out".into());
                }
            }
        }
    }
    let input = host.input_devices()?.find(|device| {
        device
            .id()
            .is_ok_and(|id| names.borrow().iter().any(|name| id.id() == name))
    });
    Ok(input.map(|device| (device, host)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratio_correction_returns_fill_to_target() {
        for reserve in [480, 2048] {
            assert_eq!(ratio_adjustment(reserve, reserve), 0.0);
            assert!(ratio_adjustment(reserve / 2, reserve) > 0.0);
            assert!(ratio_adjustment(reserve * 2, reserve) < 0.0);
            assert_eq!(ratio_adjustment(reserve * 100, reserve), -MAX_DRIFT);
            assert!(ratio_adjustment(0, reserve) <= MAX_DRIFT);
        }
    }
}
