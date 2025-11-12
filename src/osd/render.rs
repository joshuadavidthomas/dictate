//! tiny-skia rendering for OSD

use crate::osd::state::{OsdVisual, State};
use tiny_skia::*;

/// Render the OSD to a pixmap
pub fn render(pixmap: &mut Pixmap, visual: &OsdVisual, max_width: f32) {
    // Account for shadow padding (10px on each side = 20px total)
    let shadow_padding = 10.0;
    let actual_max_width = max_width - (shadow_padding * 2.0);
    let content_width = actual_max_width * visual.content_ratio;
    let height = 36.0;
    
    // Calculate x-offset to center the content (accounting for padding)
    let x_offset = shadow_padding + (actual_max_width - content_width) / 2.0;
    let y_offset = shadow_padding;

    // 1. Draw shadow (with blur simulation)
    draw_shadow(pixmap, x_offset, y_offset, content_width, height);

    // 2. Draw background rounded rect
    draw_background(pixmap, x_offset, y_offset, content_width, height);

    // 3. Draw status dot (left side)
    draw_status_dot(pixmap, x_offset, y_offset, visual.color, visual.alpha);

    // 4. Draw state text label (center-left, vertically aligned with dot)
    let label = match visual.state {
        State::Idle => "READY",
        State::Recording => "RECORDING",
        State::Transcribing => "TRANSCRIBING",
        State::Error => "ERROR",
    };
    // Dot is centered at y=18.0, text height is 14px (7 * pixel_size=2), so center at 18-7=11
    draw_text(pixmap, label, x_offset + 28.0, y_offset + 11.0, Color::from_rgba8(200, 200, 200, 255));

    // 5. Draw level bars (right side, only when recording)
    if visual.state == State::Recording {
        let bar_area_x = x_offset + content_width - 120.0;
        let bar_area_width = 100.0;
        draw_bars(
            pixmap,
            &visual.level_bars,
            bar_area_x,
            y_offset + 8.0,
            bar_area_width,
            20.0,
            visual.color,
        );
    }
}

fn draw_shadow(pixmap: &mut Pixmap, x: f32, y: f32, width: f32, height: f32) {
    // Simulate Gaussian blur by drawing many layers with decreasing opacity
    // This creates a soft, diffuse shadow effect
    let radius = 12.0;
    let shadow_offset_y = 2.0;  // Subtle vertical drop
    let blur_radius = 12;  // More blur layers for softer effect
    
    // Draw blur layers from outside in (largest to smallest)
    for i in (0..=blur_radius).rev() {
        let spread = i as f32 * 0.5;  // Half spread per layer for smoother gradient
        let opacity = if i == 0 {
            35  // Very light core
        } else {
            // Exponential falloff for smooth, diffuse blur
            (25.0 * (-spread / 4.0).exp()) as u8
        };
        
        if opacity < 3 {
            continue;  // Skip very transparent layers
        }
        
        let Some(rect) = Rect::from_xywh(
            x - spread,  // Equal horizontal spread on both sides
            y + shadow_offset_y - spread * 0.5,  // More even vertical spread
            width + spread * 2.0,
            height + spread * 2.0,
        ) else {
            continue;
        };
        
        let mut paint = Paint::default();
        paint.set_color_rgba8(0, 0, 0, opacity);
        paint.anti_alias = true;
        
        let path = create_rounded_rect(rect, radius + spread);
        pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
}

fn draw_background(pixmap: &mut Pixmap, x: f32, y: f32, width: f32, height: f32) {
    let Some(rect) = Rect::from_xywh(x, y, width, height) else {
        return;
    };

    let mut paint = Paint::default();
    paint.set_color_rgba8(30, 30, 30, 240); // ~94% opaque dark gray
    paint.anti_alias = true;

    // Create rounded rectangle path
    let radius = 12.0;
    let path = create_rounded_rect(rect, radius);

    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

fn draw_status_dot(pixmap: &mut Pixmap, x_offset: f32, y_offset: f32, color: Color, alpha: f32) {
    let dot_x = x_offset + 12.0;
    let dot_y = y_offset + 18.0;
    let dot_radius = 6.0;

    // Apply alpha to color
    let color_with_alpha = Color::from_rgba(color.red(), color.green(), color.blue(), alpha)
        .unwrap_or(color);

    let mut paint = Paint::default();
    paint.set_color(color_with_alpha);
    paint.anti_alias = true;

    let Some(circle) = PathBuilder::from_circle(dot_x, dot_y, dot_radius) else {
        return;
    };

    pixmap.fill_path(
        &circle,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

fn draw_bars(
    pixmap: &mut Pixmap,
    bars: &[f32; 10],
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    color: Color,
) {
    let bar_width = width / 10.0;
    let gap = 2.0;

    for (i, &level) in bars.iter().enumerate() {
        // Apply aggressive gain (6x) and power curve for better visibility
        // Power of 0.4 compresses dynamic range, making quiet sounds much more visible
        let amplified = (level * 6.0).clamp(0.0, 1.0).powf(0.4);
        
        // Set minimum bar height of 15% for any non-zero sound
        let normalized = if level > 0.001 {
            amplified.max(0.15)
        } else {
            0.0
        };
        
        let bar_height = height * normalized;
        let bar_x = x + i as f32 * bar_width + gap;
        let bar_y = y + (height - bar_height);
        let bar_w = bar_width - 2.0 * gap;

        if bar_w <= 0.0 || bar_height <= 2.0 {
            continue;
        }

        let Some(rect) = Rect::from_xywh(bar_x, bar_y, bar_w, bar_height) else {
            continue;
        };

        let mut paint = Paint::default();
        paint.set_color(color);
        paint.anti_alias = true;

        let path = PathBuilder::from_rect(rect);
        pixmap.fill_path(
            &path,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// Draw simple text using rectangles (bitmap-style font)
fn draw_text(pixmap: &mut Pixmap, text: &str, x: f32, y: f32, color: Color) {
    let mut paint = Paint::default();
    paint.set_color(color);
    paint.anti_alias = false; // Sharp pixels for text
    
    let pixel_size = 2.0;  // 2x scaling = 10x14 characters
    let char_width = 6.0 * pixel_size;  // (5px + 1px spacing) * scale
    let _char_height = 7.0 * pixel_size;  // 7px * scale
    
    for (i, ch) in text.chars().enumerate() {
        let char_x = x + i as f32 * char_width;
        draw_char(pixmap, ch, char_x, y, pixel_size, &paint);
    }
}

/// Draw a single character using simple 5x7 bitmap patterns
fn draw_char(pixmap: &mut Pixmap, ch: char, x: f32, y: f32, pixel_size: f32, paint: &Paint) {
    // Simple 5x7 bitmap font patterns (1 = filled, 0 = empty)
    let pattern = match ch {
        'A' => vec![
            0b01110,
            0b10001,
            0b10001,
            0b11111,
            0b10001,
            0b10001,
            0b10001,
        ],
        'C' => vec![
            0b01110,
            0b10001,
            0b10000,
            0b10000,
            0b10000,
            0b10001,
            0b01110,
        ],
        'D' => vec![
            0b11110,
            0b10001,
            0b10001,
            0b10001,
            0b10001,
            0b10001,
            0b11110,
        ],
        'E' => vec![
            0b11111,
            0b10000,
            0b10000,
            0b11110,
            0b10000,
            0b10000,
            0b11111,
        ],
        'G' => vec![
            0b01110,
            0b10001,
            0b10000,
            0b10011,
            0b10001,
            0b10001,
            0b01110,
        ],
        'I' => vec![
            0b11111,
            0b00100,
            0b00100,
            0b00100,
            0b00100,
            0b00100,
            0b11111,
        ],
        'N' => vec![
            0b10001,
            0b11001,
            0b10101,
            0b10011,
            0b10001,
            0b10001,
            0b10001,
        ],
        'O' => vec![
            0b01110,
            0b10001,
            0b10001,
            0b10001,
            0b10001,
            0b10001,
            0b01110,
        ],
        'R' => vec![
            0b11110,
            0b10001,
            0b10001,
            0b11110,
            0b10010,
            0b10001,
            0b10001,
        ],
        'S' => vec![
            0b01111,
            0b10000,
            0b10000,
            0b01110,
            0b00001,
            0b00001,
            0b11110,
        ],
        'T' => vec![
            0b11111,
            0b00100,
            0b00100,
            0b00100,
            0b00100,
            0b00100,
            0b00100,
        ],
        'Y' => vec![
            0b10001,
            0b10001,
            0b01010,
            0b00100,
            0b00100,
            0b00100,
            0b00100,
        ],
        _ => vec![0; 7], // Unknown char = blank
    };
    
    for (row, &bits) in pattern.iter().enumerate() {
        for col in 0..5 {
            if (bits >> (4 - col)) & 1 == 1 {
                let px = x + col as f32 * pixel_size;
                let py = y + row as f32 * pixel_size;
                if let Some(rect) = Rect::from_xywh(px, py, pixel_size, pixel_size) {
                    let path = PathBuilder::from_rect(rect);
                    pixmap.fill_path(
                        &path,
                        paint,
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                }
            }
        }
    }
}

fn create_rounded_rect(rect: Rect, radius: f32) -> Path {
    let mut pb = PathBuilder::new();

    let x = rect.x();
    let y = rect.y();
    let w = rect.width();
    let h = rect.height();

    // Start at top-left corner (after radius)
    pb.move_to(x + radius, y);

    // Top edge
    pb.line_to(x + w - radius, y);

    // Top-right corner
    pb.quad_to(x + w, y, x + w, y + radius);

    // Right edge
    pb.line_to(x + w, y + h - radius);

    // Bottom-right corner
    pb.quad_to(x + w, y + h, x + w - radius, y + h);

    // Bottom edge
    pb.line_to(x + radius, y + h);

    // Bottom-left corner
    pb.quad_to(x, y + h, x, y + h - radius);

    // Left edge
    pb.line_to(x, y + radius);

    // Top-left corner
    pb.quad_to(x, y, x + radius, y);

    pb.close();

    pb.finish().unwrap_or_else(|| {
        // Fallback to simple rect if path building fails
        PathBuilder::from_rect(rect)
    })
}
