use serde_json::Value;
use std::fs;
use std::process::Command;
use std::{thread, time::Duration};

fn main() {
    let mut last_track = String::new();

    loop {
        println!("Listening...");

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

        let output = Command::new("/home/admin/projects/vendor/songrec/target/release/songrec")
            .args(["recognize", "sample.wav", "--json"])
            .output()
            .expect("failed to execute songrec");

        let stdout = String::from_utf8_lossy(&output.stdout);

        if stdout.trim().is_empty() {
            println!("No JSON returned.");
            thread::sleep(Duration::from_secs(2));
            continue;
        }

        let json: Value = match serde_json::from_str(&stdout) {
            Ok(v) => v,
            Err(_) => {
                println!("No match or bad JSON.");
                thread::sleep(Duration::from_secs(2));
                continue;
            }
        };

        let title = json["track"]["title"].as_str().unwrap_or("Unknown");
        let artist = json["track"]["subtitle"].as_str().unwrap_or("Unknown");
        let cover = json["track"]["images"]["coverart"].as_str().unwrap_or("");

        let current = format!("{} - {}", artist, title);

        if current == last_track {
            println!("Same track: {}", current);
            thread::sleep(Duration::from_secs(2));
            continue;
        }

        if cover.is_empty() {
            println!("No artwork URL for {}", current);
            thread::sleep(Duration::from_secs(2));
            continue;
        }

        println!("Now playing: {}", current);
        println!("Artwork: {}", cover);

        match reqwest::blocking::get(cover) {
            Ok(resp) => match resp.bytes() {
                Ok(bytes) => {
                    if fs::write("current.jpg", &bytes).is_ok() {
                        let _ = Command::new("sudo")
                            .args(["pkill", "fbi"])
                            .status();

                        let _ = Command::new("sudo")
                            .args([
                                "fbi",
                                "-T",
                                "1",
                                "-d",
                                "/dev/fb0",
                                "--noverbose",
                                "-a",
                                "/home/admin/projects/songart/current.jpg",
                            ])
                            .status();

                        last_track = current;
                    } else {
                        println!("Failed to save artwork.");
                    }
                }
                Err(e) => println!("Failed reading image bytes: {e}"),
            },
            Err(e) => println!("Failed downloading artwork: {e}"),
        }

        thread::sleep(Duration::from_secs(2));
    }
}
