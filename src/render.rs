use std::collections::VecDeque;
use std::error::Error;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui_wgpu::ScreenDescriptor;
use wgpu::*;
use winit::dpi::PhysicalSize;
use winit::event::WindowEvent;
use winit::window::Window;

use crate::audio::{Audio, Figures};
use crate::capture::{Frame, Inbox, Mode, monotonic_now};
use crate::decode::{Color, Layout, LayoutKind};
use crate::orientation::Orientation;
use crate::scale;
use crate::ui::{Ui, video_area};

/// How long after the latest frame the video counts as live. While it is
/// live, frame arrival drives redraws and the interface requests none.
const LIVE: Duration = Duration::from_millis(100);
/// Redraw interval for animations when no video arrives.
const ANIMATION_FRAME: Duration = Duration::from_millis(8);
const ORIENTATION_OFFSET: u64 = 32;
const CROP_OFFSET: u64 = ORIENTATION_OFFSET + 16;
const FULL_CROP: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

pub struct Renderer {
    pub window: Arc<Window>,
    instance: Instance,
    surface: Surface<'static>,
    config: SurfaceConfiguration,
    size: PhysicalSize<u32>,
    device: Device,
    queue: Queue,
    pipeline: RenderPipeline,
    corners: RenderPipeline,
    sampler: Sampler,
    uniform: Buffer,
    orientation: Option<Orientation>,
    video: Option<Video>,
    egui: egui_winit::State,
    overlay: egui_wgpu::Renderer,
    stats: Stats,
    last_frame: Option<Instant>,
}

struct Video {
    layout: Layout,
    textures: [Option<Texture>; 3],
    bindings: BindGroup,
    mode: Mode,
    color: Color,
}

/// The oriented frame rendered at capture resolution into a buffer the
/// clipboard thread maps once the GPU finishes.
pub struct Snapshot {
    pub device: Device,
    pub buffer: Buffer,
    pub width: u32,
    pub height: u32,
    /// Bytes per row in `buffer`, padded to the copy alignment.
    pub stride: u32,
    pub bgra: bool,
}

/// How far back the statistics reach.
pub const WINDOW: Duration = Duration::from_secs(5);
/// How long after the latest frame the readout's figures count as stale.
pub const STALE: Duration = Duration::from_secs(1);

/// Timing of one presented frame, in milliseconds. Transfer, decode, and
/// render add up to the latency.
pub struct Sample {
    /// When the present call was made, on the monotonic clock.
    pub at: Duration,
    /// Time from capture to the present call.
    pub latency: f32,
    /// Time from capture until the capture thread dequeues the buffer.
    pub transfer: f32,
    /// Time from the dequeue until the frame is ready for the renderer.
    pub decode: f32,
    /// Time from then until the present call.
    pub render: f32,
    /// Time since the previous frame's present call.
    pub frame_time: f32,
}

/// Average, 99th percentile, and maximum of a measure over the window.
pub struct Spread {
    pub avg: f32,
    pub p99: f32,
    pub max: f32,
}

impl Spread {
    /// Sorts `values` and summarizes them.
    fn of(values: &mut [f32]) -> Self {
        values.sort_by(f32::total_cmp);
        Self {
            avg: values.iter().sum::<f32>() / values.len() as f32,
            p99: percentile(values, 0.99),
            max: values[values.len() - 1],
        }
    }
}

/// The smallest value with at least `fraction` of `sorted` at or below it.
fn percentile(sorted: &[f32], fraction: f32) -> f32 {
    let rank = (fraction * sorted.len() as f32).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// Top of the latency graph: twice the median latency, rounded up to a
/// multiple of 5 ms so the scale changes in visible steps.
fn latency_top(median: f32) -> f32 {
    (median * 2.0 / 5.0).ceil().max(1.0) * 5.0
}

/// Figures over the window, refreshed once per second so they stay readable.
pub struct Summary {
    pub latency: Spread,
    pub transfer: Spread,
    pub decode: Spread,
    pub render: Spread,
    pub frame_time: Spread,
    pub latency_top: f32,
}

#[derive(Default)]
pub struct Stats {
    started: Option<Duration>,
    frames: u32,
    /// When the latest frame was presented, on the monotonic clock.
    pub last_present: Option<Duration>,
    /// Frames presented within the window, oldest first.
    pub samples: VecDeque<Sample>,
    pub fps: Option<f64>,
    pub summary: Option<Summary>,
    /// Audio buffer depth in milliseconds and underrun count while audio
    /// plays, refreshed with the summary.
    pub audio: Option<Figures>,
}

impl Stats {
    /// Records a frame presented at `now`, given when it was captured,
    /// dequeued, and ready, all on the monotonic clock.
    fn record(&mut self, now: Duration, [captured, dequeued, ready]: [Duration; 3], audio: &Audio) {
        let ms = |time: Duration| time.as_secs_f32() * 1000.0;
        let parts = [(captured, dequeued), (dequeued, ready), (ready, now)]
            .map(|(start, end)| end.checked_sub(start));
        // The first frame has no frame time, so it adds no sample.
        if let ([Some(transfer), Some(decode), Some(render)], Some(last)) =
            (parts, self.last_present)
        {
            while self
                .samples
                .front()
                .is_some_and(|sample| now.saturating_sub(sample.at) > WINDOW)
            {
                self.samples.pop_front();
            }
            self.samples.push_back(Sample {
                at: now,
                latency: ms(transfer + decode + render),
                transfer: ms(transfer),
                decode: ms(decode),
                render: ms(render),
                frame_time: ms(now.saturating_sub(last)),
            });
        }
        self.last_present = Some(now);
        self.frames += 1;
        let started = *self.started.get_or_insert(now);
        let elapsed = now.saturating_sub(started);
        if elapsed >= Duration::from_secs(1) {
            self.fps = Some(f64::from(self.frames) / elapsed.as_secs_f64());
            self.started = Some(now);
            self.frames = 0;
            self.summary = (!self.samples.is_empty()).then(|| self.summarize());
            self.audio = audio.readings().figures;
        }
    }

    fn summarize(&self) -> Summary {
        let values = |value: fn(&Sample) -> f32| self.samples.iter().map(value).collect::<Vec<_>>();
        let mut latency = values(|sample| sample.latency);
        Summary {
            latency: Spread::of(&mut latency),
            latency_top: latency_top(percentile(&latency, 0.5)),
            transfer: Spread::of(&mut values(|sample| sample.transfer)),
            decode: Spread::of(&mut values(|sample| sample.decode)),
            render: Spread::of(&mut values(|sample| sample.render)),
            frame_time: Spread::of(&mut values(|sample| sample.frame_time)),
        }
    }
}

impl Renderer {
    pub async fn new(window: Arc<Window>) -> Result<Self, Box<dyn Error>> {
        let instance = Instance::new(InstanceDescriptor::new_without_display_handle());
        let surface = instance.create_surface(window.clone())?;
        let adapter = instance
            .request_adapter(&RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await?;
        let (device, queue) = adapter
            .request_device(&DeviceDescriptor {
                // The surface and capture textures can exceed the default
                // texture size limit.
                required_limits: Limits {
                    max_texture_dimension_2d: adapter.limits().max_texture_dimension_2d,
                    ..Default::default()
                },
                ..Default::default()
            })
            .await?;
        let caps = surface.get_capabilities(&adapter);
        let size = window.inner_size();
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|format| {
                matches!(
                    format,
                    TextureFormat::Bgra8Unorm | TextureFormat::Rgba8Unorm
                )
            })
            .ok_or("Surface does not support Bgra8Unorm or Rgba8Unorm")?;
        if !caps.present_modes.contains(&PresentMode::Mailbox) {
            return Err("Surface does not support mailbox presentation".into());
        }
        if !caps.alpha_modes.contains(&CompositeAlphaMode::Opaque) {
            return Err("Surface does not support opaque alpha".into());
        }
        if !caps
            .alpha_modes
            .contains(&CompositeAlphaMode::PreMultiplied)
        {
            return Err("Surface does not support pre-multiplied alpha".into());
        }
        let config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: SurfaceColorSpace::Auto,
            width: size.width,
            height: size.height,
            present_mode: PresentMode::Mailbox,
            desired_maximum_frame_latency: 1,
            alpha_mode: CompositeAlphaMode::Opaque,
            view_formats: vec![],
        };
        let create_pipeline = |label, source: &str, blend| {
            let shader = device.create_shader_module(ShaderModuleDescriptor {
                label: Some(label),
                source: ShaderSource::Wgsl(source.into()),
            });
            device.create_render_pipeline(&RenderPipelineDescriptor {
                label: Some(label),
                layout: None,
                vertex: VertexState {
                    module: &shader,
                    entry_point: Some("vertex"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(FragmentState {
                    module: &shader,
                    entry_point: Some("fragment"),
                    compilation_options: Default::default(),
                    targets: &[Some(ColorTargetState {
                        format,
                        blend,
                        write_mask: ColorWrites::ALL,
                    })],
                }),
                primitive: PrimitiveState::default(),
                depth_stencil: None,
                multisample: MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipeline = create_pipeline("Video", include_str!("video.wgsl"), None);
        // Multiplies the colour and alpha already drawn by the shader's alpha.
        let multiply = BlendComponent {
            src_factor: BlendFactor::Zero,
            dst_factor: BlendFactor::SrcAlpha,
            operation: BlendOperation::Add,
        };
        let corners = create_pipeline(
            "Window corners",
            include_str!("corners.wgsl"),
            Some(BlendState {
                color: multiply,
                alpha: multiply,
            }),
        );
        let sampler = device.create_sampler(&SamplerDescriptor {
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..Default::default()
        });
        let uniform = device.create_buffer(&BufferDescriptor {
            label: Some("Video parameters"),
            size: CROP_OFFSET + 16,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let egui = egui_winit::State::new(
            egui::Context::default(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(device.limits().max_texture_dimension_2d as usize),
        );
        let overlay = egui_wgpu::Renderer::new(&device, format, Default::default());
        let mut renderer = Self {
            window,
            instance,
            surface,
            config,
            size,
            device,
            queue,
            pipeline,
            corners,
            sampler,
            uniform,
            orientation: None,
            video: None,
            egui,
            overlay,
            stats: Stats::default(),
            last_frame: None,
        };
        renderer.resize(size);
        Ok(renderer)
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        self.size = size;
        if size.width != 0 && size.height != 0 {
            self.config.width = size.width;
            self.config.height = size.height;
            self.surface.configure(&self.device, &self.config);
        }
    }

    pub fn egui_ctx(&self) -> &egui::Context {
        self.egui.egui_ctx()
    }

    /// Passes an event to egui and returns whether egui wants a redraw.
    pub fn window_event(&mut self, event: &WindowEvent) -> bool {
        self.egui.on_window_event(&self.window, event).repaint
    }

    pub fn zoom(&self, ui: &mut Ui, steps: f64) {
        if let Some(video) = &self.video
            && let Some(geometry) =
                ui.video_geometry(ui.video_top(&self.window), &self.screen(), video.mode)
        {
            geometry.zoom(&mut ui.view, steps, geometry.center());
        }
    }

    fn screen(&self) -> ScreenDescriptor {
        ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: egui_winit::pixels_per_point(self.egui.egui_ctx(), &self.window),
        }
    }

    fn write_crop(&self, crop: [f32; 4]) {
        let bytes = crop.map(f32::to_le_bytes);
        self.queue
            .write_buffer(&self.uniform, CROP_OFFSET, bytes.as_flattened());
    }

    pub fn live(&self) -> bool {
        self.last_frame.is_some_and(|at| at.elapsed() < LIVE)
    }

    pub fn clear_video(&mut self) {
        self.video = None;
        self.last_frame = None;
    }

    /// Draws the latest frame and the interface, and returns when the
    /// interface needs the next redraw if no frame arrives first.
    pub fn draw(&mut self, inbox: &Mutex<Inbox>, ui: &mut Ui, audio: &Audio) -> Option<Instant> {
        if self.size.width == 0 || self.size.height == 0 {
            return None;
        }
        // Rounded corners need a surface the compositor blends. In every other
        // state the surface stays opaque so the compositor skips blending.
        let rounded = ui.rounded_corners(&self.window);
        let alpha_mode = if rounded {
            CompositeAlphaMode::PreMultiplied
        } else {
            CompositeAlphaMode::Opaque
        };
        if self.config.alpha_mode != alpha_mode {
            self.config.alpha_mode = alpha_mode;
            self.surface.configure(&self.device, &self.config);
            self.window.set_transparent(rounded);
        }
        let output = match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(output) | CurrentSurfaceTexture::Suboptimal(output) => {
                output
            }
            CurrentSurfaceTexture::Outdated => {
                self.resize(self.window.inner_size());
                self.window.request_redraw();
                return None;
            }
            CurrentSurfaceTexture::Lost => {
                if let Ok(surface) = self.instance.create_surface(self.window.clone()) {
                    self.surface = surface;
                    self.resize(self.window.inner_size());
                    self.window.request_redraw();
                }
                return None;
            }
            _ => {
                let mut inbox = inbox.lock().unwrap();
                if let Some(frame) = inbox.latest.take() {
                    inbox.spare = frame.bytes;
                }
                return None;
            }
        };

        let frame = inbox.lock().unwrap().latest.take();
        let timing = frame.map(|frame| {
            self.upload(&frame);
            let timing = [frame.timestamp, frame.dequeued, frame.ready];
            inbox.lock().unwrap().spare = frame.bytes;
            timing
        });

        let view = output
            .texture
            .create_view(&TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor::default());
        let screen = self.screen();
        let input = self.egui.take_egui_input(&self.window);
        let mut repaint = None;
        let mut paint = Vec::new();
        let mut free = Vec::new();
        let mut callback_commands = Vec::new();
        // Pointer events still update the video input while the controls are hidden.
        if ui.visible(&self.window) || !input.events.is_empty() {
            let mode = self.video.as_ref().map(|video| video.mode);
            let mut output = self.egui.egui_ctx().run_ui(input, |root| {
                ui.show(root, &self.window, mode, &self.stats, audio, &screen);
            });
            self.egui
                .handle_platform_output(&self.window, output.platform_output);
            repaint = output
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .map(|viewport| viewport.repaint_delay);
            paint = self
                .egui
                .egui_ctx()
                .tessellate(output.shapes, output.pixels_per_point);
            for (id, deltas) in output.textures_delta.set.drain() {
                for delta in deltas {
                    self.overlay
                        .update_texture(&self.device, &self.queue, id, &delta);
                }
            }
            free.extend(output.textures_delta.free.drain());
        }
        if !paint.is_empty() {
            callback_commands = self.overlay.update_buffers(
                &self.device,
                &self.queue,
                &mut encoder,
                &paint,
                &screen,
            );
        }
        let top = ui.video_top(&self.window);
        let area = video_area(top, &screen);
        let top_px = self.config.height - area[1];
        let orientation = ui.orientation();
        if self.orientation != Some(orientation) {
            let uniform = orientation.uv_rows().map(f32::to_le_bytes);
            self.queue
                .write_buffer(&self.uniform, ORIENTATION_OFFSET, uniform.as_flattened());
            self.orientation = Some(orientation);
        }

        {
            let mut pass = encoder
                .begin_render_pass(&RenderPassDescriptor {
                    color_attachments: &[Some(RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: Operations {
                            load: LoadOp::Clear(wgpu::Color::BLACK),
                            store: StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            if let Some(video) = &self.video
                && let Some(geometry) = ui.video_geometry(top, &screen, video.mode)
            {
                let ([x, y, width, height], crop) = scale::clip(geometry.place(ui.view), area);
                self.write_crop(crop);
                pass.set_viewport(x, y + top_px as f32, width, height, 0.0, 1.0);
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &video.bindings, &[]);
                pass.draw(0..3, 0..1);
            }
            if !paint.is_empty() {
                self.overlay.render(&mut pass, &paint, &screen);
            }
            // The layout can change while the interface is drawn, and a frame
            // that switched to immersive keeps its corners.
            if rounded && ui.rounded_corners(&self.window) {
                let style = self.egui.egui_ctx().global_style();
                let radius =
                    f32::from(style.visuals.window_corner_radius.nw) * screen.pixels_per_point;
                pass.set_pipeline(&self.corners);
                pass.set_scissor_rect(0, 0, self.config.width, self.config.height);
                for (mirror, x) in [(0, 0.0), (1, self.config.width as f32 - radius)] {
                    pass.set_viewport(x, 0.0, radius, radius, 0.0, 1.0);
                    pass.draw(0..3, mirror..mirror + 1);
                }
            }
        }
        self.queue
            .submit(callback_commands.into_iter().chain([encoder.finish()]));
        let now = monotonic_now();
        self.queue.present(output);
        if let Some(timing) = timing {
            self.stats.record(now, timing, audio);
            self.last_frame = Some(Instant::now());
        }
        for id in free {
            self.overlay.free_texture(&id);
        }
        let floor = if self.live() { LIVE } else { ANIMATION_FRAME };
        let next = repaint.and_then(|delay| Instant::now().checked_add(delay.max(floor)));
        // The readout replaces its figures with placeholders once frames
        // stop, which needs one redraw when they go stale.
        let stale = self
            .last_frame
            .filter(|at| ui.settings.stats && at.elapsed() < STALE)
            .map(|at| at + STALE);
        next.into_iter().chain(stale).min()
    }

    /// Renders the current frame at its oriented capture size and copies it
    /// into a mappable buffer, or returns `None` while there is no frame.
    pub fn snapshot(&self) -> Option<Snapshot> {
        let video = self.video.as_ref()?;
        let [width, height] = self
            .orientation?
            .size([video.mode.width, video.mode.height]);
        let size = Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&TextureDescriptor {
            label: Some("Snapshot"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: self.config.format,
            usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let stride = (size.width * 4).next_multiple_of(COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = self.device.create_buffer(&BufferDescriptor {
            label: Some("Snapshot"),
            size: u64::from(stride) * u64::from(size.height),
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        self.write_crop(FULL_CROP);
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor::default());
        {
            let view = texture.create_view(&TextureViewDescriptor::default());
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(wgpu::Color::BLACK),
                        store: StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &video.bindings, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            TexelCopyBufferInfo {
                buffer: &buffer,
                layout: TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(stride),
                    rows_per_image: None,
                },
            },
            size,
        );
        self.queue.submit([encoder.finish()]);
        Some(Snapshot {
            device: self.device.clone(),
            buffer,
            width: size.width,
            height: size.height,
            stride,
            bgra: self.config.format == TextureFormat::Bgra8Unorm,
        })
    }

    fn upload(&mut self, frame: &Frame) {
        let layout_changed = self
            .video
            .as_ref()
            .is_none_or(|video| video.layout != frame.layout);
        if layout_changed {
            if self.video.is_none() {
                self.stats = Stats::default();
            }
            let textures = frame.layout.planes.map(|plane| {
                plane.map(|plane| {
                    self.device.create_texture(&TextureDescriptor {
                        label: Some("Video plane"),
                        size: Extent3d {
                            width: plane.width,
                            height: plane.height,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: TextureDimension::D2,
                        format: match plane.channels {
                            1 => TextureFormat::R8Unorm,
                            2 => TextureFormat::Rg8Unorm,
                            4 => TextureFormat::Rgba8Unorm,
                            _ => unreachable!(),
                        },
                        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                        view_formats: &[],
                    })
                })
            });
            // The shader skips unused planes. Bind the luma view to their slots.
            let views = textures.each_ref().map(|texture| {
                texture
                    .as_ref()
                    .or(textures[0].as_ref())
                    .unwrap()
                    .create_view(&TextureViewDescriptor::default())
            });
            let bindings = self.device.create_bind_group(&BindGroupDescriptor {
                label: Some("Video planes"),
                layout: &self.pipeline.get_bind_group_layout(0),
                entries: &[
                    BindGroupEntry {
                        binding: 0,
                        resource: BindingResource::TextureView(&views[0]),
                    },
                    BindGroupEntry {
                        binding: 1,
                        resource: BindingResource::TextureView(&views[1]),
                    },
                    BindGroupEntry {
                        binding: 2,
                        resource: BindingResource::TextureView(&views[2]),
                    },
                    BindGroupEntry {
                        binding: 3,
                        resource: BindingResource::Sampler(&self.sampler),
                    },
                    BindGroupEntry {
                        binding: 4,
                        resource: self.uniform.as_entire_binding(),
                    },
                ],
            });
            self.video = Some(Video {
                layout: frame.layout,
                textures,
                bindings,
                mode: frame.mode,
                color: frame.color,
            });
        }
        let video = self.video.as_mut().unwrap();
        if layout_changed || video.color != frame.color || video.mode != frame.mode {
            let (y_offset, y_scale, chroma_scale) = if frame.color.full_range {
                (0.0f32, 1.0f32, 1.0f32)
            } else {
                (16.0 / 255.0, 255.0 / 219.0, 255.0 / 224.0)
            };
            let kind = match frame.layout.kind {
                LayoutKind::Planar => 0,
                LayoutKind::Nv12 => 1,
                LayoutKind::Yuyv => 2,
            };
            let words = [
                frame.color.kr.to_bits(),
                frame.color.kb.to_bits(),
                y_offset.to_bits(),
                y_scale.to_bits(),
                chroma_scale.to_bits(),
                kind,
                frame.mode.width,
                frame.mode.height,
            ];
            let uniform = words.map(u32::to_le_bytes);
            self.queue
                .write_buffer(&self.uniform, 0, uniform.as_flattened());
            video.mode = frame.mode;
            video.color = frame.color;
        }
        for (plane, texture) in frame
            .layout
            .planes
            .into_iter()
            .flatten()
            .zip(video.textures.iter().flatten())
        {
            self.queue.write_texture(
                texture.as_image_copy(),
                &frame.bytes[plane.offset..],
                TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(plane.stride),
                    rows_per_image: None,
                },
                texture.size(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{latency_top, percentile};

    #[test]
    fn percentile_takes_the_nearest_rank() {
        let sorted: Vec<f32> = (1..=200).map(|value| value as f32).collect();
        assert_eq!(percentile(&sorted, 0.99), 198.0);
        assert_eq!(percentile(&sorted[..1], 0.99), 1.0);
    }

    #[test]
    fn latency_top_doubles_the_median_up_to_a_multiple_of_five() {
        assert_eq!(latency_top(11.6), 25.0);
        assert_eq!(latency_top(10.0), 20.0);
        assert_eq!(latency_top(0.0), 5.0);
    }
}
