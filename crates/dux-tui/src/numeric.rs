//! Numeric conversions with no `From`, each with its answer for the values
//! that do not fit written down once.
//!
//! The wrapping helpers keep the low bits, exactly as the `as` casts they
//! replace did; every value the terminal UI passes them (a screen measure, a
//! millisecond count since start) is far below the width that would wrap.

// The conversions between usize and u64 below never truncate only because a
// usize is exactly 64 bits wide; a build where it is not stops here.
const _: () = assert!(
    usize::BITS == 64,
    "dux builds for 64-bit platforms only: the numeric conversions assume a 64-bit usize"
);

/// A count or position as a terminal cell coordinate, keeping its low 16 bits.
pub(crate) fn usize_to_u16_wrapping(n: usize) -> u16 {
    u16::try_from(n & usize::from(u16::MAX)).expect("masked to 16 bits")
}

/// A `u32` as a `u16`, keeping its low 16 bits.
pub(crate) fn u32_to_u16_wrapping(n: u32) -> u16 {
    u16::try_from(n & u32::from(u16::MAX)).expect("masked to 16 bits")
}

/// A `u128` (a `Duration`'s millisecond count) as a `usize`, keeping as many
/// low bits as a `usize` holds.
pub(crate) fn u128_to_usize_wrapping(n: u128) -> usize {
    let mask = u128::try_from(usize::MAX).expect("a usize fits in a u128");
    usize::try_from(n & mask).expect("masked to the width of a usize")
}

/// A `u128` (a `Duration`'s millisecond count) as a `u64`, keeping its low 64
/// bits.
pub(crate) fn u128_to_u64_wrapping(n: u128) -> u64 {
    u64::try_from(n & u128::from(u64::MAX)).expect("masked to 64 bits")
}

/// A `u32` setting as a `usize`; a `usize` is at least 32 bits wide on every
/// platform dux builds for.
pub(crate) fn u32_to_usize(n: u32) -> usize {
    usize::try_from(n).expect("a usize is at least 32 bits on every platform dux builds for")
}

/// A `u64` setting or limit as a `usize`; dux builds for 64-bit platforms only.
pub(crate) fn u64_to_usize(n: u64) -> usize {
    usize::try_from(n).expect("a u64 fits in a usize on the 64-bit platforms dux builds for")
}

/// A length as a `u64`; a `usize` is at most 64 bits wide on every platform
/// dux builds for.
pub(crate) fn usize_to_u64(n: usize) -> u64 {
    u64::try_from(n).expect("a usize fits in a u64 on every platform dux builds for")
}

/// An index or length as an `i64`. A collection holds at most `isize::MAX`
/// elements, which fits.
pub(crate) fn usize_to_i64(n: usize) -> i64 {
    i64::try_from(n).expect("an index or length is at most isize::MAX, which fits in an i64")
}

/// A byte count as an `f64` for a human-readable size: exact below 2^53, and
/// rounded to the nearest representable value above it.
#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    reason = "no From exists from u64 to f64; rounding above 2^53 is invisible in a size shown to one decimal"
)]
pub(crate) fn u64_to_f64(n: u64) -> f64 {
    n as f64
}

/// A colour channel computed in floating point as a `u8`: rounded first by
/// the caller, then clamped into 0..=255 with NaN as 0, which is what a
/// float-to-int `as` does.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a float-to-int as-cast saturates and maps NaN to 0, which is the intended clamp for a channel"
)]
pub(crate) fn f32_to_u8_saturating(x: f32) -> u8 {
    x as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wrapping_helpers_keep_the_low_bits_the_way_as_did() {
        assert_eq!(usize_to_u16_wrapping(70_000), 4_464);
        assert_eq!(usize_to_u16_wrapping(65_536 + 7), 7);
        assert_eq!(u32_to_u16_wrapping(65_536 + 9), 9);
        assert_eq!(u128_to_u64_wrapping(u128::from(u64::MAX) + 5), 4);
        assert_eq!(u128_to_usize_wrapping(42), 42);
    }

    #[test]
    fn a_channel_clamps_into_a_byte_the_way_as_did() {
        assert_eq!(f32_to_u8_saturating(-3.0), 0);
        assert_eq!(f32_to_u8_saturating(f32::NAN), 0);
        assert_eq!(f32_to_u8_saturating(254.6), 254);
        assert_eq!(f32_to_u8_saturating(300.0), 255);
    }
}
