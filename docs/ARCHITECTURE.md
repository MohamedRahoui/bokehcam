# Architecture

## Crates

Dependencies point downward; nothing below `engine` knows about front ends.

```
bokehcam-gui   bokehcam-cli          front ends: the window, the command line
        \       /
      bokehcam-engine                the running pipeline + control channel
      /      |       \
bokehcam-   bokehcam-   bokehcam-
 media        gpu        infer       devices / picture pipeline / models
      \      |       /
       bokehcam-core                 config, settings schema, paths
```

| Crate | Responsibility | Platform-specific? |
|---|---|---|
| `bokehcam-core` | Typed, validated configuration; the settings schema; file locations. | No |
| `bokehcam-media` | Camera capture, frame decoding, the virtual camera device. | Yes: `linux/` today, `unsupported.rs` elsewhere |
| `bokehcam-gpu` | Shader passes: framing, outline refinement, blur, colour, output packing. | No (wgpu: Vulkan, Metal, DirectX 12) |
| `bokehcam-infer` | Model manifests, download, inference on processor or graphics card. | No (ONNX Runtime) |
| `bokehcam-engine` | Threads, on-demand camera, live config, status, preview, control socket. | Control socket is Unix-only |
| `bokehcam-cli` | `bokehcam`: service and tools. | No |
| `bokehcam-gui` | `bokehcam-gui`: preview and settings window (GPUI). | No |

### Porting to another platform

Only `bokehcam-media` needs a new backend with the same API as
`src/linux/`: `list_cameras`, `list_virtual_cameras`, `Camera`
(open, `next_packet`, modes, controls) and `VirtualCamera`
(open, write, consumer notifications). Everything else is portable code on
portable libraries. The control channel would need a named-pipe transport on
Windows.

## One frame

```
camera ─► decode (processor) ─► upload ─► framed ─┬──────────────────────────────► composite ─► colour ─► YUYV ─► virtual camera
                                                  └─► small ─► model input ─(model)─► mask         ▲
                                                        │        smooth over time ─► guided filter ─┤
                                                        └──────────────── blurred background ───────┘
```

1. **Capture and decode** (`media`). The capture thread waits for a frame
   and decodes it to RGBA (Motion-JPEG via `zune-jpeg`, or YUYV).
2. **Upload and frame** (`gpu`, `frame.wgsl`). The crop is the largest
   area with the output's aspect ratio, divided by the zoom, centred and
   clamped inside the capture. Cropping happens on the full-resolution
   frame, so zoom up to capture size / output size loses no detail.
3. **Model input.** The framed picture is reduced 4x (`down4.wgsl`), scaled
   to the model's input size and read back (a few hundred kilobytes).
4. **Model** (`infer`). Returns person probability per pixel, uploaded as a
   small texture.
5. **Smoothing over time** (`temporal.wgsl`). Per pixel, small changes from
   the previous mask (model noise, visible as shimmer) are damped; large
   changes (movement) pass straight through.
6. **Edge-aware refinement** (`gf_*.wgsl`, `box.wgsl`). A guided filter at a
   quarter of the output size fits, around every pixel, `mask ≈ a·luminance + b`.
   `a` and `b` are then applied to the *full-resolution* luminance in the
   composite pass, which gives the outline full-resolution edges for one
   multiply-add per pixel.
7. **Background** (`bg_prepare.wgsl`, `gauss.wgsl`). A Gaussian blur in which
   person pixels have zero weight, so your colours do not smear into the
   background as a halo.
8. **Composite, colour, pack** (`composite.wgsl`, `yuyv.wgsl`). The result is
   packed as YUYV on the graphics card, read back (4 MB at 1080p) and written
   to the virtual camera.

The mask is computed from the very frame it is applied to, so the outline
cannot lag behind the picture. The price is that the model's time adds to
every frame's delay, which is why models are chosen against a time budget.

### Adding a picture effect

An effect is a fragment shader, an entry for it in `shaders.rs` that lists
its input textures, and a `Pass` in `renderer.rs`. `common.wgsl` provides the
vertex stage, a parameter block of sixteen floats and a linear sampler; a
shader declares its textures from binding 2 upward and documents which
parameter slots it reads. Settings for it go in `bokehcam-core` (see below).

## Models

A model is described by a manifest (`crates/bokehcam-infer/models/*.toml`, or
a user file in `~/.local/share/bokehcam/models/`):

```toml
name = "selfie-multiclass"
title = "MediaPipe Selfie Multiclass"
description = "…"
license = "Apache-2.0"
source = "…"
file = "selfie_multiclass_256x256.ort"   # ONNX or ONNX Runtime format
url = "https://…"
sha256 = "…"
quality = 3                              # relative accuracy, for "auto"

[input]
width = 256
height = 256
layout = "nhwc"                          # or "nchw"
range = "0..1"                           # or "-1..1"

[output]
kind = "logits"                          # class scores, class 0 = background; or "alpha"
layout = "nhwc"
```

`Segmenter::load` handles selection:

- **Backend.** With `backend = "auto"`, a model is loaded and timed on the
  graphics card (ONNX Runtime's WebGPU provider, which runs on Vulkan, Metal
  or DirectX 12) and on the processor, and the faster one is kept. If the
  graphics card cannot run it, the processor is used.
- **Model.** With `model = "auto"`, downloaded models are tried from most to
  least accurate; the first that runs within 35% of the frame interval wins.

A model kind that does not fit the manifest (for example a recurrent matting
model that carries state between frames) needs code in `segmenter.rs`.

## Settings

`bokehcam-core/src/schema.rs` is the single description of every setting:
key, label, help text, type, range, and what has to restart when it changes.
It drives

- validation (`Config::validate`),
- the documented config file (`Config::to_toml`),
- the controls in the window (generated in `bokehcam-gui/src/settings.rs`),
- how a change is applied (`Config::apply_level`).

Adding a setting: a field in `config.rs`, an entry in `schema.rs`, and its
use in the engine. A test fails if the two files disagree.

## The engine

Threads:

- **worker**: uploads, runs the model, renders, writes; owns all state.
- **capture**: opens the camera, waits for frames, decodes. It hands frames
  over through a one-slot holder: if processing ever falls behind, old frames
  are dropped rather than queued, so delay never accumulates.
- **watcher**: learns from the virtual camera whether an app is reading it.

### On-demand camera

v4l2loopback raises a private event (`V4L2_EVENT_PRI_CLIENT_USAGE`) when a
reader starts or stops streaming. The engine opens the physical camera only
while a reader (or a preview) exists, plus `idle_timeout` seconds.

Two details of the driver matter:

- With `exclusive_caps=1` (needed so browsers list the device), the device
  only answers capture requests once a producer is streaming. The engine
  therefore writes one black frame right after opening the device.
- The first report after subscribing ("nobody is reading") must not start
  the idle timer, or the camera would switch on briefly at every start.

### Failure handling

Nothing short of a stop request ends the worker. A missing camera, a busy
virtual device or a failing model becomes `State::Problem(text)`, which front
ends show, and the engine retries every two seconds. A model that cannot be
loaded disables the blur but still delivers the picture.

### Control channel

`$XDG_RUNTIME_DIR/bokehcam.sock`, length-prefixed JSON (`control.rs`).
Requests: status, get config, set config, preview. Whichever process runs
the engine serves it: `bokehcam run`, or the window when no service is
running. The window and `bokehcam status/get/set/snapshot` are clients, so
they work the same in both cases.

## Decisions worth knowing

- **Two graphics contexts.** The picture pipeline (wgpu) and the model
  runtime (ONNX Runtime with its own WebGPU device) do not share memory, so
  the model input is read back and the mask uploaded each frame. Both are
  small; sharing would save about a millisecond at the cost of coupling to
  runtime internals.
- **Decoding on the processor.** Hardware JPEG decoding varies by vendor and
  driver. At 4K it costs about 16 ms on one core, on its own thread.
- **No busy-waiting.** The model runtime's thread spinning is disabled; at
  video frame rates it burns whole cores for no gain.
- **Strict config.** Unknown keys and out-of-range values are errors.

## Not done yet

- Appearance filters (skin smoothing, eye and teeth touch-up), which need a
  face-landmark analyser alongside the person mask.
- Background replacement with an image.
- Recurrent matting models (RVM) and running a heavy model on every Nth
  frame with motion-compensated masks in between.
- Uploading luma/chroma planes instead of RGBA (less upload work).
- Windows and macOS backends in `bokehcam-media`.
