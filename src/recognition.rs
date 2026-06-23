use crate::audio::{ write_wav_snapshot, SharedAudioBuffer };
use crate::logging::{ log_blank, log_debug, log_error, log_info };
use crate::state::{ AppContext, SongState };

use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::process::Command;
use std::sync::{ atomic::{ AtomicBool, Ordering }, Arc, Mutex };
use std::thread;
use std::time::Duration;

const UNKNOWN: &str = "Unknown";
const MUSICBRAINZ_USER_AGENT: &str = "songart/0.11.1 (https://github.com/sansoo1972/songart)";

/// Looks up a metadata value by title in SongRec's nested JSON sections.
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
        .or_else(|| metadata_value(json, "Composers"))
        .or_else(|| metadata_value(json, "Songwriter"))
        .or_else(|| metadata_value(json, "Songwriters"))
        .or_else(|| metadata_value(json, "Writers"))
        .or_else(|| metadata_value(json, "Writer"))
        .or_else(|| metadata_value(json, "Written By"))
        .or_else(|| metadata_value(json, "Written by"))
        .or_else(|| metadata_value(json, "Composed By"))
        .or_else(|| metadata_value(json, "Composed by"))
        .or_else(|| metadata_value(json, "Music By"))
        .or_else(|| metadata_value(json, "Music by"))
        .unwrap_or_else(|| UNKNOWN.to_string())
}

fn is_unknown(value: &str) -> bool {
    value.trim().is_empty() || value.eq_ignore_ascii_case(UNKNOWN)
}

fn relation_artist_name(relation: &Value) -> Option<String> {
    relation["artist"]["name"]
        .as_str()
        .or_else(|| relation["artist"]["sort-name"].as_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
}

fn metadata_titles(json: &Value) -> Vec<String> {
    let Some(sections) = json["track"]["sections"].as_array() else {
        return Vec::new();
    };

    let mut titles = BTreeSet::new();
    for section in sections {
        let Some(metadata) = section["metadata"].as_array() else {
            continue;
        };

        for item in metadata {
            if let Some(title) = item["title"].as_str() {
                let title = title.trim();
                if !title.is_empty() {
                    titles.insert(title.to_string());
                }
            }
        }
    }

    titles.into_iter().collect()
}

fn artist_search_terms(artist: &str) -> Vec<String> {
    let separators = [
        " feat. ",
        " feat ",
        " featuring ",
        " ft. ",
        " ft ",
        " with ",
        " x ",
        " X ",
        " & ",
        ",",
    ];

    let mut terms = Vec::new();
    let artist = artist.trim();

    if !artist.is_empty() && !is_unknown(artist) {
        terms.push(artist.to_string());

        let mut primary = artist;
        for separator in separators {
            if let Some((left, _)) = primary.split_once(separator) {
                primary = left.trim();
            }
        }

        if !primary.is_empty() && primary != artist {
            terms.push(primary.to_string());
        }
    }

    terms
}

fn recording_matches(recording: &Value, title: &str, artist: &str) -> bool {
    let title_lower = title.trim().to_ascii_lowercase();
    let artist_terms: Vec<String> = artist_search_terms(artist)
        .into_iter()
        .map(|term| term.to_ascii_lowercase())
        .collect();

    let recording_title = recording["title"].as_str().unwrap_or("").to_ascii_lowercase();
    let title_matches = title_lower.is_empty()
        || recording_title == title_lower
        || recording_title.contains(&title_lower)
        || title_lower.contains(&recording_title);

    let artist_matches = artist_terms.is_empty()
        || recording["artist-credit"]
            .as_array()
            .map(|credits| {
                credits.iter().any(|credit| {
                    let recording_artist = credit["artist"]["name"]
                        .as_str()
                        .unwrap_or("")
                        .to_ascii_lowercase();

                    artist_terms.iter().any(|artist_term| {
                        recording_artist.contains(artist_term)
                            || artist_term.contains(&recording_artist)
                    })
                })
            })
            .unwrap_or(true);

    title_matches && artist_matches
}

fn collect_composer_names_from_relations(relations: &Value, out: &mut BTreeSet<String>) {
    let Some(relations) = relations.as_array() else {
        return;
    };

    for relation in relations {
        let relation_type = relation["type"].as_str().unwrap_or("").to_ascii_lowercase();
        if matches!(
            relation_type.as_str(),
            "composer" | "writer" | "lyricist" | "librettist"
        ) {
            if let Some(name) = relation_artist_name(relation) {
                out.insert(name);
            }
        }

        collect_composer_names_from_relations(&relation["work"]["relations"], out);
    }
}

fn composer_from_musicbrainz_response(json: &Value, title: &str, artist: &str) -> Option<String> {
    let mut names = BTreeSet::new();

    if let Some(recordings) = json["recordings"].as_array() {
        for recording in recordings {
            if recording_matches(recording, title, artist) {
                collect_composer_names_from_relations(&recording["relations"], &mut names);
            }
        }

        if names.is_empty() {
            for recording in recordings {
                collect_composer_names_from_relations(&recording["relations"], &mut names);
            }
        }
    } else {
        collect_composer_names_from_relations(&json["relations"], &mut names);
    }

    if names.is_empty() {
        None
    } else {
        Some(names.into_iter().collect::<Vec<_>>().join(", "))
    }
}

fn musicbrainz_client(ctx: &AppContext) -> Option<reqwest::blocking::Client> {
    match reqwest::blocking::Client
        ::builder()
        .user_agent(MUSICBRAINZ_USER_AGENT)
        .timeout(Duration::from_secs(5))
        .build()
    {
        Ok(client) => Some(client),
        Err(e) => {
            log_debug(ctx, &format!("MusicBrainz client build failed: {e}"));
            None
        }
    }
}

fn fetch_musicbrainz_json(
    ctx: &AppContext,
    client: &reqwest::blocking::Client,
    url: &str,
    query: &[(&str, &str)]
) -> Option<Value> {
    let response = match client.get(url).query(query).send() {
        Ok(response) => response,
        Err(e) => {
            log_debug(ctx, &format!("MusicBrainz request failed for {url}: {e}"));
            return None;
        }
    };

    if !response.status().is_success() {
        log_debug(
            ctx,
            &format!("MusicBrainz request returned HTTP {} for {url}", response.status())
        );
        return None;
    }

    let body = match response.text() {
        Ok(body) => body,
        Err(e) => {
            log_debug(ctx, &format!("MusicBrainz response read failed for {url}: {e}"));
            return None;
        }
    };

    match serde_json::from_str(&body) {
        Ok(json) => Some(json),
        Err(e) => {
            log_debug(ctx, &format!("MusicBrainz JSON parse failed for {url}: {e}"));
            None
        }
    }
}

fn lookup_composer_by_isrc(
    ctx: &AppContext,
    client: &reqwest::blocking::Client,
    isrc: &str,
    title: &str,
    artist: &str
) -> Option<String> {
    let isrc = isrc.trim();
    if is_unknown(isrc) {
        return None;
    }

    let url = format!("https://musicbrainz.org/ws/2/isrc/{isrc}");
    let json = fetch_musicbrainz_json(
        ctx,
        client,
        &url,
        &[
            ("inc", "artist-credits+work-rels+artist-rels"),
            ("fmt", "json"),
        ],
    )?;

    let recording_count = json["recordings"].as_array().map(Vec::len).unwrap_or(0);
    log_debug(
        ctx,
        &format!("MusicBrainz ISRC lookup returned {recording_count} recording candidate(s).")
    );

    composer_from_musicbrainz_response(&json, title, artist)
}

fn escape_musicbrainz_search_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn musicbrainz_recording_search_query(title: &str, artist: &str) -> String {
    let title = escape_musicbrainz_search_value(title);
    let artist = escape_musicbrainz_search_value(artist);

    if artist.trim().is_empty() || is_unknown(&artist) {
        format!("recording:\"{title}\"")
    } else {
        format!("recording:\"{title}\" AND artist:\"{artist}\"")
    }
}

fn musicbrainz_recording_search_queries(title: &str, artist: &str) -> Vec<String> {
    let mut queries = Vec::new();

    for artist_term in artist_search_terms(artist) {
        queries.push(musicbrainz_recording_search_query(title, &artist_term));
    }

    queries.push(musicbrainz_recording_search_query(title, ""));
    queries.dedup();
    queries
}

fn musicbrainz_recording_ids(json: &Value, title: &str, artist: &str) -> Vec<String> {
    let Some(recordings) = json["recordings"].as_array() else {
        return Vec::new();
    };

    let mut ids = Vec::new();

    for recording in recordings {
        if recording_matches(recording, title, artist) {
            if let Some(id) = recording["id"].as_str() {
                ids.push(id.to_string());
            }
        }
    }

    if ids.is_empty() {
        for recording in recordings {
            if let Some(id) = recording["id"].as_str() {
                ids.push(id.to_string());
            }
        }
    }

    ids.truncate(3);
    ids
}

fn work_has_recording_relation(work: &Value, recording_ids: &[String]) -> bool {
    let Some(relations) = work["relations"].as_array() else {
        return false;
    };

    relations.iter().any(|relation| {
        relation["type"].as_str().unwrap_or("") == "performance"
            && relation["recording"]["id"]
                .as_str()
                .map(|id| recording_ids.iter().any(|recording_id| recording_id == id))
                .unwrap_or(false)
    })
}

fn composer_from_musicbrainz_work_search(
    json: &Value,
    title: &str,
    recording_ids: &[String]
) -> Option<String> {
    let Some(works) = json["works"].as_array() else {
        return None;
    };

    let mut names = BTreeSet::new();
    let title_lower = title.trim().to_ascii_lowercase();

    for work in works {
        let work_title = work["title"].as_str().unwrap_or("").to_ascii_lowercase();
        if work_title != title_lower {
            continue;
        }

        if !recording_ids.is_empty() && !work_has_recording_relation(work, recording_ids) {
            continue;
        }

        collect_composer_names_from_relations(&work["relations"], &mut names);
    }

    if names.is_empty() {
        None
    } else {
        Some(names.into_iter().collect::<Vec<_>>().join(", "))
    }
}

fn lookup_composer_by_work_search(
    ctx: &AppContext,
    client: &reqwest::blocking::Client,
    title: &str,
    recording_ids: &[String]
) -> Option<String> {
    if title.trim().is_empty() || is_unknown(title) {
        return None;
    }

    let title_query = format!("work:\"{}\"", escape_musicbrainz_search_value(title));
    let search = fetch_musicbrainz_json(
        ctx,
        client,
        "https://musicbrainz.org/ws/2/work",
        &[
            ("query", &title_query),
            ("limit", "10"),
            ("inc", "artist-rels+recording-rels"),
            ("fmt", "json"),
        ],
    )?;

    let result_count = search["works"].as_array().map(Vec::len).unwrap_or(0);
    log_debug(
        ctx,
        &format!("MusicBrainz work search query '{title_query}' returned {result_count} candidate(s).")
    );

    composer_from_musicbrainz_work_search(&search, title, recording_ids)
}

fn lookup_composer_by_recording_search(
    ctx: &AppContext,
    client: &reqwest::blocking::Client,
    title: &str,
    artist: &str
) -> Option<String> {
    if title.trim().is_empty() || is_unknown(title) {
        return None;
    }

    let mut recording_ids = Vec::new();
    let queries = musicbrainz_recording_search_queries(title, artist);

    for (i, query) in queries.iter().enumerate() {
        if i > 0 {
            thread::sleep(Duration::from_secs(1));
        }

        let Some(search) = fetch_musicbrainz_json(
            ctx,
            client,
            "https://musicbrainz.org/ws/2/recording",
            &[("query", query), ("limit", "5"), ("fmt", "json")],
        ) else {
            continue;
        };

        let result_count = search["recordings"].as_array().map(Vec::len).unwrap_or(0);
        log_debug(
            ctx,
            &format!(
                "MusicBrainz recording search query '{query}' returned {result_count} candidate(s)."
            )
        );

        recording_ids = musicbrainz_recording_ids(&search, title, artist);
        if !recording_ids.is_empty() {
            break;
        }
    }

    if recording_ids.is_empty() {
        log_debug(ctx, "MusicBrainz recording search returned no usable candidate IDs.");
        return None;
    }

    for (i, recording_id) in recording_ids.iter().enumerate() {
        if i > 0 {
            thread::sleep(Duration::from_secs(1));
        }

        let url = format!("https://musicbrainz.org/ws/2/recording/{recording_id}");
        let Some(recording) = fetch_musicbrainz_json(
            ctx,
            client,
            &url,
            &[
                ("inc", "artist-credits+work-rels+artist-rels+work-level-rels"),
                ("fmt", "json"),
            ],
        ) else {
            continue;
        };

        if let Some(composer) = composer_from_musicbrainz_response(&recording, title, artist) {
            return Some(composer);
        }
    }

    thread::sleep(Duration::from_secs(1));
    lookup_composer_by_work_search(ctx, client, title, &recording_ids)
}

fn lookup_composer_from_musicbrainz(
    ctx: &AppContext,
    isrc: &str,
    title: &str,
    artist: &str
) -> Option<String> {
    let client = musicbrainz_client(ctx)?;

    if !is_unknown(isrc) {
        if let Some(composer) = lookup_composer_by_isrc(ctx, &client, isrc, title, artist) {
            log_debug(ctx, "Composer resolved through MusicBrainz ISRC lookup.");
            return Some(composer);
        }

        log_debug(ctx, "MusicBrainz ISRC lookup did not return composer metadata.");
        thread::sleep(Duration::from_secs(1));
    } else {
        log_debug(ctx, "SongRec did not provide an ISRC for MusicBrainz composer lookup.");
    }

    let composer = lookup_composer_by_recording_search(ctx, &client, title, artist);
    if composer.is_some() {
        log_debug(ctx, "Composer resolved through MusicBrainz recording search.");
    } else {
        log_debug(ctx, "MusicBrainz recording search did not return composer metadata.");
    }
    composer
}

fn resolve_composer(ctx: &AppContext, json: &Value, title: &str, artist: &str) -> String {
    let composer = extract_composer(json);
    if !is_unknown(&composer) {
        log_debug(ctx, &format!("Composer resolved from SongRec metadata: {composer}"));
        return composer;
    }

    let isrc = extract_isrc(json);
    let titles = metadata_titles(json);
    log_debug(
        ctx,
        &format!(
            "Composer missing from SongRec metadata for '{title}' by '{artist}'. ISRC='{isrc}'. Metadata titles: {}",
            if titles.is_empty() {
                "none".to_string()
            } else {
                titles.join(", ")
            }
        )
    );

    lookup_composer_from_musicbrainz(ctx, &isrc, title, artist)
        .unwrap_or_else(|| UNKNOWN.to_string())
}

fn extract_track_number(json: &Value) -> String {
    metadata_value(json, "Track")
        .or_else(|| metadata_value(json, "Track Number"))
        .unwrap_or_else(|| "Unknown".to_string())
}

fn extract_genre(json: &Value) -> String {
    json["track"]["genres"]["primary"].as_str().unwrap_or("Unknown").to_string()
}

fn extract_isrc(json: &Value) -> String {
    json["track"]["isrc"].as_str().unwrap_or("Unknown").to_string()
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

/// Builds an ordered list of possible artwork URLs, preferring higher sizes
/// when the URL pattern supports it.
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

/// Picks the first usable seed artwork URL from the JSON response.
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

/// Downloads the best available artwork and writes it atomically.
fn download_best_artwork(
    ctx: &AppContext,
    json: &Value,
    output_path: &str
) -> Result<String, String> {
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

    let client = reqwest::blocking::Client
        ::builder()
        .user_agent("songart/0.1")
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))?;

    let mut candidates = Vec::new();
    for url in base_urls {
        candidates.extend(artwork_candidates(&url));
    }

    candidates.dedup();

    for candidate in candidates {
        log_debug(ctx, &format!("Trying artwork: {candidate}"));

        let resp = match client.get(&candidate).send() {
            Ok(r) => r,
            Err(e) => {
                log_debug(ctx, &format!("Download failed: {e}"));
                continue;
            }
        };

        if !resp.status().is_success() {
            log_debug(ctx, &format!("HTTP status {} for {}", resp.status(), candidate));
            continue;
        }

        let bytes = match resp.bytes() {
            Ok(b) => b,
            Err(e) => {
                log_debug(ctx, &format!("Failed reading bytes: {e}"));
                continue;
            }
        };

        if bytes.len() < 10_000 {
            log_debug(ctx, &format!("Rejected tiny image ({} bytes): {}", bytes.len(), candidate));
            continue;
        }

        let tmp_path = format!("{output_path}.tmp");

        fs
            ::write(&tmp_path, &bytes)
            .map_err(|e| format!("Failed to save temp artwork to {}: {e}", tmp_path))?;

        fs
            ::rename(&tmp_path, output_path)
            .map_err(|e| format!("Failed to rename temp artwork to {}: {e}", output_path))?;

        return Ok(candidate);
    }

    Err("No usable artwork URL succeeded".to_string())
}

/// Recognition loop.
///
/// This loop no longer records its own audio. Instead, it takes periodic WAV
/// snapshots from the shared rolling audio buffer and sends those snapshots to
/// SongRec for identification.
pub fn run_recognition_loop(
    ctx: Arc<AppContext>,
    running: Arc<AtomicBool>,
    shared_state: Arc<Mutex<SongState>>,
    shared_audio: Arc<Mutex<SharedAudioBuffer>>
) {
    let mut last_track = String::new();
    let mut last_artwork_url = String::new();
    let mut last_composer = UNKNOWN.to_string();
    let mut last_composer_lookup_attempted = false;

    log_info(&ctx, &format!("Log file: {}", ctx.config.logging.file));
    log_info(&ctx, "Recognition loop started.");

    while running.load(Ordering::SeqCst) {
        log_info(&ctx, "Listening...");

        let snapshot = {
            let audio = shared_audio.lock().unwrap();
            audio.recent_ms(ctx.config.audio.recognition_window_ms)
        };

        let min_required = ctx.config.audio.sample_rate;
        if snapshot.len() < min_required {
            log_info(&ctx, "Not enough buffered audio yet for recognition.");
            thread::sleep(Duration::from_secs(ctx.config.audio.loop_delay_secs));
            continue;
        }

        if
            let Err(e) = write_wav_snapshot(
                &ctx.config.audio.sample_wav,
                &snapshot,
                ctx.config.audio.sample_rate,
                ctx.config.audio.channels
            )
        {
            log_error(&ctx, &format!("Failed to write WAV snapshot: {e}"));
            thread::sleep(Duration::from_secs(ctx.config.audio.loop_delay_secs));
            continue;
        }

        if !running.load(Ordering::SeqCst) {
            break;
        }

        let output = match
            Command::new(&ctx.config.paths.songrec_bin)
                .args(["recognize", &ctx.config.audio.sample_wav, "--json"])
                .output()
        {
            Ok(output) => output,
            Err(e) => {
                log_error(&ctx, &format!("Failed to execute songrec: {e}"));
                thread::sleep(Duration::from_secs(ctx.config.audio.loop_delay_secs));
                continue;
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !running.load(Ordering::SeqCst) {
            break;
        }

        log_debug(&ctx, &format!("SongRec exit status: {}", output.status));
        if !stderr.trim().is_empty() {
            log_debug(&ctx, "SongRec stderr:");
            log_debug(&ctx, stderr.trim());
        }

        if stdout.trim().is_empty() {
            log_info(&ctx, "No JSON returned.");
            thread::sleep(Duration::from_secs(ctx.config.audio.loop_delay_secs));
            continue;
        }

        let json: Value = match serde_json::from_str(&stdout) {
            Ok(v) => v,
            Err(e) => {
                log_error(&ctx, &format!("No match or bad JSON: {e}"));
                thread::sleep(Duration::from_secs(ctx.config.audio.loop_delay_secs));
                continue;
            }
        };

        let title = json["track"]["title"].as_str().unwrap_or("Unknown");
        let artist = json["track"]["subtitle"].as_str().unwrap_or("Unknown");
        let album = extract_album(&json);
        let track_number = extract_track_number(&json);
        let released = extract_released(&json);
        let genre = extract_genre(&json);
        let label = extract_label(&json);
        let notes = extract_notes(&json);

        let current = format!("{artist} - {title}");

        let preview_url = pick_artwork_url(&json).unwrap_or_default();
        if preview_url.is_empty() {
            log_info(&ctx, &format!("No artwork URL for {current}"));
            thread::sleep(Duration::from_secs(ctx.config.audio.loop_delay_secs));
            continue;
        }

        if current == last_track && preview_url == last_artwork_url {
            if is_unknown(&last_composer) && !last_composer_lookup_attempted {
                let composer = resolve_composer(&ctx, &json, title, artist);
                last_composer_lookup_attempted = true;

                if !is_unknown(&composer) {
                    {
                        let mut state = shared_state.lock().unwrap();
                        state.composer = composer.clone();
                        state.version = state.version.wrapping_add(1);
                    }

                    last_composer = composer;
                    log_info(&ctx, &format!("Updated composer metadata for same track: {current}"));
                }
            }

            log_info(&ctx, &format!("Same track and artwork: {current}"));
            thread::sleep(Duration::from_secs(ctx.config.audio.loop_delay_secs));
            continue;
        }

        let composer = resolve_composer(&ctx, &json, title, artist);

        log_blank(&ctx);
        log_info(&ctx, "========================================");
        log_info(&ctx, "NOW PLAYING");
        log_info(&ctx, &format!("Song Title:   {title}"));
        log_info(&ctx, &format!("Artist:       {artist}"));
        log_info(&ctx, &format!("Album:        {album}"));
        log_info(&ctx, &format!("Track:        {track_number}"));
        log_info(&ctx, &format!("Composer:     {composer}"));
        log_info(&ctx, &format!("Released:     {released}"));
        log_info(&ctx, &format!("Genre:        {genre}"));
        log_info(&ctx, &format!("Label:        {label}"));
        log_info(&ctx, &format!("Seed URL:     {preview_url}"));
        log_info(&ctx, &format!("Notes:        {notes}"));
        log_info(&ctx, "========================================");
        log_blank(&ctx);

        match download_best_artwork(&ctx, &json, &ctx.config.paths.artwork_file) {
            Ok(final_url) => {
                log_info(&ctx, &format!("Final URL:    {final_url}"));

                let artwork_changed = final_url != last_artwork_url;

                {
                    let mut state = shared_state.lock().unwrap();
                    state.title = title.to_string();
                    state.artist = artist.to_string();
                    state.album = album;
                    state.track_number = track_number;
                    state.composer = composer.clone();
                    state.released = released;
                    state.genre = genre;
                    state.label = label;
                    state.notes = notes;
                    state.artwork_path = ctx.config.paths.artwork_file.clone();
                    state.artwork_url = final_url.clone();
                    state.version = state.version.wrapping_add(1);
                }

                if artwork_changed {
                    log_info(&ctx, "Updated UI state with new artwork.");
                } else {
                    log_info(&ctx, "Updated UI metadata; artwork unchanged.");
                }

                last_track = current;
                last_artwork_url = final_url;
                last_composer = composer;
                last_composer_lookup_attempted = true;
            }
            Err(e) => {
                log_error(&ctx, &format!("Failed to download artwork: {e}"));
            }
        }

        if running.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_secs(ctx.config.audio.loop_delay_secs));
        }
    }

    log_info(&ctx, "Recognition loop stopped.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_composer_from_songrec_metadata() {
        let json = json!({
            "track": {
                "sections": [
                    {
                        "metadata": [
                            { "title": "Album", "text": "Sample Album" },
                            { "title": "Composer", "text": "Jane Composer" }
                        ]
                    }
                ]
            }
        });

        assert_eq!(extract_composer(&json), "Jane Composer");
    }

    #[test]
    fn extracts_writer_as_composer_fallback() {
        let json = json!({
            "track": {
                "sections": [
                    {
                        "metadata": [
                            { "title": "Writers", "text": "Jane Writer, John Writer" }
                        ]
                    }
                ]
            }
        });

        assert_eq!(extract_composer(&json), "Jane Writer, John Writer");
    }

    #[test]
    fn extracts_songwriter_as_composer_fallback() {
        let json = json!({
            "track": {
                "sections": [
                    {
                        "metadata": [
                            { "title": "Songwriters", "text": "Jane Songwriter" }
                        ]
                    }
                ]
            }
        });

        assert_eq!(extract_composer(&json), "Jane Songwriter");
    }

    #[test]
    fn extracts_composer_from_musicbrainz_work_relations() {
        let json = json!({
            "recordings": [
                {
                    "title": "Song Title",
                    "artist-credit": [
                        { "artist": { "name": "Song Artist" } }
                    ],
                    "relations": [
                        {
                            "type": "performance",
                            "work": {
                                "relations": [
                                    {
                                        "type": "composer",
                                        "artist": { "name": "Jane Composer" }
                                    },
                                    {
                                        "type": "lyricist",
                                        "artist": { "name": "John Lyricist" }
                                    }
                                ]
                            }
                        }
                    ]
                }
            ]
        });

        assert_eq!(
            composer_from_musicbrainz_response(&json, "Song Title", "Song Artist"),
            Some("Jane Composer, John Lyricist".to_string())
        );
    }

    #[test]
    fn prefers_matching_musicbrainz_recordings() {
        let json = json!({
            "recordings": [
                {
                    "title": "Song Title",
                    "artist-credit": [
                        { "artist": { "name": "Song Artist" } }
                    ],
                    "relations": [
                        {
                            "type": "performance",
                            "work": {
                                "relations": [
                                    {
                                        "type": "composer",
                                        "artist": { "name": "Right Composer" }
                                    }
                                ]
                            }
                        }
                    ]
                },
                {
                    "title": "Other Song",
                    "artist-credit": [
                        { "artist": { "name": "Different Artist" } }
                    ],
                    "relations": [
                        {
                            "type": "performance",
                            "work": {
                                "relations": [
                                    {
                                        "type": "composer",
                                        "artist": { "name": "Fallback Composer" }
                                    }
                                ]
                            }
                        }
                    ]
                }
            ]
        });

        assert_eq!(
            composer_from_musicbrainz_response(&json, "Song Title", "Song Artist"),
            Some("Right Composer".to_string())
        );
    }

    #[test]
    fn builds_musicbrainz_recording_search_query() {
        assert_eq!(
            musicbrainz_recording_search_query("Song \"Title\"", "Song Artist"),
            "recording:\"Song \\\"Title\\\"\" AND artist:\"Song Artist\""
        );
    }

    #[test]
    fn builds_musicbrainz_recording_search_queries_with_primary_artist_fallback() {
        assert_eq!(
            musicbrainz_recording_search_queries("Song Title", "Song Artist feat. Guest"),
            vec![
                "recording:\"Song Title\" AND artist:\"Song Artist feat. Guest\"".to_string(),
                "recording:\"Song Title\" AND artist:\"Song Artist\"".to_string(),
                "recording:\"Song Title\"".to_string(),
            ]
        );
    }

    #[test]
    fn prefers_matching_musicbrainz_recording_ids() {
        let json = json!({
            "recordings": [
                {
                    "id": "fallback-id",
                    "title": "Other Song",
                    "artist-credit": [
                        { "artist": { "name": "Different Artist" } }
                    ]
                },
                {
                    "id": "matching-id",
                    "title": "Song Title",
                    "artist-credit": [
                        { "artist": { "name": "Song Artist" } }
                    ]
                }
            ]
        });

        assert_eq!(
            musicbrainz_recording_ids(&json, "Song Title", "Song Artist"),
            vec!["matching-id".to_string()]
        );
    }

    #[test]
    fn extracts_composer_from_work_search_matched_by_recording_relation() {
        let json = json!({
            "works": [
                {
                    "title": "Song Title",
                    "relations": [
                        {
                            "type": "writer",
                            "artist": { "name": "Jane Writer" }
                        },
                        {
                            "type": "performance",
                            "recording": {
                                "id": "matching-recording-id",
                                "title": "Song Title"
                            }
                        }
                    ]
                },
                {
                    "title": "Song Title",
                    "relations": [
                        {
                            "type": "writer",
                            "artist": { "name": "Wrong Writer" }
                        },
                        {
                            "type": "performance",
                            "recording": {
                                "id": "other-recording-id",
                                "title": "Song Title"
                            }
                        }
                    ]
                }
            ]
        });

        assert_eq!(
            composer_from_musicbrainz_work_search(
                &json,
                "Song Title",
                &["matching-recording-id".to_string()]
            ),
            Some("Jane Writer".to_string())
        );
    }
}
