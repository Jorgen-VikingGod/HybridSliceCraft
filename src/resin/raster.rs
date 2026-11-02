//! Layer pictures. Layers are kept run-length compressed so even very high
//! resolution screens with thousands of layers fit comfortably in memory.

/// How millimetres map to screen pixels. Row 0 is the back of the plate.
#[derive(Clone, Copy, Debug)]
pub struct RasterSpec {
    pub width: u32,
    pub height: u32,
    pub size_x: f32,
    pub size_y: f32,
    pub samples: u32,
    pub mirror_x: bool,
    pub mirror_y: bool,
}

impl RasterSpec {
    #[inline]
    pub fn to_px(&self, x: f32, y: f32) -> (f32, f32) {
        let mut px = x * self.width as f32 / self.size_x;
        let mut py = (self.size_y - y) * self.height as f32 / self.size_y;
        if self.mirror_x {
            px = self.width as f32 - px;
        }
        if self.mirror_y {
            py = self.height as f32 - py;
        }
        (px, py)
    }

    pub fn pixel_area_mm2(&self) -> f64 {
        (self.size_x as f64 / self.width as f64) * (self.size_y as f64 / self.height as f64)
    }
}

/// Run-length layer: pairs of (grey value, run length as a little varint).
#[derive(Clone, Debug, Default)]
pub struct LayerImage {
    pub data: Vec<u8>,
    /// Sum of pixel brightness divided by 255: lit pixels, with partial credit.
    pub lit: f64,
    /// Lit bounding box: x0, y0, x1, y1 (exclusive).
    pub bbox: [u32; 4],
}

#[derive(Default)]
pub struct RleWriter {
    data: Vec<u8>,
    value: u8,
    run: u64,
    lit: f64,
    x: u32,
    y: u32,
    width: u32,
    min_x: u32,
    min_y: u32,
    max_x: u32,
    max_y: u32,
}

impl RleWriter {
    pub fn new(width: u32) -> Self {
        Self {
            data: Vec::with_capacity(1024),
            width,
            min_x: u32::MAX,
            min_y: u32::MAX,
            ..Default::default()
        }
    }

    fn flush(&mut self) {
        if self.run == 0 {
            return;
        }
        self.data.push(self.value);
        let mut r = self.run;
        loop {
            let b = (r & 0x7f) as u8;
            r >>= 7;
            if r == 0 {
                self.data.push(b);
                break;
            }
            self.data.push(b | 0x80);
        }
        self.run = 0;
    }

    #[inline]
    pub fn push_run(&mut self, value: u8, count: u64) {
        if count == 0 {
            return;
        }
        if value != 0 {
            self.lit += value as f64 / 255.0 * count as f64;
            // Runs of lit pixels are short enough to walk row by row.
            let w = self.width as u64;
            let start = self.y as u64 * w + self.x as u64;
            let end = start + count;
            let (y0, y1) = ((start / w) as u32, ((end - 1) / w) as u32);
            self.min_y = self.min_y.min(y0);
            self.max_y = self.max_y.max(y1 + 1);
            if y0 == y1 {
                self.min_x = self.min_x.min((start % w) as u32);
                self.max_x = self.max_x.max(((end - 1) % w) as u32 + 1);
            } else {
                self.min_x = 0;
                self.max_x = self.width;
            }
        }
        let pos = self.y as u64 * self.width as u64 + self.x as u64 + count;
        self.y = (pos / self.width as u64) as u32;
        self.x = (pos % self.width as u64) as u32;
        if value == self.value {
            self.run += count;
        } else {
            self.flush();
            self.value = value;
            self.run = count;
        }
    }

    pub fn finish(mut self) -> LayerImage {
        self.flush();
        self.data.shrink_to_fit();
        let bbox = if self.min_x == u32::MAX {
            [0, 0, 0, 0]
        } else {
            [self.min_x, self.min_y, self.max_x, self.max_y]
        };
        LayerImage {
            data: self.data,
            lit: self.lit,
            bbox,
        }
    }
}

impl LayerImage {
    /// Walk every run of the picture in reading order.
    pub fn runs(&self) -> RunIter<'_> {
        RunIter { data: &self.data, pos: 0 }
    }

    pub fn from_rows(width: u32, pixels: &[u8]) -> Self {
        let mut w = RleWriter::new(width);
        encode_into(&mut w, pixels);
        w.finish()
    }

    /// Unpack into a full picture.
    pub fn decode(&self, out: &mut [u8]) {
        let mut pos = 0usize;
        for (v, n) in self.runs() {
            let n = n as usize;
            let end = (pos + n).min(out.len());
            if pos >= out.len() {
                break;
            }
            out[pos..end].fill(v);
            pos = end;
        }
        if pos < out.len() {
            out[pos..].fill(0);
        }
    }

    /// Lit spans per row, treating partly covered pixels above half as lit.
    pub fn spans(&self, width: u32, height: u32) -> Vec<Vec<(u32, u32)>> {
        let mut rows: Vec<Vec<(u32, u32)>> = vec![Vec::new(); height as usize];
        let w = width as u64;
        let mut pos = 0u64;
        for (v, n) in self.runs() {
            if v >= 128 {
                let mut s = pos;
                let end = pos + n;
                while s < end {
                    let row = s / w;
                    if row >= height as u64 {
                        break;
                    }
                    let col = s % w;
                    let take = (w - col).min(end - s);
                    let spans = &mut rows[row as usize];
                    let x0 = col as u32;
                    let x1 = (col + take) as u32;
                    match spans.last_mut() {
                        Some(last) if last.1 == x0 => last.1 = x1,
                        _ => spans.push((x0, x1)),
                    }
                    s += take;
                }
            }
            pos += n;
        }
        rows
    }
}

pub struct RunIter<'a> {
    data: &'a [u8],
    pos: usize,
}

impl Iterator for RunIter<'_> {
    type Item = (u8, u64);
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let v = *self.data.get(self.pos)?;
        self.pos += 1;
        let mut r = 0u64;
        let mut shift = 0;
        loop {
            let b = *self.data.get(self.pos)?;
            self.pos += 1;
            r |= ((b & 0x7f) as u64) << shift;
            if b & 0x80 == 0 {
                break;
            }
            shift += 7;
        }
        Some((v, r))
    }
}

pub fn encode_into(w: &mut RleWriter, pixels: &[u8]) {
    let mut i = 0;
    let n = pixels.len();
    while i < n {
        let v = pixels[i];
        let mut j = i + 1;
        if v == 0 {
            // Most of a layer is black; scan it in big strides.
            while j + 8 <= n && u64::from_ne_bytes(pixels[j..j + 8].try_into().unwrap()) == 0 {
                j += 8;
            }
        }
        while j < n && pixels[j] == v {
            j += 1;
        }
        w.push_run(v, (j - i) as u64);
        i = j;
    }
}

/// Software rasteriser: winding-number fill with exact horizontal coverage and
/// `samples` scanlines per pixel. `neg` lists holes that always cut away.
pub fn raster_cpu(segs: &[[f32; 4]], neg: &[[f32; 4]], spec: &RasterSpec) -> LayerImage {
    let samples = spec.samples.max(1);
    let (w, h) = (spec.width as usize, spec.height as usize);
    let mut writer = RleWriter::new(spec.width);
    let total = w as u64 * h as u64;
    if segs.is_empty() {
        writer.push_run(0, total);
        return writer.finish();
    }
    // Crossings packed for a cheap sort: row in the high bits.
    let mut crossings: Vec<(u32, f32, i8, bool)> = Vec::with_capacity(segs.len() * 4);
    let collect = |list: &[[f32; 4]], is_neg: bool, out: &mut Vec<(u32, f32, i8, bool)>| {
        for s in list {
            let (x0, y0) = spec.to_px(s[0], s[1]);
            let (x1, y1) = spec.to_px(s[2], s[3]);
            let (y0s, y1s) = (y0 * samples as f32, y1 * samples as f32);
            if y0s == y1s {
                continue;
            }
            let (dir, xa, ya, xb, yb) = if y0s < y1s {
                (1i8, x0, y0s, x1, y1s)
            } else {
                (-1i8, x1, y1s, x0, y0s)
            };
            let first = (ya - 0.5).ceil().max(0.0) as i64;
            let last = ((yb - 0.5).ceil() as i64).min((h * samples as usize) as i64);
            if first >= last {
                continue;
            }
            let slope = (xb - xa) / (yb - ya);
            for row in first..last {
                let x = xa + (row as f32 + 0.5 - ya) * slope;
                out.push((row as u32, x, dir, is_neg));
            }
        }
    };
    collect(segs, false, &mut crossings);
    collect(neg, true, &mut crossings);
    crossings.sort_unstable_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));

    let scale = 255.0 / samples as f32;
    let mut row_acc = vec![0f32; w + 2];
    let mut i = 0usize;
    let n = crossings.len();
    let mut cursor = 0u64;
    while i < n {
        let py = crossings[i].0 / samples;
        let row_end = (py + 1) * samples;
        // Black rows skipped since the last written row.
        let row_start = py as u64 * w as u64;
        if row_start > cursor {
            writer.push_run(0, row_start - cursor);
        }
        let mut lo = usize::MAX;
        let mut hi = 0usize;
        while i < n && crossings[i].0 < row_end {
            let sub = crossings[i].0;
            let mut winding = 0i32;
            let mut neg_winding = 0i32;
            let mut span_start: Option<f32> = None;
            while i < n && crossings[i].0 == sub {
                let (_, x, dir, is_neg) = crossings[i];
                if is_neg {
                    neg_winding += dir as i32;
                } else {
                    winding += dir as i32;
                }
                let filled = winding != 0 && neg_winding == 0;
                match (filled, span_start) {
                    (true, None) => span_start = Some(x),
                    (false, Some(s)) => {
                        add_span(&mut row_acc, s, x, w, &mut lo, &mut hi);
                        span_start = None;
                    }
                    _ => {}
                }
                i += 1;
            }
            if let Some(s) = span_start {
                add_span(&mut row_acc, s, w as f32, w, &mut lo, &mut hi);
            }
        }
        if lo == usize::MAX {
            writer.push_run(0, w as u64);
        } else {
            let lo = lo.min(w);
            let hi = hi.min(w);
            writer.push_run(0, lo as u64);
            let mut acc = 0.0f32;
            let mut x = lo;
            let threshold = spec.samples <= 1;
            while x < hi {
                acc += row_acc[x];
                row_acc[x] = 0.0;
                let v = if threshold {
                    if acc * scale >= 127.5 { 255u8 } else { 0u8 }
                } else {
                    (acc * scale).round().clamp(0.0, 255.0) as u8
                };
                // Stretch fully covered interiors in one go.
                let mut run = 1u64;
                let mut y = x + 1;
                while y < hi && row_acc[y] == 0.0 {
                    run += 1;
                    y += 1;
                }
                writer.push_run(v, run);
                x = y;
            }
            row_acc[hi] = 0.0;
            row_acc[hi + 1] = 0.0;
            writer.push_run(0, (w - hi) as u64);
        }
        cursor = row_start + w as u64;
    }
    if cursor < total {
        writer.push_run(0, total - cursor);
    }
    writer.finish()
}

#[inline]
fn add_span(acc: &mut [f32], x0: f32, x1: f32, w: usize, lo: &mut usize, hi: &mut usize) {
    let (x0, x1) = (x0.clamp(0.0, w as f32), x1.clamp(0.0, w as f32));
    if x1 <= x0 {
        return;
    }
    let i0 = x0 as usize;
    let f0 = x0 - i0 as f32;
    acc[i0] += 1.0 - f0;
    acc[i0 + 1] += f0;
    let i1 = x1 as usize;
    let f1 = x1 - i1 as f32;
    acc[i1] -= 1.0 - f1;
    acc[i1 + 1] -= f1;
    *lo = (*lo).min(i0);
    *hi = (*hi).max(i1 + 2);
}

/// Disconnected parts of a layer that have nothing under them.
#[derive(Clone, Debug, Default)]
pub struct IslandInfo {
    pub bbox: [u32; 4],
    pub area_px: u64,
    /// One pixel known to be inside the island.
    pub seed: (u32, u32),
}

pub fn find_islands(
    current: &[Vec<(u32, u32)>],
    below: Option<&[Vec<(u32, u32)>]>,
) -> Vec<IslandInfo> {
    // Union-find over spans, joining spans that touch across rows.
    let total: usize = current.iter().map(|r| r.len()).sum();
    if total == 0 {
        return Vec::new();
    }
    let mut parent: Vec<u32> = (0..total as u32).collect();
    fn find(p: &mut [u32], mut a: u32) -> u32 {
        while p[a as usize] != a {
            p[a as usize] = p[p[a as usize] as usize];
            a = p[a as usize];
        }
        a
    }
    let mut offsets = Vec::with_capacity(current.len());
    let mut off = 0u32;
    for r in current {
        offsets.push(off);
        off += r.len() as u32;
    }
    for y in 1..current.len() {
        let (a, b) = (&current[y - 1], &current[y]);
        let (mut i, mut j) = (0usize, 0usize);
        while i < a.len() && j < b.len() {
            if a[i].0 < b[j].1 && b[j].0 < a[i].1 {
                let ra = find(&mut parent, offsets[y - 1] + i as u32);
                let rb = find(&mut parent, offsets[y] + j as u32);
                parent[rb as usize] = ra;
            }
            if a[i].1 < b[j].1 {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    let mut supported = vec![false; total];
    let mut info: Vec<Option<IslandInfo>> = vec![None; total];
    for (y, row) in current.iter().enumerate() {
        let lower = below.and_then(|b| b.get(y));
        let mut k = 0usize;
        for (si, s) in row.iter().enumerate() {
            let id = find(&mut parent, offsets[y] + si as u32) as usize;
            let e = info[id].get_or_insert(IslandInfo {
                bbox: [s.0, y as u32, s.1, y as u32 + 1],
                area_px: 0,
                seed: ((s.0 + s.1) / 2, y as u32),
            });
            e.area_px += (s.1 - s.0) as u64;
            e.bbox[0] = e.bbox[0].min(s.0);
            e.bbox[2] = e.bbox[2].max(s.1);
            e.bbox[1] = e.bbox[1].min(y as u32);
            e.bbox[3] = e.bbox[3].max(y as u32 + 1);
            if let Some(low) = lower {
                while k < low.len() && low[k].1 <= s.0 {
                    k += 1;
                }
                if k < low.len() && low[k].0 < s.1 {
                    supported[id] = true;
                }
            }
        }
    }
    let mut out = Vec::new();
    for i in 0..total {
        if let Some(e) = info[i].take() {
            if !supported[i] {
                out.push(e);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(aa: u32) -> RasterSpec {
        RasterSpec {
            width: 200,
            height: 100,
            size_x: 20.0,
            size_y: 10.0,
            samples: aa,
            mirror_x: false,
            mirror_y: false,
        }
    }

    /// A counter-clockwise rectangle as directed segments.
    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<[f32; 4]> {
        vec![
            [x0, y0, x1, y0],
            [x1, y0, x1, y1],
            [x1, y1, x0, y1],
            [x0, y1, x0, y0],
        ]
    }

    #[test]
    fn square_area_is_exact() {
        for aa in [1, 2, 4, 8] {
            let img = raster_cpu(&rect(5.0, 2.0, 15.0, 8.0), &[], &spec(aa));
            let mut buf = vec![0u8; 20000];
            img.decode(&mut buf);
            let lit: f64 = buf.iter().map(|v| *v as f64 / 255.0).sum();
            assert!((lit - 6000.0).abs() < 60.0, "aa {aa}: {lit}");
            assert!((img.lit - lit).abs() < 1.0);
            assert_eq!(buf[10 * 200 + 100], 0, "above the square is black");
            assert_eq!(buf[50 * 200 + 100], 255, "inside is white");
            assert_eq!(img.bbox[0], 50);
            assert_eq!(img.bbox[2], 150);
        }
    }

    #[test]
    fn partial_pixels_get_grey() {
        let img = raster_cpu(&rect(5.05, 2.0, 15.0, 8.0), &[], &spec(4));
        let mut buf = vec![0u8; 20000];
        img.decode(&mut buf);
        let v = buf[50 * 200 + 50];
        assert!((120..=135).contains(&v), "{v}");
    }

    #[test]
    fn holes_and_overlaps_follow_winding() {
        let mut segs = rect(2.0, 1.0, 18.0, 9.0);
        // A clockwise inner loop is a hole.
        let inner: Vec<[f32; 4]> = rect(5.0, 3.0, 15.0, 7.0)
            .iter()
            .map(|s| [s[2], s[3], s[0], s[1]])
            .collect();
        segs.extend(inner);
        // An overlapping solid piece stays solid where it overlaps.
        segs.extend(rect(1.0, 4.0, 4.0, 6.0));
        let img = raster_cpu(&segs, &[], &spec(1));
        let mut buf = vec![0u8; 20000];
        img.decode(&mut buf);
        assert_eq!(buf[50 * 200 + 100], 0, "hole");
        assert_eq!(buf[50 * 200 + 30], 255, "overlap");
        assert_eq!(buf[50 * 200 + 15], 255, "second piece");
    }

    #[test]
    fn negative_cuts_through() {
        let img = raster_cpu(&rect(2.0, 1.0, 18.0, 9.0), &rect(9.0, 4.0, 11.0, 6.0), &spec(1));
        let mut buf = vec![0u8; 20000];
        img.decode(&mut buf);
        assert_eq!(buf[50 * 200 + 100], 0);
        assert_eq!(buf[50 * 200 + 50], 255);
    }

    #[test]
    fn rle_round_trip_and_compresses() {
        let mut pixels = vec![0u8; 100 * 50];
        for p in pixels.iter_mut().skip(1000).take(500) {
            *p = 200;
        }
        pixels[4999] = 9;
        let img = LayerImage::from_rows(100, &pixels);
        assert!(img.data.len() < 40);
        let mut out = vec![7u8; pixels.len()];
        img.decode(&mut out);
        assert_eq!(out, pixels);
    }

    #[test]
    fn islands_are_found() {
        // Two blobs; the lower layer only sits under the first.
        let cur = vec![vec![(2, 5), (10, 12)], vec![(2, 5), (10, 12)], vec![]];
        let below = vec![vec![(3, 4)], vec![], vec![]];
        let found = find_islands(&cur, Some(&below));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].bbox, [10, 0, 12, 2]);
        assert_eq!(found[0].area_px, 4);
        assert_eq!(find_islands(&cur, None).len(), 2);
    }

    #[test]
    fn mirror_flips_the_picture() {
        let mut s = spec(1);
        s.mirror_x = true;
        let img = raster_cpu(&rect(0.0, 0.0, 5.0, 10.0), &[], &s);
        let mut buf = vec![0u8; 20000];
        img.decode(&mut buf);
        assert_eq!(buf[50 * 200 + 190], 255);
        assert_eq!(buf[50 * 200 + 10], 0);
    }
}
