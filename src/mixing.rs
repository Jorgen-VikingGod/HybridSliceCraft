//! Mixed filaments ("full spectrum" printing): a few filaments are alternated
//! layer by layer in stacks too thin to see, and the eye blends them into one
//! colour. A mixed filament is just a repeating pattern of tools: layer `i`
//! prints with `cycle[i % cycle.len()]`.

use serde::{Deserialize, Serialize};

/// Paint values from here up stand for mixed filaments (`MIX_BASE + k`), clear
/// of the filament values below and the surface marks at 100 and above.
pub const MIX_BASE: u8 = 32;
pub const MAX_MIXES: usize = 64 - MIX_BASE as usize;

/// A colour made by alternating filaments.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct MixedFilament {
    pub name: String,
    /// The tool (0-based) printing each layer of one repeat.
    pub cycle: Vec<u8>,
    /// The colour this was matched to, if it came from the Match tab.
    pub target: Option<[u8; 3]>,
}

impl MixedFilament {
    pub fn tool_at(&self, layer: usize) -> usize {
        self.cycle[layer % self.cycle.len().max(1)] as usize
    }
    /// The tools it uses, each once, lowest first.
    pub fn tools(&self) -> Vec<usize> {
        let mut t: Vec<usize> = self.cycle.iter().map(|&x| x as usize).collect();
        t.sort_unstable();
        t.dedup();
        t
    }
    /// The pattern as filament numbers, e.g. "1-2-2-4".
    pub fn text(&self) -> String {
        self.cycle.iter().map(|t| (t + 1).to_string()).collect::<Vec<_>>().join("-")
    }
}

/// The paint value for the mix at `index`.
pub fn paint_value(index: usize) -> u8 {
    MIX_BASE + index.min(MAX_MIXES - 1) as u8
}

/// The mix a paint value stands for, if it is one.
pub fn mix_index(value: u8) -> Option<usize> {
    (MIX_BASE..crate::paint::MARK_BASE).contains(&value).then(|| (value - MIX_BASE) as usize)
}

/// All 50/50 blends of the filaments in use, one per pair.
pub fn auto_pairs(filaments: usize) -> Vec<MixedFilament> {
    let mut out = Vec::new();
    for a in 0..filaments {
        for b in a + 1..filaments {
            out.push(MixedFilament { name: format!("{}+{}", a + 1, b + 1), cycle: vec![a as u8, b as u8], target: None });
        }
    }
    out.truncate(MAX_MIXES);
    out
}

/// How much of the pattern each of `filaments` tools takes, 0 to 1.
pub fn shares_of(cycle: &[u8], filaments: usize) -> Vec<f32> {
    let mut n = vec![0.0f32; filaments];
    for &t in cycle {
        if (t as usize) < filaments {
            n[t as usize] += 1.0;
        }
    }
    let total = cycle.len().max(1) as f32;
    n.iter().map(|c| c / total).collect()
}

/// Height of one repeat, in mm. Much over 0.2 mm and the stripes can be seen.
pub fn stack_height(cycle: &[u8], layer_height: f32) -> f32 {
    cycle.len() as f32 * layer_height
}

/// A pattern for the wanted shares: the shortest repeat (up to `max_len`
/// layers) that comes close, with the tools spread as evenly as possible.
/// `shares` are (tool, share) pairs; tools with no share are left out.
pub fn cycle_from_ratio(shares: &[(u8, f32)], max_len: usize) -> Vec<u8> {
    let used: Vec<(u8, f32)> = shares.iter().cloned().filter(|s| s.1 > 0.0).collect();
    if used.is_empty() {
        return Vec::new();
    }
    let total: f32 = used.iter().map(|s| s.1).sum();
    let want: Vec<f32> = used.iter().map(|s| s.1 / total).collect();
    let max_len = max_len.max(used.len()).max(1);
    // The shortest length whose whole-layer counts stay close to the wanted shares.
    let mut best: Option<(f32, usize, Vec<usize>)> = None;
    for len in used.len()..=max_len {
        let mut counts: Vec<usize> = want.iter().map(|w| ((w * len as f32).round() as usize).max(1)).collect();
        // Make the counts add up to the length again.
        while counts.iter().sum::<usize>() > len {
            let i = (0..counts.len()).filter(|&i| counts[i] > 1).max_by_key(|&i| counts[i]);
            match i {
                Some(i) => counts[i] -= 1,
                None => break,
            }
        }
        while counts.iter().sum::<usize>() < len {
            let i = (0..counts.len())
                .max_by(|&a, &b| (want[a] - counts[a] as f32 / len as f32).total_cmp(&(want[b] - counts[b] as f32 / len as f32)))
                .unwrap();
            counts[i] += 1;
        }
        if counts.iter().sum::<usize>() != len {
            continue;
        }
        let err: f32 = (0..counts.len()).map(|i| (counts[i] as f32 / len as f32 - want[i]).abs()).sum();
        // A longer repeat has to be clearly better to be worth its taller stack.
        if best.as_ref().map_or(true, |b| err + 0.02 < b.0) {
            best = Some((err, len, counts));
        }
    }
    let (_, len, counts) = best.unwrap();
    // Spread them out: each layer goes to the tool furthest behind its share.
    let mut given = vec![0usize; counts.len()];
    let mut out = Vec::with_capacity(len);
    for step in 1..=len {
        let i = (0..counts.len())
            .filter(|&i| given[i] < counts[i])
            .max_by(|&a, &b| {
                let da = counts[a] as f32 * step as f32 / len as f32 - given[a] as f32;
                let db = counts[b] as f32 * step as f32 / len as f32 - given[b] as f32;
                da.total_cmp(&db).then(b.cmp(&a))
            })
            .unwrap();
        given[i] += 1;
        out.push(used[i].0);
    }
    out
}

fn to_linear(c: u8) -> f32 {
    let s = c as f32 / 255.0;
    if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
}

fn from_linear(l: f32) -> u8 {
    let l = l.clamp(0.0, 1.0);
    let s = if l <= 0.0031308 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0).round().clamp(0.0, 255.0) as u8
}

/// The colour the eye should see: each tool's colour weighted by its share of
/// the layers, averaged in linear light. An estimate; real filaments differ.
pub fn blend_colour(cycle: &[u8], colours: &[[u8; 3]]) -> [u8; 3] {
    let mut acc = [0.0f32; 3];
    let mut n = 0.0;
    for &t in cycle {
        let Some(c) = colours.get(t as usize) else { continue };
        for k in 0..3 {
            acc[k] += to_linear(c[k]);
        }
        n += 1.0;
    }
    if n == 0.0 {
        return [128, 128, 128];
    }
    [from_linear(acc[0] / n), from_linear(acc[1] / n), from_linear(acc[2] / n)]
}

/// Oklab, in which distances track how different two colours look.
fn oklab(c: [u8; 3]) -> [f32; 3] {
    let (r, g, b) = (to_linear(c[0]), to_linear(c[1]), to_linear(c[2]));
    let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

pub fn colour_distance(a: [u8; 3], b: [u8; 3]) -> f32 {
    let (x, y) = (oklab(a), oklab(b));
    ((x[0] - y[0]).powi(2) + (x[1] - y[1]).powi(2) + (x[2] - y[2]).powi(2)).sqrt()
}

#[derive(Clone, Debug)]
pub struct Match {
    pub cycle: Vec<u8>,
    pub colour: [u8; 3],
    /// Oklab distance to the wanted colour; below about 0.03 is hard to tell apart.
    pub error: f32,
    /// True when the filament shares had to be raised to the minimum, so a
    /// closer blend exists that uses a filament in a very small share.
    pub adjusted: bool,
}

/// The blend of two or three of the filaments that looks closest to `target`,
/// where each filament in it takes at least `min_share` of the layers.
pub fn match_colour(target: [u8; 3], colours: &[[u8; 3]], min_share: f32, max_len: usize) -> Option<Match> {
    let best_with = |min_share: f32| -> Option<Match> {
        let n = colours.len();
        let mut best: Option<Match> = None;
        let mut consider = |tools: &[usize]| {
            for len in tools.len()..=max_len.max(tools.len()) {
                // Every way to split `len` layers among the tools, one layer at least.
                let mut counts = vec![1usize; tools.len()];
                loop {
                    if counts.iter().sum::<usize>() == len {
                        if counts.iter().all(|&c| c as f32 / len as f32 + 1e-6 >= min_share) {
                            let shares: Vec<(u8, f32)> = tools.iter().zip(&counts).map(|(&t, &c)| (t as u8, c as f32)).collect();
                            let cycle = cycle_from_ratio(&shares, len);
                            let colour = blend_colour(&cycle, colours);
                            let error = colour_distance(colour, target);
                            // Shorter stacks win ties: they are harder to see.
                            if best.as_ref().map_or(true, |b| error + 0.004 < b.error) {
                                best = Some(Match { cycle, colour, error, adjusted: false });
                            }
                        }
                    }
                    // Next split.
                    let mut i = 0;
                    loop {
                        if i == counts.len() {
                            break;
                        }
                        counts[i] += 1;
                        if counts.iter().sum::<usize>() <= len {
                            break;
                        }
                        counts[i] = 1;
                        i += 1;
                    }
                    if i == counts.len() {
                        break;
                    }
                }
            }
        };
        for a in 0..n {
            for b in a + 1..n {
                consider(&[a, b]);
                for c in b + 1..n {
                    consider(&[a, b, c]);
                }
            }
        }
        best
    };
    let mut result = best_with(min_share)?;
    if let Some(free) = best_with(0.0) {
        result.adjusted = free.error + 0.004 < result.error;
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratios_become_short_even_patterns() {
        assert_eq!(cycle_from_ratio(&[(0, 1.0), (1, 1.0)], 6), vec![0, 1]);
        let two_to_one = cycle_from_ratio(&[(0, 1.0), (1, 2.0)], 6);
        assert_eq!(two_to_one.len(), 3);
        assert_eq!(two_to_one.iter().filter(|&&t| t == 1).count(), 2);
        assert!(two_to_one.windows(2).all(|w| w[0] != w[1]) || two_to_one[0] != two_to_one[2], "spread out: {two_to_one:?}");
        let thirds = cycle_from_ratio(&[(0, 34.0), (1, 33.0), (3, 33.0)], 6);
        assert_eq!(thirds.len(), 3);
        let mut sorted = thirds.clone();
        sorted.sort();
        assert_eq!(sorted, vec![0, 1, 3]);
        // A tool with no share is left out.
        assert_eq!(cycle_from_ratio(&[(0, 1.0), (2, 0.0)], 6), vec![0]);
        assert!(cycle_from_ratio(&[], 6).is_empty());
    }

    #[test]
    fn spreading_keeps_the_same_tool_from_clumping() {
        let c = cycle_from_ratio(&[(0, 1.0), (1, 3.0)], 6);
        assert_eq!(c.len(), 4);
        assert_eq!(c.iter().filter(|&&t| t == 0).count(), 1);
        assert!(c.windows(2).filter(|w| w[0] == w[1]).count() <= 2, "{c:?}");
    }

    #[test]
    fn shares_heights_and_text() {
        let m = MixedFilament { name: "x".into(), cycle: vec![0, 1, 1, 3], target: None };
        assert_eq!(shares_of(&m.cycle, 4), vec![0.25, 0.5, 0.0, 0.25]);
        assert!((stack_height(&m.cycle, 0.08) - 0.32).abs() < 1e-6);
        assert_eq!(m.text(), "1-2-2-4");
        assert_eq!(m.tools(), vec![0, 1, 3]);
        assert_eq!([m.tool_at(0), m.tool_at(1), m.tool_at(3), m.tool_at(4), m.tool_at(7)], [0, 1, 3, 0, 3]);
    }

    #[test]
    fn paint_values_round_trip_and_stay_clear_of_filaments_and_marks() {
        for k in [0, 5, MAX_MIXES - 1] {
            assert_eq!(mix_index(paint_value(k)), Some(k));
        }
        assert_eq!(mix_index(16), None);
        assert_eq!(mix_index(crate::paint::SUPPORT_ENFORCER + crate::paint::MARK_BASE), None);
        assert!(paint_value(MAX_MIXES - 1) < crate::paint::MARK_BASE);
    }

    #[test]
    fn pairs_cover_every_combination() {
        let p = auto_pairs(4);
        assert_eq!(p.len(), 6);
        assert!(p.iter().all(|m| m.cycle.len() == 2 && m.cycle[0] < m.cycle[1]));
    }

    #[test]
    fn blending_is_between_the_colours_and_symmetric() {
        let red = [220, 30, 30];
        let yellow = [240, 220, 40];
        let colours = [red, yellow];
        let orange = blend_colour(&[0, 1], &colours);
        assert!(orange[1] > red[1] && orange[1] < yellow[1], "{orange:?}");
        assert_eq!(blend_colour(&[0, 1], &colours), blend_colour(&[1, 0], &colours));
        assert_eq!(blend_colour(&[1, 1, 1], &colours), yellow);
    }

    #[test]
    fn match_finds_a_known_blend_and_respects_the_minimum_share() {
        let colours = [[220, 30, 30], [240, 220, 40], [20, 20, 20], [250, 250, 250]];
        // Aim for what red and yellow make together.
        let target = blend_colour(&[0, 1, 1], &colours);
        let m = match_colour(target, &colours, 0.15, 6).expect("a match");
        assert!(m.error < 0.01, "error {}", m.error);
        assert_eq!(m.cycle.iter().filter(|&&t| t == 0).count() * 3, m.cycle.len());
        assert!(!m.adjusted);
        // Nearly pure yellow would need a sliver of the others: the minimum holds them back.
        let near_yellow = [238, 215, 42];
        let m = match_colour(near_yellow, &colours, 0.3, 6).expect("a match");
        for t in 0..4 {
            let share = m.cycle.iter().filter(|&&x| x as usize == t).count() as f32 / m.cycle.len() as f32;
            assert!(share == 0.0 || share >= 0.3 - 1e-3, "tool {t} share {share} in {:?}", m.cycle);
        }
        assert!(match_colour(target, &colours[..1], 0.15, 6).is_none(), "one filament can't make a mix");
    }
}
