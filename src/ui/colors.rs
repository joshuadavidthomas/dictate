//! Color constants for the UI
//!
//! This module consolidates all color definitions used throughout the UI,
//! making them reusable and ensuring consistency.

use iced::Color;

// Helper to create colors from RGB8 values
const fn rgb8(r: u8, g: u8, b: u8) -> Color {
    Color {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
}

// State indicator colors
pub const GRAY: Color = rgb8(122, 122, 122);
pub const DIM_GREEN: Color = rgb8(118, 211, 155);
pub const RED: Color = rgb8(231, 76, 60);
pub const BLUE: Color = rgb8(52, 152, 219);
pub const ORANGE: Color = rgb8(243, 156, 18);

// UI element colors
pub const GREEN: Color = rgb8(76, 217, 100);
pub const LIGHT_GRAY: Color = rgb8(200, 200, 200);
pub const DARK_GRAY: Color = rgb8(30, 30, 30);
pub const BLACK: Color = rgb8(0, 0, 0);

// Helper function for creating colors with alpha
pub const fn with_alpha(color: Color, alpha: f32) -> Color {
    Color {
        r: color.r,
        g: color.g,
        b: color.b,
        a: alpha,
    }
}
