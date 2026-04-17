# songart

Real-time music recognition and album artwork display system built on Raspberry Pi.

`songart` listens to ambient audio, identifies the currently playing song using SongRec (Shazam API), downloads high-resolution album artwork when available, and renders a configurable split-screen display with artwork on top and track metadata underneath.

The app is now driven by external TOML configuration for runtime behavior, display presets, font themes, and logging.

---

## Features

- Real-time music recognition via SongRec
- Automatic album artwork retrieval with higher-resolution artwork candidate selection
- SDL-based artwork + metadata display
- Configurable display presets for portrait and landscape layouts
- Theme-based typography with separate title and body fonts
- Theme-based font sizes
- Timestamped logging with configurable log levels
- Externalized runtime configuration via TOML
- Graceful Ctrl+C shutdown handling
- Generated runtime artifacts ignored by Git

---

## Architecture

Microphone → SongRec → JSON output → Rust app → Download artwork → SDL renderer

---

## Project Structure

```text
songart/
├── assets/
│   └── fonts/              # Custom font assets
├── config/
│   └── songart.toml        # Runtime configuration
├── src/
│   ├── config.rs           # Config structs and loader
│   └── main.rs             # Core application logic
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

Runtime configuration is stored in:

```text
config/songart.toml
```

### Current configuration model

- `logging` controls log level and log file behavior
- `audio` controls capture device and polling cadence
- `paths` defines SongRec and artwork paths
- `display` selects the active display preset
- `display_presets` define layout geometry and spacing
- `fonts` selects the active theme
- `font_themes` define title/body font paths and font sizes

### Example

```toml
[logging]
level = "debug"
file = "/home/admin/projects/songart/songart.log"
reset_on_start = true

[audio]
device = "ps3eye_mono"
sample_wav = "/home/admin/projects/songart/sample.wav"
record_seconds = 10
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

[display_presets.landscape]
width = 1280
height = 720
top_panel_ratio = 0.72
panel_x = 40
panel_y = 28
title_line_spacing = 46
body_line_spacing = 34
detail_line_spacing = 40

[fonts]
theme = "fantasy"

[font_themes.modern]
title = "/home/admin/projects/songart/assets/fonts/Orbitron-VariableFont_wght.ttf"
body = "/home/admin/projects/songart/assets/fonts/SyneMono-Regular.ttf"
title_size = 34
body_size = 24
```

### Changing layout with one line

Switch the active preset by changing:

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

### Changing typography with one line

Switch the active font theme by changing:

```toml
[fonts]
theme = "retro"
```

or another defined theme such as:
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

---

## Running

### Test SongRec manually

```bash
~/projects/vendor/songrec/target/release/songrec recognize \
  -d "<your-audio-device>" \
  --json
```

### Build the app

```bash
cd ~/projects/songart
cargo build --release
```

### Run from the Pi GUI terminal

```bash
cd ~/projects/songart
./target/release/songart
```

### Or run directly with Cargo

```bash
cd ~/projects/songart
cargo run --release
```

---

## Display behavior

`songart` now renders a configured scene and scales it to fit the actual SDL canvas.

Important note:

- The selected preset defines the intended scene size and layout
- The actual OS / SDL canvas may still differ depending on the active desktop or display backend
- For the cleanest portrait behavior, test from the rotated GUI session when the OS desktop is already rotated

---

## Logging

Logging is controlled in `config/songart.toml`.

Supported levels:
- `error`
- `info`
- `debug`

Example:

```toml
[logging]
level = "debug"
file = "/home/admin/projects/songart/songart.log"
reset_on_start = true
```

View logs live:

```bash
tail -f /home/admin/projects/songart/songart.log
```

---

## Current Status

- Song recognition working
- JSON parsing working
- High-resolution artwork candidate selection working
- TOML-based runtime configuration working
- Theme-based font selection working
- Theme-based font sizing working
- Display presets for portrait and landscape working
- Scene scaling to real SDL canvas working
- Timestamped logging with configurable log levels working
- Graceful Ctrl+C shutdown working

---

## Future Improvements

- More display presets and layout themes
- Artwork caching and reuse
- Metadata enrichment from additional sources
- Boot-time auto start / service mode
- Transition effects between tracks
- Optional color themes tied to font themes

---

## Author

Richard (`sansoo1972`)

---

## License

MIT
