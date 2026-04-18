use crate::config::DisplayPreset;
use crate::logging::{log_debug, log_error, log_info};
use crate::state::{AppContext, SongState};

use sdl2::event::Event;
use sdl2::image::{InitFlag, LoadTexture};
use sdl2::keyboard::Keycode;
use sdl2::pixels::Color;
use sdl2::rect::Rect;

use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::thread;
use std::time::Duration;

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

fn draw_text_line(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    texture_creator: &sdl2::render::TextureCreator<sdl2::video::WindowContext>,
    font: &sdl2::ttf::Font,
    text: &str,
    color: Color,
    x: i32,
    y: i32,
) -> Result<(), String> {
    let safe_text = if text.trim().is_empty() { " " } else { text };

    let surface = font
        .render(safe_text)
        .blended(color)
        .map_err(|e| e.to_string())?;

    let texture = texture_creator
        .create_texture_from_surface(&surface)
        .map_err(|e| e.to_string())?;

    let target = Rect::new(x, y, surface.width(), surface.height());
    canvas.copy(&texture, None, target)?;
    Ok(())
}

/// Draws a simple digital horizontal VU meter with optional peak line.
fn draw_vu_meter(
    canvas: &mut sdl2::render::Canvas<sdl2::video::Window>,
    level: f32,
    peak: f32,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<(), String> {
    let level = level.clamp(0.0, 1.0);
    let peak = peak.clamp(0.0, 1.0);

    canvas.set_draw_color(Color::RGB(35, 35, 35));
    canvas.fill_rect(Rect::new(x, y, width, height))?;

    let fill_w = ((width as f32) * level) as u32;
    canvas.set_draw_color(Color::RGB(80, 220, 120));
    canvas.fill_rect(Rect::new(x, y, fill_w, height))?;

    let peak_x = x + ((width as f32) * peak) as i32;
    canvas.set_draw_color(Color::RGB(255, 255, 255));
    canvas.fill_rect(Rect::new(peak_x.saturating_sub(1), y, 2, height))?;

    Ok(())
}

/// SDL display loop.
///
/// The selected display preset defines the intended scene size.
/// The actual SDL canvas may be larger; the scene is scaled to fit.
pub fn run_display_loop(
    ctx: Arc<AppContext>,
    running: Arc<AtomicBool>,
    shared_state: Arc<Mutex<SongState>>,
) -> Result<(), String> {
    let sdl = sdl2::init()?;
    let video = sdl.video()?;
    let _image_ctx = sdl2::image::init(InitFlag::JPG | InitFlag::PNG)?;
    let ttf_ctx = sdl2::ttf::init().map_err(|e| e.to_string())?;

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

    log_info(&ctx, "Display loop started.");

    while running.load(Ordering::SeqCst) {
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. }
                | Event::KeyDown {
                    keycode: Some(Keycode::Escape),
                    ..
                } => {
                    running.store(false, Ordering::SeqCst);
                }
                _ => {}
            }
        }

        let state = {
            let state_guard = shared_state.lock().unwrap();
            state_guard.clone()
        };

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
                                loaded_version, state.artwork_path
                            ),
                        );
                    }
                    Err(e) => {
                        log_error(&ctx, &format!("Renderer failed to load artwork: {e}"));
                    }
                }
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

        let scale_x = canvas_w as f32 / scene_w as f32;
        let scale_y = canvas_h as f32 / scene_h as f32;
        let scene_scale = f32::min(scale_x, scale_y);

        let render_w = (scene_w as f32 * scene_scale) as u32;
        let render_h = (scene_h as f32 * scene_scale) as u32;

        let offset_x = ((canvas_w - render_w) / 2) as i32;
        let offset_y = ((canvas_h - render_h) / 2) as i32;

        let sx = |x: i32| offset_x + ((x as f32) * scene_scale) as i32;
        let sy = |y: i32| offset_y + ((y as f32) * scene_scale) as i32;
        let sw = |w: u32| ((w as f32) * scene_scale) as u32;
        let sh = |h: u32| ((h as f32) * scene_scale) as u32;

        canvas.set_draw_color(Color::RGB(0, 0, 0));
        canvas.clear();

        if let Some(texture) = artwork_texture.as_ref() {
            let query = texture.query();
            let art_w = query.width as f32;
            let art_h = query.height as f32;

            let padding = 24.0;
            let max_w = scene_w as f32 - (padding * 2.0);
            let max_h = top_h as f32 - (padding * 2.0);

            let scale = f32::min(max_w / art_w, max_h / art_h);
            let draw_w = (art_w * scale) as u32;
            let draw_h = (art_h * scale) as u32;

            let x = ((scene_w - draw_w) / 2) as i32;
            let y = ((top_h - draw_h) / 2) as i32;

            canvas.copy(texture, None, Rect::new(sx(x), sy(y), sw(draw_w), sh(draw_h)))?;
        }

        canvas.set_draw_color(Color::RGB(18, 18, 18));
        canvas.fill_rect(Rect::new(offset_x, sy(top_h as i32), render_w, sh(bottom_h)))?;

        let panel_x = preset.panel_x;
        let mut panel_y = top_h as i32 + preset.panel_y;

        let title_line = ellipsize(
            if state.title.trim().is_empty() {
                "Waiting for music..."
            } else {
                &state.title
            },
            48,
        );

        let artist_line = ellipsize(
            if state.artist.trim().is_empty() {
                "No track identified yet"
            } else {
                &state.artist
            },
            56,
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

        draw_text_line(
            &mut canvas,
            &texture_creator,
            &title_font,
            &title_line,
            Color::RGB(255, 255, 255),
            sx(panel_x),
            sy(panel_y),
        )?;
        panel_y += preset.title_line_spacing;

        draw_text_line(
            &mut canvas,
            &texture_creator,
            &body_font,
            &artist_line,
            Color::RGB(210, 210, 210),
            sx(panel_x),
            sy(panel_y),
        )?;
        panel_y += preset.body_line_spacing;

        draw_text_line(
            &mut canvas,
            &texture_creator,
            &body_font,
            &third_line,
            Color::RGB(180, 180, 180),
            sx(panel_x),
            sy(panel_y),
        )?;
        panel_y += preset.detail_line_spacing;

        let detail_line = format!(
            "Genre: {}    Composer: {}",
            ellipsize(&state.genre, 20),
            ellipsize(&state.composer, 28)
        );

        draw_text_line(
            &mut canvas,
            &texture_creator,
            &body_font,
            &detail_line,
            Color::RGB(140, 140, 140),
            sx(panel_x),
            sy(panel_y),
        )?;

        if ctx.config.visualizer.enabled
            && ctx.config.visualizer.mode == "vu"
            && ctx.config.visualizer.position == "bottom"
        {
            let vu_h = ctx
                .config
                .visualizer
                .height
                .min(bottom_h.saturating_sub(8));

            let vu_y_scene =
                scene_h as i32 - vu_h as i32 - ctx.config.visualizer.padding as i32;
            let vu_x_scene = ctx.config.visualizer.padding as i32;
            let vu_w_scene = scene_w.saturating_sub(ctx.config.visualizer.padding * 2);

            draw_vu_meter(
                &mut canvas,
                state.meter.level,
                state.meter.peak,
                sx(vu_x_scene),
                sy(vu_y_scene),
                sw(vu_w_scene),
                sh(vu_h),
            )?;
        }

        canvas.present();

        thread::sleep(Duration::from_millis(ctx.config.display.frame_delay_ms));
    }

    log_info(&ctx, "Display loop stopped.");
    Ok(())
}