//! The pure math behind the sidebar's "operating" shimmer: a soft highlight that
//! sweeps left to right across an agent/terminal name while its PTY is streaming.
//! Kept free of ratatui layout so the wave and the color blend are unit-testable,
//! and driven by wall-clock elapsed time (per the animation tenet) so the cadence
//! is independent of how often the run loop ticks.

use ratatui::style::{Color, Style};
use ratatui::text::Span;

/// One full sweep of the highlight, in milliseconds.
const PERIOD_MS: u128 = 1500;
/// Width of the highlight band, in character cells (Gaussian sigma).
const SIGMA: f32 = 1.7;
/// How far past each edge the band travels, so it eases in and out rather than
/// popping at the ends.
const MARGIN: f32 = 3.0;

/// Brightness weight in `[0.0, 1.0]` for the character at `index` within a run of
/// `len` characters, at `elapsed_ms` since the animation's epoch. A single soft
/// band peaks at `1.0` at its center and falls off with distance; the center
/// sweeps left to right once per [`PERIOD_MS`] and loops.
pub fn shimmer_weight(index: usize, len: usize, elapsed_ms: u128) -> f32 {
    if len == 0 {
        return 0.0;
    }
    let span = len as f32 + 2.0 * MARGIN;
    let phase = (elapsed_ms % PERIOD_MS) as f32 / PERIOD_MS as f32;
    let center = -MARGIN + phase * span;
    let d = index as f32 - center;
    (-(d * d) / (2.0 * SIGMA * SIGMA)).exp()
}

/// Blend between `base` and `bright` RGB by `t`, clamped to `[0.0, 1.0]`.
pub fn lerp_rgb(base: (u8, u8, u8), bright: (u8, u8, u8), t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color::Rgb(
        mix(base.0, bright.0),
        mix(base.1, bright.1),
        mix(base.2, bright.2),
    )
}

/// Build one styled `Span` per character of `label`, foreground-blended by its
/// shimmer weight so a highlight sweeps the text. The sweep endpoints are derived
/// from `base` (the name's normal identity color): the trough sits DIMMER than
/// base and the crest BRIGHTER, so the motion reads clearly even when the base is
/// already near-white (brightening alone would barely change contrast there).
/// Char-based (never byte-sliced), so multi-byte names are safe. Returns a single
/// plain span for an empty label.
pub fn shimmer_spans(label: &str, base: (u8, u8, u8), elapsed_ms: u128) -> Vec<Span<'static>> {
    let len = label.chars().count();
    if len == 0 {
        return vec![Span::raw(String::new())];
    }
    // Sweep endpoints derived from the base: the trough is a dimmed base (so a
    // shimmering name reads visibly duller than a static idle one) and the crest
    // is lifted well toward white, giving strong contrast on any base color.
    let dim = |v: u8| (v as f32 * 0.55).round() as u8;
    let lift = |v: u8| (v as f32 + (255.0 - v as f32) * 0.85).round() as u8;
    let trough = (dim(base.0), dim(base.1), dim(base.2));
    let crest = (lift(base.0), lift(base.1), lift(base.2));
    label
        .chars()
        .enumerate()
        .map(|(i, c)| {
            let w = shimmer_weight(i, len, elapsed_ms);
            Span::styled(
                c.to_string(),
                Style::default().fg(lerp_rgb(trough, crest, w)),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shimmer_spans_yields_one_span_per_character() {
        let spans = shimmer_spans("hello", (10, 10, 10), 400);
        assert_eq!(spans.len(), 5);
        assert_eq!(
            spans.iter().map(|s| s.content.as_ref()).collect::<String>(),
            "hello"
        );
    }

    #[test]
    fn shimmer_spans_counts_characters_not_bytes() {
        // Four user-perceived characters, several multi-byte.
        let spans = shimmer_spans("café→", (0, 0, 0), 0);
        assert_eq!(spans.len(), 5);
    }

    #[test]
    fn shimmer_spans_brightest_char_tracks_the_sweep() {
        // The brightest span (highest green channel here) should move rightward
        // as time advances, matching the weight sweep.
        let brightest = |elapsed: u128| {
            shimmer_spans("abcdefghij", (0, 128, 0), elapsed)
                .into_iter()
                .enumerate()
                .max_by_key(|(_, s)| match s.style.fg {
                    Some(Color::Rgb(_, g, _)) => g,
                    _ => 0,
                })
                .map(|(i, _)| i)
                .unwrap()
        };
        assert!(brightest(0) < brightest(PERIOD_MS - 1));
    }

    #[test]
    fn shimmer_dims_the_trough_and_brightens_the_crest() {
        // The sweep must read on ANY base, including a near-white one where pure
        // brightening has no headroom: characters far from the band sit DIMMER
        // than base, and the band's crest sits BRIGHTER than base. A long label
        // guarantees both a far-from-band char and an in-band char exist.
        let base = (128u8, 128u8, 128u8);
        let reds: Vec<u8> = shimmer_spans("abcdefghijklmnopqrst", base, PERIOD_MS / 2)
            .into_iter()
            .map(|s| match s.style.fg {
                Some(Color::Rgb(r, _, _)) => r,
                other => panic!("expected an RGB fg, got {other:?}"),
            })
            .collect();
        let dimmest = *reds.iter().min().unwrap();
        let brightest = *reds.iter().max().unwrap();
        assert!(
            dimmest < base.0,
            "trough ({dimmest}) must sit dimmer than base ({})",
            base.0
        );
        assert!(
            brightest > base.0,
            "crest ({brightest}) must sit brighter than base ({})",
            base.0
        );
    }

    #[test]
    fn shimmer_weight_stays_within_the_unit_range() {
        for len in [1usize, 5, 20] {
            for index in 0..len {
                for elapsed in [0u128, 137, 750, 1499, 5000] {
                    let w = shimmer_weight(index, len, elapsed);
                    assert!(
                        (0.0..=1.0).contains(&w),
                        "weight {w} out of range for index {index}, len {len}, elapsed {elapsed}"
                    );
                }
            }
        }
    }

    #[test]
    fn shimmer_weight_is_zero_for_an_empty_run() {
        assert_eq!(shimmer_weight(0, 0, 500), 0.0);
    }

    #[test]
    fn the_bright_band_sweeps_left_to_right_over_time() {
        let len = 12;
        let argmax = |elapsed: u128| {
            (0..len)
                .max_by(|&a, &b| {
                    shimmer_weight(a, len, elapsed)
                        .partial_cmp(&shimmer_weight(b, len, elapsed))
                        .unwrap()
                })
                .unwrap()
        };
        // Within one period the peak moves rightward: early < middle < late.
        let early = argmax(0);
        let middle = argmax(PERIOD_MS / 2);
        let late = argmax(PERIOD_MS - 1);
        assert!(
            early < middle && middle < late,
            "peak did not sweep: {early} -> {middle} -> {late}"
        );
    }

    #[test]
    fn lerp_rgb_hits_the_endpoints_and_midpoint() {
        let base = (10, 20, 30);
        let bright = (250, 240, 230);
        assert_eq!(lerp_rgb(base, bright, 0.0), Color::Rgb(10, 20, 30));
        assert_eq!(lerp_rgb(base, bright, 1.0), Color::Rgb(250, 240, 230));
        // Midpoint rounds each channel to the average.
        assert_eq!(lerp_rgb(base, bright, 0.5), Color::Rgb(130, 130, 130));
    }

    #[test]
    fn lerp_rgb_clamps_out_of_range_t() {
        let base = (0, 0, 0);
        let bright = (255, 255, 255);
        assert_eq!(lerp_rgb(base, bright, -1.0), Color::Rgb(0, 0, 0));
        assert_eq!(lerp_rgb(base, bright, 2.0), Color::Rgb(255, 255, 255));
    }
}
