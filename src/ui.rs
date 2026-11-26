use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::mpsc::{Sender, SyncSender};
use std::time::{Duration, Instant};

use egui::containers::menu::{MenuConfig, SubMenuButton};
use egui::{
    Align, Align2, Button, Color32, CornerRadius, CursorIcon, FontData, FontDefinitions,
    FontFamily, FontId, Frame, Id, Margin, Order, Popup, Rect, RichText, Sense, Shadow, Slider,
    Stroke, TextStyle, pos2, vec2,
};
use egui_phosphor::regular as icon;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::window::{Fullscreen, ResizeDirection, Window};

use crate::audio::{Audio, Readings};
use crate::capture::{Command, Mode, Status, monotonic_now};
use crate::render::{STALE, Sample, Stats, WINDOW};
use crate::scale::Scale;
use crate::settings::{Layout, Settings};

const HIDE_AFTER: Duration = Duration::from_secs(2);
/// How long the copy button shows a check after a frame reaches the clipboard.
const COPIED_FOR: Duration = Duration::from_secs(1);
const SLIDE_SECONDS: f32 = 0.18;
/// Height of the framed layout's header bar, in points.
const HEADER_HEIGHT: f32 = 40.0;
/// Width of the band along the window border that resizes the window, in points.
const EDGE: f64 = 6.0;
const PANEL_WIDTH: f32 = 300.0;
const PANEL_MARGIN: f32 = 12.0;
const MENU_WIDTH: f32 = 140.0;
const CELL_WIDTH: f32 = 60.0;
const VOLUME_WIDTH: f32 = 96.0;
const ICON_SIZE: f32 = 18.0;
const READOUT_SIZE: f32 = 12.0;
/// Indent of the parts of the latency under it in the readout.
const INDENT: f32 = 10.0;
const GRAPH_HEIGHT: f32 = 36.0;

const SURFACE: Color32 = Color32::from_rgba_premultiplied(16, 17, 20, 225);
const TEXT: Color32 = Color32::from_rgb(226, 228, 233);
const WEAK: Color32 = Color32::from_rgb(140, 145, 155);
const ACCENT: Color32 = Color32::from_rgb(110, 168, 255);
const CLOSE: Color32 = Color32::from_rgb(196, 43, 28);
const BORDER: Stroke = Stroke {
    width: 1.0,
    color: Color32::from_rgba_premultiplied(18, 18, 18, 18),
};
const SHADOW: Shadow = Shadow {
    offset: [0, 4],
    blur: 18,
    spread: 0,
    color: Color32::from_black_alpha(90),
};

pub struct Ui {
    status: Status,
    no_signal: bool,
    window_size: Option<[u32; 2]>,
    pub settings: Settings,
    pub quit: bool,
    /// Requests a copy of the frame on screen. Set by the copy button and
    /// Ctrl+C.
    pub copy: bool,
    copied_at: Option<Instant>,
    commands: Sender<Command>,
    scan: SyncSender<()>,
    save: Sender<Settings>,
    source_menu_open: bool,
    panel: bool,
    last_activity: Instant,
    /// The border under the pointer. A press there resizes the window
    /// instead of reaching the interface.
    pub edge: Option<ResizeDirection>,
    /// State from the latest pass, read by code that runs between passes.
    over_controls: bool,
    menu_open: bool,
    shown: f32,
}

impl Ui {
    pub fn new(
        commands: Sender<Command>,
        scan: SyncSender<()>,
        settings: Settings,
        save: Sender<Settings>,
    ) -> Self {
        Self {
            status: Status::default(),
            no_signal: false,
            window_size: None,
            settings,
            quit: false,
            copy: false,
            copied_at: None,
            commands,
            scan,
            save,
            source_menu_open: false,
            panel: false,
            last_activity: Instant::now(),
            edge: None,
            over_controls: false,
            menu_open: false,
            shown: 0.0,
        }
    }

    fn save_settings(&self) {
        let _ = self.save.send(self.settings.clone());
    }

    pub fn set_status(&mut self, status: Status) {
        self.no_signal = status.device.is_none();
        self.status = status;
    }

    pub fn forget_mode(&mut self, source: &str, mode: Mode) {
        if self.settings.modes.get(source) == Some(&mode) {
            self.settings.modes.remove(source);
            self.save_settings();
        }
    }

    pub fn save_window_size(&mut self) {
        if let Some(size) = self.window_size
            && self.settings.window_size != Some(size)
        {
            self.settings.window_size = Some(size);
            self.save_settings();
        }
    }

    pub fn resize(&mut self, window: &Window, size: PhysicalSize<u32>) {
        let size = size.to_logical::<u32>(window.scale_factor());
        let size = [size.width, size.height];
        if window.fullscreen().is_none() && !window.is_maximized() && !size.contains(&0) {
            self.window_size = Some(size);
        }
    }

    fn immersive(&self, window: &Window) -> bool {
        self.settings.layout == Layout::Immersive || window.fullscreen().is_some()
    }

    /// Whether the window's top corners are rounded. The immersive layout
    /// keeps them square so no video is clipped.
    pub fn rounded_corners(&self, window: &Window) -> bool {
        !self.immersive(window) && !window.is_maximized()
    }

    fn awake(&self) -> bool {
        self.last_activity.elapsed() < HIDE_AFTER || self.over_controls || self.menu_open
    }

    /// Whether the next frame draws any interface. When it does not, the
    /// renderer skips the egui pass.
    pub fn visible(&self, window: &Window) -> bool {
        !self.immersive(window)
            || self.no_signal
            || self.settings.stats
            || self.shown > 0.0
            || self.awake()
    }

    /// Top of the video area in points.
    pub fn video_top(&self, window: &Window) -> f32 {
        if self.immersive(window) {
            0.0
        } else {
            HEADER_HEIGHT
        }
    }

    pub fn activity(&mut self) {
        self.last_activity = Instant::now();
    }

    /// Shows a frame reaching the clipboard on the copy button, revealing
    /// the immersive bar so the check is seen.
    pub fn copied(&mut self) {
        self.copied_at = Some(Instant::now());
        self.activity();
    }

    /// Records pointer motion and which window border, if any, is under the
    /// pointer.
    pub fn pointer_moved(&mut self, window: &Window, position: PhysicalPosition<f64>) {
        self.activity();
        let size = window.inner_size();
        let edge = EDGE * window.scale_factor();
        let resizable = window.fullscreen().is_none() && !window.is_maximized();
        let north = position.y < edge;
        let south = position.y >= f64::from(size.height) - edge;
        let west = position.x < edge;
        let east = position.x >= f64::from(size.width) - edge;
        self.edge = match (north, south, west, east) {
            _ if !resizable => None,
            (true, _, true, _) => Some(ResizeDirection::NorthWest),
            (true, _, _, true) => Some(ResizeDirection::NorthEast),
            (_, true, true, _) => Some(ResizeDirection::SouthWest),
            (_, true, _, true) => Some(ResizeDirection::SouthEast),
            (true, ..) => Some(ResizeDirection::North),
            (_, true, ..) => Some(ResizeDirection::South),
            (.., true, _) => Some(ResizeDirection::West),
            (.., true) => Some(ResizeDirection::East),
            _ => None,
        };
    }

    pub fn pointer_left(&mut self) {
        self.edge = None;
    }

    /// Closes an open menu or the panel, or otherwise leaves fullscreen.
    pub fn escape(&mut self, ctx: &egui::Context, window: &Window) {
        if Popup::is_any_open(ctx) {
            Popup::close_all(ctx);
        } else if self.panel {
            self.panel = false;
        } else if window.fullscreen().is_some() {
            window.set_fullscreen(None);
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        window: &Window,
        mode: Option<Mode>,
        stats: &Stats,
        audio: &Audio,
    ) {
        let ctx = ui.ctx().clone();
        let rect = ui.max_rect();
        let mut over_controls = false;
        let mut top = 0.0;
        if self.immersive(window) {
            self.source_menu_open = false;
            let awake = self.awake();
            if !awake {
                self.panel = false;
            } else if !self.over_controls && !self.menu_open {
                ctx.request_repaint_after(HIDE_AFTER.saturating_sub(self.last_activity.elapsed()));
            }
            self.shown = ctx.animate_bool_with_time(Id::unique("bar"), awake, SLIDE_SECONDS);
            let panel = ctx.animate_bool_with_time(Id::unique("panel"), self.panel, SLIDE_SECONDS);
            if self.shown == 0.0 {
                ctx.set_cursor_icon(CursorIcon::None);
            }
            let video = ui.interact(ui.max_rect(), Id::unique("video"), Sense::drag());
            // The compositor's move grab takes the button release, so egui
            // would otherwise still see the drag as held.
            if video.drag_started() && window.drag_window().is_ok() {
                ctx.stop_dragging();
            }
            let pointer = ctx.input(|input| input.pointer.hover_pos());
            if self.shown > 0.0 {
                let rect = self.bar(&ctx, window, panel, mode.is_some());
                over_controls |= pointer.is_some_and(|pointer| rect.contains(pointer));
            }
            if panel > 0.0 {
                let rect = self.panel(&ctx, panel, audio);
                over_controls |= pointer.is_some_and(|pointer| rect.contains(pointer));
            }
        } else {
            self.header(ui, window, audio, mode.is_some());
            top = HEADER_HEIGHT;
        }
        if self.no_signal {
            ui.painter().text(
                rect.center() + vec2(0.0, top / 2.0),
                Align2::CENTER_CENTER,
                "No signal",
                FontId::proportional(18.0),
                TEXT,
            );
        } else if self.settings.stats
            && let Some(mode) = mode
            && readout(
                &ctx,
                top,
                mode,
                stats,
                &audio.readings(),
                self.settings.breakdown,
            )
        {
            self.settings.breakdown = !self.settings.breakdown;
            self.save_settings();
        }
        if let Some(edge) = self.edge {
            ctx.set_cursor_icon(resize_cursor(edge));
        }
        self.over_controls = over_controls;
        self.menu_open = Popup::is_any_open(&ctx);
    }

    fn header(&mut self, ui: &mut egui::Ui, window: &Window, audio: &Audio, video: bool) {
        let frame = Frame::new()
            .fill(SURFACE)
            .inner_margin(Margin::symmetric(8, 0));
        egui::Panel::top("header")
            .exact_size(HEADER_HEIGHT)
            .resizable(false)
            .frame(frame)
            .show(ui, |ui| {
                // Sensing clicks as well makes egui wait for pointer movement
                // before the drag starts, so a press on a header button stays
                // a click.
                let sense = Sense::click_and_drag();
                let drag = ui.interact(ui.max_rect(), Id::unique("header"), sense);
                if drag.drag_started() && window.drag_window().is_ok() {
                    ui.ctx().stop_dragging();
                }
                // The menu bar starts its row at the interaction height and
                // grows it downward to fit its buttons. Justifying it to the
                // full header height keeps the buttons centred.
                let layout = egui::Layout::left_to_right(Align::Center).with_cross_justify(true);
                ui.with_layout(layout, |ui| {
                    // The theme already styles menus, so egui's menu style is not applied.
                    let config = MenuConfig::new().style(|_: &mut egui::Style| {});
                    egui::MenuBar::new()
                        .style(|_: &mut egui::Style| {})
                        .config(config)
                        .ui(ui, |ui| {
                            let source_menu_open = ui
                                .menu_button("Source", |ui| {
                                    ui.set_min_width(MENU_WIDTH);
                                    self.sources(ui);
                                })
                                .inner
                                .is_some();
                            if source_menu_open && !self.source_menu_open {
                                let _ = self.scan.try_send(());
                            }
                            self.source_menu_open = source_menu_open;
                            ui.menu_button("Mode", |ui| self.mode_menu(ui));
                            ui.menu_button("Scale", |ui| {
                                ui.set_min_width(MENU_WIDTH);
                                self.scales(ui);
                            });
                            self.stats_button(ui);
                            let readings = audio.readings();
                            if readings.available || readings.unavailable {
                                ui.add_space(8.0);
                                let unavailable = readings.unavailable;
                                self.audio_controls(ui, audio, VOLUME_WIDTH, unavailable);
                            }
                        });
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if close_button(ui).clicked() {
                            self.quit = true;
                        }
                        let maximized = window.is_maximized();
                        let maximize = if maximized { icon::COPY } else { icon::SQUARE };
                        if icon_button(ui, maximize, false).clicked() {
                            window.set_maximized(!maximized);
                        }
                        if icon_button(ui, icon::MINUS, false).clicked() {
                            window.set_minimized(true);
                        }
                        ui.add_space(8.0);
                        if icon_button(ui, icon::CORNERS_OUT, false).clicked() {
                            toggle_fullscreen(window);
                        }
                        if icon_button(ui, icon::MONITOR_PLAY, false).clicked() {
                            self.settings.layout = Layout::Immersive;
                            self.save_settings();
                        }
                        ui.add_space(8.0);
                        self.copy_button(ui, video);
                    });
                });
            });
    }

    /// Shows the button bar centred in the part of the window the panel
    /// leaves uncovered.
    fn bar(&mut self, ctx: &egui::Context, window: &Window, panel: f32, video: bool) -> egui::Rect {
        let x = -(PANEL_WIDTH + PANEL_MARGIN) * panel / 2.0;
        let y = -20.0 + (1.0 - self.shown) * 80.0;
        egui::Area::new(Id::unique("bar"))
            .order(Order::Foreground)
            .anchor(Align2::CENTER_BOTTOM, vec2(x, y))
            .constrain(false)
            .show(ctx, |ui| {
                overlay_frame().inner_margin(6).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if icon_button(ui, icon::SLIDERS_HORIZONTAL, self.panel).clicked() {
                            self.panel = !self.panel;
                            if self.panel {
                                let _ = self.scan.try_send(());
                            }
                        }
                        self.copy_button(ui, video);
                        if icon_button(ui, icon::APP_WINDOW, false).clicked() {
                            if self.settings.layout != Layout::Framed {
                                self.settings.layout = Layout::Framed;
                                self.save_settings();
                            }
                            window.set_fullscreen(None);
                        }
                        let fullscreen = window.fullscreen().is_some();
                        let glyph = if fullscreen {
                            icon::CORNERS_IN
                        } else {
                            icon::CORNERS_OUT
                        };
                        if icon_button(ui, glyph, false).clicked() {
                            toggle_fullscreen(window);
                        }
                        if close_button(ui).clicked() {
                            self.quit = true;
                        }
                    });
                });
            })
            .response
            .rect
    }

    fn panel(&mut self, ctx: &egui::Context, shown: f32, audio: &Audio) -> egui::Rect {
        let screen = ctx.content_rect();
        let x = screen.right() - (PANEL_WIDTH + PANEL_MARGIN) * shown;
        egui::Area::new(Id::unique("panel"))
            .order(Order::Foreground)
            .fixed_pos(pos2(x, screen.top() + PANEL_MARGIN))
            .constrain(false)
            .show(ctx, |ui| {
                overlay_frame().inner_margin(12).show(ui, |ui| {
                    ui.set_width(PANEL_WIDTH - 24.0);
                    ui.set_max_height(screen.height() - 2.0 * PANEL_MARGIN - 24.0);
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        section(ui, "Source");
                        self.sources(ui);
                        ui.add_space(8.0);
                        section(ui, "Mode");
                        self.mode_list(ui);
                        ui.add_space(8.0);
                        section(ui, "Scale");
                        ui.horizontal(|ui| {
                            ui.spacing_mut().button_padding.x = 8.0;
                            self.scales(ui);
                        });
                        ui.add_space(8.0);
                        let readings = audio.readings();
                        if readings.available || readings.unavailable {
                            section(ui, "Audio");
                            ui.horizontal(|ui| {
                                let unavailable = readings.unavailable;
                                self.audio_controls(ui, audio, f32::INFINITY, unavailable);
                            });
                            ui.add_space(8.0);
                        }
                        ui.separator();
                        self.stats_button(ui);
                    });
                });
            })
            .response
            .rect
    }

    /// Shows the copy button, disabled without `video` and showing a check
    /// for a moment after a copy.
    fn copy_button(&mut self, ui: &mut egui::Ui, video: bool) {
        let left = self
            .copied_at
            .and_then(|at| COPIED_FOR.checked_sub(at.elapsed()));
        if let Some(left) = left {
            ui.ctx().request_repaint_after(left);
        }
        let glyph = if left.is_some() {
            icon::CHECK
        } else {
            icon::IMAGES
        };
        if ui
            .add_enabled_ui(video, |ui| icon_button(ui, glyph, false))
            .inner
            .clicked()
        {
            self.copy = true;
        }
    }

    fn sources(&mut self, ui: &mut egui::Ui) {
        for source in &self.status.devices {
            let current = self.status.device.as_ref() == Some(source);
            if ui
                .add(Button::selectable(current, &source.name).truncate())
                .clicked()
            {
                if !current {
                    let _ = self.commands.send(Command::Device(source.clone()));
                }
                if self.settings.source.as_deref() != Some(&source.name) {
                    self.settings.source = Some(source.name.clone());
                    self.save_settings();
                }
            }
        }
    }

    fn stats_button(&mut self, ui: &mut egui::Ui) {
        if ui
            .add(Button::selectable(self.settings.stats, "Stats"))
            .clicked()
        {
            self.settings.stats = !self.settings.stats;
            self.save_settings();
        }
    }

    /// Shows the mute button and a volume slider up to `width` wide, or both
    /// disabled when the source's audio is unavailable. The slider sets the
    /// gain as it moves and saves the volume when the drag ends, so the
    /// settings file is not written on every step.
    fn audio_controls(&mut self, ui: &mut egui::Ui, audio: &Audio, width: f32, unavailable: bool) {
        let volume = self.settings.volume;
        let glyph = if unavailable {
            icon::SPEAKER_X
        } else if self.settings.muted {
            icon::SPEAKER_SLASH
        } else if volume == 0.0 {
            icon::SPEAKER_NONE
        } else if volume < 0.5 {
            icon::SPEAKER_LOW
        } else {
            icon::SPEAKER_HIGH
        };
        ui.add_enabled_ui(!unavailable, |ui| {
            if icon_button(ui, glyph, false).clicked() {
                self.settings.muted = !self.settings.muted;
                audio.set_gain(self.settings.volume, self.settings.muted);
                self.save_settings();
            }
            ui.spacing_mut().slider_width = width.min(ui.available_width());
            let slider = ui.add(
                Slider::new(&mut self.settings.volume, 0.0..=1.0)
                    .show_value(false)
                    .trailing_fill(true),
            );
            if slider.changed() {
                // Moving the slider is a request to hear the result.
                self.settings.muted = false;
                audio.set_gain(self.settings.volume, self.settings.muted);
            }
            if slider.drag_stopped() {
                self.save_settings();
            }
        });
    }

    fn scales(&mut self, ui: &mut egui::Ui) {
        for scale in Scale::ALL {
            if ui
                .add(Button::selectable(
                    self.settings.scale == scale,
                    scale.name(),
                ))
                .clicked()
                && self.settings.scale != scale
            {
                self.settings.scale = scale;
                self.save_settings();
            }
        }
    }

    fn mode_menu(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(MENU_WIDTH);
        let current = self.status.mode;
        for size in sizes(&self.status.modes) {
            let selected = current.is_some_and(|mode| [mode.width, mode.height] == size);
            let button = Button::selectable(selected, format!("{}x{}", size[0], size[1]));
            SubMenuButton::from_button(button).ui(ui, |ui| {
                ui.push_id(egui::IdSalt::new(&self.status.device), |ui| {
                    self.mode_grid(ui, size, current);
                });
            });
        }
    }

    fn mode_list(&mut self, ui: &mut egui::Ui) {
        let current = self.status.mode;
        for size in sizes(&self.status.modes) {
            let selected = current.is_some_and(|mode| [mode.width, mode.height] == size);
            egui::CollapsingHeader::new(format!("{}x{}", size[0], size[1]))
                .id_salt(size)
                .default_open(selected)
                .show(ui, |ui| {
                    ui.push_id(egui::IdSalt::new(&self.status.device), |ui| {
                        self.mode_grid(ui, size, current);
                    });
                });
        }
    }

    /// Shows the modes of one frame size as a table with a column per pixel
    /// format and a row per frame rate.
    fn mode_grid(&mut self, ui: &mut egui::Ui, size: [u32; 2], current: Option<Mode>) {
        let modes: Vec<Mode> = self
            .status
            .modes
            .iter()
            .copied()
            .filter(|mode| [mode.width, mode.height] == size)
            .collect();
        let mut formats: Vec<_> = modes.iter().map(|mode| mode.pixel).collect();
        formats.dedup();
        let mut rates: Vec<_> = modes.iter().map(|mode| rate(*mode)).collect();
        rates.sort_by(|a, b| b.total_cmp(a));
        rates.dedup();
        egui::Grid::new(("modes", size))
            .spacing(vec2(4.0, 2.0))
            .show(ui, |ui| {
                for format in &formats {
                    let label = egui::Label::new(RichText::new(format.name()).small().color(WEAK));
                    ui.add_sized([CELL_WIDTH, 16.0], label);
                }
                ui.end_row();
                for &fps in &rates {
                    for &format in &formats {
                        let mode = modes
                            .iter()
                            .find(|mode| mode.pixel == format && rate(**mode) == fps);
                        let Some(&mode) = mode else {
                            ui.label("");
                            continue;
                        };
                        let button = Button::selectable(current == Some(mode), fps.to_string())
                            .min_size(vec2(CELL_WIDTH, 0.0));
                        if ui.add(button).clicked()
                            && current != Some(mode)
                            && let Some(source) = &self.status.device
                        {
                            let _ = self.commands.send(Command::Mode(source.clone(), mode));
                            self.settings.modes.insert(source.name.clone(), mode);
                            self.save_settings();
                        }
                    }
                    ui.end_row();
                }
            });
    }
}

/// Frame rate rounded to hundredths, which tells apart rates such as 60 and
/// 59.94 and prints without trailing zeros.
fn rate(mode: Mode) -> f64 {
    (mode.fps() * 100.0).round() / 100.0
}

/// Frame sizes, largest first.
fn sizes(modes: &[Mode]) -> Vec<[u32; 2]> {
    let mut sizes: Vec<_> = modes.iter().map(|mode| [mode.width, mode.height]).collect();
    sizes.sort_by_key(|&[width, height]| std::cmp::Reverse(u64::from(width) * u64::from(height)));
    sizes.dedup();
    sizes
}

pub fn toggle_fullscreen(window: &Window) {
    let fullscreen = window
        .fullscreen()
        .is_none()
        .then_some(Fullscreen::Borderless(None));
    window.set_fullscreen(fullscreen);
}

fn resize_cursor(edge: ResizeDirection) -> CursorIcon {
    match edge {
        ResizeDirection::North => CursorIcon::ResizeNorth,
        ResizeDirection::South => CursorIcon::ResizeSouth,
        ResizeDirection::West => CursorIcon::ResizeWest,
        ResizeDirection::East => CursorIcon::ResizeEast,
        ResizeDirection::NorthWest => CursorIcon::ResizeNorthWest,
        ResizeDirection::NorthEast => CursorIcon::ResizeNorthEast,
        ResizeDirection::SouthWest => CursorIcon::ResizeSouthWest,
        ResizeDirection::SouthEast => CursorIcon::ResizeSouthEast,
    }
}

fn icon_button(ui: &mut egui::Ui, glyph: &str, selected: bool) -> egui::Response {
    ui.add(
        Button::selectable(selected, RichText::new(glyph).size(ICON_SIZE))
            .min_size(vec2(34.0, 30.0)),
    )
}

fn close_button(ui: &mut egui::Ui) -> egui::Response {
    ui.scope(|ui| {
        let hovered = &mut ui.visuals_mut().widgets.hovered;
        hovered.weak_bg_fill = CLOSE;
        hovered.fg_stroke.color = Color32::WHITE;
        let active = &mut ui.visuals_mut().widgets.active;
        active.weak_bg_fill = CLOSE.gamma_multiply(0.8);
        active.fg_stroke.color = Color32::WHITE;
        icon_button(ui, icon::X, false)
    })
    .inner
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).small().color(WEAK));
}

fn overlay_frame() -> Frame {
    Frame::new()
        .fill(SURFACE)
        .stroke(BORDER)
        .corner_radius(12)
        .shadow(SHADOW)
}

/// Shows the readout, with the parts of the latency under it while
/// `breakdown` is set, and returns whether the "Latency" label or the arrow
/// after it, which toggle them, was clicked. The Audio row follows `audio`
/// as it is now, while its figures come from `stats`.
fn readout(
    ctx: &egui::Context,
    top: f32,
    mode: Mode,
    stats: &Stats,
    audio: &Readings,
    breakdown: bool,
) -> bool {
    // Figures from a source that stopped sending frames show as placeholders.
    let fresh = stats
        .last_present
        .is_some_and(|at| monotonic_now().saturating_sub(at) < STALE);
    let fps = stats.fps.filter(|_| fresh);
    let summary = stats.summary.as_ref().filter(|_| fresh);
    let audio_figures = stats.audio.filter(|sample| {
        fresh
            && audio
                .figures
                .is_some_and(|live| live.stream_id == sample.stream_id)
    });
    let mut toggle = Rect::NOTHING;
    let readout = egui::Area::new(Id::unique("readout"))
        .order(Order::Foreground)
        .fixed_pos(pos2(12.0, top + 12.0))
        .interactable(false)
        .show(ctx, |ui| {
            overlay_frame().inner_margin(10).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(10.0, 2.0);
                // Rows take the height of their text rather than of a button,
                // which keeps the readout inside the smallest window.
                ui.spacing_mut().interact_size.y = 0.0;
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                ui.style_mut().override_font_id = Some(FontId::proportional(READOUT_SIZE));
                let number = text_width(ui, "999.9");
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "{} {}x{} @ {} fps",
                        mode.pixel.name(),
                        mode.width,
                        mode.height,
                        rate(mode)
                    ));
                    ui.add_space(16.0);
                    ui.spacing_mut().item_spacing.x = 4.0;
                    let fps = figure(fps.map(|fps| fps as f32));
                    cell(ui, &fps, number, Align2::RIGHT_TOP, TEXT);
                    ui.label("fps presented");
                });
                ui.add_space(4.0);
                let rows = [
                    ("Latency", 0.0, summary.map(|summary| &summary.latency)),
                    ("Transfer", INDENT, summary.map(|summary| &summary.transfer)),
                    ("Decode", INDENT, summary.map(|summary| &summary.decode)),
                    ("Render", INDENT, summary.map(|summary| &summary.render)),
                    (
                        "Frame time",
                        0.0,
                        summary.map(|summary| &summary.frame_time),
                    ),
                ];
                // The label column fits every row, shown or not, and the
                // arrow after "Latency", so toggling the parts changes only
                // the height.
                let gap = 4.0;
                let label = rows
                    .iter()
                    .map(|&(name, indent, _)| indent + text_width(ui, name))
                    .fold(text_width(ui, "Latency") + gap + READOUT_SIZE, f32::max);
                ui.horizontal(|ui| {
                    // Room for the label cell of the rows below and the item
                    // spacing after it.
                    ui.add_space(label + ui.spacing().item_spacing.x);
                    for name in ["avg", "99%", "max"] {
                        cell(ui, name, number, Align2::RIGHT_TOP, WEAK);
                    }
                });
                for (name, indent, spread) in rows {
                    if indent > 0.0 && !breakdown {
                        continue;
                    }
                    ui.horizontal(|ui| {
                        ui.add_space(indent);
                        let rect = cell(ui, name, label - indent, Align2::LEFT_TOP, WEAK);
                        if name == "Latency" {
                            let right = rect.left() + text_width(ui, name) + gap + READOUT_SIZE;
                            toggle = Rect::from_min_max(rect.min, pos2(right, rect.bottom()));
                        }
                        for value in [
                            spread.map(|spread| spread.avg),
                            spread.map(|spread| spread.p99),
                            spread.map(|spread| spread.max),
                        ] {
                            cell(ui, &figure(value), number, Align2::RIGHT_TOP, TEXT);
                        }
                        if indent == 0.0 {
                            ui.label(RichText::new("ms").color(WEAK));
                        }
                    });
                }
                if audio.sound_card {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        cell(ui, "Audio", label, Align2::LEFT_TOP, WEAK);
                        if audio.unavailable {
                            // The word takes the width of the figures it
                            // replaces, so the readout keeps its width.
                            let spacing = ui.spacing().item_spacing.x;
                            let width = 2.0 * number
                                + text_width(ui, "ms buffer")
                                + text_width(ui, "underruns")
                                + 3.0 * spacing;
                            cell(ui, "unavailable", width, Align2::LEFT_TOP, WEAK);
                            return;
                        }
                        let depth = figure(audio_figures.map(|sample| sample.depth_ms));
                        cell(ui, &depth, number, Align2::RIGHT_TOP, TEXT);
                        ui.label(RichText::new("ms buffer").color(WEAK));
                        let underruns = audio_figures
                            .map_or_else(|| "-".into(), |sample| sample.underruns.to_string());
                        cell(ui, &underruns, number, Align2::RIGHT_TOP, TEXT);
                        ui.label(RichText::new("underruns").color(WEAK));
                    });
                }
                // The graphs span the width of the text above them.
                let width = ui.min_rect().width();
                let samples = summary.map(|_| &stats.samples);
                let latency_top = summary.map(|summary| summary.latency_top);
                graph(ui, "Latency", width, latency_top, samples, |sample| {
                    sample.latency
                });
                // A steady signal sits at mid-height and a dropped frame reaches the top.
                let frame_top = (2000.0 / mode.fps()) as f32;
                graph(
                    ui,
                    "Frame time",
                    width,
                    Some(frame_top),
                    samples,
                    |sample| sample.frame_time,
                );
            });
        });
    // The readout takes no input, so it never covers the controls and menus
    // around it. Only the "Latency" label and the arrow after it, in an area
    // of their own drawn just above the readout, take clicks.
    let toggle = egui::Area::new(Id::unique("breakdown"))
        .order(Order::Foreground)
        .fixed_pos(toggle.min)
        .movable(false)
        .show(ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(toggle.size(), Sense::click());
            // A drawn triangle, since the text font has its own glyphs at
            // the icon font's caret code points.
            let center = pos2(rect.right() - READOUT_SIZE / 2.0, rect.center().y);
            let mark = Rect::from_center_size(center, vec2(7.0, 7.0));
            let points = if breakdown {
                vec![mark.left_top(), mark.right_top(), mark.center_bottom()]
            } else {
                vec![mark.left_top(), mark.right_center(), mark.left_bottom()]
            };
            let color = if response.hovered() { TEXT } else { WEAK };
            ui.painter()
                .add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
            response.clicked()
        });
    ctx.set_sublayer(readout.response.layer_id, toggle.response.layer_id);
    toggle.inner
}

/// A readout number with one decimal, or `-` while there is none.
fn figure(value: Option<f32>) -> String {
    value.map_or_else(|| "-".into(), |value| format!("{value:.1}"))
}

fn text_width(ui: &egui::Ui, text: &str) -> f32 {
    let font = FontId::proportional(READOUT_SIZE);
    let galley = ui.painter().layout_no_wrap(text.into(), font, TEXT);
    galley.size().x
}

/// Draws text placed by `anchor` in a cell of a fixed width, so the cells
/// around it stay put whatever the text, and returns the cell.
fn cell(ui: &mut egui::Ui, text: &str, width: f32, anchor: Align2, color: Color32) -> Rect {
    let font = FontId::proportional(READOUT_SIZE);
    let galley = ui.painter().layout_no_wrap(text.into(), font, color);
    let (rect, _) = ui.allocate_exact_size(vec2(width, galley.size().y), Sense::hover());
    let pos = anchor.align_size_within_rect(galley.size(), rect).min;
    ui.painter().galley(pos, galley, color);
    rect
}

/// Plots one value per frame across the statistics window, newest on the
/// right, scaled from zero to `top` and clipped there. Without a scale or
/// samples, it draws the empty box with its marks.
fn graph(
    ui: &mut egui::Ui,
    name: &str,
    width: f32,
    top: Option<f32>,
    samples: Option<&VecDeque<Sample>>,
    value: fn(&Sample) -> f32,
) {
    ui.add_space(4.0);
    section(ui, name);
    let (rect, _) = ui.allocate_exact_size(vec2(width, GRAPH_HEIGHT), Sense::hover());
    let painter = ui.painter_at(rect);
    let marks = ui.visuals().widgets.noninteractive.bg_stroke;
    painter.rect_filled(rect, 4, marks.color);
    painter.hline(rect.x_range(), rect.center().y, marks);
    let (Some(top), Some(samples)) = (top, samples) else {
        return;
    };
    let now = monotonic_now();
    let window = WINDOW.as_secs_f32();
    let points = samples
        .iter()
        .map(|sample| {
            let age = now.saturating_sub(sample.at).as_secs_f32() / window;
            pos2(
                rect.right() - age * rect.width(),
                rect.bottom() - (value(sample) / top).min(1.0) * rect.height(),
            )
        })
        .collect();
    painter.line(points, Stroke::new(1.0, ACCENT));
}

pub fn theme(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::empty();
    fonts.font_data.insert(
        "Inter".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/Inter-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "Phosphor".into(),
        Arc::new(FontData::from_static(
            egui_phosphor::Variant::Regular.font_bytes(),
        )),
    );
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .insert(family, vec!["Inter".into(), "Phosphor".into()]);
    }
    ctx.set_fonts(fonts);
    ctx.all_styles_mut(|style| {
        style.visuals = egui::Visuals::dark();
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(11.0)),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(14.0)),
            (TextStyle::Heading, FontId::proportional(18.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
        style.spacing.item_spacing = vec2(4.0, 4.0);
        style.spacing.button_padding = vec2(10.0, 5.0);
        style.spacing.menu_margin = Margin::same(6);
        style.interaction.selectable_labels = false;
        let visuals = &mut style.visuals;
        visuals.panel_fill = SURFACE;
        visuals.window_fill = SURFACE;
        visuals.window_stroke = BORDER;
        visuals.window_corner_radius = CornerRadius::same(12);
        visuals.menu_corner_radius = CornerRadius::same(12);
        visuals.popup_shadow = SHADOW;
        visuals.selection.bg_fill = ACCENT.gamma_multiply(0.3);
        visuals.selection.stroke = Stroke::new(1.0, ACCENT);
        let widgets = &mut visuals.widgets;
        for state in [
            &mut widgets.noninteractive,
            &mut widgets.inactive,
            &mut widgets.hovered,
            &mut widgets.active,
            &mut widgets.open,
        ] {
            state.corner_radius = CornerRadius::same(8);
            state.bg_stroke = Stroke::NONE;
            state.expansion = 0.0;
            state.fg_stroke.color = TEXT;
        }
        widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
        widgets.hovered.weak_bg_fill = Color32::from_white_alpha(22);
        widgets.active.weak_bg_fill = Color32::from_white_alpha(36);
        widgets.open.weak_bg_fill = Color32::from_white_alpha(28);
        widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_white_alpha(16));
    });
}
