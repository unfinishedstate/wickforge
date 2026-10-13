use iced::{Color, Theme, theme};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub background: Color,
    pub foreground: Color,
    pub red: Color,
    pub green: Color,
    pub accent: Color,
    pub muted: Color,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            background: Color::from_rgb8(16, 19, 21),
            foreground: Color::from_rgb8(202, 204, 204),
            red: Color::from_rgb8(86, 93, 96),
            green: Color::from_rgb8(159, 165, 169),
            accent: Color::from_rgb8(121, 129, 134),
            muted: Color::from_rgb8(75, 78, 85),
        }
        .legible()
    }
}

impl Palette {
    pub fn parse(contents: &str) -> Result<Self, String> {
        let values = contents
            .parse::<toml::Table>()
            .map_err(|error| error.to_string())?;
        let get = |names: &[&str]| -> Result<Color, String> {
            let (name, value) = names
                .iter()
                .find_map(|name| values.get(*name).map(|value| (*name, value)))
                .ok_or_else(|| format!("missing theme color {}", names[0]))?;
            let value = value
                .as_str()
                .ok_or_else(|| format!("{name} must be a hex string"))?;
            let hex = value.strip_prefix('#').unwrap_or(value);
            if hex.len() != 6 || !hex.is_ascii() {
                return Err(format!("invalid color {name}: {value}"));
            }
            let number = u32::from_str_radix(hex, 16)
                .map_err(|_| format!("invalid color {name}: {value}"))?;
            Ok(Color::from_rgb8(
                (number >> 16) as u8,
                (number >> 8) as u8,
                number as u8,
            ))
        };
        Ok(Self {
            background: get(&["background", "bg", "color0"])?,
            foreground: get(&["foreground", "fg", "color7"])?,
            red: get(&["red", "color1"])?,
            green: get(&["green", "color2"])?,
            accent: get(&["accent", "blue", "color4"])?,
            muted: get(&[
                "muted",
                "color8",
                "blue",
                "color4",
                "foreground",
                "fg",
                "color7",
            ])?,
        }
        .legible())
    }

    // Resolve contrast once per palette load, never inside chart drawing.
    fn legible(mut self) -> Self {
        self.foreground = contrasting(self.foreground, self.background, 7.0);
        self.red = contrasting(self.red, self.background, 4.5);
        self.green = contrasting(self.green, self.background, 4.5);
        self.accent = contrasting(self.accent, self.background, 4.5);
        self.muted = contrasting(self.muted, self.background, 4.5);
        self
    }

    pub fn theme(self) -> Theme {
        Theme::custom(
            "Omarchy".into(),
            theme::Palette {
                background: self.background,
                text: self.foreground,
                primary: self.accent,
                success: self.green,
                danger: self.red,
            },
        )
    }
}

fn luminance(color: Color) -> f32 {
    let linear = |component: f32| {
        if component <= 0.04045 {
            component / 12.92
        } else {
            ((component + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

fn contrast(a: Color, b: Color) -> f32 {
    let a = luminance(a);
    let b = luminance(b);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

fn contrasting(color: Color, background: Color, ratio: f32) -> Color {
    if contrast(color, background) >= ratio {
        return color;
    }
    let target = if contrast(Color::BLACK, background) > contrast(Color::WHITE, background) {
        Color::BLACK
    } else {
        Color::WHITE
    };
    let blend = |amount: f32| {
        Color::from_rgb(
            color.r + (target.r - color.r) * amount,
            color.g + (target.g - color.g) * amount,
            color.b + (target.b - color.b) * amount,
        )
    };
    let mut low = 0.0;
    let mut high = 1.0;
    for _ in 0..16 {
        let middle = (low + high) * 0.5;
        if contrast(blend(middle), background) >= ratio {
            high = middle;
        } else {
            low = middle;
        }
    }
    blend(high)
}

pub struct ThemeFile {
    pub palette: Palette,
    pub theme: Theme,
    pub error: Option<String>,
    contents: Option<String>,
    paths: Vec<PathBuf>,
    selected: Option<PathBuf>,
}

impl ThemeFile {
    pub fn new() -> Self {
        let paths = if let Some(path) = std::env::var_os("MEXC_CHART_THEME") {
            vec![PathBuf::from(path)]
        } else {
            let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
            let state = std::env::var_os("XDG_STATE_HOME")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".local/state"));
            let config = std::env::var_os("XDG_CONFIG_HOME")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".config"));
            vec![
                state.join("omarchy/current/theme/colors.toml"),
                config.join("omarchy/current/theme/colors.toml"),
            ]
        };
        let palette = Palette::default();
        let mut file = Self {
            palette,
            theme: palette.theme(),
            error: None,
            contents: None,
            paths,
            selected: None,
        };
        file.reload();
        file
    }

    /// Retains the last valid palette throughout atomic replacement or malformed edits.
    pub fn reload(&mut self) -> bool {
        let mut failure = String::new();
        for path in &self.paths {
            let contents = match std::fs::read_to_string(path) {
                Ok(contents) => contents,
                Err(error) => {
                    failure = format!("Theme {}: {error}", path.display());
                    // An existing primary palette is never replaced by a fallback
                    // merely because its contents temporarily became unreadable.
                    if error.kind() != std::io::ErrorKind::NotFound
                        || self.selected.as_ref() == Some(path)
                    {
                        break;
                    }
                    continue;
                }
            };
            if self.contents.as_ref() == Some(&contents) {
                self.error = None;
                if self.selected.as_ref() != Some(path) {
                    self.selected = Some(path.clone());
                }
                return false;
            }
            match Palette::parse(&contents) {
                Ok(palette) => {
                    let changed = self.palette != palette;
                    if changed {
                        self.theme = palette.theme();
                    }
                    self.palette = palette;
                    self.contents = Some(contents);
                    self.selected = Some(path.clone());
                    self.error = None;
                    return changed;
                }
                Err(error) => {
                    failure = format!("Theme {}: {error}", path.display());
                    break;
                }
            }
        }
        self.error = Some(failure);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aliases_and_invalid_replacement() {
        let valid = "bg = '#101315'\nfg = '#cacccc'\ncolor1 = '#ff0000'\ncolor2 = '#00ff00'\ncolor4 = '#0000ff'";
        assert_eq!(
            Palette::parse(valid).unwrap().background,
            Palette::default().background
        );
        assert!(Palette::parse("background = 'broken'").is_err());
    }

    #[test]
    fn low_contrast_dark_light_and_monochrome_colors_remain_visible() {
        for color in ["#111111", "#eeeeee", "#777777"] {
            let palette = Palette::parse(&format!(
                "background='{color}'\nforeground='{color}'\nred='{color}'\ngreen='{color}'\naccent='{color}'\nmuted='{color}'"
            )).unwrap();
            for ink in [
                palette.foreground,
                palette.red,
                palette.green,
                palette.accent,
                palette.muted,
            ] {
                assert!(contrast(ink, palette.background) >= 4.5);
            }
        }
    }
}
