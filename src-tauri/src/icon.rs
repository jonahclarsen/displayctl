//! Menu bar icons, drawn at runtime: a laptop seen from the side.
//! Lid open with light coming off the screen = built-in display on.
//! Lid half shut = waiting for an external monitor before turning it off.
//! Lid shut = built-in display off.

use crate::State;

/// Points per side of the menu bar icon; rendered at 2x for Retina.
const POINTS: f32 = 18.0;
const SCALE: usize = 2;
pub const SIZE: usize = POINTS as usize * SCALE;
const HINGE: (f32, f32) = (4.6, 13.4);
const LID: f32 = 10.6;

type Segment = ((f32, f32), (f32, f32), f32);

fn segments(state: State) -> Vec<Segment> {
    let angle: f32 = match state {
        State::On => 100.0,
        State::Armed => 40.0,
        State::Off => 1.5,
    };
    let (sin, cos) = angle.to_radians().sin_cos();
    let mut lines = vec![
        ((3.6, 15.3), (16.2, 15.3), 1.7),
        (HINGE, (HINGE.0 + LID * cos, HINGE.1 - LID * sin), 1.7),
    ];
    if state == State::On {
        lines.extend([
            ((7.3, 9.5), (10.3, 10.6), 1.3),
            ((7.4, 7.5), (10.6, 7.0), 1.3),
            ((6.6, 5.8), (9.1, 3.7), 1.3),
        ]);
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
