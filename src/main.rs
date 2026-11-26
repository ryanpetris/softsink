mod audio;
mod capture;
mod decode;
mod inhibit;
mod render;
mod scale;
mod settings;
mod ui;

use std::error::Error;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::window::{Window, WindowId};

use capture::{Event, Inbox};
use render::Renderer;
use settings::Settings;
use ui::Ui;

struct App {
    audio: audio::Audio,
    inbox: Arc<Mutex<Inbox>>,
    controls: Option<Ui>,
    inhibition: Sender<bool>,
    renderer: Option<(Renderer, Ui)>,
    redraw_at: Option<Instant>,
    error: Option<Box<dyn Error>>,
}

impl ApplicationHandler<Event> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let Some(mut ui) = self.controls.take() else {
            return;
        };
        let result = (|| {
            let mut attributes = Window::default_attributes()
                .with_title("softsink")
                .with_name("dev.petris.softsink", "softsink")
                .with_decorations(false)
                // A smaller window puts the side panel over the button bar.
                .with_min_inner_size(LogicalSize::new(640, 360));
            if let Some([width, height]) = ui.settings.window_size.filter(|size| !size.contains(&0))
            {
                attributes = attributes.with_inner_size(LogicalSize::new(width, height));
            }
            let window = Arc::new(event_loop.create_window(attributes)?);
            let renderer = pollster::block_on(Renderer::new(window.clone()))?;
            ui::theme(renderer.egui_ctx());
            ui.resize(&window, window.inner_size());
            window.request_redraw();
            Ok((renderer, ui))
        })();
        match result {
            Ok(renderer) => self.renderer = Some(renderer),
            Err(error) => {
                self.error = Some(error);
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, _: &ActiveEventLoop, event: Event) {
        let Some((renderer, ui)) = &mut self.renderer else {
            return;
        };
        match event {
            Event::Frame => renderer.window.request_redraw(),
            Event::Stopped => {
                let _ = self.inhibition.send(false);
            }
            Event::ModeFailed(source, mode) => ui.forget_mode(&source, mode),
            Event::Status(status) => {
                self.audio
                    .select(status.device.as_ref().map(|source| source.path.as_path()));
                if status.mode.is_some() {
                    let _ = self.inhibition.send(true);
                } else {
                    renderer.clear_video();
                }
                ui.set_status(status);
                if !renderer.live() {
                    renderer.window.request_redraw();
                }
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some((renderer, ui)) = &mut self.renderer else {
            return;
        };
        let window = renderer.window.clone();
        let mut redraw = false;
        match &event {
            WindowEvent::CursorMoved { position, .. } => ui.pointer_moved(&window, *position),
            WindowEvent::CursorLeft { .. } => ui.pointer_left(),
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } if let Some(edge) = ui.edge => {
                let _ = window.drag_resize_window(edge);
                return;
            }
            WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. } => ui.activity(),
            // Keys go to the program only, which keeps egui's keyboard
            // navigation off.
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && !event.repeat =>
            {
                match event.physical_key {
                    PhysicalKey::Code(KeyCode::F11) => {
                        // Shows the controls so the egui pass runs and hides
                        // the cursor when they slide away.
                        ui.activity();
                        ui::toggle_fullscreen(&window);
                    }
                    PhysicalKey::Code(KeyCode::Escape) => ui.escape(renderer.egui_ctx(), &window),
                    _ => {}
                }
                redraw = true;
            }
            _ => {}
        }
        if !matches!(event, WindowEvent::KeyboardInput { .. }) {
            redraw |= renderer.window_event(&event);
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                renderer.resize(size);
                ui.resize(&window, size);
                window.request_redraw();
            }
            WindowEvent::Occluded(false) | WindowEvent::ScaleFactorChanged { .. } => {
                window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                self.redraw_at = renderer.draw(&self.inbox, ui, &self.audio);
                if ui.quit {
                    event_loop.exit();
                }
            }
            // While video is live, the next frame picks up the input.
            _ if redraw && !renderer.live() => window.request_redraw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some((renderer, _)) = &self.renderer else {
            return;
        };
        match self.redraw_at {
            Some(at) if at <= Instant::now() => {
                self.redraw_at = None;
                renderer.window.request_redraw();
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            Some(at) => event_loop.set_control_flow(ControlFlow::WaitUntil(at)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    if let Some(arg) = args.next() {
        if arg == "--version" && args.next().is_none() {
            let version = option_env!("SOFTSINK_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"));
            println!("softsink {version}");
            return Ok(());
        }
        return Err("expected no arguments or --version".into());
    }

    let path = settings::path();
    let settings = Settings::load(path.as_deref());
    let event_loop = EventLoop::with_user_event().build()?;
    let audio = audio::Audio::new(settings.volume, settings.muted)?;
    let inbox = Arc::new(Mutex::new(Inbox::default()));
    let proxy = event_loop.create_proxy();
    let (commands, receiver) = mpsc::channel();
    let (scan, requests) = mpsc::sync_channel(1);
    let scan_commands = commands.clone();
    std::thread::Builder::new()
        .name("scan".into())
        .spawn(move || capture::scan_on_request(requests, scan_commands))?;
    let (save, changes) = mpsc::channel();
    let saver = std::thread::Builder::new()
        .name("settings".into())
        .spawn(move || settings::save_changes(path, changes))?;
    let (inhibition, states) = mpsc::channel();
    let inhibitor = std::thread::Builder::new()
        .name("inhibit".into())
        .spawn(move || inhibit::run(states))?;
    let capture_inbox = Arc::clone(&inbox);
    let source = settings.source.clone();
    let modes = settings.modes.clone();
    std::thread::Builder::new()
        .name("capture".into())
        .spawn(move || capture::run(capture_inbox, proxy, receiver, source, modes))?;
    let mut app = App {
        audio,
        inbox,
        controls: Some(Ui::new(commands, scan, settings, save)),
        inhibition,
        renderer: None,
        redraw_at: None,
        error: None,
    };
    let result = event_loop.run_app(&mut app);
    if let Some((_, ui)) = &mut app.renderer {
        ui.save_window_size();
    }
    let error = app.error.take();
    drop(app);
    let _ = saver.join();
    let _ = inhibitor.join();
    result?;
    error.map_or(Ok(()), Err)
}
