//! The pure math behind the one "working" cue both surfaces paint.
//!
//! While an agent or a terminal is busy, its state word ("Working", "Running")
//! pulses between full brightness and a floor, and a cycling ellipsis runs after
//! it in a slot the width of three dots so nothing behind the word ever shifts.
//! The web pulses its glyph on the same clock; the terminal UI keeps its spinner.
//!
//! Everything here is a function of wall-clock elapsed milliseconds, per the
//! animation tenet, so the cadence does not depend on how often either surface
//! ticks. It lives in core because both surfaces read the same period, the same
//! step and the same floor, and a second copy of those numbers is exactly how
//! the two cues drift apart.

/// One full brightness cycle of the pulse, in milliseconds: bright, down to the
/// floor at the halfway point, back to bright.
pub const WORKING_CUE_PERIOD_MS: u64 = 1600;

/// How long each ellipsis state holds. Four states fill one pulse period, which
/// is what keeps the dots and the pulse on a single clock.
pub const ELLIPSIS_STEP_MS: u64 = 400;

/// The dimmest the pulse ever gets, as a fraction of full brightness. The word
/// must stay readable at the bottom of the cycle, so this is a dip and not a
/// blink.
pub const PULSE_FLOOR: f32 = 0.4;

/// How far toward the surrounding muted tone the word travels at the bottom of
/// the pulse, on a surface that has no opacity to dip.
///
/// A pixel can simply become 40% as bright as it was, so [`PULSE_FLOOR`] is the
/// whole answer there. A terminal cell cannot: it has one foreground color and
/// no alpha, so the same dip has to be expressed as a blend between the working
/// color and the quiet tone it sits beside, and how far along that line the
/// floor lands is a separate judgement from how much light is left. Two thirds
/// keeps a visible third of the working hue, which still reads as that hue next
/// to the grey around it.
pub const PULSE_MUTE_FRACTION: f32 = 2.0 / 3.0;

/// How far toward the muted tone a [`pulse_step`] cell sits, in `[0.0, 1.0]`:
/// `0.0` is the untouched working color and [`PULSE_MUTE_FRACTION`] is the
/// floor. The quantized cells give `0`, `1/3`, `2/3`, `1/3`.
pub fn mute_blend(step: usize) -> f32 {
    PULSE_MUTE_FRACTION * (1.0 - step_level(step)) / (1.0 - PULSE_FLOOR)
}

/// How many distinct ellipsis states there are: none, one dot, two, three.
pub const ELLIPSIS_STATES: u64 = WORKING_CUE_PERIOD_MS / ELLIPSIS_STEP_MS;

/// Width of the reserved ellipsis slot, in character cells.
pub const ELLIPSIS_SLOT_WIDTH: usize = (ELLIPSIS_STATES - 1) as usize;

/// The ellipsis at `elapsed_ms`: `""`, `"."`, `".."`, `"..."`, one step every
/// [`ELLIPSIS_STEP_MS`], looping.
pub fn ellipsis_at(elapsed_ms: u64) -> &'static str {
    match (elapsed_ms / ELLIPSIS_STEP_MS) % ELLIPSIS_STATES {
        0 => "",
        1 => ".",
        2 => "..",
        _ => "...",
    }
}

/// The same ellipsis padded out to [`ELLIPSIS_SLOT_WIDTH`] cells, so the slot is
/// a constant width and whatever follows the state word (a tab count, a viewer
/// count) never moves as the dots cycle.
pub fn ellipsis_slot(elapsed_ms: u64) -> String {
    let dots = ellipsis_at(elapsed_ms);
    format!("{dots:<ELLIPSIS_SLOT_WIDTH$}")
}

/// The continuous brightness level at `elapsed_ms`, in `[PULSE_FLOOR, 1.0]`:
/// `1.0` at the start of the period, [`PULSE_FLOOR`] at its midpoint, back to
/// `1.0` at its end. A triangle wave shaped by a smoothstep, which is the same
/// curve the web gets from `ease-in-out` on a two-keyframe opacity animation.
pub fn pulse_level(elapsed_ms: u64) -> f32 {
    let u = (elapsed_ms % WORKING_CUE_PERIOD_MS) as f32 / WORKING_CUE_PERIOD_MS as f32;
    let triangle = 1.0 - (2.0 * u - 1.0).abs();
    let eased = triangle * triangle * (3.0 - 2.0 * triangle);
    // Clamped because the arithmetic lands a hair outside at the endpoints, and
    // callers scale a color by this.
    (1.0 - (1.0 - PULSE_FLOOR) * eased).clamp(PULSE_FLOOR, 1.0)
}

/// Which [`ELLIPSIS_STEP_MS`] cell of the period `elapsed_ms` falls in, `0..4`.
/// A terminal cell has no opacity, so the pulse is quantized to these four cells
/// and each one picks a shade. Sampled at the cell's start, the levels run
/// bright, mid, floor, mid: cell 0 is full brightness, cells 1 and 3 sit halfway
/// down, and cell 2 is the floor.
pub fn pulse_step(elapsed_ms: u64) -> usize {
    ((elapsed_ms / ELLIPSIS_STEP_MS) % ELLIPSIS_STATES) as usize
}

/// The brightness level a [`pulse_step`] cell paints at: [`pulse_level`] sampled
/// at the cell's own start, so the quantized shades sit on the continuous curve
/// rather than beside it.
pub fn step_level(step: usize) -> f32 {
    pulse_level(step as u64 % ELLIPSIS_STATES * ELLIPSIS_STEP_MS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ellipsis_cycles_four_states_one_step_at_a_time() {
        assert_eq!(ellipsis_at(0), "");
        assert_eq!(ellipsis_at(399), "");
        assert_eq!(ellipsis_at(400), ".");
        assert_eq!(ellipsis_at(800), "..");
        assert_eq!(ellipsis_at(1200), "...");
        // And it loops with the pulse period.
        assert_eq!(ellipsis_at(1600), "");
        assert_eq!(ellipsis_at(2000), ".");
    }

    #[test]
    fn the_ellipsis_slot_is_always_three_cells_wide() {
        for elapsed in [0u64, 400, 800, 1200, 1599, 4321] {
            assert_eq!(
                ellipsis_slot(elapsed).chars().count(),
                ELLIPSIS_SLOT_WIDTH,
                "slot width moved at {elapsed}"
            );
        }
        assert_eq!(ellipsis_slot(0), "   ");
        assert_eq!(ellipsis_slot(400), ".  ");
        assert_eq!(ellipsis_slot(1200), "...");
    }

    #[test]
    fn the_pulse_runs_from_full_brightness_down_to_the_floor_and_back() {
        assert!((pulse_level(0) - 1.0).abs() < 1e-6);
        assert!((pulse_level(WORKING_CUE_PERIOD_MS / 2) - PULSE_FLOOR).abs() < 1e-6);
        assert!((pulse_level(WORKING_CUE_PERIOD_MS) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_pulse_never_leaves_its_range_and_is_symmetric() {
        for elapsed in 0..WORKING_CUE_PERIOD_MS {
            let level = pulse_level(elapsed);
            assert!(
                (PULSE_FLOOR..=1.0).contains(&level),
                "level {level} out of range at {elapsed}"
            );
        }
        // The second half mirrors the first.
        for offset in [0u64, 137, 400, 799] {
            let rising = pulse_level(offset);
            let falling = pulse_level(WORKING_CUE_PERIOD_MS - offset);
            assert!(
                (rising - falling).abs() < 1e-5,
                "asymmetric at {offset}: {rising} vs {falling}"
            );
        }
    }

    #[test]
    fn the_pulse_descends_monotonically_over_the_first_half() {
        let mut previous = pulse_level(0);
        for elapsed in (10..=WORKING_CUE_PERIOD_MS / 2).step_by(10) {
            let level = pulse_level(elapsed);
            assert!(
                level <= previous,
                "rose at {elapsed}: {previous} -> {level}"
            );
            previous = level;
        }
    }

    #[test]
    fn the_quantized_steps_run_bright_mid_floor_mid() {
        assert_eq!(pulse_step(0), 0);
        assert_eq!(pulse_step(400), 1);
        assert_eq!(pulse_step(800), 2);
        assert_eq!(pulse_step(1200), 3);
        assert_eq!(pulse_step(1600), 0);

        assert!((step_level(0) - 1.0).abs() < 1e-6);
        assert!((step_level(2) - PULSE_FLOOR).abs() < 1e-6);
        // The two mid cells are the same shade, and they sit between the ends.
        assert!((step_level(1) - step_level(3)).abs() < 1e-6);
        assert!(step_level(1) < step_level(0) && step_level(1) > step_level(2));
    }

    #[test]
    fn the_mute_blend_runs_none_third_two_thirds_third() {
        let thirds = |value: f32| (value * 3.0).round() as i32;
        assert!(mute_blend(0).abs() < 1e-6);
        assert_eq!(thirds(mute_blend(1)), 1);
        assert_eq!(thirds(mute_blend(2)), 2);
        assert!((mute_blend(2) - PULSE_MUTE_FRACTION).abs() < 1e-6);
        assert!((mute_blend(1) - mute_blend(3)).abs() < 1e-6);
    }

    #[test]
    fn the_mute_blend_never_leaves_the_unit_range() {
        for step in 0..4 {
            let blend = mute_blend(step);
            assert!(
                (0.0..=1.0).contains(&blend),
                "blend {blend} out of range at step {step}"
            );
        }
    }

    #[test]
    fn the_ellipsis_and_the_pulse_share_one_clock() {
        // Four ellipsis states fill exactly one pulse period, so a surface can
        // drive both from a single counter.
        assert_eq!(ELLIPSIS_STATES * ELLIPSIS_STEP_MS, WORKING_CUE_PERIOD_MS);
        for elapsed in [0u64, 400, 800, 1200, 3700] {
            assert_eq!(
                pulse_step(elapsed),
                ellipsis_at(elapsed).chars().count(),
                "the step and the dot count disagree at {elapsed}"
            );
        }
    }
}
