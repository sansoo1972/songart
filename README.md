# songart

Real-time music recognition, artwork display, and live audio metering for Raspberry Pi.

`songart` listens to ambient audio, identifies the currently playing song using SongRec (Shazam API), downloads high-resolution album artwork when available, and renders a configurable SDL-based display with artwork, metadata, and a live digital VU meter.

Version 2.0 introduces a modular codebase, config-driven display presets, theme-based typography, and a live visualizer.

---

## Features

- Real-time music recognition via SongRec
- Automatic high-resolution album artwork retrieval
- SDL-based artwork and metadata display
- Live digital VU meter driven from captured audio
- Configurable display presets for portrait and landscape layouts
- Theme-based typography with separate title and body fonts
- Theme-based font sizes
- Timestamped logging with configurable log levels
- Externalized runtime configuration via TOML
- Graceful Ctrl+C shutdown handling
- Runtime artifacts ignored by Git

---

## Architecture

Microphone → audio capture → SongRec JSON → Rust app → artwork download → SDL display + VU meter

---

## Project Structure

```text
songart/
├── assets/
│   └── fonts/              # Custom font assets
├── config/
│   └── songart.toml        # Runtime configuration
├── src/
│   ├── main.rs             # App bootstrap and thread startup
│   ├── config.rs           # Config structs and loader
│   ├── logging.rs          # Logging helpers and log levels
│   ├── state.rs            # Shared app/song/meter state
│   ├── audio.rs            # Audio meter helpers
│   ├── recognition.rs      # SongRec recognition loop
│   └── display.rs          # SDL rendering loop
├── Cargo.toml              # Rust dependencies
├── README.md
├── CHANGELOG.md
└── LICENSE
```

---

## Requirements

### Raspberry Pi

- Raspberry Pi OS
- USB microphone or supported audio input device
- HDMI-connected display

### System packages

```bash
sudo apt update
sudo apt install -y \
  libsdl2-dev \
  libsdl2-image-dev \
  libsdl2-ttf-dev \
  pkg-config
```

### SongRec

Installed separately:

```bash
cd ~/projects/vendor/songrec
cargo build --release
```

---

## Configuration

Runtime configuration lives in:

```text
config/songart.toml
```

### Configuration model

- `logging` controls log level and log file behavior
- `audio` controls capture device and cadence
- `paths` defines SongRec and artwork paths
- `display` selects the active display preset
- `display_presets` define scene geometry and spacing
- `fonts` selects the active theme
- `font_themes` define title/body font paths and font sizes
- `visualizer` controls the live VU meter

### Example

```toml
[logging]
level = "debug"
file = "/home/admin/projects/songart/songart.log"
reset_on_start = true

[audio]
device = "ps3eye_mono"
sample_wav = "/home/admin/projects/songart/sample.wav"
record_seconds = 2
loop_delay_secs = 2

[paths]
songrec_bin = "/home/admin/projects/vendor/songrec/target/release/songrec"
artwork_file = "/home/admin/projects/songart/current.jpg"

[display]
window_title = "songart"
fullscreen = true
orientation = "portrait"
frame_delay_ms = 33

[display_presets.portrait]
width = 720
height = 1280
top_panel_ratio = 0.72
panel_x = 40
panel_y = 28
title_line_spacing = 46
body_line_spacing = 34
detail_line_spacing = 40

[fonts]
theme = "fantasy"

[font_themes.fantasy]
title = "/home/admin/projects/songart/assets/fonts/Elvencommonspeak-0WXz.ttf"
body = "/home/admin/projects/songart/assets/fonts/SyneMono-Regular.ttf"
title_size = 38
body_size = 22

[visualizer]
enabled = true
mode = "vu"
position = "bottom"
style = "digital"
height = 60
padding = 16
peak_hold = true
smoothing = 0.10
```

### Change layout with one line

```toml
[display]
orientation = "portrait"
```

or

```toml
[display]
orientation = "landscape"
```

The selected preset controls:

- width
- height
- top panel ratio
- metadata panel origin
- line spacing

### Change typography with one line

```toml
[fonts]
theme = "retro"
```

Available theme names can include:

- `modern`
- `simple`
- `retro`
- `techy`
- `grungy`
- `fantasy`
- `scripted`

Each theme controls:

- title font path
- body font path
- title font size
- body font size

### Visualizer settings

```toml
[visualizer]
enabled = true
mode = "vu"
position = "bottom"
style = "digital"
height = 60
padding = 16
peak_hold = true
smoothing = 0.10
```

Current implementation:
- mono digital VU meter
- bottom strip placement
- RMS from recent audio tail
- peak hold support

---

## Running

### Build

```bash
cd ~/projects/songart
cargo build --release
```

### Run from the Raspberry Pi GUI terminal

```bash
cd ~/projects/songart
./target/release/songart
```

### Or run directly with Cargo

```bash
cd ~/projects/songart
cargo run --release
```

### Test SongRec manually

```bash
~/projects/vendor/songrec/target/release/songrec recognize \
  -d "<your-audio-device>" \
  --json
```

---

## Display behavior

`songart` renders a configured scene and scales it to fit the actual SDL canvas.

Important notes:

- The selected preset defines the intended scene size and layout.
- The actual OS / SDL canvas may still differ depending on the active desktop or display backend.
- Portrait mode behaves best when the Pi desktop session itself is already rotated to portrait.
- Running from the Pi GUI session is currently the most reliable path.

---

## Logging

Logging is controlled in `config/songart.toml`.

Supported levels:

- `error`
- `info`
- `debug`

View logs live:

```bash
tail -f /home/admin/projects/songart/songart.log
```

---

## Versioning

This project is now at **2.0.0**.

Recommended version tracking on GitHub:

1. Update the version in `Cargo.toml`
2. Commit the release changes
3. Create a Git tag such as `v2.0.0`
4. Push the tag
5. Create a GitHub Release from that tag

Example:

```bash
git tag -a v2.0.0 -m "songart 2.0.0"
git push origin main --tags
```

Suggested versioning approach:
- `2.0.0` = major release with module split and VU meter
- `2.0.1` = small fixes
- `2.1.0` = backward-compatible feature additions
- `3.0.0` = major architectural change

---

## Current Status

- Song recognition working
- JSON parsing working
- High-resolution artwork candidate selection working
- TOML-based runtime configuration working
- Modular source layout working
- Theme-based font selection working
- Theme-based font sizing working
- Display presets for portrait and landscape working
- Scene scaling to real SDL canvas working
- Digital VU meter working
- Timestamped logging with configurable log levels working
- Graceful Ctrl+C shutdown working

---

## Future Improvements

- Stereo VU meter
- Spectrum / EQ visualizer
- More display presets and layout themes
- Artwork caching and reuse
- Metadata enrichment from additional sources
- Boot-time auto start / service mode
- Transition effects between tracks
- Theme-based color palettes

---

## Author

Richard (`sansoo1972`)

---

## License

MIT
