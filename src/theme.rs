//! Theme loading and palette derivation.

use std::collections::BTreeMap;

use anyhow::{anyhow, bail, Context, Result};
use opaline::{builtins, OpalineColor, Theme};
use ratatui::style::{Color, Modifier, Style};

/// A configurable color role.
pub struct Role {
    /// Key used in the `[colors]` table of the config file.
    pub key: &'static str,
    /// Primary theme token.
    pub token: &'static str,
    /// Token used when the primary token is missing from a theme.
    pub fallback: &'static str,
}

/// All color roles that can be overridden in the config file.
pub const ROLES: &[Role] = &[
    Role {
        key: "text",
        token: "text.primary",
        fallback: "text.primary",
    },
    Role {
        key: "background",
        token: "bg.base",
        fallback: "bg.base",
    },
    Role {
        key: "heading",
        token: "accent.primary",
        fallback: "accent.primary",
    },
    Role {
        key: "accent",
        token: "accent.secondary",
        fallback: "accent.primary",
    },
    Role {
        key: "muted",
        token: "text.muted",
        fallback: "text.dim",
    },
    Role {
        key: "search",
        token: "warning",
        fallback: "accent.tertiary",
    },
    Role {
        key: "search_text",
        token: "bg.base",
        fallback: "bg.base",
    },
    Role {
        key: "status_bg",
        token: "bg.panel",
        fallback: "bg.base",
    },
    Role {
        key: "status_fg",
        token: "text.primary",
        fallback: "text.primary",
    },
    Role {
        key: "border",
        token: "border.unfocused",
        fallback: "text.dim",
    },
    Role {
        key: "selection",
        token: "bg.selection",
        fallback: "bg.highlight",
    },
];

/// Resolved colors used by the interface.
#[derive(Debug, Clone)]
pub struct Palette {
    pub text: Color,
    pub background: Color,
    pub heading: Color,
    pub accent: Color,
    pub muted: Color,
    pub search: Color,
    pub search_text: Color,
    pub status_bg: Color,
    pub status_fg: Color,
    pub border: Color,
    pub selection: Color,
    /// Five colors cycled by the paragraph indicators.
    pub markers: [Color; 5],
}

/// Return the role for a config key.
pub fn role_for_key(key: &str) -> Option<&'static Role> {
    ROLES.iter().find(|role| role.key == key)
}

/// Names of all color roles, for error messages.
pub fn role_keys() -> Vec<&'static str> {
    ROLES.iter().map(|role| role.key).collect()
}

/// Check that every override names a known role and holds a valid color.
pub fn validate_overrides(overrides: &BTreeMap<String, String>) -> Result<()> {
    for (key, value) in overrides {
        if role_for_key(key).is_none() {
            bail!(
                "unknown color '{key}' in config; valid keys: {}",
                role_keys().join(", ")
            );
        }
        OpalineColor::from_hex(value).map_err(|err| {
            anyhow!("invalid color for '{key}': {value} ({err})")
        })?;
    }
    Ok(())
}

/// Load a theme by built-in name or by path, apply color overrides, and
/// derive the palette.
pub fn load(
    name: &str,
    overrides: &BTreeMap<String, String>,
) -> Result<(Theme, Palette)> {
    validate_overrides(overrides)?;

    let mut theme = if name.ends_with(".toml") || name.contains('/') {
        opaline::load_from_file(name)
            .with_context(|| format!("cannot load theme file {name}"))?
    } else {
        builtins::load_by_name(name).ok_or_else(|| {
            anyhow!("unknown theme '{name}'; use --list-themes to see the built-in themes")
        })?
    };

    for (key, value) in overrides {
        let role = role_for_key(key).expect("validated above");
        let color = OpalineColor::from_hex(value).map_err(|err| {
            anyhow!("invalid color for '{key}': {value} ({err})")
        })?;
        theme.register_token(role.token, color);
    }

    let color = |role: &Role| -> Color {
        let value = theme
            .try_color(role.token)
            .or_else(|| theme.try_color(role.fallback))
            .unwrap_or(OpalineColor::new(255, 255, 255));
        Color::from(value)
    };

    let mut palette = Palette {
        text: color(&ROLES[0]),
        background: color(&ROLES[1]),
        heading: color(&ROLES[2]),
        accent: color(&ROLES[3]),
        muted: color(&ROLES[4]),
        search: color(&ROLES[5]),
        search_text: color(&ROLES[6]),
        status_bg: color(&ROLES[7]),
        status_fg: color(&ROLES[8]),
        border: color(&ROLES[9]),
        selection: color(&ROLES[10]),
        markers: [Color::Reset; 5],
    };
    palette.markers = marker_colors(&theme, &palette);

    // A faint theme panel can look like body text. Derive a strip that always
    // differs from the page unless the user names a status color.
    if !overrides.contains_key("status_bg") {
        palette.status_bg = blend(palette.background, palette.text, 0.16);
    }

    Ok((theme, palette))
}

fn blend(base: Color, toward: Color, amount: f32) -> Color {
    let (
        Color::Rgb(base_r, base_g, base_b),
        Color::Rgb(other_r, other_g, other_b),
    ) = (base, toward)
    else {
        return base;
    };
    let mix = |from: u8, to: u8| -> u8 {
        (f32::from(from) + (f32::from(to) - f32::from(from)) * amount).round()
            as u8
    };
    Color::Rgb(
        mix(base_r, other_r),
        mix(base_g, other_g),
        mix(base_b, other_b),
    )
}

/// Theme tokens tried, in order, for the five indicator colors.
const MARKER_TOKENS: &[&str] = &[
    "accent.primary",
    "accent.secondary",
    "accent.tertiary",
    "accent.deep",
    "success",
    "warning",
    "error",
    "info",
    "text.primary",
    "text.muted",
];

/// Five indicator colors from the theme's accent roles, ordered so that
/// neighboring bars are as far apart as the palette allows. This keeps the
/// cycle readable even when a terminal quantizes RGB to a small palette.
fn marker_colors(theme: &Theme, palette: &Palette) -> [Color; 5] {
    /// Squared RGB distance from the page color a bar needs to stay visible.
    const MIN_CONTRAST: u32 = 15000;

    let mut themed: Vec<Color> = Vec::with_capacity(16);
    let mut shades: Vec<Color> = Vec::with_capacity(20);
    let mut all: Vec<Color> = Vec::with_capacity(40);
    let mut add = |color: Color| {
        if color == palette.background || all.contains(&color) {
            return;
        }
        all.push(color);
        if color_distance(color, palette.background) >= MIN_CONTRAST {
            themed.push(color);
        }
    };
    for token in MARKER_TOKENS {
        if let Some(color) = theme.try_color(token) {
            add(Color::from(color));
        }
    }
    for color in [
        palette.heading,
        palette.accent,
        palette.search,
        palette.text,
        palette.muted,
    ] {
        add(color);
    }
    for step in 1..=20 {
        let color = blend(
            palette.background,
            palette.text,
            step as f32 / 20.0,
        );
        if color != palette.background
            && !all.contains(&color)
            && !shades.contains(&color)
        {
            shades.push(color);
            all.push(color);
        }
    }

    // Theme colors first. Shades of the text color are only needed when a
    // theme defines fewer than five usable accent colors.
    let mut candidates = themed;
    if candidates.len() < 5 {
        for color in &shades {
            if !candidates.contains(color) {
                candidates.push(*color);
            }
        }
    }
    if candidates.is_empty() {
        candidates.push(palette.text);
    }

    // Greedy farthest-point selection: each new color is the candidate with
    // the largest distance to every color already picked.
    let mut colors: Vec<Color> = Vec::with_capacity(5);
    colors.push(
        candidates
            .iter()
            .copied()
            .find(|color| *color == palette.heading)
            .unwrap_or(candidates[0]),
    );
    while colors.len() < 5 {
        let next = candidates
            .iter()
            .copied()
            .filter(|color| !colors.contains(color))
            .max_by_key(|color| {
                colors
                    .iter()
                    .map(|chosen| color_distance(*color, *chosen))
                    .min()
                    .unwrap_or(0)
            });
        match next {
            Some(color) => colors.push(color),
            None => break,
        }
    }
    while colors.len() < 5 {
        colors.push(palette.text);
    }
    colors.try_into().expect("filled to five")
}

/// Squared RGB distance, used to spread the indicator colors.
fn color_distance(left: Color, right: Color) -> u32 {
    let (Color::Rgb(lr, lg, lb), Color::Rgb(rr, rg, rb)) = (left, right) else {
        return 0;
    };
    let diff = |a: u8, b: u8| u32::from(a.abs_diff(b)).pow(2);
    diff(lr, rr) + diff(lg, rg) + diff(lb, rb)
}

impl Palette {
    /// Base style: text color on the reader background.
    pub fn base(&self) -> Style {
        Style::default().fg(self.text).bg(self.background)
    }

    /// Style for headings.
    pub fn heading(&self) -> Style {
        self.base().fg(self.heading).add_modifier(Modifier::BOLD)
    }

    /// Style for secondary text.
    pub fn muted(&self) -> Style {
        self.base().fg(self.muted)
    }

    /// Style for emphasized text.
    pub fn accent(&self) -> Style {
        self.base().fg(self.accent)
    }

    /// Style for inline code.
    pub fn code(&self) -> Style {
        self.base().fg(self.accent)
    }

    /// Style for a search match.
    pub fn highlight(&self) -> Style {
        Style::default()
            .fg(self.search_text)
            .bg(self.search)
            .add_modifier(Modifier::BOLD)
    }

    /// Style for the header and footer bars.
    pub fn status(&self) -> Style {
        Style::default().fg(self.status_fg).bg(self.status_bg)
    }

    /// Style for borders.
    pub fn border_style(&self) -> Style {
        Style::default().fg(self.border).bg(self.background)
    }

    /// Style for the selected item in a list.
    pub fn selected(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .bg(self.selection)
            .add_modifier(Modifier::BOLD)
    }
}

/// All built-in theme identifiers and display names.
pub fn builtin_names() -> &'static [(&'static str, &'static str)] {
    builtins::builtin_names()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_default_theme() {
        let (_theme, palette) =
            load("silkcircuit-neon", &BTreeMap::new()).unwrap();
        assert_ne!(palette.text, palette.background);
    }

    #[test]
    fn applies_color_override() {
        let mut overrides = BTreeMap::new();
        overrides.insert("text".to_string(), "#010203".to_string());
        let (_theme, palette) = load("catppuccin-mocha", &overrides).unwrap();
        assert_eq!(palette.text, Color::Rgb(1, 2, 3));
    }

    fn rgb(color: Color) -> (u8, u8, u8) {
        match color {
            Color::Rgb(r, g, b) => (r, g, b),
            other => panic!("expected an RGB color, got {other:?}"),
        }
    }

    fn distance(left: Color, right: Color) -> u32 {
        let (lr, lg, lb) = rgb(left);
        let (rr, rg, rb) = rgb(right);
        u32::from(lr.abs_diff(rr))
            + u32::from(lg.abs_diff(rg))
            + u32::from(lb.abs_diff(rb))
    }

    #[test]
    fn status_background_differs_from_the_page() {
        for &(id, _) in builtin_names() {
            let (_theme, palette) = load(id, &BTreeMap::new()).unwrap();
            let distance = distance(palette.status_bg, palette.background);
            assert!(distance > 30, "{id} has too little contrast: {distance}");
        }
    }

    #[test]
    fn status_background_override_is_kept() {
        let mut overrides = BTreeMap::new();
        overrides.insert("status_bg".to_string(), "#010203".to_string());
        let (_theme, palette) = load("nord", &overrides).unwrap();
        assert_eq!(palette.status_bg, Color::Rgb(1, 2, 3));
    }

    #[test]
    fn marker_colors_are_five_distinct_theme_colors() {
        for &(id, _) in builtin_names() {
            let (_theme, palette) = load(id, &BTreeMap::new()).unwrap();
            for (index, color) in palette.markers.iter().enumerate() {
                assert_ne!(
                    *color, palette.background,
                    "{id} marker {index} is the page color"
                );
                assert!(
                    !palette.markers[..index].contains(color),
                    "{id} repeats marker color {index}"
                );
            }
        }
    }

    #[test]
    fn marker_neighbors_are_far_apart() {
        for &(id, _) in builtin_names() {
            let (_theme, palette) = load(id, &BTreeMap::new()).unwrap();
            for index in 0..5 {
                let next = (index + 1) % 5;
                let distance = color_distance(
                    palette.markers[index],
                    palette.markers[next],
                );
                assert!(
                    distance >= 2500,
                    "{id}: markers {index} and {next} are too close \
                     ({distance})"
                );
            }
        }
    }

    #[test]
    fn rejects_unknown_theme() {
        assert!(load("does-not-exist", &BTreeMap::new()).is_err());
    }

    #[test]
    fn rejects_bad_color() {
        let mut overrides = BTreeMap::new();
        overrides.insert("text".to_string(), "red".to_string());
        assert!(load("nord", &overrides).is_err());
    }
}
