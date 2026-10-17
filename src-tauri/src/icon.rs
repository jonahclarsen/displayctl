//! Front-facing laptop icons: lit screen, waiting dots, or a crossed-out screen.

use crate::State;

/// Points per side of the menu bar icon; rendered at 2x for Retina.
const POINTS: f32 = 18.0;
const SCALE: usize = 2;
pub const SIZE: usize = POINTS as usize * SCALE;

type Segment = ((f32, f32), (f32, f32), f32);

fn segments(state: State) -> Vec<Segment> {
    let mut lines = vec![
        ((4.0, 3.0), (14.0, 3.0), 1.3),
        ((4.0, 3.0), (4.0, 12.0), 1.3),
        ((14.0, 3.0), (14.0, 12.0), 1.3),
        ((4.0, 12.0), (14.0, 12.0), 1.3),
        ((4.0, 12.0), (2.0, 15.0), 1.3),
        ((14.0, 12.0), (16.0, 15.0), 1.3),
        ((2.0, 15.0), (16.0, 15.0), 1.3),
    ];
    match state {
        State::On => lines.extend([
            ((6.5, 6.0), (11.5, 6.0), 1.3),
            ((6.5, 9.0), (11.5, 9.0), 1.3),
        ]),
        State::Armed => lines.extend([
            ((6.5, 7.5), (6.55, 7.5), 1.4),
            ((9.0, 7.5), (9.05, 7.5), 1.4),
            ((11.5, 7.5), (11.55, 7.5), 1.4),
        ]),
        State::Off => lines.extend([
            ((7.0, 5.5), (11.0, 9.5), 1.3),
            ((11.0, 5.5), (7.0, 9.5), 1.3),
        ]),
    }
    lines
}

fn distance(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    ((p.0 - a.0 - t * dx).powi(2) + (p.1 - a.1 - t * dy).powi(2)).sqrt()
}

/// Black RGBA pixels with antialiased alpha, for use as a template image.
pub fn rgba(state: State) -> Vec<u8> {
    render(state, SCALE)
}

fn render(state: State, scale: usize) -> Vec<u8> {
    let size = POINTS as usize * scale;
    let lines = segments(state);
    let pixel = 1.0 / scale as f32;
    let mut out = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        for x in 0..size {
            let p = ((x as f32 + 0.5) * pixel, (y as f32 + 0.5) * pixel);
            let coverage = lines
                .iter()
                .map(|&(a, b, width)| ((width / 2.0 - distance(p, a, b)) / pixel + 0.5).clamp(0.0, 1.0))
                .fold(0.0, f32::max);
            out.extend([0, 0, 0, (coverage * 255.0).round() as u8]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ICON_PREVIEW_DIR=/some/dir cargo test icon` writes the icons as PNGs to look at.
    #[test]
    fn icon_previews() {
        let Some(dir) = std::env::var_os("ICON_PREVIEW_DIR") else { return };
        let dir = std::path::PathBuf::from(dir);
        for (state, name) in [(State::On, "on"), (State::Armed, "armed"), (State::Off, "off")] {
            for scale in [SCALE, 16] {
                let size = POINTS as u32 * scale as u32;
                let file = std::fs::File::create(dir.join(format!("{name}@{scale}x.png"))).unwrap();
                let mut encoder = png::Encoder::new(file, size, size);
                encoder.set_color(png::ColorType::Rgba);
                encoder.write_header().unwrap().write_image_data(&render(state, scale)).unwrap();
            }
        }
    }
}
