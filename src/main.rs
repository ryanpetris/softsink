mod audio;
mod capture;
mod clipboard;
mod decode;
mod inhibit;
mod orientation;
mod render;
mod scale;
mod settings;
mod ui;

use std::error::Error;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, KeyCode, ModifiersState, PhysicalKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::platform::startup_notify::{
    EventLoopExtStartupNotify, WindowAttributesExtStartupNotify,
};
use winit::platform::wayland::WindowAttributesExtWayland;
use winit::window::{Window, WindowId};

use capture::{Event, Inbox};
use render::Renderer;
use scale::View;
use settings::Settings;
use ui::Ui;

/// The deadline an otherwise idle loop waits for. Winit runs no callback after
/// a wake that carries only clipboard events unless a deadline is set, and a
/// paste request from another program is such a wake.
const IDLE: Duration = Duration::from_secs(3600);

struct App {
    audio: audio::Audio,
    inbox: Arc<Mutex<Inbox>>,
    controls: Option<Ui>,
    inhibition: Sender<bool>,
    clipboard: Option<clipboard::Clipboard>,
    modifiers: ModifiersState,
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
                .with_title("Softsink")
                .with_name("dev.petris.softsink", "softsink")
                .with_decorations(false)
                // A smaller window puts the side panel over the button bar.
                .with_min_inner_size(LogicalSize::new(640, 360));
            if let Some([width, height]) = ui.settings.window_size.filter(|size| !size.contains(&0))
            {
                attributes = attributes.with_inner_size(LogicalSize::new(width, height));
            }
            // Handing the launcher's token back ends the desktop's startup
            // sequence, so the window counts as the launched application.
            if let Some(token) = event_loop.read_token_from_env() {
                attributes = attributes.with_activation_token(token);
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
            Event::Encoded(png) => {
                if let Some(clipboard) = &mut self.clipboard
                    && let Err(error) = clipboard.publish(png)
                {
                    eprintln!("Cannot copy the frame: {error}");
                }
            }
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
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
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
                    _ if self.modifiers == ModifiersState::CONTROL
                        && event.key_without_modifiers().as_ref() == Key::Character("c") =>
                    {
                        ui.copy = true;
                    }
                    _ if self.modifiers.control_key()
                        && !self
                            .modifiers
                            .intersects(ModifiersState::ALT | ModifiersState::SUPER) =>
                    {
                        match event.logical_key.as_ref() {
                            Key::Character("=" | "+") => {
                                ui.activity();
                                renderer.zoom(ui, 1.0);
                            }
                            Key::Character("-") => {
                                ui.activity();
                                renderer.zoom(ui, -1.0);
                            }
                            Key::Character("0") => {
                                ui.activity();
                                ui.view = View::default();
                            }
                            _ => {}
                        }
                    }
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
        // Copies the frame on screen after a press of the copy button or
        // Ctrl+C. The next frame's upload queues after the snapshot, so it
        // cannot change the copied image.
        if std::mem::take(&mut ui.copy)
            && let Some(clipboard) = &self.clipboard
            && let Some(snapshot) = renderer.snapshot()
        {
            clipboard.copy(snapshot);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some((renderer, ui)) = &mut self.renderer else {
            return;
        };
        if self.redraw_at.is_some_and(|at| at <= Instant::now()) {
            self.redraw_at = None;
            renderer.window.request_redraw();
        }
        let Some(clipboard) = &mut self.clipboard else {
            event_loop.set_control_flow(
                self.redraw_at
                    .map_or(ControlFlow::Wait, ControlFlow::WaitUntil),
            );
            return;
        };
        if clipboard.dispatch() {
            ui.copied();
            renderer.window.request_redraw();
        }
        let wake = self.redraw_at.unwrap_or_else(|| Instant::now() + IDLE);
        event_loop.set_control_flow(ControlFlow::WaitUntil(wake));
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
    let (clipboard, copier) =
        match clipboard::Clipboard::start(event_loop.owned_display_handle(), proxy.clone()) {
            Ok((clipboard, copier)) => (Some(clipboard), Some(copier)),
            Err(error) => {
                eprintln!("Clipboard unavailable: {error}");
                (None, None)
            }
        };
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
        clipboard,
        modifiers: ModifiersState::empty(),
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
    if let Some(copier) = copier {
        let _ = copier.join();
    }
    result?;
    error.map_or(Ok(()), Err)
}
