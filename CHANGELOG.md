# Changelog

Notable changes to bokehcam are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-11-17

The first release: an early preview for Linux. Expect rough edges and
breaking changes; see the README for what is not verified yet.

### Added

- A virtual camera, fed through v4l2loopback, that any app can select.
- Background blur with an outline that follows real edges in the picture and
  is smoothed over time without trailing behind movement.
- A picture pipeline on the graphics card: framing and zoom from the
  camera's full resolution, colour controls, mirror.
- Three person-detection models (`selfie-landscape`, `selfie-multiclass`,
  `modnet`), run on the graphics card or the processor, whichever is faster.
  The most accurate model that fits the frame time is chosen automatically.
- The physical camera is only switched on while an app uses the virtual one.
- A settings window with a live preview, and the `bokehcam` command with
  `run`, `status`, `get`, `set`, `doctor`, `devices`, `models`, `snapshot`
  and `init`.
- A documented config file; changes from the window, the command line or an
  editor apply while bokehcam is running.

### Known limitations

- Linux only.
- The install and uninstall scripts have not been verified end to end.
- A second app can fail to open the virtual camera while another is using it.
- The settings window follows the system appearance, which some desktops
  report as light.

[Unreleased]: https://github.com/MohamedRahoui/bokehcam/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/MohamedRahoui/bokehcam/releases/tag/v0.1.0
