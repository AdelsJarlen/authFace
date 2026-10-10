# authFace — IR Camera Face Unlock for Linux

By [Peter Falkingham](https://peterfalkingham.com)

<p align="center">
  <img src="data/com.github.pfalkingham.face-auth-gtk.svg" width="128" height="128" alt="authFace logo">
</p>

**Windows Hello–style biometric login for Linux.** IR camera facial authentication via PAM — works on **immutable distros** (Bazzite, Bluefin, Fedora Silverblue, Fedora Kinoite, etc.) with zero system packages, daemons, or layering.

> [!NOTE]
> Includes improvements contributed via [SamVivan1/authFace](https://github.com/SamVivan1/authFace) — robust IR camera detection, distro-aware PAM configuration, and the GNOME Shell lock-screen scan indicator. See [Upstream Merges & Security Pass](#upstream-merges--security-pass).

> [!IMPORTANT]
> **This is the `local-patches` branch of [AdelsJarlen/authFace](https://github.com/AdelsJarlen/authFace)**,
> a fork that adds landmark-aligned recognition (better range), a failed-scan
> lockout, face unlock in GUI admin prompts, and a Face ID–style animation on
> the lock screen, the GDM login screen, `sudo` and admin prompts. See
> [This fork's changes](#this-forks-changes). Upgrading from upstream needs a
> **re-enrolment**.

- **Face unlock for sudo, lock screen (GNOME/Sway), `gdm-password` and polkit admin prompts**
- **~2 seconds** from camera poll to authenticated
- **Static musl binary** — no dependencies, no runtime
- **No daemon, no D-Bus** — only a `tmpfiles.d` entry and, for admin prompts, a polkit-helper drop-in
- **GUI settings panel** (optional GTK4 app) for camera selection and enrollment
- **Immutable-first core** — binaries and models fit in `/usr/local`; the login-screen animation is the one part that needs `/usr/share`

## This fork's changes

Seven commits on top of upstream `96a2fda`, developed and tested on a ThinkPad
T490 (Chicony `04f2:b681` IR camera), Fedora 44, GNOME 50 on Wayland.

| Change | Upstream | This fork |
|--------|----------|-----------|
| **Face alignment** | The whole 640×360 frame is squashed to 112×112 for the recognizer; the detector is only a yes/no gate. Recognition fades past ~30 cm because the face is a few pixels of the input. | SCRFD-500M (`det_500m.onnx`, from the same InsightFace `buffalo_sc` pack) finds the face and five landmarks. The face is warped onto ArcFace's standard landmark positions and only that 112×112 crop is encoded, as the model was trained. Templates move to version 2, so **re-enrol**. |
| **tract Resize workaround** | — | tract 0.21 mis-evaluates SCRFD's two run-time-sized Resize nodes (garbage scores when optimised). The loader rewrites them to constant 2× scales; output verified identical to onnxruntime. |
| **Failed-scan lockout** | Unlimited scans at every prompt | After `max_failures` (default 3) failed scans in a row, face unlock pauses and the prompt goes straight to fingerprint or password. Any successful login resets it, through a PAM `account` line. |
| **Corrupt first frame** | Enrolment aborts with `short frame`; the GUI preview loops on it | Enrolment and preview tolerate 3 consecutive capture errors, like authentication already did |
| **Scan animation** | Text pill, lock screen only; doesn't load on GNOME 50 | Face ID–style animation (scan beam, check mark, head-shake, amber "paused" state) on the lock screen, GDM login screen, `sudo` and polkit dialogs. GNOME 50 supported. |
| **Status file** | `/run/user/<uid>/face-auth-status`, unreadable before first login | One shared `/run/face-auth/status` (`<state> <user>`), created at boot by `tmpfiles.d` |
| **Admin prompts** | Password only | `deploy.sh` adds face-auth to polkit's PAM stack and opens the helper's sandbox to the IR camera |

Upgrade: rebuild, `sudo ./deploy.sh` (downloads the new detector), then
`sudo face-enroll --user $USER`. Until you re-enrol, old templates are refused
and the prompt falls back to fingerprint or password. For the login-screen
animation, also run `sudo extensions/authface-scan-indicator/install-greeter.sh`
and log out.

## Upstream Merges & Security Pass

Improvements merged from [SamVivan1/authFace](https://github.com/SamVivan1/authFace):

| Change | Before | Now |
|--------|--------|-----|
| **Multi-IR-camera support** | Returned the *first* IR-named device in `/sys/class/video4linux` | Collects all IR candidates and uses the first that **actually opens** as a GREY capture device |
| **Distro-aware PAM** | Only Fedora (`pam_selinux_permit.so` insertion point) | Also handles Ubuntu/Debian `gdm-password` (`#%PAM-1.0`) |
| **PAM `quiet` flag** | no `quiet` | `pam_exec.so quiet` suppresses `pam_exec` chatter |
| **Detector model download** | Required `models/version-slim-320.onnx` to be present | `deploy.sh` fetches it (now pinned to a commit and SHA-256 verified) |
| **Lock screen scan indicator** | None (silent scan) | `face-auth` writes a status file; a GNOME Shell extension renders scanning/ok/fail |

Followed by a security pass over the whole tree — see [CHANGELOG.md](CHANGELOG.md)
for the full list. The changes that affect how you use it:

- **Enrolment needs root** (`sudo face-enroll`, or one click in the GUI via
  `pkexec`). Face templates are authentication data; when the store was
  world-writable, any local user could enrol a face for an account that had not
  enrolled yet, then log in as it.
- **The login prompt trusts only `/etc/face-auth.toml`.** Your own config can
  make matching stricter, never looser. See [Configuration](#configuration).
- **`face-auth` requires `PAM_USER`** rather than falling back to `USER`,
  `LOGNAME` or `id -un`, and refuses remote (`PAM_RHOST`) sessions.

Upgrading re-secures an existing template store in place, so **no re-enrolment
is needed**.

## Features

- **Windows Hello–compatible IR camera support** — raw GREY format, no RGB camera needed
- **Automatic password fallback** — if face auth fails, times out, or no camera, PAM falls through to password
- **Static musl binary** (~20 MB, zero runtime dependencies) — copy to any Linux system
- **No daemon, no systemd, no D-Bus** — just `pam_exec.so` triggered by PAM
- **Configurable** via `/etc/face-auth.toml`, `~/.config/face-auth.toml`, or environment variables
- **Built-in capture timeout** (5s default) — camera hang won't lock you out
- **GTK4 settings GUI** — select IR camera, adjust threshold, preview live feed, enroll, improve matching, and test face recognition
- **Works on immutable distros** — no `rpm-ostree layer`, no package installs, no `/usr` modification

## Quick Start

```bash
# 1. Install core authentication (PAM, models, binaries)
sudo ./deploy.sh

# 2. Enroll your face (templates are root-owned, so this needs sudo)
sudo face-enroll --user $USER

# 3. Test sudo
sudo -k && sudo true   # triggers IR camera → exit 0

# 4. (Optional) Install the settings GUI
sudo ./deploy-gui.sh

# 5. Launch the GUI from app menu: "Face Authentication Settings"
#    or run: face-auth-gtk
```

## GUI — Face Authentication Settings

A native GTK4/libadwaita settings panel for configuring and testing face unlock:

| Feature | Description |
|---------|-------------|
| **Live IR preview** | Real-time camera feed with face-detection overlay |
| **Camera picker** | Dropdown to select between IR cameras (pre-filtered to openable devices) |
| **Threshold slider** | Adjust similarity threshold (0.1–0.95) — higher = stricter match |
| **Enroll** | Captures 5 frames and stores face embeddings (replaces existing) |
| **Improve Matching** | Captures 5 more frames and appends to existing embeddings |
| **Test** | Captures a single frame and compares against enrolled embeddings |
| **Automatic config save** | Camera and threshold changes persist to `~/.config/face-auth.toml` |

The GUI is optional and deployed separately (no GTK dependencies bundled with the core auth binary).

## Requirements

### Hardware

- **IR camera** exposing raw GREY format (Windows Hello compatible, e.g. Shinetech ASUS FHD webcam)
- **Linux kernel** with `uvcvideo` (standard on all distros)

> **Multiple IR cameras?** The fork's auto-detection finds *all* IR devices and picks the first one that opens. If you have more than one, set `device` explicitly in config to pin a specific one (see [Configuration](#configuration)).

### Software (target system — where you deploy)

- PAM with `pam_exec.so` (standard on all distros)
- SELinux (Fedora/Bluefin/Silverblue) — deploy script installs policy automatically
- `policycoreutils` for SELinux policy compilation (installed by default on Fedora)
- For the GUI: GTK4 + libadwaita runtime libraries (system-installed, not bundled)

### Software (build system — where you compile)

You need a Rust toolchain. For the core auth (musl), add `x86_64-unknown-linux-musl` target.
For the GUI (dynamic GTK), the host target is sufficient.

## Building from Source

### Core auth (static musl — no runtime deps)

```bash
# Install Rust if needed
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Add musl target
rustup target add x86_64-unknown-linux-musl

# Clone and build
git clone https://github.com/pfalkingham/authFace.git
cd authFace
cargo build --release --target x86_64-unknown-linux-musl -p face-auth -p face-enroll

# Deploy
sudo ./deploy.sh
```

### GUI (dynamic GTK — needs GTK4 + libadwaita devel packages)

```bash
# Install GTK development libraries
sudo pacman -S --needed gtk4 libadwaita        # Arch / CachyOS
sudo dnf install gtk4-devel libadwaita-devel   # Fedora
sudo apt install libgtk-4-dev libadwaita-1-dev # Debian / Ubuntu

# Build
cargo build --release -p face-auth-gtk

# Deploy
sudo ./deploy-gui.sh
```

### Without installing a toolchain (container build)

If `deploy.sh` finds no toolchain it prints this command for you to run. It
does not run it for you: `deploy.sh` runs under `sudo`, and rootless podman
driven through `sudo -u` often fails on a missing `XDG_RUNTIME_DIR`.

```bash
podman run --rm -v "$PWD":/src:Z -w /src docker.io/library/rust:alpine \
  sh -c 'apk add --no-cache musl-dev && \
         cargo build --release --target x86_64-unknown-linux-musl \
           -p face-auth -p face-enroll'
sudo ./deploy.sh
```

`rust:alpine` targets musl natively, so the result is the same static binary.
Run the container as your own user (not under `sudo`) so the files in `target/`
stay yours. This does not work for the GTK GUI, which links against the host's
GTK4 and must be built on the host.

### On immutable distros via distrobox

```bash
# Create a Fedora development container
distrobox create --image registry.fedoraproject.org/fedora:latest --name authface-dev
distrobox enter authface-dev

# Inside the container, install build deps (once).
# Note: Fedora does not package a musl std for Rust, so the musl build needs
# rustup rather than the distro `rust` package.
sudo dnf install -y gcc musl-gcc gtk4-devel libadwaita-devel
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y \
  --target x86_64-unknown-linux-musl
source "$HOME/.cargo/env"

# Build
cd ~/Projects/authFace
cargo build --release --target x86_64-unknown-linux-musl -p face-auth -p face-enroll
cargo build --release -p face-auth-gtk

# Exit container, then deploy on host
exit
sudo ./deploy.sh
sudo ./deploy-gui.sh
```

The GUI binary links against GTK4 dynamically, so build it in an environment
whose GTK version matches the host's — a distrobox sharing the host is fine, an
unrelated container image may not be.

> **`sudo ./deploy.sh` and `cargo`:** if you installed Rust with rustup, `cargo`
> lives in `~/.cargo/bin`, which is not on root's `PATH`. The script looks there
> for the invoking user and runs the build as that user rather than as root, so
> `sudo ./deploy.sh` works and does not leave root-owned files in `target/`.

## Deployment

### Core (PAM authentication)

```bash
sudo ./deploy.sh
```

| Step | What | Details |
|------|------|---------|
| Build | Compiles if `cargo` is available | Falls back to pre-built binaries in `target/` |
| Binaries | Installs to `/usr/local/bin` | `face-auth` + `face-enroll` |
| Models | Downloads InsightFace `buffalo_sc` once | `det_500m.onnx` (detector) and `w600k_mbf.onnx` (~13 MB, recognizer) to `/usr/local/share/face-auth/`, each SHA-256 verified |
| Config | Installs default config | `/etc/face-auth.toml` |
| PAM | Patches PAM service files | Adds `sufficient` `pam_exec.so quiet` to `sudo`, `gdm-password`, `swaylock`, `polkit-1`, plus an `account optional` line that resets the lockout counter |
| polkit | Copies `/usr/lib/pam.d/polkit-1` to `/etc/pam.d` | Plus a `polkit-agent-helper@.service` drop-in that lets the helper open the IR camera |
| Status dir | `/etc/tmpfiles.d/face-auth.conf` | Creates `/run/face-auth` at boot, labelled `xdm_var_run_t` |
| SELinux | Compiles and loads policy | Allows `xdm_t` to mmap camera for lock-screen auth |
| Storage | Creates embeddings directory | `/var/lib/face-auth/<user>/` with sticky bit |

GDM lock-screen patching is **distro-aware**:
- **Fedora/Bluefin/Silverblue** — inserts after `pam_selinux_permit.so`
- **Ubuntu/Debian** — inserts after `#%PAM-1.0`

Each PAM file is backed up with a `.face-auth.bak` suffix.

### GUI (optional settings panel)

```bash
sudo ./deploy-gui.sh
```

Automatically detects whether `/usr` is writable:
- **Mutable systems**: installs to `/usr/local/bin`, `/usr/share/applications/`, `/usr/share/icons/`
- **Immutable systems**: installs to `~/.local/bin`, `~/.local/share/applications/`, `~/.local/share/icons/`

Launch from the application menu: **Face Authentication Settings**, or run `face-auth-gtk`.

### Uninstall

```bash
# Remove everything (core + GUI + models + config)
sudo ./uninstall.sh

# Remove only the optional GUI
sudo ./uninstall.sh --gui

# Remove everything including face embeddings
sudo ./uninstall.sh --purge
```

Restores PAM backups, removes binaries, models, config, SELinux policy, desktop entries, and icons.

## Configuration

The authentication path and the unprivileged tools trust different things.

**During PAM authentication** (`face-auth`, i.e. sudo / lock screen / login):

| Source | Effect |
|--------|--------|
| `/etc/face-auth.toml` (root-owned) | Authoritative for everything |
| `~/.config/face-auth.toml` | May only make authentication **stricter** — see below |
| `FACE_AUTH_*` environment | **Ignored entirely** |

**For `face-enroll` and the settings GUI**, the usual layering applies:
environment variables, then `~/.config/face-auth.toml`, then `/etc/face-auth.toml`.

### What a user may override at the login prompt

A user's own config is read (resolved via `getent passwd`, so it is *their* home
and not whoever happened to invoke the PAM stack), but it is applied as a
narrowing overlay:

| Key | At the login prompt |
|-----|--------------------|
| `threshold`, `detector_threshold` | Honoured only if **>= the system value**. A lower number is ignored. |
| `device` | Honoured only if the path is a real IR capture device on this machine (IR-looking sysfs name, opens as GREY). |
| `scan_duration_ms`, `scan_interval_ms`, `capture_timeout_ms` | Honoured within built-in bounds. |
| `model_path`, `detector_model_path`, `embeddings_dir` | **Ignored** — system policy only. |

This is what stops code running as you — which does not know your password —
from writing a permissive `~/.config/face-auth.toml` and turning your next
`sudo` into a root shell. To *loosen* matching, edit `/etc/face-auth.toml` as
root; the GUI's slider starts at the system value for the same reason.

Example `/etc/face-auth.toml`:
```toml
device = "/dev/video2"   # usually best left unset; see below
threshold = 0.6
model_path = "/usr/local/share/face-auth/w600k_mbf.onnx"
embeddings_dir = "/var/lib/face-auth"
capture_timeout_ms = 5000
max_failures = 3         # failed scans before face unlock pauses; 0 = no limit
```

Environment variable names follow the field names, so the capture timeout is
`FACE_AUTH_CAPTURE_TIMEOUT_MS` (not `FACE_AUTH_CAPTURE_TIMEOUT`).

> **Leave `device` unset unless you must pin it.** UVC cameras normally expose
> a metadata node right beside the capture node under the same name — on the
> reference ASUS FHD webcam, `/dev/video2` captures and `/dev/video3` does not.
> Auto-detection opens each IR-named candidate and takes the first that is
> really a GREY capture device, which gets this right; a hand-written path
> often does not. Exception: on a ThinkPad T490 (Chicony `04f2:b681`) it
> picked the metadata node and failed with `ioctl failed: Invalid argument`;
> pin `device` if you see that.

The GUI writes camera and threshold changes to `~/.config/face-auth.toml`.

## Enrollment

Face templates live in a root-owned directory (`/var/lib/face-auth`, mode
`0700`), so enrolment is a privileged operation:

```bash
# Replace existing embeddings with a new capture
sudo face-enroll --user $USER

# Append new embeddings to improve recognition across lighting/angles
sudo face-enroll --improve --user $USER
```

CLI options: `--frames`, `--interval`, `--device`, `--threshold`, `--model`,
`--embeddings-dir`, `--improve`, `-v`.

The GUI's **Enroll Face**, **Improve Matching** and **Test Authentication**
buttons run the same helpers through `pkexec`, so you get a graphical
authentication prompt instead of a terminal. polkit's default for
`org.freedesktop.policykit.exec` is `auth_admin_keep`, so consecutive actions
within a few minutes will not re-prompt.

Why this is not user-writable: whatever can write a face template decides whose
face unlocks that account. If your own login could rewrite it, then so could
anything running as you, and a stolen browser session would become a root
shell at the next `sudo`.

## PAM Integration

The deploy script adds a `sufficient` `pam_exec.so quiet` line to:

| Service | File | Insertion point |
|---------|------|----------------|
| `sudo` | `/etc/pam.d/sudo` | After `#%PAM-1.0` |
| `gdm-password` | `/etc/pam.d/gdm-password` | After `pam_selinux_permit.so` (Fedora) / after `#%PAM-1.0` (Ubuntu/Debian) |
| `swaylock` | `/etc/pam.d/swaylock` | After `#%PAM-1.0` |
| `polkit-1` | `/etc/pam.d/polkit-1` (copied from `/usr/lib/pam.d`) | After `#%PAM-1.0` |

`sufficient` means: if face-auth exits 0, the user is authenticated immediately.
If it fails (no match, no camera, timeout), PAM falls through to password prompt.

Every service except `swaylock` also gets
`account optional pam_exec.so quiet /usr/local/bin/face-auth` at the end, and
`gdm-fingerprint` gets only that line. The account phase runs only after the
user has authenticated by some method, so face-auth uses it to reset the
failed-scan counter: a fingerprint or password login lifts a lockout.

`quiet` suppresses PAM chatter on the lock screen so the unlock UI stays clean.

No `timeout`, `setenv`, or `env_pass` flags are needed — face-auth reads the camera
(not stdin) and resolves `PAM_USER` via its own fallback chain.

## Scan Animation (GNOME Shell extension)

By default the scan is silent: `face-auth` runs headless inside PAM, so the only
feedback is the camera LED. A companion GNOME Shell extension draws a Face
ID–style animation while the face is being scanned, on the **lock screen**, the
**GDM login screen**, and in your session for **`sudo`** and **admin prompts**:

| Status | Animation |
|--------|-----------|
| Scanning | Face glyph in rounded corner brackets with a sweeping cyan scan line |
| Success | A green check mark draws in (finishes over the fading lock screen) |
| Failure | Red head-shake with a frown — "use your password" |
| Paused | Amber pause badge — "Face unlock paused — use fingerprint or password" |

### How it works

1. `face-auth` writes `<state> <user>` to `/run/face-auth/status` for every
   PAM service: `scanning`, then `ok`, `fail` or `paused`. The directory is
   created at boot by `tmpfiles.d`, so it exists before the login screen
   starts; the file is root-owned, written with `O_NOFOLLOW`, and readable by
   the `gdm` user and your session.
2. The extension watches the file with a `Gio.FileMonitor` in every session
   mode (`user`, `unlock-dialog`, `gdm`). A user session ignores scans for
   other accounts, results older than 15 s are ignored, and the card is raised
   above modal dialogs such as the polkit prompt.

No daemon, no D-Bus server — just a small status file, keeping the zero-footprint
design of the core.

### Install

```bash
# System-wide, including the GDM login screen (recommended)
sudo extensions/authface-scan-indicator/install-greeter.sh
# Then log out and back in (Wayland loads extensions only at login)

# Or per-user only (lock screen, sudo, admin prompts — not the login screen)
extensions/authface-scan-indicator/install-extension.sh
```

The login screen runs GNOME Shell as the `gdm` user, which cannot read your
home directory, so `install-greeter.sh` installs to
`/usr/share/gnome-shell/extensions/` and enables the extension in GDM's dconf
database. It removes a per-user copy, which would otherwise shadow it.
Remove with `sudo extensions/authface-scan-indicator/install-greeter.sh --remove`.
Requires GNOME Shell 45–50.

## How It Works

```
PAM (sudo / gdm-password / swaylock / polkit-1)
  │
  ▼
face-auth (static binary)
  ├─ Resolve PAM_USER via getent (refuses to guess from USER/LOGNAME)
  ├─ Refuse if PAM_RHOST names a remote host
  ├─ Load /etc/face-auth.toml + strictly-narrowing user overlay
  ├─ Paused after max_failures failed scans? → exit 1 without opening the camera
  ├─ V4L2 capture from IR camera (GREY, auto-detected or pinned /dev/videoN)
  │   └─ poll() with 5s timeout — exits cleanly if camera hangs
  ├─ Histogram equalization
  ├─ Face detection: SCRFD-500M → box + 5 landmarks
  ├─ Similarity warp onto ArcFace landmark positions → 112×112 crop, [-1, 1]
  ├─ tract-onnx inference (MobileFaceNet, 512-d embedding)
  ├─ Cosine similarity vs stored embeddings (default threshold 0.6)
  ├─ Write /run/face-auth/status; count or reset failures
  └─ Exit 0 (match) or exit 1 (no match → fingerprint / password)

PAM account phase (after any successful login) → face-auth resets the failure count
```

## Model

Uses two models from InsightFace's **`buffalo_sc`** pack:

- **`det_500m.onnx`** — SCRFD-500M face detector (box + five landmarks), run at
  640×384 so a 640×360 IR frame is not downscaled.
- **`w600k_mbf.onnx`** — MobileFaceNet @ WebFace600K, ~13 MB, 512-d embedding,
  fed the landmark-aligned 112×112 crop it was trained on.

Neither model is **bundled** in this repository. `deploy.sh` downloads the pack
once from InsightFace's official GitHub releases and verifies each file's
SHA-256 checksum.

> **Licence:** InsightFace's *code* is MIT, but its **pretrained models are
> released for non-commercial research use only**. Check whether that fits
> your use, for example on a work machine.

## SELinux

On Fedora/Bluefin/Silverblue with SELinux enforcing, the GNOME lock screen runs in the
`xdm_t` domain. This domain cannot `mmap` video devices by default. The deploy script
installs a minimal policy module:

```
allow xdm_t v4l_device_t:chr_file map;
```

To remove: `sudo semodule -r face_auth`

If the deploy script reported missing SELinux tools:
```bash
sudo dnf install -y policycoreutils
sudo checkmodule -M -m -o face_auth.mod selinux/face-auth.te
sudo semodule_package -o face_auth.pp -m face_auth.mod
sudo semodule -i face_auth.pp
```

## Troubleshooting

```bash
# List available IR cameras
ls /sys/class/video4linux/*/name

# Grant video group access (log out/in after)
sudo usermod -aG video $USER

# Debug output from a live sudo attempt
sudo -k; RUST_LOG=face_auth_core=debug,face_auth=debug sudo true

# Check PAM logs
journalctl | grep -i "pam_exec\|face-auth"

# SELinux denials
journalctl -k | grep face-auth | grep denied

# Test a stored face directly (skips PAM; needs root to read templates)
sudo face-auth --verify $USER
echo $?   # 0 = match, 1 = no match, 2 = error

# Raise the capture timeout (note the _MS suffix)
FACE_AUTH_CAPTURE_TIMEOUT_MS=10000 sudo face-auth --verify $USER

# GUI not launching from app menu?
face-auth-gtk    # run from terminal to see errors
```

### "PAM_USER is not set"

`face-auth` no longer guesses the account from `USER`/`LOGNAME`. If you are
invoking it by hand, use `--verify` rather than setting `PAM_USER` yourself.

### "reports pixel format ... requires raw 8-bit GREY"

The selected device is not an IR sensor — it is an ordinary RGB webcam, or the
metadata node that sits next to the real capture node. Let auto-detection pick
one, or check `v4l2-ctl --device /dev/videoN --list-formats`.

### Preview flickers, or "no face detected" every time

Most Windows Hello IR modules **strobe their illuminator**, emitting a lit frame
and a near-black one alternately. Check what yours does:

```bash
cargo run --example frame-stats
```

On the reference ASUS sensor the lit frames mean 71–237 (of 255) and the unlit
ones 2–5, strictly alternating at 15 fps. authFace captures frames in pairs and
keeps the brighter, so this is handled — but if `frame-stats` shows *every*
frame dark, the illuminator is not firing and no amount of software will help.

A dark frame is worse than useless: histogram equalisation stretches its narrow
range across the full scale and turns sensor noise into a high-contrast grey
field, which is what a flickering preview is showing you.

**Every frame dark: turn the emitter on.** On many laptops (ThinkPads, Dells)
the IR illuminator stays off under Linux until something sends the vendor's
UVC command. [linux-enable-ir-emitter](https://github.com/EmixamPP/linux-enable-ir-emitter)
finds and replays it. In 6.x, options go **before** the subcommand
(`sudo linux-enable-ir-emitter --device /dev/video2 configure`), then
`sudo systemctl enable --now linux-enable-ir-emitter`. Answer its prompts by
looking at the camera through a phone camera: the emitter shows as a purple
flash, while the small white LED is only the camera-on light. Its author warns
that `configure` may damage camera firmware, so read the project's notes first.

### "short frame: got N bytes, expected M"

Some UVC IR cameras send one truncated, error-flagged frame right after the
stream starts. This fork tolerates up to three in a row during enrolment and in
the GUI preview; if you still see it, you are running upstream binaries.

### "Invalid or outdated face template — re-enrol with face-enroll"

Your templates predate landmark-aligned recognition. Run
`sudo face-enroll --user $USER`. To see what the recognizer sees (box,
landmarks and the aligned crop as PNGs):

```bash
cargo run --release -p face-auth-core --example align-probe -- \
  /dev/video2 /usr/local/share/face-auth/det_500m.onnx . 3
```

### No animation on the login screen, but only after a reboot

`/run/face-auth` must exist when GDM starts, or the extension's file watch
falls back to slow polling and misses the scan. Check that
`/etc/tmpfiles.d/face-auth.conf` exists and `ls -ldZ /run/face-auth` shows
`xdm_var_run_t`; re-run `sudo ./deploy.sh` if not.

### Face unlock is paused

After `max_failures` failed scans, face unlock waits for a fingerprint or
password login. To clear it by hand: `sudo rm /var/lib/face-auth/$USER/failures`.

If it stays paused after a successful login at the login or lock screen, check
the store's SELinux label. GDM runs face-auth as `xdm_t`, which may only read
the generic `var_lib_t`, so its reset is denied (`avc: denied { unlink } ...
name="failures"`). `ls -dZ /var/lib/face-auth` should show
`face_auth_var_lib_t`; re-run `sudo ./deploy.sh` if it doesn't.

### Which camera will it use?

```bash
cargo run --example detect-camera
```

Lists every V4L2 node, whether its name looks like an IR sensor, whether it
really opens as a GREY capture device, and which one authentication would pick.

### Multi-camera picks the wrong device / no device selected

```bash
# See which IR device is detected
sudo face-auth --verify $USER   # with RUST_LOG=face_auth_core=debug

# Pin a specific camera in your own config (must be a real IR device)
echo 'device = "/dev/video2"' >> ~/.config/face-auth.toml
```

### My threshold change did nothing

A user config may only make matching *stricter*. To loosen it, lower
`threshold` in `/etc/face-auth.toml` as root — see [Configuration](#configuration).

## Security & Limitations

### Trust model

- **Face templates are root-owned.** `/var/lib/face-auth` is mode `0700`,
  root:root, with templates at `0600`. Whatever can write a template decides
  whose face unlocks that account, so enrolment goes through `sudo`/`pkexec`.
- **The PAM path trusts only `/etc/face-auth.toml`.** A user's own config may
  make matching stricter, never looser, and may not redirect the model or
  template paths. `FACE_AUTH_*` environment variables are ignored during
  authentication. See [Configuration](#configuration).
- **Identity comes from `PAM_USER` only.** `face-auth` refuses to run if PAM
  did not set it, rather than falling back to `USER`, `LOGNAME` or `id -un`.
- **Remote sessions are refused.** If `PAM_RHOST` names a non-local host,
  face authentication is declined — the camera is at the console, so otherwise
  whoever is sitting at the desk would authenticate an SSH session.

### Known limitations

- **IR-only, no liveness detection:** an IR camera resists casual photo
  spoofing, but there is no structured-light or dot-projection depth check.
  A high-quality IR-visible print or a 3D mask may bypass verification. This is
  the main residual risk and it is inherent to the approach — treat face unlock
  as a convenience over a password you still have, not as a stronger factor.
- **Lockout is per face, not per password.** After `max_failures` failed scans
  (default 3) face unlock pauses until a successful login by any method.
  The counter lives in `/var/lib/face-auth/<user>/failures`, root-only, and
  survives reboots. A user's own config may only lower the limit.
- **polkit helper sandbox is loosened.** For face unlock in admin prompts,
  a drop-in sets `PrivateDevices=no` on `polkit-agent-helper@.service`
  and allows only video4linux devices plus write access to
  `/var/lib/face-auth` and `/run/face-auth`.
- **`sufficient` bypasses the rest of the auth stack.** A successful match
  satisfies authentication outright; any other `auth` module below the
  face-auth line is skipped. That is the point, but it means the strength of
  the whole stack becomes the strength of the face match.
- **SELinux policy scope:** the lock-screen policy grants `xdm_t` mmap access
  to all V4L2 devices. A trade-off for drop-in compatibility; narrowing it
  requires custom udev device types.
- **x86_64 only:** V4L2 ioctl numbers and struct layouts are hardcoded.
  ARM/aarch64 requires switching to the `v4l` crate.
- **Model integrity:** both ONNX models are pinned by SHA-256 and the detector
  URL is pinned to a commit, not a branch. `deploy.sh` aborts on mismatch.

## Project Structure

```
authFace/
  crates/
    face-auth-core/          # Core library
      src/
        capture.rs           # V4L2 capture + poll() timeout + IR camera auto-detect
        config.rs            # Layered config + narrowing overlay for PAM + per-user load
        detector.rs          # SCRFD face detection: box + 5 landmarks
        error.rs             # Error types
        inference.rs         # tract-onnx model loading + encoding
        lib.rs               # FaceAuth struct, auth + enroll + scan
        preprocess.rs        # Histogram equalize, landmark alignment, normalize
        storage.rs           # Binary embedding I/O (versioned, atomic, 0600) + failure counter
      examples/              # align-probe, bench, detect-camera, frame-stats
        user.rs              # NSS lookup + username validation
        verify.rs            # Cosine similarity
    face-auth/               # PAM binary (stdin-less, PAM_USER fallback)
    face-enroll/             # Enrollment CLI
    face-auth-gtk/           # GTK4 settings GUI
  config/
    face-auth.toml.example   # Documented config template
  data/
    desktop file + icon      # App launcher assets
  selinux/
    face-auth.te             # SELinux policy source
  extensions/
    authface-scan-indicator/ # GNOME Shell scan animation
      extension.js
      metadata.json
      install-extension.sh   # per-user install
      install-greeter.sh     # system-wide install incl. GDM login screen
  deploy.sh                  # Core auth installer
  deploy-gui.sh              # Optional GUI installer
  uninstall.sh               # Removal script (--gui, --purge flags)
```

## License

MIT

This is a fork of [pfalkingham/authFace](https://github.com/pfalkingham/authFace) (MIT).
The models are not part of this repository: InsightFace's `w600k_mbf.onnx` and
`det_500m.onnx` are downloaded at install time and are licensed by InsightFace
for non-commercial research use only (see [Model](#model)).
