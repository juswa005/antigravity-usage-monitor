use ratatui::style::{Color, Modifier, Style};

// Palette harmonized with the system Kitty + Waybar + Sway dark theme:
// Background #171d1b, Foreground #efe1c6, Teal #6c9994 / #5cd3c8,
// Amber #ddb876, Rose #cf6679, Muted #879393, Border #546f7a.
pub const BG: Color = Color::Rgb(23, 29, 27);
pub const SURFACE: Color = Color::Rgb(30, 38, 35);
pub const FG: Color = Color::Rgb(239, 225, 198);
pub const MUTED: Color = Color::Rgb(135, 147, 147);
pub const BORDER: Color = Color::Rgb(84, 111, 122);
pub const ACCENT: Color = Color::Rgb(108, 153, 148);
pub const BRIGHT_TEAL: Color = Color::Rgb(92, 211, 200);
pub const GREEN: Color = Color::Rgb(152, 195, 121);
pub const WARNING: Color = Color::Rgb(221, 184, 118);
pub const CRITICAL: Color = Color::Rgb(207, 102, 121);

pub fn base() -> Style {
    Style::default().fg(FG).bg(BG)
}

pub fn surface() -> Style {
    Style::default().fg(FG).bg(SURFACE)
}

pub fn muted() -> Style {
    Style::default().fg(MUTED)
}

pub fn border() -> Style {
    Style::default().fg(BORDER)
}

pub fn title() -> Style {
    Style::default()
        .fg(BRIGHT_TEAL)
        .add_modifier(Modifier::BOLD)
}

pub fn highlight_row() -> Style {
    Style::default()
        .bg(Color::Rgb(42, 61, 58))
        .fg(FG)
        .add_modifier(Modifier::BOLD)
}

pub fn ok_style() -> Style {
    Style::default().fg(GREEN).add_modifier(Modifier::BOLD)
}

pub fn warn_style() -> Style {
    Style::default().fg(WARNING).add_modifier(Modifier::BOLD)
}

pub fn crit_style() -> Style {
    Style::default().fg(CRITICAL).add_modifier(Modifier::BOLD)
}
