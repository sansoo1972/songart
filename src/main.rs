use serde_json::Value;
use std::process::Command;
use std::{thread, time::Duration};

fn main() {
    let mut last_track = String::new();

    loop {
        println!("🎤 Listening...");

        // Record audio
        let _ = Command::new("timeout")
            .args([
                "10s",
                "parecord",
                "--device=ps3eye_mono",
                "--rate=16000",
                "--channels=1",
                "--format=s16le",
                "sample.wav",
            ])
            .status();

        // Run SongRec
        let output = Command::new("/home/admin/projects/vendor/songrec/target/release/songrec")
            .args(["recognize", "sample.wav", "--json"])
            .output()
            .expect("failed to execute songrec");

        let stdout = String::from_utf8_lossy(&output.stdout);

        if stdout.trim().is_empty() {
            println!("❌ No match");
            thread::sleep(Duration::from_secs(2));
            continue;
        }

        let json: Value = match serde_json::from_str(&stdout) {
            Ok(v) => v,
            Err(_) => {
                println!("⚠️ Bad JSON");
                continue;
            }
        };

        let title = json["track"]["title"].as_str().unwrap_or("Unknown");
        let artist = json["track"]["subtitle"].as_str().unwrap_or("Unknown");
        let cover = json["track"]["images"]["coverart"].as_str().unwrap_or("");

        let current = format!("{} - {}", artist, title);

        // Only print if song changed
        if current != last_track {
            println!("\n🎵 Now Playing:");
            println!("{}", current);
            println!("🖼️ {}", cover);
            last_track = current;
        } else {
            println!("⏸️ Same track...");
        }

        thread::sleep(Duration::from_secs(2));
    }
}
