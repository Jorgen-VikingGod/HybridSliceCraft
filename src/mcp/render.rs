//! Pictures for agents: one layer of a slice as a PNG.

use crate::fdm::{FdmResult, Feature};
use crate::resin::ResinResult;

pub fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

fn colour(f: u32) -> [u8; 3] {
    match f {
        0 => [255, 200, 60],
        1 => [235, 130, 70],
        2 => [70, 100, 240],
        3 => [240, 240, 240],
        4 => [190, 70, 130],
        5 => [160, 80, 210],
        6 => [235, 70, 70],
        7 => [70, 190, 130],
        8 => [100, 150, 240],
        9 => [230, 230, 120],
        10 => [120, 200, 200],
        11 => [160, 220, 230],
        12 => [80, 190, 190],
        13 => [210, 160, 90],
        14 => [150, 150, 150],
        _ => [100, 100, 100],
    }
}

/// One layer of a filament print seen from above, on the bed, with the layer
/// below faintly behind it. Lines are drawn as wide as they print.
pub fn fdm_layer(res: &FdmResult, layer: usize, bed: (f32, f32), width_px: u32) -> Result<Vec<u8>, String> {
    let info = res.layers.get(layer).ok_or_else(|| format!("the print has {} layers, numbered from 0", res.layers.len()))?;
    let travel = Feature::Travel as u32;
    let here = &res.segs[info.first_seg as usize..(info.first_seg + info.seg_count) as usize];
    // Zoom in on what is printed on this layer, with a margin; the whole bed when it is empty.
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for s in here.iter().filter(|s| s.feature != travel) {
        for p in [s.a, s.b] {
            lo = [lo[0].min(p[0]), lo[1].min(p[1])];
            hi = [hi[0].max(p[0]), hi[1].max(p[1])];
        }
    }
    let (origin, span) = if lo[0] <= hi[0] {
        let margin = 3.0 + 0.04 * (hi[0] - lo[0]).max(hi[1] - lo[1]);
        ([lo[0] - margin, lo[1] - margin], [(hi[0] - lo[0] + 2.0 * margin).max(20.0), (hi[1] - lo[1] + 2.0 * margin).max(20.0)])
    } else {
        ([0.0, 0.0], [bed.0.max(1.0), bed.1.max(1.0)])
    };
    let scale = width_px as f32 / span[0];
    let (w, h) = (width_px, ((span[1] * scale).ceil() as u32).clamp(1, 4000));
    let mut px = vec![0u8; (w * h * 3) as usize];
    for p in px.chunks_mut(3) {
        p.copy_from_slice(&[18, 20, 26]);
    }
    let mut draw = |a: [f32; 3], b: [f32; 3], width: f32, rgb: [u8; 3]| {
        let (ax, ay, bx, by) = ((a[0] - origin[0]) * scale, (span[1] - (a[1] - origin[1])) * scale, (b[0] - origin[0]) * scale, (span[1] - (b[1] - origin[1])) * scale);
        let r = (width * scale * 0.5).max(0.6);
        let steps = ((bx - ax).abs().max((by - ay).abs()) * 2.0).ceil().max(1.0) as i32;
        for s in 0..=steps {
            let t = s as f32 / steps as f32;
            let (cx, cy) = (ax + (bx - ax) * t, ay + (by - ay) * t);
            let (x0, x1) = ((cx - r).floor().max(0.0) as i32, (cx + r).ceil().min(w as f32 - 1.0) as i32);
            let (y0, y1) = ((cy - r).floor().max(0.0) as i32, (cy + r).ceil().min(h as f32 - 1.0) as i32);
            for y in y0..=y1 {
                for x in x0..=x1 {
                    if (x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2) <= r * r {
                        let i = ((y as u32 * w + x as u32) * 3) as usize;
                        px[i..i + 3].copy_from_slice(&rgb);
                    }
                }
            }
        }
    };
    if layer > 0 {
        let below = &res.layers[layer - 1];
        for s in &res.segs[below.first_seg as usize..(below.first_seg + below.seg_count) as usize] {
            if s.feature != travel {
                draw(s.a, s.b, s.width, [48, 52, 62]);
            }
        }
    }
    for s in here {
        if s.feature != travel {
            draw(s.a, s.b, s.width, colour(s.feature));
        }
    }
    crate::resin::export::encode_png(w, h, &px, png::ColorType::Rgb)
}

/// One layer of a resin print as a grey picture, shrunk to at most `max_px` wide.
pub fn resin_layer(res: &ResinResult, layer: usize, max_px: u32) -> Result<Vec<u8>, String> {
    let spec = res.spec.ok_or("the slice has no layer pictures")?;
    let img = res.layers.get(layer).ok_or_else(|| format!("the print has {} layers, numbered from 0", res.layers.len()))?;
    let (w, h) = (spec.width as usize, spec.height as usize);
    let mut full = vec![0u8; w * h];
    img.decode(&mut full);
    let k = (w as u32).div_ceil(max_px.max(1)).max(1) as usize;
    let (ow, oh) = (w / k, h / k);
    let mut out = vec![0u8; ow * oh];
    for y in 0..oh {
        for x in 0..ow {
            let mut sum = 0u32;
            for dy in 0..k {
                let row = (y * k + dy) * w + x * k;
                sum += full[row..row + k].iter().map(|v| *v as u32).sum::<u32>();
            }
            out[y * ow + x] = (sum / (k * k) as u32) as u8;
        }
    }
    crate::resin::export::encode_png(ow as u32, oh as u32, &out, png::ColorType::Grayscale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_standard_examples() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
