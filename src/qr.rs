//! QR code rendering in the terminal (for the device link).

use crossterm::style::{Color, Stylize};
use qrcode::QrCode;

/// Prints `data` as a QR code in true black on true white.
///
/// The named ANSI colors (what qr2term uses) follow the terminal theme and can come out
/// e.g. purple, which the Signal app struggles to scan; explicit RGB colors do not.
pub fn print_qr(data: &str) {
    const QUIET_ZONE: usize = 4;
    const BLACK: Color = Color::Rgb { r: 0, g: 0, b: 0 };
    const WHITE: Color = Color::Rgb { r: 255, g: 255, b: 255 };

    let code = QrCode::new(data).expect("data too long for a QR code");
    let width = code.width();
    let colors = code.to_colors();
    let size = width + 2 * QUIET_ZONE;
    let is_dark = |x: usize, y: usize| {
        let (x, y) = (x.wrapping_sub(QUIET_ZONE), y.wrapping_sub(QUIET_ZONE));
        x < width && y < width && colors[y * width + x] == qrcode::Color::Dark
    };
    let color = |dark: bool| if dark { BLACK } else { WHITE };

    // Each character holds two modules: the upper one as foreground of '▀', the lower as background.
    for y in (0..size).step_by(2) {
        let line: String = (0..size)
            .map(|x| format!("{}", '▀'.with(color(is_dark(x, y))).on(color(is_dark(x, y + 1)))))
            .collect();
        println!("{line}");
    }
}
