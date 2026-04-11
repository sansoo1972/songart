# songart

Real-time music recognition and album artwork display system built on Raspberry Pi.

This project listens to ambient audio, identifies the currently playing song using SongRec (Shazam API), and displays the corresponding album artwork fullscreen.

---

## 🚀 Features

- 🎧 Real-time music recognition via SongRec
- 🖼️ Automatic album artwork retrieval
- 📺 Fullscreen display using framebuffer (`fbi`)
- ⚡ Lightweight, runs headless (no desktop environment required)
- 🧠 Built in Rust for performance and control

---

## 🏗️ Architecture

Microphone → SongRec → JSON output → Rust app → Download artwork → Display via `fbi`

---

## 📁 Project Structure

```bash
songart/
├── src/
│   └── main.rs          # Core Rust application
├── current.jpg          # Latest downloaded artwork
├── Cargo.toml           # Rust dependencies
└── README.md
```

---

## ⚙️ Requirements

### Raspberry Pi

- Raspberry Pi OS
- USB microphone or supported audio input device
- HDMI-connected display

### System packages

```bash
sudo apt update
sudo apt install -y fbi libssl-dev pkg-config
```

### SongRec

Installed separately:

```bash
cd ~/projects/vendor/songrec
cargo build --release
```

---

## 🔧 Configuration

Make sure your audio input device is correct:

```bash
~/projects/vendor/songrec/target/release/songrec recognize --list-devices
```

If needed, update the device name used by your Rust app.

---

## ▶️ Running

### Test SongRec manually

```bash
~/projects/vendor/songrec/target/release/songrec recognize   -d "<your-audio-device>"   --json
```

### Run the Rust app

```bash
cd ~/projects/songart
cargo run --release
```

---

## 🖥️ Display

The app uses `fbi` to render images directly to the framebuffer.

- No X11 required
- Works from the Linux console
- Best run from the Pi’s local display session

To test image display manually:

```bash
sudo fbi -T 1 -d /dev/fb0 --noverbose -a current.jpg
```

---

## ⚠️ Notes / Known Issues

- Direct SDL/X11 display attempts may fail on CLI-only setups
- Framebuffer display via `fbi` is currently the most reliable path
- Correct audio device configuration is required for recognition
- Artwork sources depend on metadata returned by SongRec
- Some Rust crates may require OpenSSL development packages unless configured to use Rustls

---

## 🧪 Current Status

- ✅ Song recognition working
- ✅ JSON parsing working
- ✅ Artwork download working
- ✅ Fullscreen display working via framebuffer
- ⬜ Continuous auto-refresh loop polish
- ⬜ Smarter duplicate-track suppression
- ⬜ UI transitions / overlay text

---

## 🔮 Future Improvements

- Continuous listening loop with better debounce logic
- Smarter artwork resolution selection
- Fade/transition effects between songs
- On-screen artist/title overlay
- Boot-time auto start
- Additional metadata enrichment from streaming services

---

## 👤 Author

Richard (`sansoo1972`)

---

## 📄 License

TBD

