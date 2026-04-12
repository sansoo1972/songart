use serde_json::Value;
use std::fs;
use std::process::Command;
use std::{thread, time::Duration};

fn metadata_value(json: &Value, wanted_title: &str) -> Option<String> {
    let sections = json["track"]["sections"].as_array()?;

    for section in sections {
        let metadata = section["metadata"].as_array()?;
        for item in metadata {
            let title = item["title"].as_str().unwrap_or("");
            if title.eq_ignore_ascii_case(wanted_title) {
                let text = item["text"].as_str().unwrap_or("").trim();
                if !text.is_empty() {
                    return Some(text.to_string());
                }
            }
        }
    }

    None
}

fn extract_album(json: &Value) -> String {
    metadata_value(json, "Album").unwrap_or_else(|| "Unknown".to_string())
}

fn extract_label(json: &Value) -> String {
    metadata_value(json, "Label").unwrap_or_else(|| "Unknown".to_string())
}

fn extract_released(json: &Value) -> String {
    metadata_value(json, "Released").unwrap_or_else(|| "Unknown".to_string())
}

fn extract_composer(json: &Value) -> String {
    metadata_value(json, "Composer")
        .or_else(|| metadata_value(json, "Writers"))
        .or_else(|| metadata_value(json, "Writer"))
        .unwrap_or_else(|| "Unknown".to_string())
}

fn extract_track_number(json: &Value) -> String {
    metadata_value(json, "Track")
        .or_else(|| metadata_value(json, "Track Number"))
        .unwrap_or_else(|| "Unknown".to_string())
}

fn extract_genre(json: &Value) -> String {
    json["track"]["genres"]["primary"]
        .as_str()
        .unwrap_or("Unknown")
        .to_string()
}

fn extract_isrc(json: &Value) -> String {
    json["track"]["isrc"]
        .as_str()
        .unwrap_or("Unknown")
        .to_string()
}

fn extract_notes(json: &Value) -> String {
    let mut bits = Vec::new();

    let genre = extract_genre(json);
    if genre != "Unknown" {
        bits.push(format!("Genre: {genre}"));
    }

    let label = extract_label(json);
    if label != "Unknown" {
        bits.push(format!("Label: {label}"));
    }

    let isrc = extract_isrc(json);
    if isrc != "Unknown" {
        bits.push(format!("ISRC: {isrc}"));
    }

    if bits.is_empty() {
        "None".to_string()
    } else {
        bits.join(" | ")
    }
}

fn artwork_candidates(url: &str) -> Vec<String> {
    let mut out = Vec::new();

    if url.contains("mzstatic.com") {
        let replacements = [
            ("400x400cc.jpg", "3000x3000bb.jpg"),
            ("400x400cc.jpg", "2000x2000bb.jpg"),
            ("400x400cc.jpg", "1400x1400bb.jpg"),
            ("400x400cc.jpg", "1200x1200bb.jpg"),
            ("400x400cc.jpg", "800x800bb.jpg"),
            ("400x400cc.jpg", "600x600bb.jpg"),
            ("400x400cc.jpg", "400x400bb.jpg"),
            ("400x400cc.jpg", "3000x3000cc.jpg"),
            ("400x400cc.jpg", "1400x1400cc.jpg"),
            ("400x400cc.jpg", "1200x1200cc.jpg"),
            ("400x400cc.jpg", "800x800cc.jpg"),
        ];

        for (from, to) in replacements {
            if url.contains(from) {
                out.push(url.replace(from, to));
            }
        }
    }

    out.push(url.to_string());
    out.dedup();
    out
}

fn pick_artwork_url(json: &Value) -> Option<String> {
    let mut base_urls = Vec::new();

    if let Some(url) = json["track"]["images"]["coverarthq"].as_str() {
        if !url.is_empty() {
            base_urls.push(url.to_string());
        }
    }

    if let Some(url) = json["track"]["images"]["coverart"].as_str() {
        if !url.is_empty() {
            base_urls.push(url.to_string());
        }
    }

    if let Some(url) = json["track"]["images"]["background"].as_str() {
        if !url.is_empty() {
            base_urls.push(url.to_string());
        }
    }

    if base_urls.is_empty() {
        return None;
    }

    let mut candidates = Vec::new();
    for url in base_urls {
        candidates.extend(artwork_candidates(&url));
    }

    candidates.dedup();
    candidates.into_iter().next()
}

fn download_best_artwork(json: &Value, output_path: &str) -> Result<String, String> {
    let mut base_urls = Vec::new();

    if let Some(url) = json["track"]["images"]["coverarthq"].as_str() {
        if !url.is_empty() {
            base_urls.push(url.to_string());
        }
    }

    if let Some(url) = json["track"]["images"]["coverart"].as_str() {
        if !url.is_empty() {
            base_urls.push(url.to_string());
        }
    }

    if let Some(url) = json["track"]["images"]["background"].as_str() {
        if !url.is_empty() {
            base_urls.push(url.to_string());
        }
    }

    if base_urls.is_empty() {
        return Err("No artwork URL found in JSON".to_string());
    }

    let client = reqwest::blocking::Client::builder()
        .user_agent("songart/0.1")
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))?;

    let mut candidates = Vec::new();
    for url in base_urls {
        candidates.extend(artwork_candidates(&url));
    }

    candidates.dedup();

    for candidate in candidates {
        println!("Trying artwork: {}", candidate);

        let resp = match client.get(&candidate).send() {
            Ok(r) => r,
            Err(e) => {
                println!("Download failed: {e}");
                continue;
            }
        };

        if !resp.status().is_success() {
            println!("HTTP status {} for {}", resp.status(), candidate);
            continue;
        }

        let bytes = match resp.bytes() {
            Ok(b) => b,
            Err(e) => {
                println!("Failed reading bytes: {e}");
                continue;
            }
        };

        if bytes.len() < 10_000 {
            println!("Rejected tiny image ({} bytes): {}", bytes.len(), candidate);
            continue;
        }

        let tmp_path = format!("{output_path}.tmp");

        fs::write(&tmp_path, &bytes)
            .map_err(|e| format!("Failed to save temp artwork to {}: {e}", tmp_path))?;

        fs::rename(&tmp_path, output_path)
            .map_err(|e| format!("Failed to rename temp artwork to {}: {e}", output_path))?;

        return Ok(candidate);
    }

    Err("No usable artwork URL succeeded".to_string())
}

fn main() {
    let mut last_track = String::new();
    let mut last_artwork_url = String::new();

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
        let album = extract_album(&json);
        let track_number = extract_track_number(&json);
        let composer = extract_composer(&json);
        let released = extract_released(&json);
        let genre = extract_genre(&json);
        let label = extract_label(&json);
        let notes = extract_notes(&json);

        let current = format!("{} - {}", artist, title);

        let preview_url = pick_artwork_url(&json).unwrap_or_default();
        if preview_url.is_empty() {
            println!("No artwork URL for {}", current);
            thread::sleep(Duration::from_secs(2));
            continue;
        }

        if current == last_track && preview_url == last_artwork_url {
            println!("Same track and artwork: {}", current);
            thread::sleep(Duration::from_secs(2));
            continue;
        }

        println!();
        println!("========================================");
        println!("NOW PLAYING");
        println!("Song Title:   {}", title);
        println!("Artist:       {}", artist);
        println!("Album:        {}", album);
        println!("Track:        {}", track_number);
        println!("Composer:     {}", composer);
        println!("Released:     {}", released);
        println!("Genre:        {}", genre);
        println!("Label:        {}", label);
        println!("Seed URL:     {}", preview_url);
        println!("Notes:        {}", notes);
        println!("========================================");
        println!();

        match download_best_artwork(&json, "current.jpg") {
            Ok(final_url) => {
                println!("Final URL:    {}", final_url);

                let artwork_changed = final_url != last_artwork_url;

                if artwork_changed {
                    println!("Refreshing display...");

                    let _ = Command::new("sudo").args(["pkill", "fbi"]).status();

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
                } else {
                    println!("Artwork unchanged, skipping display refresh.");
                }

                last_track = current;
                last_artwork_url = final_url;
            }
            Err(e) => {
                println!("Failed to download artwork: {}", e);
            }
        }

        thread::sleep(Duration::from_secs(2));
    }
}
