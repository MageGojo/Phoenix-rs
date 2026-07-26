//! Pure-Rust SVG rendering for captcha challenges (no image or C dependencies).

use std::fmt::Write as _;

use rand::RngExt;

use crate::CaptchaConfig;

const GLYPH_COLORS: &[&str] = &[
    "#1f2937", "#374151", "#4338ca", "#0f766e", "#9d174d", "#92400e", "#1d4ed8",
];
const NOISE_COLORS: &[&str] = &["#94a3b8", "#a8a29e", "#c084fc", "#7dd3fc", "#f9a8d4"];

/// Render one challenge as a self-contained SVG document.
///
/// Every answer character becomes its own `<text>` glyph with random rotation,
/// translation, and font-size jitter, so the full answer never appears as one
/// contiguous string in the markup. The charset is validated to be ASCII
/// alphanumeric before rendering, so no XML escaping is required.
pub(crate) fn render(config: &CaptchaConfig, answer: &str) -> String {
    let mut rng = rand::rng();
    let width = f64::from(config.width);
    let height = f64::from(config.height);
    let mut svg = String::with_capacity(2048);
    let _ = write!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
         viewBox=\"0 0 {w} {h}\" role=\"img\" aria-label=\"captcha\">",
        w = config.width,
        h = config.height,
    );
    let _ = write!(
        svg,
        "<rect width=\"{w}\" height=\"{h}\" fill=\"#f5f7fa\"/>",
        w = config.width,
        h = config.height,
    );

    for _ in 0..config.noise_curves {
        let start_y = rng.random_range(height * 0.15..=height * 0.85);
        let control_one = rng.random_range(-height * 0.4..=height * 1.4);
        let control_two = rng.random_range(-height * 0.4..=height * 1.4);
        let end_y = rng.random_range(height * 0.15..=height * 0.85);
        let stroke = NOISE_COLORS[rng.random_range(0..NOISE_COLORS.len())];
        let stroke_width = rng.random_range(1.0..=1.8);
        let _ = write!(
            svg,
            "<path d=\"M0 {start_y:.1} C {x1:.1} {control_one:.1}, {x2:.1} {control_two:.1}, \
             {width:.1} {end_y:.1}\" stroke=\"{stroke}\" stroke-width=\"{stroke_width:.1}\" \
             fill=\"none\" opacity=\"0.7\"/>",
            x1 = width * 0.33,
            x2 = width * 0.66,
        );
    }

    let count = f64::from(config.length.max(1));
    let margin = width * 0.08;
    let step = (width - 2.0 * margin) / count;
    let mut anchor = margin + step * 0.5;
    for glyph in answer.chars() {
        let x = anchor + rng.random_range(-step * 0.12..=step * 0.12);
        let y = height * 0.68 + rng.random_range(-height * 0.10..=height * 0.10);
        let font_size = height * 0.52 * rng.random_range(0.85..=1.2);
        let rotation = rng.random_range(-24.0..=24.0);
        let fill = GLYPH_COLORS[rng.random_range(0..GLYPH_COLORS.len())];
        let _ = write!(
            svg,
            "<text x=\"{x:.1}\" y=\"{y:.1}\" font-family=\"monospace\" font-weight=\"bold\" \
             font-size=\"{font_size:.1}\" fill=\"{fill}\" text-anchor=\"middle\" \
             transform=\"rotate({rotation:.1} {x:.1} {y:.1})\">{glyph}</text>",
        );
        anchor += step;
    }

    for _ in 0..config.noise_dots {
        let x = rng.random_range(0.0..=width);
        let y = rng.random_range(0.0..=height);
        let radius = rng.random_range(0.8..=1.8);
        let fill = NOISE_COLORS[rng.random_range(0..NOISE_COLORS.len())];
        let _ = write!(
            svg,
            "<circle cx=\"{x:.1}\" cy=\"{y:.1}\" r=\"{radius:.1}\" fill=\"{fill}\" \
             opacity=\"0.55\"/>",
        );
    }

    svg.push_str("</svg>");
    svg
}
