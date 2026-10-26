//! Numeric conversions with a stated answer for the values that do not fit.
//!
//! Each kind of conversion lives here once, so every call site gets the same
//! rule and the reason it holds is written down in one place.

// The conversions between usize and u64 below never truncate only because a
// usize is exactly 64 bits wide; a build where it is not stops here.
const _: () = assert!(
    usize::BITS == 64,
    "dux builds for 64-bit platforms only: the numeric conversions assume a 64-bit usize"
);

/// A length or count as a `u64`. Every platform dux builds for has a 64-bit
/// or narrower `usize`, so the value always fits.
pub(crate) fn usize_to_u64(n: usize) -> u64 {
    u64::try_from(n).expect("a usize fits in a u64 on every platform dux builds for")
}

/// A `u64` as a `usize`, for a length or count that is bounded by memory dux
/// already holds or a cap below 4 GiB. dux builds for 64-bit platforms only,
/// where every `u64` fits.
pub(crate) fn u64_to_usize(n: u64) -> usize {
    usize::try_from(n).expect("a u64 fits in a usize on the 64-bit platforms dux builds for")
}

/// A `u32` setting or count as a `usize`; a `usize` is at least 32 bits wide
/// on every platform dux builds for.
pub(crate) fn u32_to_usize(n: u32) -> usize {
    usize::try_from(n).expect("a u32 fits in a usize on every platform dux builds for")
}

/// A count or position as a SQLite or TOML integer, which is an `i64`. Two's
/// complement wrapping above `i64::MAX`, exactly as an `as` cast did, though
/// no count dux keeps comes near it.
pub(crate) fn usize_to_i64_wrapping(n: usize) -> i64 {
    usize_to_u64(n).cast_signed()
}

/// A count as an `f64`, exact below 2^53 and rounded to the nearest
/// representable value above it, which is what a ratio of counts wants.
#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    reason = "no From exists from usize to f64; rounding above 2^53 is the intended answer"
)]
pub(crate) fn usize_to_f64(n: usize) -> f64 {
    n as f64
}

/// A non-negative float rounded down to a `u32`, saturating: below zero or NaN
/// is 0, above `u32::MAX` is `u32::MAX`. That is what a float-to-int `as` does.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a float-to-int as-cast saturates and maps NaN to 0, which is the intended clamp"
)]
pub(crate) fn f64_to_u32_saturating(x: f64) -> u32 {
    x as u32
}

/// Milliseconds as an `f32`, for a fraction of a period. Exact below 2^24 ms
/// (about 4.6 hours) and rounded above, which a fraction does not notice.
#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    reason = "no From exists from u64 to f32; the rounding is below what a fraction of a period shows"
)]
pub(crate) fn u64_to_f32(n: u64) -> f32 {
    n as f32
}

/// A grid row, column or line count as the terminal engine's `i32` line index,
/// wrapping exactly as the engine's own `as i32` casts do. A grid is far smaller
/// than `i32::MAX` lines, so nothing wraps in practice.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "mirrors the terminal engine's own as-cast to i32 in its line arithmetic, wrap included"
)]
pub(crate) fn grid_i32(n: usize) -> i32 {
    n as i32
}

/// A viewport row or column as a `u16`. A viewport position is below the grid
/// size, which is itself a `u16`, so nothing is cut off.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "a viewport position is below the grid's u16 size, so the value fits"
)]
pub(crate) fn grid_u16(n: usize) -> u16 {
    n as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_float_clamps_into_u32_the_way_as_did() {
        assert_eq!(f64_to_u32_saturating(-3.0), 0);
        assert_eq!(f64_to_u32_saturating(f64::NAN), 0);
        assert_eq!(f64_to_u32_saturating(1234.9), 1234);
        assert_eq!(f64_to_u32_saturating(1e20), u32::MAX);
    }

    #[test]
    fn a_huge_count_wraps_into_i64_the_way_as_did() {
        assert_eq!(usize_to_i64_wrapping(5), 5);
        assert_eq!(usize_to_i64_wrapping(usize::MAX), -1);
    }
}
