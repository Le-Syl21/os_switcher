/// Draws the mark at `size`×`size`, as top-down RGBA8 rows.
pub fn rgba(size: u32) -> Vec<u8> {
    /// Subsamples per axis. Three is enough to hide the staircase at 16 px.
    const SS: u32 = 3;
    /// The plate: a dark, slightly blue slate.
    const PLATE: [f32; 3] = [22.0, 30.0, 44.0];
    const GLYPH: [f32; 3] = [255.0, 255.0, 255.0];

    let s = size as f32;
    let mut out = vec![0u8; (size * size * 4) as usize];
    let samples = (SS * SS) as f32;

    for y in 0..size {
        for x in 0..size {
            let (mut plate, mut glyph) = (0u32, 0u32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let fx = x as f32 + (sx as f32 + 0.5) / SS as f32;
                    let fy = y as f32 + (sy as f32 + 0.5) / SS as f32;
                    if in_plate(fx, fy, s) {
                        plate += 1;
                        if in_glyph(fx, fy, s) {
                            glyph += 1;
                        }
                    }
                }
            }
            if plate == 0 {
                continue;
            }

            let alpha = plate as f32 / samples;
            let ink = glyph as f32 / plate as f32;
            let i = ((y * size + x) * 4) as usize;
            for channel in 0..3 {
                let value = PLATE[channel] + (GLYPH[channel] - PLATE[channel]) * ink;
                out[i + channel] = value.round() as u8;
            }
            out[i + 3] = (alpha * 255.0).round() as u8;
        }
    }
    out
}

/// The rounded square the glyph sits on.
fn in_plate(x: f32, y: f32, s: f32) -> bool {
    let (dx, dy) = ((x - s / 2.0).abs(), (y - s / 2.0).abs());
    let half = s * 0.47;
    let radius = s * 0.14;
    let flat = half - radius;
    if dx > half || dy > half {
        return false;
    }
    // Inside the cross of the rounded square, or within a corner's arc.
    let (ox, oy) = ((dx - flat).max(0.0), (dy - flat).max(0.0));
    ox * ox + oy * oy <= radius * radius
}

/// The power symbol: a ring open at the top, plus the stem that closes it.
fn in_glyph(x: f32, y: f32, s: f32) -> bool {
    let (dx, dy) = (x - s / 2.0, y - s / 2.0);
    let stroke = s * 0.055;

    let radius = (dx * dx + dy * dy).sqrt();
    let gap = dy < -s * 0.12 && dx.abs() < stroke * 2.0;
    let ring = (radius - s * 0.26).abs() <= stroke && !gap;

    let stem = dx.abs() <= stroke && (-s * 0.33..=-s * 0.04).contains(&dy);

    ring || stem
}
