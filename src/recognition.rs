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
const MUSICBRAINZ_USER_AGENT: &str = "songart/0.18.0 (https://github.com/sansoo1972/songart)";

/// Looks up a metadata value by title in SongRec's nested JSON sections.
fn metadata_value(json: &Value, wanted_title: &str) -> Option<String> {
    let sections = json["track"]["sections"].as_array()?;

    for section in sections {
        let Some(metadata) = section["metadata"].as_array() else {
            continue;
        };
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

fn metadata_value_any(json: &Value, titles: &[&str]) -> Option<String> {
    titles.iter().find_map(|title| metadata_value(json, title))
}

fn clean_metadata_value(value: Option<String>) -> String {
    value
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| UNKNOWN.to_string())
}

fn extract_album_artist(json: &Value) -> String {
    clean_metadata_value(metadata_value_any(
        json,
        &["Album Artist", "Album Artists"],
    ))
}

fn extract_label(json: &Value, artist: &str) -> String {
    let label = clean_metadata_value(metadata_value_any(
        json,
        &["Record Label", "Label", "Publisher"],
    ));

    if !is_unknown(&label) && label.trim().eq_ignore_ascii_case(artist.trim()) {
        UNKNOWN.to_string()
    } else {
        label
    }
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

fn normalize_match_text(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .map(|ch| if ch.is_alphanumeric() { ch } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
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
    let title_lower = normalize_match_text(title);
    let artist_terms: Vec<String> = artist_search_terms(artist)
        .into_iter()
        .map(|term| normalize_match_text(&term))
        .collect();

    let recording_title = normalize_match_text(recording["title"].as_str().unwrap_or(""));
    let title_matches = title_lower.is_empty()
        || recording_title == title_lower
        || recording_title.contains(&title_lower)
        || title_lower.contains(&recording_title);

    let artist_matches = artist_terms.is_empty()
        || recording["artist-credit"]
            .as_array()
            .map(|credits| {
                credits.iter().any(|credit| {
                    let recording_artist =
                        normalize_match_text(credit["artist"]["name"].as_str().unwrap_or(""));

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

#[derive(Debug, Default, PartialEq)]
struct TrackMetadataEnrichment {
    release_id: String,
    album: String,
    album_artist: String,
    track_number: String,
    track_total: String,
    disc_number: String,
    disc_total: String,
    duration: String,
    released: String,
    label: String,
    catalog_number: String,
}

fn artist_credit_names(value: &Value) -> String {
    value
        .as_array()
        .map(|credits| {
            credits
                .iter()
                .filter_map(|credit| credit["artist"]["name"].as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| UNKNOWN.to_string())
}

fn musicbrainz_metadata_enrichment(
    json: &Value,
    title: &str,
    artist: &str,
    album: &str,
) -> TrackMetadataEnrichment {
    let Some(recordings) = json["recordings"].as_array() else {
        return TrackMetadataEnrichment::default();
    };
    let Some(recording) = recordings
        .iter()
        .find(|recording| recording_matches(recording, title, artist))
        .or_else(|| recordings.first())
    else {
        return TrackMetadataEnrichment::default();
    };

    let mut result = TrackMetadataEnrichment {
        duration: recording["length"]
            .as_u64()
            .map(format_duration_millis)
            .unwrap_or_default(),
        ..TrackMetadataEnrichment::default()
    };

    let Some(releases) = recording["releases"].as_array() else {
        return result;
    };
    let album_match = normalize_match_text(album);
    let exact_release = releases.iter().find(|release| {
        !album_match.is_empty()
            && normalize_match_text(release["title"].as_str().unwrap_or("")) == album_match
    });
    let Some(release) = exact_release.or_else(|| {
        if is_unknown(album) {
            releases.first()
        } else {
            None
        }
    })
    else {
        return result;
    };

    result.release_id = release["id"].as_str().unwrap_or("").to_string();
    result.album = release["title"].as_str().unwrap_or("").to_string();
    result.album_artist = artist_credit_names(&release["artist-credit"]);
    result.released = release["date"].as_str().unwrap_or("").to_string();

    if let Some(label_info) = release["label-info"]
        .as_array()
        .and_then(|labels| labels.first())
    {
        result.label = label_info["label"]["name"].as_str().unwrap_or("").to_string();
        result.catalog_number = label_info["catalog-number"]
            .as_str()
            .unwrap_or("")
            .to_string();
    }

    if let Some(media) = release["media"].as_array() {
        result.disc_total = media.len().to_string();
        let recording_id = recording["id"].as_str().unwrap_or("");

        for medium in media {
            let tracks = medium["tracks"]
                .as_array()
                .or_else(|| medium["track"].as_array());
            let matching_track = tracks.and_then(|tracks| {
                tracks
                    .iter()
                    .find(|track| {
                        track["recording"]["id"].as_str().unwrap_or("") == recording_id
                    })
                    .or_else(|| tracks.first())
            });

            if let Some(track) = matching_track {
                result.disc_number = medium["position"]
                    .as_u64()
                    .map(|value| value.to_string())
                    .unwrap_or_default();
                result.track_number = track["position"]
                    .as_u64()
                    .map(|value| value.to_string())
                    .or_else(|| track["number"].as_str().map(str::to_string))
                    .unwrap_or_default();
                result.track_total = medium["track-count"]
                    .as_u64()
                    .map(|value| value.to_string())
                    .unwrap_or_default();
                break;
            }
        }
    }

    result
}

fn lookup_metadata_by_isrc(
    ctx: &AppContext,
    isrc: &str,
    title: &str,
    artist: &str,
    album: &str,
) -> Option<TrackMetadataEnrichment> {
    if is_unknown(isrc) {
        return None;
    }

    let client = musicbrainz_client(ctx)?;
    let query = format!("isrc:{}", isrc.trim());
    let json = fetch_musicbrainz_json(
        ctx,
        &client,
        "https://musicbrainz.org/ws/2/recording",
        &[
            ("query", &query),
            ("limit", "1"),
            ("fmt", "json"),
        ],
    )?;
    let mut enrichment = musicbrainz_metadata_enrichment(&json, title, artist, album);
    if !enrichment.release_id.is_empty()
        && (enrichment.label.is_empty() || enrichment.catalog_number.is_empty())
    {
        thread::sleep(Duration::from_secs(1));
        let release_url = format!(
            "https://musicbrainz.org/ws/2/release/{}",
            enrichment.release_id
        );
        if let Some(release) = fetch_musicbrainz_json(
            ctx,
            &client,
            &release_url,
            &[("inc", "labels"), ("fmt", "json")],
        ) {
            if let Some(label_info) = release["label-info"]
                .as_array()
                .and_then(|labels| labels.first())
            {
                if enrichment.label.is_empty() {
                    enrichment.label =
                        label_info["label"]["name"].as_str().unwrap_or("").to_string();
                }
                if enrichment.catalog_number.is_empty() {
                    enrichment.catalog_number = label_info["catalog-number"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                }
            }
        }
    }

    Some(enrichment)
}

fn fill_missing(target: &mut String, fallback: String) {
    if is_unknown(target) && !fallback.trim().is_empty() && !is_unknown(&fallback) {
        *target = fallback;
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
    let title_lower = normalize_match_text(title);

    for work in works {
        let work_title = normalize_match_text(work["title"].as_str().unwrap_or(""));
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
    split_position(
        metadata_value_any(json, &["Track", "Track Number"]).as_deref(),
    )
    .0
}

fn split_position(value: Option<&str>) -> (String, String) {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return (UNKNOWN.to_string(), UNKNOWN.to_string());
    };

    for separator in [" of ", "/", "／"] {
        if let Some((position, total)) = value.split_once(separator) {
            return (
                clean_metadata_value(Some(position.trim().to_string())),
                clean_metadata_value(Some(total.trim().to_string())),
            );
        }
    }

    (value.to_string(), UNKNOWN.to_string())
}

fn extract_track_total(json: &Value) -> String {
    let (_, embedded_total) = split_position(
        metadata_value_any(json, &["Track", "Track Number"]).as_deref(),
    );
    if !is_unknown(&embedded_total) {
        embedded_total
    } else {
        clean_metadata_value(metadata_value_any(
            json,
            &["Track Count", "Total Tracks", "Tracks"],
        ))
    }
}

fn extract_disc_position(json: &Value) -> (String, String) {
    let value = metadata_value_any(json, &["Disc", "Disc Number"]);
    let (number, embedded_total) = split_position(value.as_deref());
    let total = if !is_unknown(&embedded_total) {
        embedded_total
    } else {
        clean_metadata_value(metadata_value_any(
            json,
            &["Disc Count", "Total Discs", "Discs"],
        ))
    };
    (number, total)
}

fn extract_duration(json: &Value) -> String {
    clean_metadata_value(
        metadata_value_any(json, &["Duration", "Time", "Length"]).or_else(|| {
            json["track"]["duration"]
                .as_u64()
                .map(format_duration_millis)
        }),
    )
}

fn format_duration_millis(value: u64) -> String {
    let total_seconds = if value > 10_000 { value / 1000 } else { value };
    format!("{}:{:02}", total_seconds / 60, total_seconds % 60)
}

fn extract_lyricist(json: &Value) -> String {
    clean_metadata_value(metadata_value_any(
        json,
        &["Lyricist", "Lyricists", "Lyrics By", "Lyrics by"],
    ))
}

fn extract_producer(json: &Value) -> String {
    clean_metadata_value(metadata_value_any(
        json,
        &["Producer", "Producers", "Produced By", "Produced by"],
    ))
}

fn extract_genre(json: &Value) -> String {
    json["track"]["genres"]["primary"].as_str().unwrap_or("Unknown").to_string()
}

fn extract_isrc(json: &Value) -> String {
    json["track"]["isrc"].as_str().unwrap_or("Unknown").to_string()
}

fn extract_explicit(json: &Value) -> String {
    if let Some(explicit) = json["track"]["hub"]["explicit"].as_bool() {
        return if explicit { "Explicit" } else { "Clean" }.to_string();
    }

    clean_metadata_value(metadata_value_any(
        json,
        &["Content Rating", "Rating", "Explicit"],
    ))
}

fn extract_copyright(json: &Value) -> String {
    clean_metadata_value(metadata_value_any(
        json,
        &["Copyright", "Copyright Line"],
    ))
}

fn extract_catalog_number(json: &Value) -> String {
    clean_metadata_value(metadata_value_any(
        json,
        &["Catalog Number", "Catalogue Number", "Catalog No.", "UPC"],
    ))
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
        let mut album = extract_album(&json);
        let mut album_artist = extract_album_artist(&json);
        let mut track_number = extract_track_number(&json);
        let mut track_total = extract_track_total(&json);
        let (mut disc_number, mut disc_total) = extract_disc_position(&json);
        let mut duration = extract_duration(&json);
        let mut released = extract_released(&json);
        let genre = extract_genre(&json);
        let mut label = extract_label(&json, artist);
        let isrc = extract_isrc(&json);
        let explicit = extract_explicit(&json);
        let copyright = extract_copyright(&json);
        let mut catalog_number = extract_catalog_number(&json);
        let lyricist = extract_lyricist(&json);
        let producer = extract_producer(&json);

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

        if [
            &album_artist,
            &track_number,
            &track_total,
            &disc_number,
            &duration,
            &label,
            &catalog_number,
        ]
        .iter()
        .any(|value| is_unknown(value))
        {
            if let Some(enrichment) =
                lookup_metadata_by_isrc(&ctx, &isrc, title, artist, &album)
            {
                fill_missing(&mut album, enrichment.album);
                fill_missing(&mut album_artist, enrichment.album_artist);
                fill_missing(&mut track_number, enrichment.track_number);
                fill_missing(&mut track_total, enrichment.track_total);
                fill_missing(&mut disc_number, enrichment.disc_number);
                fill_missing(&mut disc_total, enrichment.disc_total);
                fill_missing(&mut duration, enrichment.duration);
                fill_missing(&mut released, enrichment.released);
                fill_missing(&mut label, enrichment.label);
                fill_missing(&mut catalog_number, enrichment.catalog_number);
                log_debug(&ctx, "Filled missing structured metadata from MusicBrainz ISRC lookup.");
            }
        }

        let composer = resolve_composer(&ctx, &json, title, artist);

        log_blank(&ctx);
        log_info(&ctx, "========================================");
        log_info(&ctx, "NOW PLAYING");
        log_info(&ctx, &format!("Song Title:   {title}"));
        log_info(&ctx, &format!("Artist:       {artist}"));
        log_info(&ctx, &format!("Album:        {album}"));
        log_info(&ctx, &format!("Album Artist: {album_artist}"));
        log_info(&ctx, &format!("Track:        {track_number}"));
        log_info(&ctx, &format!("Track Total:  {track_total}"));
        log_info(&ctx, &format!("Disc:         {disc_number}"));
        log_info(&ctx, &format!("Disc Total:   {disc_total}"));
        log_info(&ctx, &format!("Duration:     {duration}"));
        log_info(&ctx, &format!("Composer:     {composer}"));
        log_info(&ctx, &format!("Lyricist:     {lyricist}"));
        log_info(&ctx, &format!("Producer:     {producer}"));
        log_info(&ctx, &format!("Released:     {released}"));
        log_info(&ctx, &format!("Genre:        {genre}"));
        log_info(&ctx, &format!("Label:        {label}"));
        log_info(&ctx, &format!("ISRC:         {isrc}"));
        log_info(&ctx, &format!("Rating:       {explicit}"));
        log_info(&ctx, &format!("Copyright:    {copyright}"));
        log_info(&ctx, &format!("Catalog No.:  {catalog_number}"));
        log_debug(&ctx, &format!("Seed URL:     {preview_url}"));
        log_info(&ctx, "========================================");
        log_blank(&ctx);

        match download_best_artwork(&ctx, &json, &ctx.config.paths.artwork_file) {
            Ok(final_url) => {
                log_debug(&ctx, &format!("Final URL:    {final_url}"));

                let artwork_changed = final_url != last_artwork_url;

                {
                    let mut state = shared_state.lock().unwrap();
                    state.title = title.to_string();
                    state.artist = artist.to_string();
                    state.album = album;
                    state.album_artist = album_artist;
                    state.track_number = track_number;
                    state.track_total = track_total;
                    state.disc_number = disc_number;
                    state.disc_total = disc_total;
                    state.duration = duration;
                    state.composer = composer.clone();
                    state.lyricist = lyricist;
                    state.producer = producer;
                    state.released = released;
                    state.genre = genre;
                    state.label = label;
                    state.isrc = isrc;
                    state.explicit = explicit;
                    state.copyright = copyright;
                    state.catalog_number = catalog_number;
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
    fn extracts_structured_track_and_release_metadata() {
        let json = json!({
            "track": {
                "duration": 245000,
                "isrc": "US-ABC-24-12345",
                "hub": { "explicit": true },
                "sections": [
                    { "type": "LYRICS", "text": ["First line"] },
                    {
                        "metadata": [
                            { "title": "Album Artist", "text": "Various Artists" },
                            { "title": "Track", "text": "3 of 12" },
                            { "title": "Disc Number", "text": "1/2" },
                            { "title": "Record Label", "text": "Example Records" },
                            { "title": "Producer", "text": "Pat Producer" },
                            { "title": "Lyricist", "text": "Lee Lyricist" },
                            { "title": "Catalog Number", "text": "CAT-123" }
                        ]
                    }
                ]
            }
        });

        assert_eq!(extract_album_artist(&json), "Various Artists");
        assert_eq!(extract_track_number(&json), "3");
        assert_eq!(extract_track_total(&json), "12");
        assert_eq!(extract_disc_position(&json), ("1".to_string(), "2".to_string()));
        assert_eq!(extract_duration(&json), "4:05");
        assert_eq!(extract_label(&json, "Song Artist"), "Example Records");
        assert_eq!(extract_producer(&json), "Pat Producer");
        assert_eq!(extract_lyricist(&json), "Lee Lyricist");
        assert_eq!(extract_isrc(&json), "US-ABC-24-12345");
        assert_eq!(extract_explicit(&json), "Explicit");
        assert_eq!(extract_catalog_number(&json), "CAT-123");
    }

    #[test]
    fn rejects_artist_name_as_ambiguous_record_label() {
        let json = json!({
            "track": {
                "sections": [{
                    "metadata": [
                        { "title": "Label", "text": "Neil Diamond" }
                    ]
                }]
            }
        });

        assert_eq!(extract_label(&json, "Neil Diamond"), UNKNOWN);
    }

    #[test]
    fn record_label_precedes_generic_label_metadata() {
        let json = json!({
            "track": {
                "sections": [{
                    "metadata": [
                        { "title": "Label", "text": "Ambiguous Label" },
                        { "title": "Record Label", "text": "Authoritative Records" }
                    ]
                }]
            }
        });

        assert_eq!(
            extract_label(&json, "Song Artist"),
            "Authoritative Records"
        );
    }

    #[test]
    fn enriches_missing_fields_from_matching_musicbrainz_release() {
        let json = json!({
            "recordings": [{
                "id": "recording-id",
                "title": "I Am...I Said",
                "length": 214693,
                "artist-credit": [{
                    "artist": { "name": "Neil Diamond" }
                }],
                "releases": [{
                    "id": "release-id",
                    "title": "All-Time Greatest Hits",
                    "date": "2014-07-08",
                    "artist-credit": [{
                        "artist": { "name": "Neil Diamond" }
                    }],
                    "label-info": [{
                        "catalog-number": "B0020837-02",
                        "label": { "name": "Capitol Records" }
                    }],
                    "media": [{
                        "position": 1,
                        "track-count": 23,
                        "track": [{
                            "number": "4",
                            "position": 4,
                            "recording": { "id": "recording-id" }
                        }]
                    }]
                }]
            }]
        });

        let result = musicbrainz_metadata_enrichment(
            &json,
            "I Am...I Said",
            "Neil Diamond",
            "All-Time Greatest Hits",
        );

        assert_eq!(result.release_id, "release-id");
        assert_eq!(result.album_artist, "Neil Diamond");
        assert_eq!(result.track_number, "4");
        assert_eq!(result.track_total, "23");
        assert_eq!(result.disc_number, "1");
        assert_eq!(result.disc_total, "1");
        assert_eq!(result.duration, "3:34");
        assert_eq!(result.released, "2014-07-08");
        assert_eq!(result.label, "Capitol Records");
        assert_eq!(result.catalog_number, "B0020837-02");
    }

    #[test]
    fn does_not_apply_release_details_from_a_different_album() {
        let json = json!({
            "recordings": [{
                "id": "recording-id",
                "title": "Song",
                "length": 180000,
                "artist-credit": [{ "artist": { "name": "Artist" } }],
                "releases": [{
                    "id": "wrong-release",
                    "title": "Different Compilation",
                    "date": "2020"
                }]
            }]
        });

        let result =
            musicbrainz_metadata_enrichment(&json, "Song", "Artist", "Wanted Album");

        assert_eq!(result.duration, "3:00");
        assert!(result.release_id.is_empty());
        assert!(result.track_number.is_empty());
        assert!(result.label.is_empty());
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

    #[test]
    fn work_search_matches_curly_and_straight_apostrophes() {
        let json = json!({
            "works": [
                {
                    "title": "Don’t Tell Me",
                    "relations": [
                        {
                            "type": "writer",
                            "artist": { "name": "Neil Arthur" }
                        },
                        {
                            "type": "writer",
                            "artist": { "name": "Stephen Luscombe" }
                        },
                        {
                            "type": "performance",
                            "recording": {
                                "id": "matching-recording-id",
                                "title": "Don’t Tell Me"
                            }
                        }
                    ]
                }
            ]
        });

        assert_eq!(
            composer_from_musicbrainz_work_search(
                &json,
                "Don't Tell Me",
                &["matching-recording-id".to_string()]
            ),
            Some("Neil Arthur, Stephen Luscombe".to_string())
        );
    }
}
