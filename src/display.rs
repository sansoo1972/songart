use crate::audio::{ build_oscilloscope_points, compute_rms, SharedAudioBuffer };
use crate::config::DisplayPreset;
use crate::logging::{ log_debug, log_error, log_info };
use crate::state::{ AppContext, SongState };
use crate::visualizer::VisualizerMode;

use sdl2::event::Event;
use sdl2::image::{ InitFlag, LoadTexture };
use sdl2::keyboard::Keycode;
use sdl2::pixels::Color;
use sdl2::rect::{ Point, Rect };
use sdl2::render::{ Texture, TextureCreator };
use sdl2::video::WindowContext;

use std::path::Path;
use std::sync::{ atomic::{ AtomicBool, Ordering }, Arc, Mutex };
use std::thread;
use std::time::{ Duration, Instant };

fn ellipsize(input: &str, max_chars: usize) -> String {
    let chars: Vec<char> = input.chars().collect();
    if chars.len() <= max_chars {
        return input.to_string();
    }

    let trimmed: String = chars.into_iter().take(max_chars.saturating_sub(1)).collect();

    format!("{trimmed}…")
}

/// Resolves the configured font theme into title/body font paths and sizes.
fn selected_fonts<'a>(ctx: &'a AppContext) -> (&'a str, &'a str, u16, u16) {
    let theme_name = ctx.config.fonts.theme.to_ascii_lowercase();

    if let Some(theme) = ctx.config.font_themes.get(&theme_name) {
        (&theme.title, &theme.body, theme.title_size, theme.body_size)
    } else {
        (
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            34,
            24,
        )
    }
}

/// Resolves the selected display preset from config.
fn selected_display_preset<'a>(ctx: &'a AppContext) -> Option<&'a DisplayPreset> {
    let key = ctx.config.display.orientation.to_ascii_lowercase();
    ctx.config.display_presets.get(&key)
}

/// A cached text texture plus its native-size target rectangle in scene space.
struct CachedText<'a> {
    texture: Texture<'a>,
    rect: Rect,
}

impl<'a> CachedText<'a> {
    fn new(
        texture_creator: &'a TextureCreator<WindowContext>,
        font: &sdl2::ttf::Font,
        text: &str,
        color: Color,
        x: i32,
        y: i32
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

        Ok(Self { texture, rect })
    }

    fn draw(
        &self,
        canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
        sx: &dyn Fn(i32) -> i32,
        sy: &dyn Fn(i32) -> i32,
        sw: &dyn Fn(u32) -> u32,
        sh: &dyn Fn(u32) -> u32
    ) -> Result<(), String> {
        let target = Rect::new(
            sx(self.rect.x()),
            sy(self.rect.y()),
            sw(self.rect.width()),
            sh(self.rect.height())
        );
        canvas.copy(&self.texture, None, target)?;
        Ok(())
    }
}

/// Cached text block for song metadata.
/// Rebuilt only when the metadata version changes.
struct TextCache<'a> {
    version: u64,
    title: CachedText<'a>,
    artist: CachedText<'a>,
    third: CachedText<'a>,
    detail: CachedText<'a>,
}

fn build_text_cache<'a>(
    texture_creator: &'a TextureCreator<WindowContext>,
    title_font: &sdl2::ttf::Font,
    body_font: &sdl2::ttf::Font,
    state: &SongState,
    preset: &DisplayPreset,
    top_h: u32
) -> Result<TextCache<'a>, String> {
    let panel_x = preset.panel_x;
    let mut panel_y = (top_h as i32) + preset.panel_y;

    let title_line = ellipsize(
        if state.title.trim().is_empty() {
            "Waiting for music..."
        } else {
            &state.title
        },
        48
    );

    let artist_line = ellipsize(
        if state.artist.trim().is_empty() {
            "No track identified yet"
        } else {
            &state.artist
        },
        56
    );

    let mut third_line = state.album.clone();
    if !state.released.is_empty() && state.released != "Unknown" {
        if third_line.is_empty() || third_line == "Unknown" {
            third_line = state.released.clone();
        } else {
            third_line = format!("{} • {}", state.album, state.released);
        }
    }

    let third_line = if third_line.trim().is_empty() {
        "Album unknown".to_string()
    } else {
        ellipsize(&third_line, 56)
    };

    let detail_line = format!(
        "Genre: {}    Composer: {}",
        ellipsize(&state.genre, 20),
        ellipsize(&state.composer, 28)
    );

    let title = CachedText::new(
        texture_creator,
        title_font,
        &title_line,
        Color::RGB(255, 255, 255),
        panel_x,
        panel_y
    )?;
    panel_y += preset.title_line_spacing;

    let artist = CachedText::new(
        texture_creator,
        body_font,
        &artist_line,
        Color::RGB(210, 210, 210),
        panel_x,
        panel_y
    )?;
    panel_y += preset.body_line_spacing;

    let third = CachedText::new(
        texture_creator,
        body_font,
        &third_line,
        Color::RGB(180, 180, 180),
        panel_x,
        panel_y
    )?;
    panel_y += preset.detail_line_spacing;

    let detail = CachedText::new(
        texture_creator,
        body_font,
        &detail_line,
        Color::RGB(140, 140, 140),
        panel_x,
        panel_y
    )?;

    Ok(TextCache {
        version: state.version,
        title,
        artist,
        third,
        detail,
    })
}

fn draw_polyline(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    points: &[(f32, f32)],
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    color: Color
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

fn draw_oscilloscope(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    left_points: &[(f32, f32)],
    right_points: &[(f32, f32)],
    x: i32,
    y: i32,
    width: u32,
    height: u32
) -> Result<(), String> {
    canvas.set_draw_color(Color::RGB(10, 10, 10));
    canvas.fill_rect(Rect::new(x, y, width, height))?;

    canvas.set_draw_color(Color::RGB(40, 40, 40));
    canvas.draw_line(
        Point::new(x, y + (height as i32) / 4),
        Point::new(x + (width as i32), y + (height as i32) / 4)
    )?;
    canvas.draw_line(
        Point::new(x, y + ((height as i32) * 3) / 4),
        Point::new(x + (width as i32), y + ((height as i32) * 3) / 4)
    )?;

    draw_polyline(canvas, left_points, x, y, width, height, Color::RGB(80, 220, 120))?;

    draw_polyline(canvas, right_points, x, y, width, height, Color::RGB(80, 160, 255))?;

    Ok(())
}

fn draw_visualizer(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    mode: VisualizerMode,
    left_points: &[(f32, f32)],
    right_points: &[(f32, f32)],
    x: i32,
    y: i32,
    width: u32,
    height: u32
) -> Result<(), String> {
    match mode {
        VisualizerMode::None => Ok(()),
        VisualizerMode::Oscilloscope | VisualizerMode::Spectrum | VisualizerMode::AnalogVu => {
            draw_oscilloscope(canvas, left_points, right_points, x, y, width, height)
        }
    }
}

/// SDL display loop.
///
/// Performance note:
/// - text textures are cached and only rebuilt when `SongState.version` changes
/// - oscilloscope points are rebuilt every frame from the shared live audio buffer
pub fn run_display_loop(
    ctx: Arc<AppContext>,
    running: Arc<AtomicBool>,
    shared_state: Arc<Mutex<SongState>>,
    shared_audio: Arc<Mutex<SharedAudioBuffer>>
) -> Result<(), String> {
    let sdl = sdl2::init()?;
    let video = sdl.video()?;
    let _image_ctx = sdl2::image::init(InitFlag::JPG | InitFlag::PNG)?;
    let ttf_ctx = sdl2::ttf::init().map_err(|e| e.to_string())?;

    let preset = selected_display_preset(&ctx).ok_or_else(||
        format!("Unknown display preset: {}", ctx.config.display.orientation)
    )?;

    let mut window_builder = video.window(
        &ctx.config.display.window_title,
        preset.width,
        preset.height
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
        .present_vsync()
        .build()
        .map_err(|e| e.to_string())?;

    let texture_creator = canvas.texture_creator();

    let (title_font_path, body_font_path, title_font_size, body_font_size) = selected_fonts(&ctx);

    let title_font = ttf_ctx
        .load_font(title_font_path, title_font_size)
        .map_err(|e| format!("Failed to load title font from {}: {e}", title_font_path))?;

    let body_font = ttf_ctx
        .load_font(body_font_path, body_font_size)
        .map_err(|e| format!("Failed to load body font from {}: {e}", body_font_path))?;

    let mut event_pump = sdl.event_pump()?;
    let mut loaded_version: u64 = u64::MAX;
    let mut artwork_texture: Option<sdl2::render::Texture<'_>> = None;
    let mut last_canvas_size: Option<(u32, u32)> = None;
    let mut display_peak = 0.0f32;
    let mut last_vis_debug = Instant::now();
    let mut text_cache: Option<TextCache<'_>> = None;

    log_info(&ctx, "Display loop started.");

    while running.load(Ordering::SeqCst) {
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. } | Event::KeyDown { keycode: Some(Keycode::Escape), .. } => {
                    running.store(false, Ordering::SeqCst);
                }
                _ => {}
            }
        }

        let mut state = {
            let state_guard = shared_state.lock().unwrap();
            state_guard.clone()
        };

        let (audio_len, sample_len, live_level, left_points, right_points) = {
            let audio = shared_audio.lock().unwrap();
            let audio_len = audio.len();
            let vis_samples = audio.recent_ms(ctx.config.visualizer.window_ms);
            let sample_len = vis_samples.len();

            let level = compute_rms(&vis_samples).unwrap_or(0.0);

            let left = build_oscilloscope_points(
                &vis_samples,
                ctx.config.visualizer.point_count,
                ctx.config.visualizer.left_y_offset,
                ctx.config.visualizer.y_scale,
                ctx.config.visualizer.gain,
                ctx.config.visualizer.visible_sample_count,
                ctx.config.visualizer.max_gain
            );

            let right = build_oscilloscope_points(
                &vis_samples,
                ctx.config.visualizer.point_count,
                ctx.config.visualizer.right_y_offset,
                ctx.config.visualizer.y_scale,
                ctx.config.visualizer.gain,
                ctx.config.visualizer.visible_sample_count,
                ctx.config.visualizer.max_gain
            );

            (audio_len, sample_len, level, left, right)
        };

        display_peak = if live_level > display_peak { live_level } else { display_peak * 0.96 };

        state.meter.level = live_level;
        state.meter.peak = if ctx.config.visualizer.peak_hold { display_peak } else { live_level };
        state.visualizer.enabled = ctx.config.visualizer.enabled;
        state.visualizer.mode = match ctx.config.visualizer.mode.to_ascii_lowercase().as_str() {
            "oscilloscope" => VisualizerMode::Oscilloscope,
            "spectrum" => VisualizerMode::Spectrum,
            "analog_vu" => VisualizerMode::AnalogVu,
            _ => VisualizerMode::None,
        };
        state.visualizer.frame.left_points = left_points;
        state.visualizer.frame.right_points = right_points;

        if
            last_vis_debug.elapsed() >=
            Duration::from_millis(ctx.config.visualizer.debug_log_interval_ms)
        {
            let left_head = state.visualizer.frame.left_points
                .first()
                .copied()
                .unwrap_or((0.0, 0.0));

            let left_mid = if state.visualizer.frame.left_points.is_empty() {
                (0.0, 0.0)
            } else {
                state.visualizer.frame.left_points[state.visualizer.frame.left_points.len() / 2]
            };

            log_debug(
                &ctx,
                &format!(
                    "display vis: audio_len={} sample_len={} level={:.3} left_points={} head=({:.3},{:.3}) mid=({:.3},{:.3})",
                    audio_len,
                    sample_len,
                    live_level,
                    state.visualizer.frame.left_points.len(),
                    left_head.0,
                    left_head.1,
                    left_mid.0,
                    left_mid.1
                )
            );

            last_vis_debug = Instant::now();
        }

        if state.version != loaded_version {
            if !state.artwork_path.is_empty() && Path::new(&state.artwork_path).exists() {
                match texture_creator.load_texture(&state.artwork_path) {
                    Ok(texture) => {
                        artwork_texture = Some(texture);
                        loaded_version = state.version;
                        log_debug(
                            &ctx,
                            &format!(
                                "Renderer loaded artwork version {} from {}",
                                loaded_version,
                                state.artwork_path
                            )
                        );
                    }
                    Err(e) => {
                        log_error(&ctx, &format!("Renderer failed to load artwork: {e}"));
                    }
                }
            } else {
                artwork_texture = None;
                loaded_version = state.version;
            }
        }

        let (canvas_w, canvas_h) = canvas.output_size().map_err(|e| e.to_string())?;
        if last_canvas_size != Some((canvas_w, canvas_h)) {
            log_debug(&ctx, &format!("Canvas output size: {}x{}", canvas_w, canvas_h));
            last_canvas_size = Some((canvas_w, canvas_h));
        }

        let scene_w = preset.width;
        let scene_h = preset.height;
        let top_h = ((scene_h as f32) * preset.top_panel_ratio) as u32;
        let bottom_h = scene_h - top_h;

        // Rebuild cached text only when metadata changes.
        let needs_text_rebuild = text_cache
            .as_ref()
            .map(|c| c.version != state.version)
            .unwrap_or(true);

        if needs_text_rebuild {
            text_cache = Some(
                build_text_cache(&texture_creator, &title_font, &body_font, &state, preset, top_h)?
            );
            log_debug(&ctx, &format!("Rebuilt text cache for version {}", state.version));
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

        canvas.set_draw_color(Color::RGB(0, 0, 0));
        canvas.clear();

        if let Some(texture) = artwork_texture.as_ref() {
            let query = texture.query();
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

            canvas.copy(texture, None, Rect::new(sx(x), sy(y), sw(draw_w), sh(draw_h)))?;
        }

        canvas.set_draw_color(Color::RGB(18, 18, 18));
        canvas.fill_rect(Rect::new(offset_x, sy(top_h as i32), render_w, sh(bottom_h)))?;

        if let Some(cache) = text_cache.as_ref() {
            cache.title.draw(&mut canvas, &sx, &sy, &sw, &sh)?;
            cache.artist.draw(&mut canvas, &sx, &sy, &sw, &sh)?;
            cache.third.draw(&mut canvas, &sx, &sy, &sw, &sh)?;
            cache.detail.draw(&mut canvas, &sx, &sy, &sw, &sh)?;
        }

        if ctx.config.visualizer.enabled && state.visualizer.enabled {
            let vis_h = ctx.config.visualizer.height.min(bottom_h.saturating_sub(8));

            let vis_y_scene =
                (scene_h as i32) - (vis_h as i32) - (ctx.config.visualizer.padding as i32);
            let vis_x_scene = ctx.config.visualizer.padding as i32;
            let vis_w_scene = scene_w.saturating_sub(ctx.config.visualizer.padding * 2);

            draw_visualizer(
                &mut canvas,
                state.visualizer.mode,
                &state.visualizer.frame.left_points,
                &state.visualizer.frame.right_points,
                sx(vis_x_scene),
                sy(vis_y_scene),
                sw(vis_w_scene),
                sh(vis_h)
            )?;
        }

        canvas.present();
        thread::sleep(Duration::from_millis(ctx.config.display.frame_delay_ms));
    }

    log_info(&ctx, "Display loop stopped.");
    Ok(())
}