# songart

Real-time music recognition and album artwork display system built on Raspberry Pi.

`songart` listens to ambient audio, identifies the currently playing song using SongRec (Shazam API), downloads high-resolution album artwork when available, and renders a fullscreen split-screen display with artwork on top and track metadata underneath.

---

## 🚀 Features

- 🎧 Real-time music recognition via SongRec
- 🖼️ Automatic album artwork retrieval with higher-resolution artwork candidate selection
- 🖥️ Fullscreen SDL-based display with:
  - artwork panel
  - metadata panel
- 📝 Timestamped logging with configurable log levels
- ⚙️ Externalized runtime configuration via TOML
- ⚡ Runs from Raspberry Pi console without requiring a full desktop workflow
- 🧠 Built in Rust for performance and control

---

## 🏗️ Architecture

Microphone → SongRec → JSON output → Rust app → Download artwork → SDL fullscreen renderer

---

## 📁 Project Structure

```text
songart/
├── config/
│   └── songart.toml     # Runtime configuration
├── src/
│   ├── config.rs        # Config structs and loader
│   └── main.rs          # Core application logic
├── Cargo.toml           # Rust dependencies
├── README.md
└── CHANGELOG.md
```

---

## ⚙️ Requirements

### Raspberry Pi

- Raspberry Pi OS
- USB microphone or supported audio input device
- HDMI-connected display
- Local console access for fullscreen display testing

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

## 🔧 Configuration

Runtime configuration is stored in:

```text
config/songart.toml
```

Example:

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
font_path = "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"

[display]
window_title = "songart"
width = 1280
height = 720
fullscreen = true
top_panel_ratio = 0.72
title_font_size = 34
body_font_size = 24
frame_delay_ms = 33
```

### Find your audio device

List devices with:

```bash
~/projects/vendor/songrec/target/release/songrec recognize --list-devices
```

If needed, remap or adjust the configured audio device in `config/songart.toml`.

---

## ▶️ Running

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

### Run the app

From the Raspberry Pi console:

```bash
cd ~/projects/songart
SDL_VIDEODRIVER=kmsdrm cargo run --release
```

If needed, adjust the SDL video driver depending on your Pi environment.

---

## 🖥️ Display

The display is now rendered directly by the Rust application using SDL.

Layout:
- top panel: album artwork
- bottom panel: track metadata

Metadata shown includes:
- song title
- artist
- album
- track number
- composer/writer
- release year
- genre
- label
- notes/trivia fields derived from available metadata

---

## 📝 Logging

Logging is controlled by the configured log level in `config/songart.toml`.

Supported levels:
- `error`
- `info`
- `debug`

The log file path is also configured in TOML.

Example:

```toml
[logging]
level = "debug"
file = "/home/admin/projects/songart/songart.log"
reset_on_start = true
```

To follow logs live:

```bash
tail -f /home/admin/projects/songart/songart.log
```

---

## ⚠️ Notes / Known Issues

- SDL fullscreen behavior on Raspberry Pi may depend on the active video backend
- `kmsdrm` is currently the preferred fullscreen console path
- Correct audio input configuration is required for reliable recognition
- SongRec metadata availability varies by track
- Some fields may show as `Unknown` when not present in the Shazam response
- Artwork quality depends on source availability, but Apple-hosted artwork is now upgraded through higher-resolution candidate URLs when possible

---

## 🧪 Current Status

- ✅ Song recognition working
- ✅ JSON parsing working
- ✅ High-resolution artwork candidate selection working
- ✅ Timestamped logging with configurable log levels
- ✅ TOML-based runtime configuration working
- ✅ SDL split-screen display working
- ✅ Empty-text rendering edge case fixed
- ✅ Graceful Ctrl+C shutdown working

---

## 🔮 Future Improvements

- Smarter metadata enrichment from additional sources
- Better fallback handling for missing album/track details
- Artwork caching and reuse across repeated plays
- Boot-time auto start / service mode
- Transition/fade effects between tracks
- Optional UI theming and layout customization
- Operational packaging and deployment scripts

---

## 👤 Author

Richard (`sansoo1972`)

---

## 📄 License

MIT
