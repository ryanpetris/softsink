# Using Softsink

Softsink displays live video and audio from UVC capture cards in a borderless Linux Wayland window.

## Capture Sources and Modes

On startup, Softsink scans `/dev/video*` for UVC (`uvcvideo`) capture devices and opens the last source you selected, or the first UVC device it finds. If no source is available or none starts, the window shows **No signal** and rescans every second. Opening the header bar's **Source** menu or the immersive side panel also triggers a rescan.

Softsink lists discrete-resolution **MJPEG**, **YUYV**, and **NV12** modes. For stepwise frame intervals, it uses the shortest interval.

- **Automatic mode selection**: Picks the highest frame rate, breaks ties by largest resolution area, and prefers uncompressed **YUYV** or **NV12** over **MJPEG**.
- **Manual mode selection**: Choosing a mode saves it per device and reuses it as long as the device still advertises it. If a saved mode fails to start while the device remains connected, Softsink drops the saved mode and falls back to automatic selection.
- **Stream recovery**: If the active source is unplugged or its stream fails, Softsink rescans UVC devices. If that source is still listed and had delivered frames, Softsink retries it first; otherwise it switches to another UVC device. If no source starts, Softsink clears the picture, shows **No signal**, and rescans every second. A source that starts without sending frames stays selected with a black picture.

## Layouts and Window Controls

Softsink draws a borderless window with a minimum size of 640x360 logical pixels. Drag any window edge or corner to resize while the window is neither maximized nor fullscreen.

Two layouts are available:

- **Framed** (default): Places a fixed header bar above the video with rounded top window corners when unmaximized. The left side holds icon buttons for four menus and a toggle: **Source**, an arrow entering a square; **Mode**, a square wave; **Scale**, a square inside a dashed outline; **Orientation**, a rectangle with rotation arrows; and **Stats**, a line chart. The mute button and volume slider follow when the source has audio. **Mode** and **Orientation** are unavailable while no source is open. The right side holds the copy button, toggles for the immersive layout and fullscreen, and minimize, maximize, and close buttons. Drag the header bar to move the window, or double-click it to maximize or restore the window.
- **Immersive**: Fills the entire window with video (fullscreen always uses this layout). Moving the pointer reveals a bottom bar with buttons for the side panel, copy, framed layout, fullscreen, and close. The side panel holds **Source**, **Mode**, **Scale**, **Orientation**, **Audio** (when present), and **Stats**. After 2 seconds without pointer activity, the bar and side panel slide away and the cursor hides unless the pointer rests over them or a menu is open. Drag the video to pan when the scaling mode allows it. To move the window, use your compositor's modifier+drag shortcut, such as `Super+drag` on GNOME.

The copy button puts the current video frame on the clipboard with its rotation and flips applied. The image keeps the capture resolution, with width and height swapped at 90 and 270 degrees. It excludes the interface, scaling, zoom, pan, and black bars. The button is disabled while there is no video, and shows a check for a second once the image is on the clipboard. In the immersive layout, the check also brings back the bottom bar. You can paste the image while Softsink runs.

### Keyboard Shortcuts

| Key | Action |
| --- | --- |
| `F11` | Toggle borderless fullscreen |
| `Esc` | Close an open menu, then the immersive side panel, otherwise exit fullscreen |
| `Ctrl+C` | Copy the current frame to the clipboard |
| `Ctrl+=` or `Ctrl++` | Zoom in around the center of the video area |
| `Ctrl+-` | Zoom out around the center of the video area |
| `Ctrl+0` | Reset zoom and pan |

## Scaling

Select a scaling mode from the header bar's **Scale** menu or the side panel's **Scale** section:

| Mode | Behavior | Zoom | Pan |
| --- | --- | --- | --- |
| **Fit** (default) | Fits the video inside the area and adds black bars when aspect ratios differ | Smooth, from 100% to 800% of the Fit size | Along any overflowing axis |
| **Fill** | Scales the video to cover the area and crops the overflow | Smooth, from 100% to 800% of the Fill size | Along any overflowing axis |
| **Stretch** | Scales width and height independently to fill the area | No | No |
| **Integer** | Uses the largest whole-number multiple that fits, or shows 1x cropped when the video is larger than the area | Whole-number steps, capped at eight times the normal Integer size when zooming in | Along any overflowing axis |
| **1:1** | Maps each video pixel to one physical screen pixel | No | Along any overflowing axis |

- **Controls**: Scroll over uncovered video with the mouse wheel or touchpad to zoom around the pointer, or press `Ctrl+=`, `Ctrl++`, or `Ctrl+-` to zoom around the center of the video area. Drag uncovered video with the primary mouse button to pan. Panning stops at the edges of the video.
- **Resetting the view**: While zoomed, **Scale** shows the zoom level next to the selected mode, such as **Fill 250%** or **Integer 3x**, and offers **Reset zoom**. **Reset zoom** and `Ctrl+0` restore the default zoom and centered view. Changing the scaling mode, capture source, capture mode, or orientation also resets the view, while stream recovery on the same source and mode preserves it. Softsink does not save zoom or pan when you quit.
- **Window resizing**: Zoom stays relative to the size of the video area in **Fit** and **Fill**. **Integer** keeps the chosen multiple when you resize the window, even if shrinking the window puts that multiple above the zoom-in limit. If the window grows enough for a higher multiple to fit, **Integer** uses the normal size.

## Orientation

Use the header bar's **Orientation** menu or the side panel's **Orientation** section to rotate the video 0, 90, 180, or 270 degrees clockwise. **Flip horizontal** swaps the displayed left and right, and **Flip vertical** swaps the displayed top and bottom. The flips are independent and apply after rotation. Softsink scales the resulting image and remembers the orientation for each capture device.

## Audio Passthrough

Audio passthrough is enabled by default. Softsink finds an audio capture device on the same USB device as the active video source and plays it to the default PipeWire output. Audio starts and stops with the video stream.

- **Adaptive sync**: Keeps roughly 10 ms of audio buffered and subtly adjusts the resampling rate to absorb clock drift between the capture card and the output.
- **Controls**: Mute and volume changes take effect immediately, and moving the volume slider unmutes.
- **Missing or unavailable audio**: If a source has no USB sound card, Softsink hides the audio controls. If the source has a sound card whose audio cannot start (including a 5-second startup timeout) or stops, the mute button shows a dimmed speaker with an X and the volume slider is disabled while Softsink retries about once per second.

## Stats Readout

Toggling **Stats** in the header bar or side panel shows an overlay in the top-left corner of the video area while video streams:

- **Stream header**: Active pixel format, resolution, configured frame rate, and measured presentation rate in FPS.
- **Timing table**: Average (`avg`), 99th percentile (`99%`), and maximum (`max`) times in milliseconds over the last 5 seconds, refreshed once per second:
  - **Latency**: From the V4L2 capture timestamp to the present call. Click **Latency** or its arrow to expand or collapse its three stages: **Transfer** (capture to buffer dequeue), **Decode** (buffer copy and MJPEG decode), and **Render** (decode ready to present).
  - **Frame time**: Gap between consecutive present calls.
- **Audio row**: Shown only when the source has a sound card. Reports current buffer depth in milliseconds and buffer underruns for the active stream, refreshed with the table. Shows `-` while audio starts and `unavailable` when audio cannot start or has stopped.
- **Graphs**: Per-frame plots covering the last 5 seconds. The latency graph scales to twice the median latency (rounded up to a multiple of 5 ms). The frame-time graph scales to twice the configured frame interval, so a steady stream sits at mid-height and a dropped frame reaches the top.

Before initial figures arrive, or whenever no frame has been presented for more than a second, measured values display `-` and the graphs stay empty.

## Screen Blanking

While video is streaming, Softsink inhibits screen blanking over the session D-Bus via `org.freedesktop.ScreenSaver`, releasing the hold when capture stops or Softsink exits.

## Settings

Softsink stores preferences in `$XDG_CONFIG_HOME/softsink/config.toml` when `XDG_CONFIG_HOME` is an absolute path, or `~/.config/softsink/config.toml` otherwise.

| Key | Description |
| --- | --- |
| `source` | Name of the last manually selected capture source |
| `modes` | Per-device saved capture modes |
| `orientations` | Per-device rotation and flips, omitted for devices at the default orientation |
| `scale` | Scaling mode (`fit`, `fill`, `stretch`, `integer`, or `native`) |
| `layout` | Window layout (`framed` or `immersive`) |
| `window_size` | Logical `[width, height]`, tracked while unmaximized and non-fullscreen and saved on exit |
| `stats` | Whether the stats readout is enabled |
| `breakdown` | Whether the latency breakdown is expanded in the stats readout |
| `muted` | Audio mute state |
| `volume` | Audio volume (`0.0` to `1.0`), saved when a slider drag ends |
