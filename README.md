# bokehcam

A virtual webcam that gives every app the same picture: sharp, well framed,
with a clean background blur. Set it up once; Meet, Zoom, Teams, Doctolib,
OBS and browsers just see a camera called `bokehcam`.

- **Clean blur anywhere.** A person-segmentation model, an outline that
  snaps to real edges in the picture, and smoothing over time that removes
  shimmer without trailing behind movement.
- **Uses the whole machine.** The picture pipeline runs as shaders on the
  graphics card. Models run on the graphics card too when that is faster,
  otherwise on the processor; bokehcam measures both and picks.
- **Swappable models.** A model is described by a small manifest file. The
  most accurate model that fits the frame time is chosen automatically.
- **Sharper picture.** Captures at the camera's full resolution and scales
  down, instead of letting the webcam do a soft internal 1080p.
- **A settings window with live preview**, a command line, and a documented
  config file. All three edit the same settings, and changes apply live.
- **Camera off when unused.** The physical camera (and its light) only turns
  on while an app is reading the virtual camera.

## Status

bokehcam is in early development. Expect rough edges and breaking changes.

- **Works today:** Linux, with the features described on this page.
- **Not verified yet:** the install and uninstall scripts end to end, and
  every control in the settings window.
- **Not written yet:** Windows and macOS support (only the capture and
  virtual-camera backend is platform-specific, see
  [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)), appearance filters, and
  background replacement.

Known gaps are tracked in the
[issues](https://github.com/MohamedRahoui/bokehcam/issues).

## Requirements

- Linux with a USB (UVC) webcam.
- A graphics driver with Vulkan support (Mesa or the vendor's driver).
- [`v4l2loopback`](https://github.com/v4l2loopback/v4l2loopback) 0.13 or
  newer: the kernel module that provides the virtual camera device.
- To build: a recent stable [Rust](https://rustup.rs) toolchain and `clang`.

## Install

```sh
git clone https://github.com/MohamedRahoui/bokehcam && cd bokehcam
scripts/install.sh              # add --service to start it at login
```

The script lists every file it creates at the top;
`scripts/uninstall.sh` removes them again. On Arch-based systems it installs
`v4l2loopback-dkms` with pacman; elsewhere, install v4l2loopback first.

### From a release archive

The [releases page](https://github.com/MohamedRahoui/bokehcam/releases) has
a prebuilt archive for Linux x86_64. It needs glibc 2.39 or newer (Ubuntu
24.04, Fedora 40, Debian 13, current Arch), a Vulkan driver and
v4l2loopback.

```sh
tar -xzf bokehcam-*-linux-x86_64.tar.gz && cd bokehcam-*-linux-x86_64
sudo modprobe v4l2loopback devices=1 video_nr=10 card_label=bokehcam exclusive_caps=1
./bokehcam models fetch
./bokehcam doctor
./bokehcam-gui
```

Keep the files of the archive together: the programs load the library next
to them. The `modprobe` line lasts until the next reboot.

### From source, without installing

To try it without installing anything permanently:

```sh
cargo build --release
scripts/load-module.sh                       # virtual camera until the next reboot
target/release/bokehcam models fetch
target/release/bokehcam doctor
target/release/bokehcam-gui                  # the window; it runs the camera itself
```

## Use

Open the **bokehcam** window, or run the service without a window:

```sh
bokehcam run                # Ctrl+C to stop
```

Then pick the camera named **bokehcam** in your app, and turn off the app's
own background effect.

The window shows the live picture and every setting. It connects to a
running `bokehcam run` if there is one, and otherwise runs the camera itself
for as long as it is open.

| Command | What it does |
|---|---|
| `bokehcam run [--stats N]` | Run the virtual camera. `--stats 5` logs the frame rate and per-step timings every 5 s. |
| `bokehcam status` | What the running bokehcam is doing. |
| `bokehcam get [key]` / `bokehcam set key value` | Read or change settings; changes apply immediately. |
| `bokehcam doctor` | Check graphics, camera, virtual camera and models on this machine. |
| `bokehcam devices [--modes] [--controls]` | List cameras, their capture modes and hardware controls. |
| `bokehcam models [list\|fetch\|bench]` | Show, download or time the models. |
| `bokehcam snapshot out.png` | Save one finished picture. |
| `bokehcam init` | Write the documented config file. |

## Settings

Three ways, one source of truth (`~/.config/bokehcam/config.toml`):

```sh
bokehcam-gui                              # sliders and a live preview
bokehcam set background.blur 3.5          # scripts
$EDITOR ~/.config/bokehcam/config.toml    # the file documents every setting
```

A change is validated first; an invalid value is refused with a message and
the previous settings stay active. Most changes apply on the next frame.
Changing the camera mode reopens the camera; changing the output size
reopens the virtual camera.

Hardware controls of the camera (focus, exposure, power-line frequency ...)
go under `[camera.controls]` and are applied whenever the camera opens. List
them with `bokehcam devices --controls`.

## Models

```sh
bokehcam models          # what exists and what is downloaded
bokehcam models bench    # how fast each runs here, on processor and graphics card
```

| Model | Character | Licence |
|---|---|---|
| `selfie-landscape` | Very fast; fine on any machine. | Apache-2.0 |
| `selfie-multiclass` | Clean, accurate masks. Wants a graphics card. | Apache-2.0 |
| `modnet` | A matting model: soft edges, hair strands, fingers. Wants a graphics card. | Apache-2.0 |

With `segmentation.model = "auto"` (the default) bokehcam uses the most
accurate downloaded model that runs within about a third of the frame time.
To add a model, drop a manifest next to the model files; see
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#models).

## Performance

Measured on a Ryzen 9 5900X with a Radeon RX 6600 XT, 4K Motion-JPEG capture,
1080p output, `selfie-multiclass` on the graphics card:

- 30 fps, about 20 ms of work per frame
- a little under one processor core while an app uses the camera, none when idle
- about 280 MB of memory while streaming

`bokehcam run --stats 5` shows the numbers on your machine.

## Limits

- The outline is only as good as the model. Objects touching your silhouette
  (a headrest, something in your hand) can be classed either way.
- One camera in, one virtual camera out. Background replacement with an
  image and appearance filters are planned, not implemented.
- Two apps reading the virtual camera at the same time may not both work;
  that is a v4l2loopback limit.

## Licence and credits

bokehcam is free software, licensed under the
[GNU General Public License, version 3](LICENSE) or (at your option) any
later version. Models are downloaded separately and keep their own licences
(shown by `bokehcam models`).

The outline refinement is a fast guided filter (He, Sun and Tang, "Guided
Image Filtering"). The overall approach follows Google's description of
[background features in Meet](https://research.google/blog/background-features-in-google-meet-powered-by-web-ml/).
The window is built with [GPUI](https://www.gpui.rs) and
[GPUI Kit](https://github.com/longbridge/gpui-kit).
