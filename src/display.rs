use crate::audio::{build_oscilloscope_points, compute_rms, SharedAudioBuffer};
use crate::config::DisplayPreset;
use crate::fft::compute_spectrum_bins;
use crate::logging::{log_debug, log_error, log_info};
use crate::state::{AppContext, SongState};
use crate::visualizer::VisualizerMode;

use sdl2::event::Event;
use sdl2::image::{InitFlag, LoadSurface, LoadTexture};
use sdl2::keyboard::Keycode;
use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::rect::{Point, Rect};
use sdl2::render::{Texture, TextureCreator, TextureQuery};
use sdl2::surface::Surface;
use sdl2::video::WindowContext;

use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

// ==============================================================================
// Text + Metadata Helpers
// ==============================================================================

fn album_line(state: &SongState) -> String {
    if state.album.trim().is_empty() || state.album == "Unknown" {
        "Album unknown".to_string()
    } else {
        state.album.clone()
    }
}

fn release_year_line(state: &SongState) -> String {
    if let Some(year) = parse_release_year(&state.released) {
        year.to_string()
    } else if state.released.trim().is_empty() || state.released == "Unknown" {
        "Unknown".to_string()
    } else {
        state.released.clone()
    }
}

// ==============================================================================
// Color Helpers
// ==============================================================================

fn parse_hex_color(value: &str, fallback: Color) -> Color {
    let hex = value.trim().trim_start_matches('#');

    if hex.len() != 6 {
        return fallback;
    }

    let r = u8::from_str_radix(&hex[0..2], 16);
    let g = u8::from_str_radix(&hex[2..4], 16);
    let b = u8::from_str_radix(&hex[4..6], 16);

    match (r, g, b) {
        (Ok(r), Ok(g), Ok(b)) => Color::RGB(r, g, b),
        _ => fallback,
    }
}

fn dim_color(color: Color, factor: f32) -> Color {
    let factor = factor.clamp(0.0, 1.0);
    Color::RGB(
        ((color.r as f32) * factor) as u8,
        ((color.g as f32) * factor) as u8,
        ((color.b as f32) * factor) as u8,
    )
}

fn canvas_background_color(ctx: &AppContext) -> Color {
    parse_hex_color(&ctx.config.display.colors.background, Color::RGB(0, 0, 0))
}

fn artwork_background_color(ctx: &AppContext) -> Color {
    parse_hex_color(
        &ctx.config.display.colors.artwork_background,
        canvas_background_color(ctx),
    )
}

fn metadata_background_color(ctx: &AppContext) -> Color {
    parse_hex_color(
        &ctx.config.display.colors.metadata_background,
        canvas_background_color(ctx),
    )
}

fn visualizer_background_color(ctx: &AppContext) -> Color {
    parse_hex_color(
        &ctx.config.display.colors.visualizer_background,
        canvas_background_color(ctx),
    )
}

#[derive(Clone, Debug)]
struct VisualizerDrawColors {
    upper: Color,
    lower: Color,
    palette: Vec<Color>,
}

impl VisualizerDrawColors {
    fn fixed(ctx: &AppContext) -> Self {
        let upper = parse_hex_color(
            &ctx.config.visualizer.colors.upper,
            Color::RGB(80, 220, 120),
        );
        let lower = parse_hex_color(
            &ctx.config.visualizer.colors.lower,
            Color::RGB(80, 160, 255),
        );

        Self {
            upper,
            lower,
            palette: vec![upper, lower],
        }
    }

    fn fallback(ctx: &AppContext) -> Self {
        let upper = parse_hex_color(
            &ctx.config.visualizer.colors.fallback_upper,
            Color::RGB(80, 220, 120),
        );
        let lower = parse_hex_color(
            &ctx.config.visualizer.colors.fallback_lower,
            Color::RGB(80, 160, 255),
        );

        Self {
            upper,
            lower,
            palette: vec![upper, lower],
        }
    }
}

#[derive(Clone, Copy)]
struct ArtworkColorCandidate {
    color: Color,
    hue: f32,
    score: f32,
}

#[derive(Clone, Copy, Default)]
struct HueBucket {
    red_sum: f32,
    green_sum: f32,
    blue_sum: f32,
    score_sum: f32,
    count: usize,
}

impl HueBucket {
    fn push(&mut self, r: u8, g: u8, b: u8, score: f32) {
        self.red_sum += (r as f32) * score;
        self.green_sum += (g as f32) * score;
        self.blue_sum += (b as f32) * score;
        self.score_sum += score;
        self.count += 1;
    }

    fn candidate(self, hue: f32) -> Option<ArtworkColorCandidate> {
        if self.count == 0 || self.score_sum <= 0.0 {
            return None;
        }

        Some(ArtworkColorCandidate {
            color: Color::RGB(
                (self.red_sum / self.score_sum).round().clamp(0.0, 255.0) as u8,
                (self.green_sum / self.score_sum).round().clamp(0.0, 255.0) as u8,
                (self.blue_sum / self.score_sum).round().clamp(0.0, 255.0) as u8,
            ),
            hue,
            score: self.score_sum,
        })
    }
}

fn color_channels(color: Color) -> (u8, u8, u8) {
    (color.r, color.g, color.b)
}

fn perceived_brightness(r: u8, g: u8, b: u8) -> f32 {
    (0.299 * (r as f32)) + (0.587 * (g as f32)) + (0.114 * (b as f32))
}

fn rgb_saturation(r: u8, g: u8, b: u8) -> f32 {
    let max = r.max(g).max(b) as f32;
    let min = r.min(g).min(b) as f32;

    if max <= 0.0 {
        0.0
    } else {
        (max - min) / max
    }
}

fn rgb_hue(r: u8, g: u8, b: u8) -> f32 {
    let rf = (r as f32) / 255.0;
    let gf = (g as f32) / 255.0;
    let bf = (b as f32) / 255.0;
    let max = rf.max(gf).max(bf);
    let min = rf.min(gf).min(bf);
    let delta = max - min;

    if delta <= f32::EPSILON {
        return 0.0;
    }

    let hue = if (max - rf).abs() <= f32::EPSILON {
        60.0 * (((gf - bf) / delta) % 6.0)
    } else if (max - gf).abs() <= f32::EPSILON {
        60.0 * (((bf - rf) / delta) + 2.0)
    } else {
        60.0 * (((rf - gf) / delta) + 4.0)
    };

    if hue < 0.0 {
        hue + 360.0
    } else {
        hue
    }
}

fn hue_distance(a: f32, b: f32) -> f32 {
    let diff = (a - b).abs();
    diff.min(360.0 - diff)
}

fn pick_spread_palette(
    candidates: &[ArtworkColorCandidate],
    palette_size: usize,
) -> Vec<ArtworkColorCandidate> {
    let target_size = palette_size.clamp(2, 12);
    let mut palette = Vec::new();

    for min_distance in [55.0, 40.0, 25.0, 0.0] {
        for candidate in candidates {
            if palette.len() >= target_size {
                return palette;
            }

            let is_distinct = palette.iter().all(|selected: &ArtworkColorCandidate| {
                hue_distance(selected.hue, candidate.hue) >= min_distance
            });

            if is_distinct {
                palette.push(*candidate);
            }
        }
    }

    palette
}

fn lerp_channel(a: u8, b: u8, t: f32) -> u8 {
    ((a as f32) + ((b as f32) - (a as f32)) * t)
        .round()
        .clamp(0.0, 255.0) as u8
}

fn palette_color_at(palette: &[Color], index: usize, count: usize) -> Color {
    match palette {
        [] => Color::RGB(80, 220, 120),
        [color] => *color,
        colors => {
            let denom = count.saturating_sub(1).max(1) as f32;
            let position = (index as f32 / denom) * ((colors.len() - 1) as f32);
            let left = position.floor() as usize;
            let right = (left + 1).min(colors.len() - 1);
            let t = position - (left as f32);
            let a = colors[left];
            let b = colors[right];

            Color::RGB(
                lerp_channel(a.r, b.r, t),
                lerp_channel(a.g, b.g, t),
                lerp_channel(a.b, b.b, t),
            )
        }
    }
}

fn extract_visualizer_colors_from_artwork(
    ctx: &AppContext,
    artwork_path: &str,
) -> Result<VisualizerDrawColors, String> {
    let surface = Surface::from_file(artwork_path)?;
    let surface = surface.convert_format(PixelFormatEnum::RGB24)?;
    let pixels = surface
        .without_lock()
        .ok_or_else(|| "Artwork pixel buffer requires locking".to_string())?;

    let width = surface.width() as usize;
    let height = surface.height() as usize;
    let pitch = surface.pitch() as usize;
    let pixel_count = width.saturating_mul(height);

    if width == 0 || height == 0 || pixel_count == 0 {
        return Err("Artwork has no pixels".to_string());
    }

    let sample_stride = (pixel_count / 4096).max(1);
    let min_brightness = ctx.config.visualizer.colors.min_brightness as f32;
    let min_saturation = ctx.config.visualizer.colors.min_saturation.clamp(0.0, 1.0);
    let hue_bucket_count = ctx.config.visualizer.colors.hue_bucket_count.clamp(3, 36);
    let mut buckets = vec![HueBucket::default(); hue_bucket_count];

    for i in (0..pixel_count).step_by(sample_stride) {
        let x = i % width;
        let y = i / width;
        let offset = y.saturating_mul(pitch) + x.saturating_mul(3);

        if offset + 2 >= pixels.len() {
            continue;
        }

        let r = pixels[offset];
        let g = pixels[offset + 1];
        let b = pixels[offset + 2];
        let brightness = perceived_brightness(r, g, b);
        let saturation = rgb_saturation(r, g, b);

        if brightness < min_brightness || saturation < min_saturation {
            continue;
        }

        let hue = rgb_hue(r, g, b);
        let bucket_index =
            ((hue / 360.0) * (hue_bucket_count as f32)).floor() as usize % hue_bucket_count;
        let score = saturation * 2.0 + (brightness / 255.0);

        buckets[bucket_index].push(r, g, b, score);
    }

    let bucket_width = 360.0 / (hue_bucket_count as f32);
    let mut candidates: Vec<ArtworkColorCandidate> = buckets
        .into_iter()
        .enumerate()
        .filter_map(|(index, bucket)| bucket.candidate((index as f32 + 0.5) * bucket_width))
        .collect();

    candidates.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let palette = pick_spread_palette(&candidates, ctx.config.visualizer.colors.palette_size);
    let upper = palette
        .first()
        .copied()
        .ok_or_else(|| "Artwork did not contain enough bright saturated pixels".to_string())?;

    let lower = palette
        .iter()
        .copied()
        .find(|candidate| hue_distance(candidate.hue, upper.hue) >= 35.0)
        .or_else(|| palette.get(1).copied())
        .unwrap_or(upper);

    Ok(VisualizerDrawColors {
        upper: upper.color,
        lower: lower.color,
        palette: palette
            .into_iter()
            .map(|candidate| candidate.color)
            .collect(),
    })
}

fn visualizer_colors_for_artwork(
    ctx: &AppContext,
    artwork_path: Option<&str>,
) -> VisualizerDrawColors {
    match ctx
        .config
        .visualizer
        .colors
        .mode
        .to_ascii_lowercase()
        .as_str()
    {
        "artwork" => {
            if let Some(path) = artwork_path {
                match extract_visualizer_colors_from_artwork(ctx, path) {
                    Ok(colors) => {
                        let (ur, ug, ub) = color_channels(colors.upper);
                        let (lr, lg, lb) = color_channels(colors.lower);
                        log_debug(
                            ctx,
                            &format!(
                                "Visualizer colors derived from artwork: upper=#{:02X}{:02X}{:02X} lower=#{:02X}{:02X}{:02X} palette_colors={}",
                                ur,
                                ug,
                                ub,
                                lr,
                                lg,
                                lb,
                                colors.palette.len()
                            ),
                        );
                        colors
                    }
                    Err(e) => {
                        log_error(
                            ctx,
                            &format!(
                                "Failed to derive visualizer colors from artwork; using fallback colors: {e}"
                            ),
                        );
                        VisualizerDrawColors::fallback(ctx)
                    }
                }
            } else {
                VisualizerDrawColors::fallback(ctx)
            }
        }
        "fixed" => VisualizerDrawColors::fixed(ctx),
        other => {
            log_error(
                ctx,
                &format!(
                    "Unknown visualizer color mode '{}'; using fallback colors",
                    other
                ),
            );
            VisualizerDrawColors::fallback(ctx)
        }
    }
}

// ==============================================================================
// Font Theme Selection
// ==============================================================================

fn parse_release_year(released: &str) -> Option<i32> {
    let digits: String = released
        .chars()
        .filter(|c| c.is_ascii_digit())
        .take(4)
        .collect();

    if digits.len() == 4 {
        digits.parse::<i32>().ok()
    } else {
        None
    }
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

fn metadata_font_theme(ctx: &AppContext, state: &SongState) -> String {
    let genre = state.genre.to_ascii_lowercase();
    let year = parse_release_year(&state.released);

    // Strong era/vibe override for synth-heavy 80s music.
    if contains_any(
        &genre,
        &["electronic", "synth", "synth-pop", "new wave", "dance"],
    ) || matches!(year, Some(1980..=1989))
    {
        return "techy".to_string();
    }

    // 90s rock/alternative/grunge gets a rougher style.
    if contains_any(&genre, &["rock", "alternative", "grunge", "punk"])
        && matches!(year, Some(1990..=1999))
    {
        return "grungy".to_string();
    }

    // Older music gets a retro display treatment.
    if matches!(year, Some(0..=1979)) {
        return "retro".to_string();
    }

    // Soundtracks, scores, classical, and orchestral music.
    if contains_any(&genre, &["classical", "soundtrack", "score", "orchestral"]) {
        return "fantasy".to_string();
    }

    // Acoustic / folk / singer-songwriter / country / latin.
    if contains_any(
        &genre,
        &["folk", "acoustic", "country", "singer-songwriter", "latin"],
    ) {
        return "scripted".to_string();
    }

    // Modern mainstream genres.
    if contains_any(&genre, &["pop", "r&b", "hip-hop", "rap"]) || matches!(year, Some(2000..=9999))
    {
        return "modern".to_string();
    }

    ctx.config.fonts.fallback_theme.to_ascii_lowercase()
}

fn selected_font_theme(ctx: &AppContext, state: &SongState) -> String {
    match ctx.config.fonts.mode.to_ascii_lowercase().as_str() {
        "metadata" => metadata_font_theme(ctx, state),
        _ => ctx.config.fonts.theme.to_ascii_lowercase(),
    }
}

fn selected_fonts<'a>(
    ctx: &'a AppContext,
    state: &SongState,
) -> (&'a str, &'a str, u16, u16, String) {
    let mut theme_name = selected_font_theme(ctx, state);

    if !ctx.config.font_themes.contains_key(&theme_name) {
        log_error(
            ctx,
            &format!(
                "Configured font theme '{}' was not found; falling back to '{}'",
                theme_name, ctx.config.fonts.fallback_theme
            ),
        );

        theme_name = ctx.config.fonts.fallback_theme.to_ascii_lowercase();
    }

    if let Some(theme) = ctx.config.font_themes.get(&theme_name) {
        (
            &theme.title,
            &theme.body,
            theme.title_size,
            theme.body_size,
            theme_name,
        )
    } else {
        (
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            34,
            24,
            "system-fallback".to_string(),
        )
    }
}

// ==============================================================================
// Display Preset Selection
// ==============================================================================

fn selected_display_preset<'a>(ctx: &'a AppContext) -> Option<&'a DisplayPreset> {
    let key = ctx.config.display.orientation.to_ascii_lowercase();
    ctx.config.display_presets.get(&key)
}

// ==============================================================================
// Cached Text
// ==============================================================================

struct CachedText<'a> {
    texture: Texture<'a>,
    rect: Rect,
    viewport_width: u32,
}

impl<'a> CachedText<'a> {
    fn new(
        texture_creator: &'a TextureCreator<WindowContext>,
        font: &sdl2::ttf::Font,
        text: &str,
        color: Color,
        x: i32,
        y: i32,
        viewport_width: u32,
    ) -> Result<Self, String> {
        let safe_text = if text.trim().is_empty() { " " } else { text };

        let surface = font
            .render(safe_text)
            .blended(color)
            .map_err(|e| e.to_string())?;

        let texture = texture_creator
            .create_texture_from_surface(&surface)
            .map_err(|e| e.to_string())?;

        let rect = Rect::new(x, y, surface.width(), surface.height());

        Ok(Self {
            texture,
            rect,
            viewport_width,
        })
    }

    fn scroll_offset(&self, elapsed: Duration) -> i32 {
        const START_PAUSE_SECS: f32 = 5.0;
        const SCROLL_PIXELS_PER_SEC: f32 = 55.0;
        const LOOP_GAP_PIXELS: u32 = 28;

        if self.rect.width() <= self.viewport_width {
            return 0;
        }

        let loop_distance = self.rect.width() + LOOP_GAP_PIXELS;
        let scroll_secs = (loop_distance as f32) / SCROLL_PIXELS_PER_SEC;
        let cycle_secs = START_PAUSE_SECS + scroll_secs;
        let cycle_pos = elapsed.as_secs_f32() % cycle_secs;

        if cycle_pos < START_PAUSE_SECS {
            0
        } else {
            ((cycle_pos - START_PAUSE_SECS) * SCROLL_PIXELS_PER_SEC)
                .round()
                .clamp(0.0, loop_distance as f32) as i32
        }
    }

    fn draw(
        &self,
        canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
        offset_x: i32,
        offset_y: i32,
        scale: f32,
        elapsed: Duration,
    ) -> Result<(), String> {
        const LOOP_GAP_PIXELS: i32 = 28;

        let scroll_x = self.scroll_offset(elapsed);
        let base_x = offset_x + (((self.rect.x() - scroll_x) as f32 * scale) as i32);
        let base_y = offset_y + (((self.rect.y()) as f32 * scale) as i32);
        let texture_w = ((self.rect.width() as f32) * scale).max(1.0) as u32;
        let texture_h = ((self.rect.height() as f32) * scale).max(1.0) as u32;
        let viewport_x = offset_x + (((self.rect.x()) as f32 * scale) as i32);

        let clip = Rect::new(
            viewport_x,
            base_y,
            ((self.viewport_width as f32) * scale).max(1.0) as u32,
            texture_h,
        );

        canvas.set_clip_rect(Some(clip));
        let result = canvas.copy(
            &self.texture,
            None,
            Rect::new(base_x, base_y, texture_w, texture_h),
        );

        let second_result = if result.is_ok() && self.rect.width() > self.viewport_width {
            let second_x =
                base_x + (((self.rect.width() as i32 + LOOP_GAP_PIXELS) as f32 * scale) as i32);
            canvas.copy(
                &self.texture,
                None,
                Rect::new(second_x, base_y, texture_w, texture_h),
            )
        } else {
            Ok(())
        };

        canvas.set_clip_rect(None);

        result?;
        second_result?;
        Ok(())
    }
}

struct TextField<'a> {
    label: CachedText<'a>,
    value: CachedText<'a>,
}

impl<'a> TextField<'a> {
    fn new(
        texture_creator: &'a TextureCreator<WindowContext>,
        font: &sdl2::ttf::Font,
        label: &str,
        value: &str,
        label_color: Color,
        value_color: Color,
        x: i32,
        y: i32,
        viewport_width: u32,
    ) -> Result<Self, String> {
        let label = CachedText::new(
            texture_creator,
            font,
            label,
            label_color,
            x,
            y,
            viewport_width,
        )?;
        let value_x = x + label.rect.width() as i32;
        let value_viewport_width = viewport_width.saturating_sub(label.rect.width()).max(1);
        let value = CachedText::new(
            texture_creator,
            font,
            value,
            value_color,
            value_x,
            y,
            value_viewport_width,
        )?;

        Ok(Self { label, value })
    }

    fn draw(
        &self,
        canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
        offset_x: i32,
        offset_y: i32,
        scale: f32,
        elapsed: Duration,
    ) -> Result<(), String> {
        self.label
            .draw(canvas, offset_x, offset_y, scale, Duration::ZERO)?;
        self.value
            .draw(canvas, offset_x, offset_y, scale, elapsed)?;
        Ok(())
    }
}

struct TextCache<'a> {
    title: TextField<'a>,
    artist: TextField<'a>,
    album: TextField<'a>,
    year: TextField<'a>,
    genre: TextField<'a>,
    composer: TextField<'a>,
}

fn build_text_cache<'a>(
    texture_creator: &'a TextureCreator<WindowContext>,
    title_font: &sdl2::ttf::Font,
    body_font: &sdl2::ttf::Font,
    state: &SongState,
    preset: &DisplayPreset,
    top_h: u32,
) -> Result<TextCache<'a>, String> {
    let panel_x = preset.panel_x;
    let mut panel_y = (top_h as i32) + preset.panel_y;
    let viewport_width = preset
        .width
        .saturating_sub((panel_x as u32).saturating_mul(2));

    let title_line = if state.title.trim().is_empty() {
        "Waiting for music...".to_string()
    } else {
        state.title.clone()
    };

    let artist_line = if state.artist.trim().is_empty() {
        "No track identified yet".to_string()
    } else {
        state.artist.clone()
    };

    let album_line = album_line(state);
    let year_line = release_year_line(state);

    let title = TextField::new(
        texture_creator,
        title_font,
        "Title: ",
        &title_line,
        Color::RGB(255, 255, 255),
        Color::RGB(255, 255, 255),
        panel_x,
        panel_y,
        viewport_width,
    )?;
    panel_y += preset.title_line_spacing;

    let artist = TextField::new(
        texture_creator,
        body_font,
        "Artist: ",
        &artist_line,
        Color::RGB(170, 170, 170),
        Color::RGB(210, 210, 210),
        panel_x,
        panel_y,
        viewport_width,
    )?;
    panel_y += preset.body_line_spacing;

    let year_x = panel_x + ((viewport_width as f32) * 0.76) as i32;
    let album_viewport_width = (year_x - panel_x).max(1) as u32;
    let year_viewport_width = (panel_x + viewport_width as i32)
        .saturating_sub(year_x)
        .max(1) as u32;

    let album = TextField::new(
        texture_creator,
        body_font,
        "Album: ",
        &album_line,
        Color::RGB(150, 150, 150),
        Color::RGB(180, 180, 180),
        panel_x,
        panel_y,
        album_viewport_width,
    )?;

    let year = TextField::new(
        texture_creator,
        body_font,
        "Year: ",
        &year_line,
        Color::RGB(150, 150, 150),
        Color::RGB(180, 180, 180),
        year_x,
        panel_y,
        year_viewport_width,
    )?;
    panel_y += preset.detail_line_spacing;

    let composer_x = panel_x + ((viewport_width as f32) * 0.38) as i32;
    let genre_viewport_width = (composer_x - panel_x).max(1) as u32;
    let composer_viewport_width = (panel_x + viewport_width as i32)
        .saturating_sub(composer_x)
        .max(1) as u32;

    let genre = TextField::new(
        texture_creator,
        body_font,
        "Genre: ",
        &state.genre,
        Color::RGB(120, 120, 120),
        Color::RGB(140, 140, 140),
        panel_x,
        panel_y,
        genre_viewport_width,
    )?;

    let composer = TextField::new(
        texture_creator,
        body_font,
        "Composer: ",
        &state.composer,
        Color::RGB(120, 120, 120),
        Color::RGB(140, 140, 140),
        composer_x,
        panel_y,
        composer_viewport_width,
    )?;

    Ok(TextCache {
        title,
        artist,
        album,
        year,
        genre,
        composer,
    })
}

// ==============================================================================
// Visualizer Drawing
// ==============================================================================

fn draw_polyline(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    points: &[(f32, f32)],
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    color: Color,
) -> Result<(), String> {
    if points.len() < 2 {
        return Ok(());
    }

    canvas.set_draw_color(color);

    for pair in points.windows(2) {
        let (x1n, y1n) = pair[0];
        let (x2n, y2n) = pair[1];

        let x1 = x + ((x1n.clamp(0.0, 1.0) * (width as f32)) as i32);
        let y1 = y + ((y1n.clamp(0.0, 1.0) * (height as f32)) as i32);
        let x2 = x + ((x2n.clamp(0.0, 1.0) * (width as f32)) as i32);
        let y2 = y + ((y2n.clamp(0.0, 1.0) * (height as f32)) as i32);

        canvas.draw_line(Point::new(x1, y1), Point::new(x2, y2))?;
    }

    Ok(())
}

fn draw_polyline_thick(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    points: &[(f32, f32)],
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    color: Color,
    thickness: i32,
) -> Result<(), String> {
    if thickness <= 1 {
        return draw_polyline(canvas, points, x, y, width, height, color);
    }

    canvas.set_draw_color(color);

    let radius = thickness / 2;
    for pair in points.windows(2) {
        let (x1n, y1n) = pair[0];
        let (x2n, y2n) = pair[1];

        let x1 = x + ((x1n.clamp(0.0, 1.0) * (width as f32)) as i32);
        let y1 = y + ((y1n.clamp(0.0, 1.0) * (height as f32)) as i32);
        let x2 = x + ((x2n.clamp(0.0, 1.0) * (width as f32)) as i32);
        let y2 = y + ((y2n.clamp(0.0, 1.0) * (height as f32)) as i32);

        for offset_x in -radius..=radius {
            for offset_y in -radius..=radius {
                if offset_x.abs() + offset_y.abs() <= radius {
                    canvas.draw_line(
                        Point::new(x1 + offset_x, y1 + offset_y),
                        Point::new(x2 + offset_x, y2 + offset_y),
                    )?;
                }
            }
        }
    }

    Ok(())
}

fn draw_scope_graticule(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<(), String> {
    let w = width as i32;
    let h = height as i32;

    canvas.set_draw_color(Color::RGB(18, 36, 30));
    for division in 1..10 {
        let line_x = x + (w * division) / 10;
        canvas.draw_line(Point::new(line_x, y), Point::new(line_x, y + h))?;
    }

    for division in 1..8 {
        let line_y = y + (h * division) / 8;
        canvas.draw_line(Point::new(x, line_y), Point::new(x + w, line_y))?;
    }

    canvas.set_draw_color(Color::RGB(38, 78, 62));
    for y_ratio in [0.25_f32, 0.75_f32] {
        let line_y = y + ((height as f32) * y_ratio) as i32;
        canvas.draw_line(Point::new(x, line_y), Point::new(x + w, line_y))?;
    }

    canvas.set_draw_color(Color::RGB(42, 70, 60));
    canvas.draw_rect(Rect::new(x, y, width, height))?;

    Ok(())
}

fn angle_point(cx: i32, cy: i32, radius: f32, angle_deg: f32) -> Point {
    let angle = angle_deg.to_radians();
    Point::new(
        cx + (radius * angle.cos()) as i32,
        cy + (radius * angle.sin()) as i32,
    )
}

fn draw_arc(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    cx: i32,
    cy: i32,
    radius: f32,
    start_deg: f32,
    end_deg: f32,
    segments: usize,
) -> Result<(), String> {
    let segments = segments.max(2);
    let mut prev = angle_point(cx, cy, radius, start_deg);

    for i in 1..=segments {
        let t = (i as f32) / (segments as f32);
        let angle = start_deg + (end_deg - start_deg) * t;
        let next = angle_point(cx, cy, radius, angle);
        canvas.draw_line(prev, next)?;
        prev = next;
    }

    Ok(())
}

fn draw_circle(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    cx: i32,
    cy: i32,
    radius: i32,
) -> Result<(), String> {
    if radius <= 0 {
        return Ok(());
    }

    let mut x = radius;
    let mut y = 0;
    let mut err = 0;

    while x >= y {
        for (px, py) in [
            (cx + x, cy + y),
            (cx + y, cy + x),
            (cx - y, cy + x),
            (cx - x, cy + y),
            (cx - x, cy - y),
            (cx - y, cy - x),
            (cx + y, cy - x),
            (cx + x, cy - y),
        ] {
            canvas.draw_point(Point::new(px, py))?;
        }

        y += 1;
        if err <= 0 {
            err += 2 * y + 1;
        } else {
            x -= 1;
            err -= 2 * x + 1;
        }
    }

    Ok(())
}

fn fill_circle(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    cx: i32,
    cy: i32,
    radius: i32,
) -> Result<(), String> {
    if radius <= 0 {
        return Ok(());
    }

    for dy in -radius..=radius {
        let dx = ((radius * radius - dy * dy) as f32).sqrt() as i32;
        canvas.draw_line(Point::new(cx - dx, cy + dy), Point::new(cx + dx, cy + dy))?;
    }

    Ok(())
}

fn draw_meter_screw(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    cx: i32,
    cy: i32,
    radius: i32,
) -> Result<(), String> {
    canvas.set_draw_color(Color::RGB(104, 92, 72));
    fill_circle(canvas, cx, cy, radius)?;
    canvas.set_draw_color(Color::RGB(34, 30, 28));
    draw_circle(canvas, cx, cy, radius)?;
    canvas.draw_line(
        Point::new(cx - radius + 2, cy),
        Point::new(cx + radius - 2, cy),
    )?;
    canvas.set_draw_color(Color::RGB(190, 166, 116));
    canvas.draw_line(
        Point::new(cx - radius / 2, cy - radius / 2),
        Point::new(cx + radius / 2, cy + radius / 2),
    )?;
    Ok(())
}

fn draw_segment_digit(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    digit: char,
    x: i32,
    y: i32,
    scale: i32,
) -> Result<(), String> {
    let w = scale * 3;
    let h = scale * 5;
    let mid = y + h / 2;
    let right = x + w;
    let bottom = y + h;

    let segments = match digit {
        '0' => [true, true, true, false, true, true, true],
        '1' => [false, false, true, false, false, true, false],
        '2' => [true, false, true, true, true, false, true],
        '3' => [true, false, true, true, false, true, true],
        '4' => [false, true, true, true, false, true, false],
        '5' => [true, true, false, true, false, true, true],
        '6' => [true, true, false, true, true, true, true],
        '7' => [true, false, true, false, false, true, false],
        '8' => [true, true, true, true, true, true, true],
        '9' => [true, true, true, true, false, true, true],
        _ => [false; 7],
    };

    if segments[0] {
        canvas.draw_line(Point::new(x, y), Point::new(right, y))?;
    }
    if segments[1] {
        canvas.draw_line(Point::new(x, y), Point::new(x, mid))?;
    }
    if segments[2] {
        canvas.draw_line(Point::new(right, y), Point::new(right, mid))?;
    }
    if segments[3] {
        canvas.draw_line(Point::new(x, mid), Point::new(right, mid))?;
    }
    if segments[4] {
        canvas.draw_line(Point::new(x, mid), Point::new(x, bottom))?;
    }
    if segments[5] {
        canvas.draw_line(Point::new(right, mid), Point::new(right, bottom))?;
    }
    if segments[6] {
        canvas.draw_line(Point::new(x, bottom), Point::new(right, bottom))?;
    }

    Ok(())
}

fn meter_label_width(label: &str, scale: i32) -> i32 {
    label
        .chars()
        .map(|ch| match ch {
            '0'..='9' => scale * 4,
            '-' | '+' => scale * 4,
            '%' => scale * 5,
            _ => scale * 2,
        })
        .sum::<i32>()
        .saturating_sub(scale)
}

fn draw_meter_label(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    label: &str,
    center_x: i32,
    baseline_y: i32,
    scale: i32,
    color: Color,
) -> Result<(), String> {
    canvas.set_draw_color(color);
    let mut x = center_x - meter_label_width(label, scale) / 2;
    let y = baseline_y - scale * 5;

    for ch in label.chars() {
        match ch {
            '0'..='9' => {
                draw_segment_digit(canvas, ch, x, y, scale)?;
                x += scale * 4;
            }
            '-' => {
                canvas.draw_line(
                    Point::new(x, y + scale * 2),
                    Point::new(x + scale * 3, y + scale * 2),
                )?;
                x += scale * 4;
            }
            '+' => {
                canvas.draw_line(
                    Point::new(x, y + scale * 2),
                    Point::new(x + scale * 3, y + scale * 2),
                )?;
                canvas.draw_line(
                    Point::new(x + scale + scale / 2, y + scale),
                    Point::new(x + scale + scale / 2, y + scale * 3),
                )?;
                x += scale * 4;
            }
            '%' => {
                fill_circle(canvas, x + scale, y + scale, scale / 2)?;
                fill_circle(canvas, x + scale * 4, y + scale * 4, scale / 2)?;
                canvas.draw_line(Point::new(x, y + scale * 5), Point::new(x + scale * 5, y))?;
                x += scale * 5;
            }
            _ => {
                x += scale * 2;
            }
        }
    }

    Ok(())
}

fn draw_vu_mark(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    cx: i32,
    y: i32,
    scale: i32,
) -> Result<(), String> {
    canvas.set_draw_color(Color::RGB(50, 28, 10));
    let v_x = cx - scale * 5;
    let u_x = cx + scale;
    let top = y;
    let bottom = y + scale * 6;

    canvas.draw_line(Point::new(v_x, top), Point::new(v_x + scale * 2, bottom))?;
    canvas.draw_line(
        Point::new(v_x + scale * 4, top),
        Point::new(v_x + scale * 2, bottom),
    )?;
    canvas.draw_line(Point::new(u_x, top), Point::new(u_x, bottom - scale))?;
    canvas.draw_line(
        Point::new(u_x + scale * 4, top),
        Point::new(u_x + scale * 4, bottom - scale),
    )?;
    canvas.draw_line(
        Point::new(u_x, bottom - scale),
        Point::new(u_x + scale * 4, bottom - scale),
    )?;

    Ok(())
}

fn draw_analog_meter(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    rect: Rect,
    value: f32,
    peak: f32,
    needle_color: Color,
) -> Result<(), String> {
    let x = rect.x();
    let y = rect.y();
    let width = rect.width();
    let height = rect.height();
    let w = width as i32;
    let h = height as i32;

    canvas.set_draw_color(Color::RGB(9, 8, 7));
    canvas.fill_rect(rect)?;

    let cx = x + w / 2;
    let cy = y + h / 2;
    let outer_r = ((width.min(height) as f32) * 0.48).max(24.0) as i32;
    let bezel_r = (outer_r as f32 * 0.94) as i32;
    let glass_r = (outer_r as f32 * 0.82) as i32;

    canvas.set_draw_color(Color::RGB(3, 3, 4));
    fill_circle(canvas, cx + 4, cy + 5, outer_r)?;
    canvas.set_draw_color(Color::RGB(12, 12, 13));
    fill_circle(canvas, cx, cy, outer_r)?;
    canvas.set_draw_color(Color::RGB(36, 32, 27));
    draw_circle(canvas, cx, cy, outer_r)?;
    canvas.set_draw_color(Color::RGB(88, 72, 42));
    draw_circle(canvas, cx, cy, bezel_r)?;

    for band in (0..=10).rev() {
        let t = (band as f32) / 10.0;
        let warm = 1.0 - t;
        canvas.set_draw_color(Color::RGB(
            (196.0 + 55.0 * warm) as u8,
            (112.0 + 86.0 * warm) as u8,
            (22.0 + 38.0 * warm) as u8,
        ));
        fill_circle(canvas, cx, cy, ((glass_r as f32) * t).max(1.0) as i32)?;
    }
    canvas.set_draw_color(Color::RGB(255, 202, 78));
    fill_circle(
        canvas,
        cx - glass_r / 8,
        cy - glass_r / 5,
        (glass_r as f32 * 0.58) as i32,
    )?;
    canvas.set_draw_color(Color::RGB(255, 174, 39));
    fill_circle(
        canvas,
        cx + glass_r / 8,
        cy + glass_r / 18,
        (glass_r as f32 * 0.78) as i32,
    )?;
    canvas.set_draw_color(Color::RGB(255, 216, 116));
    draw_arc(canvas, cx, cy, glass_r as f32 * 0.86, -166.0, -72.0, 28)?;
    canvas.set_draw_color(Color::RGB(176, 92, 16));
    draw_circle(canvas, cx, cy, glass_r)?;

    let mask_h = (glass_r as f32 * 0.34) as u32;
    let mask_w = (glass_r as f32 * 1.65) as u32;
    canvas.set_draw_color(Color::RGB(8, 8, 9));
    canvas.fill_rect(Rect::new(
        cx - (mask_w as i32) / 2,
        cy + glass_r / 3,
        mask_w,
        mask_h,
    ))?;

    let pivot_y = cy + (glass_r as f32 * 0.43) as i32;
    let radius = (glass_r as f32 * 0.75).max(18.0);
    let start_deg = -156.0;
    let end_deg = -24.0;

    canvas.set_draw_color(Color::RGB(56, 36, 18));
    draw_arc(canvas, cx, pivot_y, radius, start_deg, end_deg, 46)?;
    draw_arc(canvas, cx, pivot_y, radius * 0.78, start_deg, end_deg, 46)?;

    for tick in 0..=10 {
        let t = (tick as f32) / 10.0;
        let angle = start_deg + (end_deg - start_deg) * t;
        let inner = if tick % 5 == 0 {
            radius * 0.75
        } else {
            radius * 0.82
        };
        let outer = radius;
        canvas.set_draw_color(if tick >= 8 {
            Color::RGB(172, 34, 22)
        } else {
            Color::RGB(42, 24, 12)
        });
        canvas.draw_line(
            angle_point(cx, pivot_y, inner, angle),
            angle_point(cx, pivot_y, outer, angle),
        )?;

        if tick % 2 == 0 {
            let dot = angle_point(cx, pivot_y, radius * 0.62, angle);
            canvas.set_draw_color(Color::RGB(55, 36, 18));
            fill_circle(canvas, dot.x(), dot.y(), 2)?;
        }
    }

    canvas.set_draw_color(Color::RGB(178, 28, 20));
    draw_arc(canvas, cx, pivot_y, radius * 0.94, -54.0, end_deg, 14)?;
    draw_arc(canvas, cx, pivot_y, radius * 0.95, -54.0, end_deg, 14)?;

    let major_labels = [
        ("20", -146.0, false),
        ("10", -132.0, false),
        ("7", -116.0, false),
        ("5", -101.0, false),
        ("3", -86.0, false),
        ("2", -72.0, false),
        ("1", -60.0, false),
        ("0", -50.0, true),
        ("3", -37.0, true),
        ("5", -26.0, true),
    ];
    let label_scale = (glass_r / 38).clamp(2, 5);
    for (label, angle, red) in major_labels {
        let p = angle_point(cx, pivot_y, radius * 1.14, angle);
        draw_meter_label(
            canvas,
            label,
            p.x(),
            p.y(),
            label_scale,
            if red {
                Color::RGB(166, 28, 18)
            } else {
                Color::RGB(50, 28, 10)
            },
        )?;
    }

    let percent_labels = [
        ("0", -141.0),
        ("20", -118.0),
        ("50", -96.0),
        ("60", -82.0),
        ("80", -68.0),
        ("100%", -49.0),
    ];
    let small_scale = (glass_r / 55).clamp(1, 3);
    for (label, angle) in percent_labels {
        let p = angle_point(cx, pivot_y, radius * 0.58, angle);
        draw_meter_label(
            canvas,
            label,
            p.x(),
            p.y(),
            small_scale,
            Color::RGB(70, 36, 10),
        )?;
    }

    draw_vu_mark(canvas, cx, cy + glass_r / 8, (glass_r / 26).clamp(3, 7))?;

    canvas.set_draw_color(Color::RGB(44, 25, 12));
    draw_arc(canvas, cx, pivot_y, radius * 0.46, -120.0, -60.0, 16)?;

    let needle_value = value.clamp(0.0, 1.0).powf(0.68);
    let angle = start_deg + (end_deg - start_deg) * needle_value;
    let tip = angle_point(cx, pivot_y, radius * 0.92, angle);

    canvas.set_draw_color(Color::RGB(112, 58, 22));
    canvas.draw_line(
        Point::new(cx - 1, pivot_y + 1),
        Point::new(tip.x() - 1, tip.y() + 1),
    )?;
    canvas.draw_line(
        Point::new(cx + 1, pivot_y + 1),
        Point::new(tip.x() + 1, tip.y() + 1),
    )?;
    canvas.set_draw_color(Color::RGB(82, 36, 12));
    canvas.draw_line(Point::new(cx, pivot_y), tip)?;

    canvas.set_draw_color(Color::RGB(124, 92, 50));
    canvas.fill_rect(Rect::new(
        cx - glass_r / 4,
        pivot_y - 8,
        (glass_r / 2) as u32,
        16,
    ))?;
    canvas.set_draw_color(Color::RGB(62, 48, 38));
    canvas.draw_rect(Rect::new(
        cx - glass_r / 4,
        pivot_y - 8,
        (glass_r / 2) as u32,
        16,
    ))?;
    canvas.set_draw_color(Color::RGB(34, 28, 24));
    fill_circle(canvas, cx, pivot_y, 9)?;
    canvas.set_draw_color(Color::RGB(154, 126, 80));
    draw_circle(canvas, cx, pivot_y, 9)?;
    canvas.draw_line(Point::new(cx - 5, pivot_y), Point::new(cx + 5, pivot_y))?;

    draw_meter_screw(canvas, cx - (glass_r as f32 * 0.72) as i32, pivot_y - 8, 7)?;
    draw_meter_screw(canvas, cx + (glass_r as f32 * 0.72) as i32, pivot_y - 8, 7)?;

    canvas.set_draw_color(if peak > 0.82 {
        Color::RGB(255, 74, 36)
    } else {
        Color::RGB(82, 28, 18)
    });
    fill_circle(
        canvas,
        cx + (glass_r as f32 * 0.58) as i32,
        cy - glass_r / 2,
        5,
    )?;

    canvas.set_draw_color(dim_color(needle_color, 0.32));
    draw_circle(canvas, cx, cy, glass_r - 2)?;

    Ok(())
}

fn draw_analog_vu(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    ctx: &AppContext,
    colors: &VisualizerDrawColors,
    level: f32,
    peak: f32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<(), String> {
    canvas.set_draw_color(visualizer_background_color(ctx));
    canvas.fill_rect(Rect::new(x, y, width, height))?;

    let padding = 6_u32.min(width / 8).min(height / 8);
    let gap = 18_u32.min(width / 8);
    let meter_w = width.saturating_sub(padding * 2 + gap) / 2;
    let meter_h = height.saturating_sub(padding * 2).max(1);
    let meter_y = y + padding as i32;
    let left_x = x + padding as i32;
    let right_x = left_x + meter_w as i32 + gap as i32;

    let value = (level * 1.35).clamp(0.0, 1.0);
    let peak = (peak * 1.35).clamp(0.0, 1.0);

    draw_analog_meter(
        canvas,
        Rect::new(left_x, meter_y, meter_w, meter_h),
        value,
        peak,
        colors.upper,
    )?;
    draw_analog_meter(
        canvas,
        Rect::new(right_x, meter_y, meter_w, meter_h),
        value,
        peak,
        colors.lower,
    )?;

    Ok(())
}

fn draw_oscilloscope(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    ctx: &AppContext,
    colors: &VisualizerDrawColors,
    left_points: &[(f32, f32)],
    right_points: &[(f32, f32)],
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<(), String> {
    canvas.set_draw_color(visualizer_background_color(ctx));
    canvas.fill_rect(Rect::new(x, y, width, height))?;

    let inset = 10_i32;
    let scope_x = x + inset;
    let scope_y = y + inset;
    let scope_w = width.saturating_sub((inset as u32) * 2).max(1);
    let scope_h = height.saturating_sub((inset as u32) * 2).max(1);

    draw_scope_graticule(canvas, scope_x, scope_y, scope_w, scope_h)?;

    let clip = Rect::new(scope_x, scope_y, scope_w, scope_h);
    canvas.set_clip_rect(Some(clip));

    let trace_result = (|| -> Result<(), String> {
        draw_polyline_thick(
            canvas,
            left_points,
            scope_x,
            scope_y,
            scope_w,
            scope_h,
            dim_color(colors.upper, 0.38),
            5,
        )?;
        draw_polyline_thick(
            canvas,
            right_points,
            scope_x,
            scope_y,
            scope_w,
            scope_h,
            dim_color(colors.lower, 0.38),
            5,
        )?;
        draw_polyline_thick(
            canvas,
            left_points,
            scope_x,
            scope_y,
            scope_w,
            scope_h,
            colors.upper,
            2,
        )?;
        draw_polyline_thick(
            canvas,
            right_points,
            scope_x,
            scope_y,
            scope_w,
            scope_h,
            colors.lower,
            2,
        )?;
        Ok(())
    })();

    canvas.set_clip_rect(None);
    trace_result?;

    Ok(())
}

fn draw_spectrum(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    ctx: &AppContext,
    colors: &VisualizerDrawColors,
    upper_bins: &[f32],
    lower_bins: &[f32],
    upper_peaks: &[f32],
    lower_peaks: &[f32],
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    bar_gap: u32,
) -> Result<(), String> {
    canvas.set_draw_color(visualizer_background_color(ctx));
    canvas.fill_rect(Rect::new(x, y, width, height))?;

    if upper_bins.is_empty() || lower_bins.is_empty() {
        return Ok(());
    }

    let half_h = height / 2;
    if half_h == 0 {
        return Ok(());
    }

    let count = upper_bins.len().min(lower_bins.len()) as u32;
    if count == 0 {
        return Ok(());
    }

    let total_gap = bar_gap.saturating_mul(count.saturating_sub(1));
    let bar_w = (width.saturating_sub(total_gap) / count).max(1);
    let top_only = ctx
        .config
        .visualizer
        .spectrum
        .render_style
        .eq_ignore_ascii_case("top_only");
    let upper_h = if top_only { height } else { half_h };
    let baseline_y = if top_only {
        y + (height as i32)
    } else {
        y + (half_h as i32)
    };

    if !top_only {
        canvas.set_draw_color(Color::RGB(40, 40, 40));
        canvas.draw_line(
            Point::new(x, baseline_y),
            Point::new(x + (width as i32), baseline_y),
        )?;
    }

    for (i, value) in upper_bins.iter().enumerate() {
        let i = i as u32;
        let bar_x = x + ((i * (bar_w + bar_gap)) as i32);
        let bar_h = ((*value).clamp(0.0, 1.0) * (upper_h as f32)) as u32;

        canvas.set_draw_color(palette_color_at(
            &colors.palette,
            i as usize,
            count as usize,
        ));

        if let Some(rect) = spectrum_bar_rect(
            bar_x,
            baseline_y,
            bar_w,
            bar_h,
            true,
            &ctx.config.visualizer.spectrum.render_style,
            ctx.config.visualizer.spectrum.top_only_height_ratio,
        ) {
            canvas.fill_rect(rect)?;
        }
    }

    if !top_only {
        for (i, value) in lower_bins.iter().enumerate() {
            let i = i as u32;
            let bar_x = x + ((i * (bar_w + bar_gap)) as i32);
            let bar_h = ((*value).clamp(0.0, 1.0) * (half_h as f32)) as u32;

            canvas.set_draw_color(palette_color_at(
                &colors.palette,
                count.saturating_sub(1).saturating_sub(i) as usize,
                count as usize,
            ));

            if let Some(rect) = spectrum_bar_rect(
                bar_x,
                y + (half_h as i32),
                bar_w,
                bar_h,
                false,
                &ctx.config.visualizer.spectrum.render_style,
                ctx.config.visualizer.spectrum.top_only_height_ratio,
            ) {
                canvas.fill_rect(rect)?;
            }
        }
    }

    if ctx.config.visualizer.peaks.enabled {
        let peak_marker_h = ctx.config.visualizer.peaks.drop_pixels.max(1).min(half_h);
        let peak_color = parse_hex_color(
            &ctx.config.visualizer.peaks.color,
            Color::RGB(255, 255, 255),
        );

        for (i, value) in upper_peaks.iter().take(count as usize).enumerate() {
            let i = i as u32;
            let bar_x = x + ((i * (bar_w + bar_gap)) as i32);
            let peak_h = ((*value).clamp(0.0, 1.0) * (upper_h as f32)) as u32;

            if peak_h == 0 {
                continue;
            }

            canvas.set_draw_color(if ctx.config.visualizer.peaks.use_bar_color {
                palette_color_at(&colors.palette, i as usize, count as usize)
            } else {
                peak_color
            });

            let marker_y = baseline_y - (peak_h as i32);
            canvas.fill_rect(Rect::new(bar_x, marker_y, bar_w, peak_marker_h))?;
        }

        if !top_only {
            for (i, value) in lower_peaks.iter().take(count as usize).enumerate() {
                let i = i as u32;
                let bar_x = x + ((i * (bar_w + bar_gap)) as i32);
                let peak_h = ((*value).clamp(0.0, 1.0) * (half_h as f32)) as u32;

                if peak_h == 0 {
                    continue;
                }

                canvas.set_draw_color(if ctx.config.visualizer.peaks.use_bar_color {
                    palette_color_at(
                        &colors.palette,
                        count.saturating_sub(1).saturating_sub(i) as usize,
                        count as usize,
                    )
                } else {
                    peak_color
                });

                let marker_y = y + (half_h as i32) + (peak_h as i32) - (peak_marker_h as i32);
                canvas.fill_rect(Rect::new(bar_x, marker_y, bar_w, peak_marker_h))?;
            }
        }
    }

    Ok(())
}

fn spectrum_bar_rect(
    bar_x: i32,
    baseline_y: i32,
    bar_w: u32,
    bar_h: u32,
    extends_up: bool,
    render_style: &str,
    top_only_height_ratio: f32,
) -> Option<Rect> {
    if bar_h == 0 {
        return None;
    }

    let top_only = render_style.eq_ignore_ascii_case("top_only");
    let visible_h = if top_only {
        ((bar_h as f32) * top_only_height_ratio.clamp(0.0, 1.0))
            .ceil()
            .max(1.0) as u32
    } else {
        bar_h
    };

    let rect_y = if extends_up {
        baseline_y - (bar_h as i32)
    } else if top_only {
        baseline_y + (bar_h as i32) - (visible_h as i32)
    } else {
        baseline_y
    };

    Some(Rect::new(bar_x, rect_y, bar_w, visible_h))
}

fn draw_visualizer(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    ctx: &AppContext,
    colors: &VisualizerDrawColors,
    mode: VisualizerMode,
    left_points: &[(f32, f32)],
    right_points: &[(f32, f32)],
    upper_bins: &[f32],
    lower_bins: &[f32],
    upper_peaks: &[f32],
    lower_peaks: &[f32],
    meter_level: f32,
    meter_peak: f32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    bar_gap: u32,
) -> Result<(), String> {
    match mode {
        VisualizerMode::None => Ok(()),
        VisualizerMode::Oscilloscope => draw_oscilloscope(
            canvas,
            ctx,
            colors,
            left_points,
            right_points,
            x,
            y,
            width,
            height,
        ),
        VisualizerMode::AnalogVu => draw_analog_vu(
            canvas,
            ctx,
            colors,
            meter_level,
            meter_peak,
            x,
            y,
            width,
            height,
        ),
        VisualizerMode::Spectrum => draw_spectrum(
            canvas,
            ctx,
            colors,
            upper_bins,
            lower_bins,
            upper_peaks,
            lower_peaks,
            x,
            y,
            width,
            height,
            bar_gap,
        ),
    }
}

// ==============================================================================
// Static Scene Drawing
// ==============================================================================

fn compute_artwork_rect(query: TextureQuery, scene_w: u32, top_h: u32) -> Rect {
    let art_w = query.width as f32;
    let art_h = query.height as f32;

    let padding = 24.0;
    let max_w = (scene_w as f32) - padding * 2.0;
    let max_h = (top_h as f32) - padding * 2.0;

    let scale = f32::min(max_w / art_w, max_h / art_h);
    let draw_w = (art_w * scale) as u32;
    let draw_h = (art_h * scale) as u32;

    let x = ((scene_w - draw_w) / 2) as i32;
    let y = ((top_h - draw_h) / 2) as i32;

    Rect::new(x, y, draw_w, draw_h)
}

struct StaticSceneCache<'a> {
    version: u64,
    text: TextCache<'a>,
    artwork_rect: Option<Rect>,
}

impl<'a> StaticSceneCache<'a> {
    fn draw_static_scene(
        &self,
        canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
        ctx: &AppContext,
        artwork_texture: Option<&Texture<'a>>,
        scene_w: u32,
        top_h: u32,
        bottom_h: u32,
    ) -> Result<(), String> {
        canvas.set_draw_color(canvas_background_color(ctx));
        canvas.clear();

        canvas.set_draw_color(artwork_background_color(ctx));
        canvas.fill_rect(Rect::new(0, 0, scene_w, top_h))?;

        canvas.set_draw_color(metadata_background_color(ctx));
        canvas.fill_rect(Rect::new(0, top_h as i32, scene_w, bottom_h))?;

        if let (Some(texture), Some(target)) = (artwork_texture, self.artwork_rect) {
            canvas.copy(texture, None, target)?;
        }

        Ok(())
    }

    fn draw_text(
        &self,
        canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
        offset_x: i32,
        offset_y: i32,
        scale: f32,
        elapsed: Duration,
    ) -> Result<(), String> {
        self.text
            .title
            .draw(canvas, offset_x, offset_y, scale, elapsed)?;
        self.text
            .artist
            .draw(canvas, offset_x, offset_y, scale, elapsed)?;
        self.text
            .album
            .draw(canvas, offset_x, offset_y, scale, elapsed)?;
        self.text
            .year
            .draw(canvas, offset_x, offset_y, scale, elapsed)?;
        self.text
            .genre
            .draw(canvas, offset_x, offset_y, scale, elapsed)?;
        self.text
            .composer
            .draw(canvas, offset_x, offset_y, scale, elapsed)?;

        Ok(())
    }
}

// ==============================================================================
// Runtime Helpers
// ==============================================================================

fn visualizer_mode_from_config(mode: &str) -> VisualizerMode {
    match mode.to_ascii_lowercase().as_str() {
        "oscilloscope" => VisualizerMode::Oscilloscope,
        "spectrum" => VisualizerMode::Spectrum,
        "analog_vu" => VisualizerMode::AnalogVu,
        _ => VisualizerMode::None,
    }
}

fn update_smoothed_bins(smoothed: &mut [f32], raw: &[f32], rise: f32, fall: f32) {
    for (i, value) in raw.iter().enumerate() {
        if i < smoothed.len() {
            let current = smoothed[i];
            smoothed[i] = if *value > current {
                current * rise + *value * (1.0 - rise)
            } else {
                current * fall + *value * (1.0 - fall)
            };
        }
    }
}

fn update_spectrum_peaks(
    peaks: &mut Vec<f32>,
    current: &[f32],
    drop_amount: f32,
    should_drop: bool,
) -> bool {
    if peaks.len() != current.len() {
        peaks.resize(current.len(), 0.0);
    }

    let mut rose = false;

    if should_drop {
        for peak in peaks.iter_mut() {
            *peak = (*peak - drop_amount).max(0.0);
        }
    }

    for (peak, value) in peaks.iter_mut().zip(current.iter()) {
        let value = value.clamp(0.0, 1.0);
        if value > *peak {
            *peak = value;
            rose = true;
        }
    }

    rose
}

fn log_visualizer_debug(
    ctx: &AppContext,
    audio_len: usize,
    sample_len: usize,
    live_level: f32,
    smoothed_upper_bins: &[f32],
) {
    let smooth_max_bin = smoothed_upper_bins.iter().copied().fold(0.0f32, f32::max);

    let smooth_avg_bin = if smoothed_upper_bins.is_empty() {
        0.0
    } else {
        smoothed_upper_bins.iter().sum::<f32>() / (smoothed_upper_bins.len() as f32)
    };

    log_debug(
        ctx,
        &format!(
            "Visualizer debug: audio_len={} sample_len={} level={:.3} bins={} smooth_max={:.3} smooth_avg={:.3} upper0={:.3} upper8={:.3} upper16={:.3}",
            audio_len,
            sample_len,
            live_level,
            smoothed_upper_bins.len(),
            smooth_max_bin,
            smooth_avg_bin,
            smoothed_upper_bins.get(0).copied().unwrap_or(0.0),
            smoothed_upper_bins.get(8).copied().unwrap_or(0.0),
            smoothed_upper_bins.get(16).copied().unwrap_or(0.0)
        ),
    );
}

// ==============================================================================
// Display Loop
// ==============================================================================

pub fn run_display_loop(
    ctx: Arc<AppContext>,
    running: Arc<AtomicBool>,
    shared_state: Arc<Mutex<SongState>>,
    shared_audio: Arc<Mutex<SharedAudioBuffer>>,
) -> Result<(), String> {
    let sdl = sdl2::init()?;
    let video = sdl.video()?;
    let _image_ctx = sdl2::image::init(InitFlag::JPG | InitFlag::PNG)?;
    let ttf_ctx = sdl2::ttf::init().map_err(|e| e.to_string())?;

    log_info(
        &ctx,
        &format!("SDL video driver: {}", video.current_video_driver()),
    );

    let preset = selected_display_preset(&ctx)
        .ok_or_else(|| format!("Unknown display preset: {}", ctx.config.display.orientation))?;

    let mut window_builder = video.window(
        &ctx.config.display.window_title,
        preset.width,
        preset.height,
    );
    window_builder.position_centered();

    let window = if ctx.config.display.fullscreen {
        window_builder
            .fullscreen_desktop()
            .build()
            .map_err(|e| e.to_string())?
    } else {
        window_builder.build().map_err(|e| e.to_string())?
    };

    let mut canvas = window
        .into_canvas()
        .accelerated()
        .build()
        .map_err(|e| e.to_string())?;

    let info = canvas.info();
    log_info(
        &ctx,
        &format!("SDL renderer: name='{}' flags={:?}", info.name, info.flags),
    );

    let texture_creator = canvas.texture_creator();

    let initial_state = SongState::default();
    let (title_font_path, body_font_path, title_font_size, body_font_size, selected_theme) =
        selected_fonts(&ctx, &initial_state);

    log_info(&ctx, &format!("Selected font theme: {}", selected_theme));

    let mut title_font = ttf_ctx
        .load_font(title_font_path, title_font_size)
        .map_err(|e| format!("Failed to load title font from {}: {e}", title_font_path))?;

    let mut body_font = ttf_ctx
        .load_font(body_font_path, body_font_size)
        .map_err(|e| format!("Failed to load body font from {}: {e}", body_font_path))?;

    let mut loaded_font_theme = selected_theme;

    let scene_w = preset.width;
    let scene_h = preset.height;
    let top_h = ((scene_h as f32) * preset.top_panel_ratio) as u32;
    let bottom_h = scene_h - top_h;

    let mut event_pump = sdl.event_pump()?;
    let mut loaded_version: u64 = u64::MAX;
    let mut artwork_texture: Option<Texture<'_>> = None;
    let mut visualizer_colors = visualizer_colors_for_artwork(&ctx, None);
    let mut last_canvas_size: Option<(u32, u32)> = None;
    let mut display_peak = 0.0f32;
    let mut vu_level = 0.0f32;
    let mut vu_peak = 0.0f32;
    let mut last_vis_debug = Instant::now();
    let mut last_frame_log = Instant::now();
    let mut frame_counter: u32 = 0;
    let mut frame_timer = Instant::now();
    let mut text_scroll_started_at = Instant::now();

    let mut smoothed_upper_bins = vec![0.0f32; ctx.config.visualizer.spectrum_bin_count];
    let mut smoothed_lower_bins = vec![0.0f32; ctx.config.visualizer.spectrum_bin_count];
    let mut upper_peak_bins = vec![0.0f32; ctx.config.visualizer.spectrum_bin_count];
    let mut lower_peak_bins = vec![0.0f32; ctx.config.visualizer.spectrum_bin_count];
    let mut last_spectrum_peak_drop = Instant::now();

    let mut static_scene_cache: Option<StaticSceneCache<'_>> = None;
    let mut static_scene_texture = texture_creator
        .create_texture_target(PixelFormatEnum::RGBA8888, scene_w, scene_h)
        .map_err(|e| e.to_string())?;

    log_info(&ctx, "Display loop started.");

    while running.load(Ordering::SeqCst) {
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. }
                | Event::KeyDown {
                    keycode: Some(Keycode::Escape),
                    ..
                } => running.store(false, Ordering::SeqCst),
                _ => {}
            }
        }

        let mut state = {
            let state_guard = shared_state.lock().unwrap();
            state_guard.clone()
        };

        let (
            audio_len,
            sample_len,
            live_level,
            left_points,
            right_points,
            raw_upper_bins,
            raw_lower_bins,
        ) = {
            let audio = shared_audio.lock().unwrap();
            let vis_samples = audio.recent_ms(ctx.config.visualizer.window_ms);

            let level = compute_rms(&vis_samples).unwrap_or(0.0);

            let left = build_oscilloscope_points(
                &vis_samples,
                ctx.config.visualizer.point_count,
                ctx.config.visualizer.left_y_offset,
                ctx.config.visualizer.y_scale,
                ctx.config.visualizer.gain,
                ctx.config.visualizer.visible_sample_count,
                ctx.config.visualizer.max_gain,
            );

            let right = build_oscilloscope_points(
                &vis_samples,
                ctx.config.visualizer.point_count,
                ctx.config.visualizer.right_y_offset,
                ctx.config.visualizer.y_scale,
                ctx.config.visualizer.gain,
                ctx.config.visualizer.visible_sample_count,
                ctx.config.visualizer.max_gain,
            );

            let bins = compute_spectrum_bins(
                &vis_samples,
                ctx.config.audio.sample_rate,
                ctx.config.visualizer.spectrum_fft_size,
                ctx.config.visualizer.spectrum_bin_count,
                ctx.config.visualizer.spectrum_min_hz,
                ctx.config.visualizer.spectrum_max_hz,
                ctx.config.visualizer.gain,
                ctx.config.visualizer.max_gain,
                ctx.config.visualizer.spectrum_log_epsilon,
                ctx.config.visualizer.spectrum_log_scale,
                ctx.config.visualizer.spectrum_log_offset,
                ctx.config.visualizer.spectrum_noise_floor,
                ctx.config.visualizer.spectrum_contrast,
            );

            (
                audio.len(),
                vis_samples.len(),
                level,
                left,
                right,
                bins.clone(),
                bins,
            )
        };

        // Faster rise, slower fall makes the spectrum feel lively without looking jittery.
        let rise = ctx.config.visualizer.spectrum_attack.clamp(0.0, 1.0);
        let fall = ctx.config.visualizer.spectrum_smoothing.clamp(0.0, 0.98);

        update_smoothed_bins(&mut smoothed_upper_bins, &raw_upper_bins, rise, fall);
        update_smoothed_bins(&mut smoothed_lower_bins, &raw_lower_bins, rise, fall);

        if ctx.config.visualizer.peaks.enabled {
            let hold = Duration::from_millis(ctx.config.visualizer.peaks.hold_ms);
            let should_drop = last_spectrum_peak_drop.elapsed() >= hold;
            let top_only = ctx
                .config
                .visualizer
                .spectrum
                .render_style
                .eq_ignore_ascii_case("top_only");
            let peak_scale = if top_only {
                ctx.config.visualizer.height.max(1)
            } else {
                (ctx.config.visualizer.height / 2).max(1)
            } as f32;
            let drop_amount = (ctx.config.visualizer.peaks.drop_pixels as f32) / peak_scale;

            let upper_rose = update_spectrum_peaks(
                &mut upper_peak_bins,
                &smoothed_upper_bins,
                drop_amount,
                should_drop,
            );
            let lower_rose = update_spectrum_peaks(
                &mut lower_peak_bins,
                &smoothed_lower_bins,
                drop_amount,
                should_drop,
            );

            if upper_rose || lower_rose || should_drop {
                last_spectrum_peak_drop = Instant::now();
            }
        } else {
            upper_peak_bins.fill(0.0);
            lower_peak_bins.fill(0.0);
            last_spectrum_peak_drop = Instant::now();
        }

        display_peak = if live_level > display_peak {
            live_level
        } else {
            display_peak * 0.96
        };

        vu_level = if live_level > vu_level {
            vu_level + (live_level - vu_level) * 0.24
        } else {
            vu_level + (live_level - vu_level) * 0.055
        };
        vu_peak = if display_peak > vu_peak {
            vu_peak + (display_peak - vu_peak) * 0.18
        } else {
            vu_peak + (display_peak - vu_peak) * 0.035
        };

        state.meter.level = vu_level;
        state.meter.peak = if ctx.config.visualizer.peak_hold {
            vu_peak
        } else {
            vu_level
        };

        state.visualizer.enabled = ctx.config.visualizer.enabled;
        state.visualizer.mode = visualizer_mode_from_config(&ctx.config.visualizer.mode);
        state.visualizer.frame.left_points = left_points;
        state.visualizer.frame.right_points = right_points;

        if last_vis_debug.elapsed()
            >= Duration::from_millis(ctx.config.visualizer.debug_log_interval_ms)
        {
            log_visualizer_debug(
                &ctx,
                audio_len,
                sample_len,
                live_level,
                &smoothed_upper_bins,
            );
            last_vis_debug = Instant::now();
        }

        if state.version != loaded_version {
            if !state.artwork_path.is_empty() && Path::new(&state.artwork_path).exists() {
                match texture_creator.load_texture(&state.artwork_path) {
                    Ok(texture) => {
                        artwork_texture = Some(texture);
                        visualizer_colors =
                            visualizer_colors_for_artwork(&ctx, Some(&state.artwork_path));
                        loaded_version = state.version;
                        log_debug(
                            &ctx,
                            &format!(
                                "Renderer loaded artwork version {} from {}",
                                loaded_version, state.artwork_path
                            ),
                        );
                    }
                    Err(e) => {
                        log_error(&ctx, &format!("Renderer failed to load artwork: {e}"));
                        artwork_texture = None;
                        visualizer_colors = visualizer_colors_for_artwork(&ctx, None);
                    }
                }
            } else {
                artwork_texture = None;
                visualizer_colors = visualizer_colors_for_artwork(&ctx, None);
                loaded_version = state.version;
            }
        }

        let (canvas_w, canvas_h) = canvas.output_size().map_err(|e| e.to_string())?;
        if last_canvas_size != Some((canvas_w, canvas_h)) {
            log_debug(
                &ctx,
                &format!("Canvas output size: {}x{}", canvas_w, canvas_h),
            );
            last_canvas_size = Some((canvas_w, canvas_h));
        }

        let needs_static_rebuild = static_scene_cache
            .as_ref()
            .map(|c| c.version != state.version)
            .unwrap_or(true);

        if needs_static_rebuild {
            let (title_font_path, body_font_path, title_font_size, body_font_size, selected_theme) =
                selected_fonts(&ctx, &state);

            if selected_theme != loaded_font_theme {
                log_info(
                    &ctx,
                    &format!(
                        "Changing font theme from '{}' to '{}' for genre='{}' released='{}'",
                        loaded_font_theme, selected_theme, state.genre, state.released
                    ),
                );

                title_font = ttf_ctx
                    .load_font(title_font_path, title_font_size)
                    .map_err(|e| {
                        format!("Failed to load title font from {}: {e}", title_font_path)
                    })?;

                body_font = ttf_ctx
                    .load_font(body_font_path, body_font_size)
                    .map_err(|e| {
                        format!("Failed to load body font from {}: {e}", body_font_path)
                    })?;

                loaded_font_theme = selected_theme;
            }

            let text = build_text_cache(
                &texture_creator,
                &title_font,
                &body_font,
                &state,
                preset,
                top_h,
            )?;

            let artwork_rect = artwork_texture
                .as_ref()
                .map(|texture| compute_artwork_rect(texture.query(), scene_w, top_h));

            let cache = StaticSceneCache {
                version: state.version,
                text,
                artwork_rect,
            };

            canvas
                .with_texture_canvas(&mut static_scene_texture, |tex_canvas| {
                    let _ = cache.draw_static_scene(
                        tex_canvas,
                        &ctx,
                        artwork_texture.as_ref(),
                        scene_w,
                        top_h,
                        bottom_h,
                    );
                })
                .map_err(|e| e.to_string())?;

            static_scene_cache = Some(cache);
            text_scroll_started_at = Instant::now();
            log_debug(
                &ctx,
                &format!("Rebuilt static scene for version {}", state.version),
            );
        }

        let scale_x = (canvas_w as f32) / (scene_w as f32);
        let scale_y = (canvas_h as f32) / (scene_h as f32);
        let scene_scale = f32::min(scale_x, scale_y);

        let render_w = ((scene_w as f32) * scene_scale) as u32;
        let render_h = ((scene_h as f32) * scene_scale) as u32;

        let offset_x = ((canvas_w - render_w) / 2) as i32;
        let offset_y = ((canvas_h - render_h) / 2) as i32;

        let sx = |x: i32| offset_x + (((x as f32) * scene_scale) as i32);
        let sy = |y: i32| offset_y + (((y as f32) * scene_scale) as i32);
        let sw = |w: u32| ((w as f32) * scene_scale) as u32;
        let sh = |h: u32| ((h as f32) * scene_scale) as u32;

        canvas.set_draw_color(canvas_background_color(&ctx));
        canvas.clear();

        let static_target = Rect::new(offset_x, offset_y, render_w, render_h);
        canvas.copy(&static_scene_texture, None, static_target)?;

        if let Some(cache) = static_scene_cache.as_ref() {
            cache.draw_text(
                &mut canvas,
                offset_x,
                offset_y,
                scene_scale,
                text_scroll_started_at.elapsed(),
            )?;
        }

        if ctx.config.visualizer.enabled && state.visualizer.enabled {
            let vis_h = ctx.config.visualizer.height.min(bottom_h.saturating_sub(8));

            let vis_y_scene =
                (scene_h as i32) - (vis_h as i32) - (ctx.config.visualizer.padding as i32);
            let vis_x_scene = ctx.config.visualizer.padding as i32;
            let vis_w_scene = scene_w.saturating_sub(ctx.config.visualizer.padding * 2);

            draw_visualizer(
                &mut canvas,
                &ctx,
                &visualizer_colors,
                state.visualizer.mode,
                &state.visualizer.frame.left_points,
                &state.visualizer.frame.right_points,
                &smoothed_upper_bins,
                &smoothed_lower_bins,
                &upper_peak_bins,
                &lower_peak_bins,
                state.meter.level,
                state.meter.peak,
                sx(vis_x_scene),
                sy(vis_y_scene),
                sw(vis_w_scene),
                sh(vis_h),
                ctx.config.visualizer.spectrum_bar_gap,
            )?;
        }

        canvas.present();

        frame_counter += 1;
        if last_frame_log.elapsed() >= Duration::from_secs(5) {
            let avg_ms = (frame_timer.elapsed().as_secs_f64() * 1000.0) / (frame_counter as f64);
            log_info(&ctx, &format!("Display avg frame time: {:.1} ms", avg_ms));
            last_frame_log = Instant::now();
            frame_timer = Instant::now();
            frame_counter = 0;
        }

        thread::sleep(Duration::from_millis(ctx.config.display.frame_delay_ms));
    }

    log_info(&ctx, "Display loop stopped.");
    Ok(())
}
