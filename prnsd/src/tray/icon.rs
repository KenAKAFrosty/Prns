const FAVICON_BACKGROUND: Rgba = Rgba::opaque(0x0b, 0x0e, 0x13);
const MARK: Rgba = Rgba::opaque(0x6e, 0xe7, 0xb7);
const CENTER: Rgba = Rgba::opaque(0x34, 0xd3, 0x99);
const SUPERSAMPLING: u32 = 4;

pub(super) struct TrayIcon {
    pub(super) rgba: Vec<u8>,
    pub(super) size: u32,
}

pub(super) fn render(size: u32) -> TrayIcon {
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for pixel_y in 0..size {
        for pixel_x in 0..size {
            let mut premultiplied = [0.0; 3];
            let mut alpha = 0.0;
            for sample_y in 0..SUPERSAMPLING {
                for sample_x in 0..SUPERSAMPLING {
                    let x = sample_coordinate(pixel_x, sample_x, size);
                    let y = sample_coordinate(pixel_y, sample_y, size);
                    let sample = favicon_sample(x, y);
                    let sample_alpha = f32::from(sample.alpha) / 255.0;
                    premultiplied[0] += f32::from(sample.red) * sample_alpha;
                    premultiplied[1] += f32::from(sample.green) * sample_alpha;
                    premultiplied[2] += f32::from(sample.blue) * sample_alpha;
                    alpha += sample_alpha;
                }
            }
            let samples = (SUPERSAMPLING * SUPERSAMPLING) as f32;
            if alpha == 0.0 {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            rgba.extend_from_slice(&[
                channel(premultiplied[0] / alpha),
                channel(premultiplied[1] / alpha),
                channel(premultiplied[2] / alpha),
                channel(alpha / samples * 255.0),
            ]);
        }
    }
    TrayIcon { rgba, size }
}

fn sample_coordinate(pixel: u32, sample: u32, size: u32) -> f32 {
    (pixel as f32 + (sample as f32 + 0.5) / SUPERSAMPLING as f32) * 100.0 / size as f32
}

fn channel(value: f32) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

fn favicon_sample(x: f32, y: f32) -> Rgba {
    if !inside_rounded_square(x, y) {
        return Rgba::TRANSPARENT;
    }

    let mut color = FAVICON_BACKGROUND;
    let mark_x = (x - 13.0) / 0.74;
    let mark_y = (y - 13.0) / 0.74;
    let dx = mark_x - 50.0;
    let dy = mark_y - 50.0;
    let radius = dx.hypot(dy);

    if (radius - 37.0).abs() <= 1.7 || on_cardinal_tick(mark_x, mark_y) {
        color = MARK;
    }
    if on_radio_arc(dx, dy, 13.0, 1.5) {
        color = MARK.over(color, 0.9);
    }
    if on_radio_arc(dx, dy, 21.0, 1.5) {
        color = MARK.over(color, 0.45);
    }
    if radius <= 6.5 {
        color = CENTER;
    }
    color
}

fn inside_rounded_square(x: f32, y: f32) -> bool {
    let nearest_x = x.clamp(22.0, 78.0);
    let nearest_y = y.clamp(22.0, 78.0);
    (x - nearest_x).hypot(y - nearest_y) <= 22.0
}

fn on_cardinal_tick(x: f32, y: f32) -> bool {
    let radians = -46.0_f32.to_radians();
    let dx = x - 50.0;
    let dy = y - 50.0;
    let unrotated_x = 50.0 + dx * radians.cos() - dy * radians.sin();
    let unrotated_y = 50.0 + dx * radians.sin() + dy * radians.cos();
    const TICKS: [((f32, f32), (f32, f32)); 4] = [
        ((50.0, 7.0), (50.0, 16.0)),
        ((50.0, 84.0), (50.0, 93.0)),
        ((7.0, 50.0), (16.0, 50.0)),
        ((84.0, 50.0), (93.0, 50.0)),
    ];
    TICKS
        .iter()
        .any(|&(start, end)| distance_to_segment((unrotated_x, unrotated_y), start, end) <= 1.7)
}

fn on_radio_arc(dx: f32, dy: f32, radius: f32, half_width: f32) -> bool {
    let angle = dy.atan2(dx).to_degrees();
    (dx.hypot(dy) - radius).abs() <= half_width && (angle.abs() <= 55.0 || angle.abs() >= 125.0)
}

fn distance_to_segment(point: (f32, f32), start: (f32, f32), end: (f32, f32)) -> f32 {
    let segment = (end.0 - start.0, end.1 - start.1);
    let length_squared = segment.0 * segment.0 + segment.1 * segment.1;
    let projection =
        ((point.0 - start.0) * segment.0 + (point.1 - start.1) * segment.1) / length_squared;
    let projection = projection.clamp(0.0, 1.0);
    let nearest = (
        start.0 + projection * segment.0,
        start.1 + projection * segment.1,
    );
    (point.0 - nearest.0).hypot(point.1 - nearest.1)
}

#[derive(Clone, Copy)]
struct Rgba {
    red: u8,
    green: u8,
    blue: u8,
    alpha: u8,
}

impl Rgba {
    const TRANSPARENT: Self = Self {
        red: 0,
        green: 0,
        blue: 0,
        alpha: 0,
    };

    const fn opaque(red: u8, green: u8, blue: u8) -> Self {
        Self {
            red,
            green,
            blue,
            alpha: 255,
        }
    }

    fn over(self, background: Self, opacity: f32) -> Self {
        let inverse = 1.0 - opacity;
        Self::opaque(
            channel(f32::from(self.red) * opacity + f32::from(background.red) * inverse),
            channel(f32::from(self.green) * opacity + f32::from(background.green) * inverse),
            channel(f32::from(self.blue) * opacity + f32::from(background.blue) * inverse),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(icon: &TrayIcon, x: u32, y: u32) -> [u8; 4] {
        let start = ((y * icon.size + x) * 4) as usize;
        icon.rgba[start..start + 4].try_into().unwrap()
    }

    #[test]
    fn renders_the_favicon_geometry_at_tray_sizes() {
        for size in [32, 64] {
            let icon = render(size);
            assert_eq!(icon.rgba.len(), (size * size * 4) as usize);
            assert_eq!(pixel(&icon, 0, 0), [0, 0, 0, 0]);
            assert_eq!(
                pixel(&icon, size / 2, size / 2),
                [CENTER.red, CENTER.green, CENTER.blue, 255]
            );
            assert_eq!(
                pixel(&icon, size / 2, size / 10),
                [
                    FAVICON_BACKGROUND.red,
                    FAVICON_BACKGROUND.green,
                    FAVICON_BACKGROUND.blue,
                    255,
                ]
            );
        }
    }
}
