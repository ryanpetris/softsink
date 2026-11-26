# Using softsink

softsink displays live video and audio from UVC capture cards in a borderless Linux Wayland window.

## Capture Sources and Modes

On startup, softsink scans `/dev/video*` for UVC (`uvcvideo`) capture devices and opens the last source you selected, or the first UVC device it finds. If no source is available or none starts, the window shows **No signal** and rescans every second. Opening the **Source** menu or the immersive side panel also triggers a rescan.

softsink lists discrete-resolution **MJPEG**, **YUYV**, and **NV12** modes. For stepwise frame intervals, it uses the shortest interval.

- **Automatic mode selection**: Picks the highest frame rate, breaks ties by largest resolution area, and prefers uncompressed **YUYV** or **NV12** over **MJPEG**.
- **Manual mode selection**: Choosing a mode saves it per device and reuses it as long as the device still advertises it. If a saved mode fails to start while the device remains connected, softsink drops the saved mode and falls back to automatic selection.
- **Stream recovery**: If the active source is unplugged or its stream fails, softsink rescans UVC devices. If that source is still listed and had delivered frames, softsink retries it first; otherwise it switches to another UVC device. If no source starts, softsink clears the picture, shows **No signal**, and rescans every second. A source that starts without sending frames stays selected with a black picture.

## Layouts and Window Controls

softsink draws a borderless window with a minimum size of 640x360 logical pixels. Drag any window edge or corner to resize while the window is neither maximized nor fullscreen.

Two layouts are available:

- **Framed** (default): Places a fixed header bar above the video with rounded top window corners when unmaximized. The left side holds **Source**, **Mode**, **Scale**, and **Stats**, plus the mute button and volume slider when the source has audio. The right side holds toggles for the immersive layout and fullscreen, along with minimize, maximize, and close buttons. Drag the header bar to move the window.
- **Immersive**: Fills the entire window with video (fullscreen always uses this layout). Moving the pointer reveals a bottom bar with buttons for the side panel, framed layout, fullscreen, and close. The side panel holds **Source**, **Mode**, **Scale**, **Audio** (when present), and **Stats**. After 2 seconds without pointer activity, the bar and side panel slide away and the cursor hides unless the pointer rests over them or a menu is open. Drag the video area to move the window.

### Keyboard Shortcuts

| Key | Action |
| --- | --- |
| `F11` | Toggle borderless fullscreen |
| `Esc` | Close an open menu, then the immersive side panel, otherwise exit fullscreen |

## Scaling

Select a scaling mode from **Scale** in the header bar or side panel:

| Mode | Behavior |
| --- | --- |
| **Fit** (default) | Scales uniformly to fit inside the video area, adding black bars when aspect ratios differ |
| **Fill** | Scales uniformly to cover the video area, cropping overflow evenly on opposite sides |
| **Stretch** | Scales width and height independently to fill the video area |
| **Integer** | Scales by the largest whole-number multiple of at least 1x that fits, or shows 1x cropped when the video is larger than the area |
| **1:1** | Maps each video pixel to one physical screen pixel, centered with black padding or cropping |

## Audio Passthrough

Audio passthrough is enabled by default. softsink finds an audio capture device on the same USB device as the active video source and plays it to the default PipeWire output. Audio starts and stops with the video stream.

- **Adaptive sync**: Keeps roughly 10 ms of audio buffered and subtly adjusts the resampling rate to absorb clock drift between the capture card and the output.
- **Controls**: Mute and volume changes take effect immediately, and moving the volume slider unmutes.
- **Missing or unavailable audio**: If a source has no USB sound card, softsink hides the audio controls. If the source has a sound card whose audio cannot start (including a 5-second startup timeout) or stops, the mute button shows a dimmed speaker with an X and the volume slider is disabled while softsink retries about once per second.

## Stats Readout

Toggling **Stats** shows an overlay in the top-left corner of the video area while video streams:

- **Stream header**: Active pixel format, resolution, configured frame rate, and measured presentation rate in FPS.
- **Timing table**: Average (`avg`), 99th percentile (`99%`), and maximum (`max`) times in milliseconds over the last 5 seconds, refreshed once per second:
  - **Latency**: From the V4L2 capture timestamp to the present call. Click **Latency** or its arrow to expand or collapse its three stages: **Transfer** (capture to buffer dequeue), **Decode** (buffer copy and MJPEG decode), and **Render** (decode ready to present).
  - **Frame time**: Gap between consecutive present calls.
- **Audio row**: Shown only when the source has a sound card. Reports current buffer depth in milliseconds and buffer underruns for the active stream, refreshed with the table. Shows `-` while audio starts and `unavailable` when audio cannot start or has stopped.
- **Graphs**: Per-frame plots covering the last 5 seconds. The latency graph scales to twice the median latency (rounded up to a multiple of 5 ms). The frame-time graph scales to twice the configured frame interval, so a steady stream sits at mid-height and a dropped frame reaches the top.

Before initial figures arrive, or whenever no frame has been presented for more than a second, measured values display `-` and the graphs stay empty.

## Screen Blanking

While video is streaming, softsink inhibits screen blanking over the session D-Bus via `org.freedesktop.ScreenSaver`, releasing the hold when capture stops or softsink exits.

## Settings

softsink stores preferences in `$XDG_CONFIG_HOME/softsink/config.toml` when `XDG_CONFIG_HOME` is an absolute path, or `~/.config/softsink/config.toml` otherwise.

| Key | Description |
| --- | --- |
| `source` | Name of the last manually selected capture source |
| `modes` | Per-device saved capture modes |
| `scale` | Scaling mode (`fit`, `fill`, `stretch`, `integer`, or `native`) |
| `layout` | Window layout (`framed` or `immersive`) |
| `window_size` | Logical `[width, height]`, tracked while unmaximized and non-fullscreen and saved on exit |
| `stats` | Whether the stats readout is enabled |
| `breakdown` | Whether the latency breakdown is expanded in the stats readout |
| `muted` | Audio mute state |
| `volume` | Audio volume (`0.0` to `1.0`), saved when a slider drag ends |
