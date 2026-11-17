# Security policy

## Reporting a vulnerability

Please do not open a public issue for a security problem.

Report it privately through
[GitHub's vulnerability reporting form](https://github.com/MohamedRahoui/bokehcam/security/advisories/new).
Include what you found, how to reproduce it, and which version or commit is
affected. This is a one-person project: reports are read and answered as soon
as the maintainer sees them.

## Supported versions

bokehcam is in early development and has no stable release yet. Fixes are
made on `main` only.

## What bokehcam touches

Useful when judging impact:

- It reads the camera and writes to a v4l2loopback device, as the user
  running it.
- It listens on a Unix socket in `$XDG_RUNTIME_DIR`, reachable only by that
  user. It opens no network ports.
- It downloads model files over HTTPS from the URLs in the model manifests
  and verifies each against a pinned SHA-256 checksum before use.
- The install script uses `sudo` to install the kernel module package and to
  write two files under `/etc` that load the module at boot.
