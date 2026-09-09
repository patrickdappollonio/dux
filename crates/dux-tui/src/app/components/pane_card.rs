//! An in-pane empty state drawn as a card: a titled border ring, a comfortable
//! measure capped by the pane, one column of padding inside the ring, blocks
//! separated by one blank row, and an optional button on the bottom edge.
//!
//! It is not a modal: no [`crate::app::PromptState`], no entry in the modal
//! registry, and no rect in the click-outside dismissal engine, so pane and tab
//! navigation keep working over it.
//!
//! Degradation, as the pane shrinks, in order:
//!
//! - Blocks by the caller's own rank, highest first; only the caller knows which
//!   of its sentences is the point of the card.
//! - Then the border ring, then the last ranked block.
//! - The button last, and only when it cannot be drawn whole, because it is the
//!   way out. A card down to its button alone takes the ring back.
//! - A pane too narrow for the button at any measure gives up the button rather
//!   than the whole card.
//! - One block left, no button behind it, still not fitting: truncated to the
//!   rows there are rather than dropped.
//!
//! Blocks are measured per candidate width rather than once, because giving up
//! the ring widens the content.

use ratatui::layout::{Alignment, Rect};

use super::button::ButtonState;

/// One block, as the caller hands it over: its drop rank and its content. The
/// paint lives on `App`, which owns the theme.
pub(crate) struct PaneCardBlock {
    /// Higher drops first. See [`CardBlockPlan::rank`].
    pub rank: u16,
    pub content: CardContent,
}

impl PaneCardBlock {
    pub(crate) fn new(rank: u16, content: CardContent) -> Self {
        Self { rank, content }
    }

    pub(crate) fn plan(&self) -> CardBlockPlan {
        CardBlockPlan {
            rank: self.rank,
            is_button: matches!(self.content, CardContent::Button { .. }),
        }
    }
}

/// One block's CONTENT.
pub(crate) enum CardContent {
    /// The card's body sentence, wrapped and centred in the desc tone.
    Prose(String),
    /// Secondary detail in the dim tone: an output excerpt (left aligned, it is
    /// terminal output) or a folder path (centred, it is part of the sentence).
    Detail {
        lines: Vec<String>,
        align: Alignment,
    },
    /// "Press <key> to <verb>." The key text is looked up by the caller through
    /// the bindings, never spelled literally, because every binding is
    /// rebindable.
    KeyHint {
        before: String,
        key: String,
        after: String,
    },
    /// The card's one act.
    Button { label: String, state: ButtonState },
}

/// The rows a button block occupies: its own line inside a one-row frame.
pub(crate) const BUTTON_HEIGHT: u16 = 3;

/// Blank columns kept between the card's content and its border ring, on each
/// side. Content that touches the ring reads as clipped.
pub(crate) const SIDE_PADDING: u16 = 1;
/// The blank row under the title border, so the body sits inside a ring of
/// space rather than starting hard against the line that names the card.
pub(crate) const TOP_PADDING: u16 = 1;
/// The blank row between two blocks.
pub(crate) const BLOCK_GAP: u16 = 1;
/// The narrowest content measure worth drawing a ring around. Below this the
/// ring costs more than it is worth and the pane itself is the card's surface.
pub(crate) const MIN_INNER: u16 = 12;

/// One block, as the pure planner sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CardBlockPlan {
    /// The highest rank is dropped first. Supplied by the caller, because only
    /// the caller knows which of its own sentences is the point of the card.
    /// Ties drop the later block first, so the reading order of what survives is
    /// unchanged.
    pub rank: u16,
    /// A button is never dropped and sits on the card's bottom edge.
    pub is_button: bool,
}

/// Where one surviving block landed, paired with its index in the caller's
/// original list, so a caller can paint blocks it did not have to re-identify.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PlacedBlock {
    pub index: usize,
    pub area: Rect,
    /// The block did not fit whole and was cut to `area.height` rows, so the
    /// painter must mark the cut. Only ever true for the last-resort case.
    pub truncated: bool,
}

/// The result of planning a card into a pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CardPlan {
    /// The bordered card's outer rect, or `None` when the ring was dropped and
    /// the surviving blocks are painted bare on the pane.
    pub ring: Option<Rect>,
    /// The surviving blocks, in the caller's order, top to bottom.
    pub blocks: Vec<PlacedBlock>,
    /// The width every surviving block was measured and laid out at. The caller
    /// wraps at this width to paint it, and it is NOT knowable in advance:
    /// giving up the ring widens it.
    pub content_width: u16,
}

/// The content measure with no ring: the pane itself, less the padding.
fn bare_content_width(area: Rect) -> u16 {
    area.width.saturating_sub(SIDE_PADDING * 2)
}

/// Plan a card into `area`.
///
/// `button_width` is the width the caller's button paints at, or `None` for a
/// card with no button. It decides the narrowest ring worth drawing and whether
/// the button can be painted at all: `Button::render` does not clip, so a pane
/// too narrow for the button drops it and keeps the words rather than painting a
/// truncated label.
///
/// `measure` answers "how many rows does block `i` need at `width` columns". It
/// is asked once per candidate width rather than once overall, because giving up
/// the ring widens the content.
///
/// Precondition: a button, if present, is the last block and `button_width` is
/// its painted width. The layout puts it on the card's bottom edge and the
/// height arithmetic assumes it ends the stack, so a mid-list button would
/// silently mis-size the card. Checked with a `debug_assert!`.
///
/// Returns `None` only when not even one row of one block fits.
pub(crate) fn plan_pane_card(
    area: Rect,
    preferred_inner: u16,
    button_width: Option<u16>,
    blocks: &[CardBlockPlan],
    measure: impl Fn(usize, u16) -> u16,
) -> Option<CardPlan> {
    debug_assert!(
        blocks
            .iter()
            .rposition(|block| block.is_button)
            .is_none_or(|position| position + 1 == blocks.len()),
        "a pane card's button must be its last block; see plan_pane_card's precondition"
    );
    if area.width == 0 || area.height == 0 || blocks.is_empty() {
        return None;
    }
    let bare_w = bare_content_width(area);
    if bare_w == 0 {
        return None;
    }
    // The ring is always narrower than the bare layout, so a button the bare
    // width cannot hold is one no layout can hold: give it up and keep the words.
    let paintable_button = button_width.filter(|width| bare_w >= *width);
    let button_unpaintable = button_width.is_some() && paintable_button.is_none();
    // The narrowest INNER measure worth a ring. The button's own width plus the
    // padding it sits inside: forgetting the padding here is what let a button
    // paint clipped inside a ring that looked wide enough.
    let min_inner = paintable_button
        .map(|width| width.saturating_add(SIDE_PADDING * 2))
        .unwrap_or(MIN_INNER)
        .max(MIN_INNER);
    // WIDTH decides whether a ring is possible at all, exactly as the take-over
    // card does: a pane too narrow to hold the measure plus two columns of
    // border spends those two columns on content instead.
    let ring_possible = area.width >= min_inner + 2;
    // Clamped up to `min_inner`, never merely capped down to the pane: a caller
    // whose comfortable measure is narrower than its own button would otherwise
    // draw a ring the button does not fit in. Only meaningful when a ring is
    // possible at all, which is exactly when the clamp's bounds are ordered.
    let ring_inner = if ring_possible {
        preferred_inner.clamp(min_inner, area.width.saturating_sub(2))
    } else {
        0
    };
    let ring_w = ring_inner.saturating_sub(SIDE_PADDING * 2).max(1);

    let mut kept: Vec<usize> = (0..blocks.len())
        .filter(|index| !(button_unpaintable && blocks[*index].is_button))
        .collect();
    if kept.is_empty() {
        return None;
    }
    loop {
        if ring_possible {
            let heights = measure_all(&kept, blocks, ring_w, &measure);
            if stack_height(&kept, blocks, &heights) + 2 <= area.height {
                return Some(place(area, Some(ring_inner), &kept, blocks, &heights));
            }
        }
        // The ring is given up only once every block ranked above the last one
        // has gone; a button-only card takes it back above, since nothing is
        // left for those columns to hold.
        let extras = kept
            .iter()
            .filter(|index| !blocks[**index].is_button)
            .count();
        if extras <= 1 {
            let heights = measure_all(&kept, blocks, bare_w, &measure);
            if stack_height(&kept, blocks, &heights) <= area.height {
                return Some(place(area, None, &kept, blocks, &heights));
            }
        }
        // Last resort, where the alternative is painting nothing: one block left,
        // no button behind it, not fitting whole. Show its first rows.
        if kept.len() == 1 && !blocks[kept[0]].is_button {
            let rows = area.height.saturating_sub(TOP_PADDING * 2).max(1);
            let y = area.y + (area.height.saturating_sub(rows)) / 2;
            return Some(CardPlan {
                ring: None,
                blocks: vec![PlacedBlock {
                    index: kept[0],
                    area: Rect::new(area.x + SIDE_PADDING, y, bare_w, rows),
                    truncated: true,
                }],
                content_width: bare_w,
            });
        }
        // Drop the highest-ranked survivor; ties go to the last one.
        let victim = kept
            .iter()
            .enumerate()
            .filter(|(_, index)| !blocks[**index].is_button)
            .max_by_key(|(position, index)| (blocks[**index].rank, *position))
            .map(|(position, _)| position)?;
        kept.remove(victim);
        if kept.is_empty() {
            return None;
        }
    }
}

fn measure_all(
    kept: &[usize],
    blocks: &[CardBlockPlan],
    width: u16,
    measure: &impl Fn(usize, u16) -> u16,
) -> Vec<u16> {
    kept.iter()
        .map(|index| {
            if blocks[*index].is_button {
                BUTTON_HEIGHT
            } else {
                measure(*index, width)
            }
        })
        .collect()
}

/// Lay the surviving blocks out, with the ring when `ring_inner_w` is `Some`.
fn place(
    area: Rect,
    ring_inner_w: Option<u16>,
    kept: &[usize],
    blocks: &[CardBlockPlan],
    heights: &[u16],
) -> CardPlan {
    let inner_h = stack_height(kept, blocks, heights);
    match ring_inner_w {
        Some(inner_w) => {
            let card = Rect::new(
                area.x + (area.width - (inner_w + 2)) / 2,
                area.y + (area.height - (inner_h + 2)) / 2,
                inner_w + 2,
                inner_h + 2,
            );
            let content_w = inner_w.saturating_sub(SIDE_PADDING * 2).max(1);
            CardPlan {
                ring: Some(card),
                blocks: stack(
                    kept,
                    heights,
                    card.x + 1 + SIDE_PADDING,
                    card.y + 1,
                    content_w,
                ),
                content_width: content_w,
            }
        }
        None => {
            let content_w = bare_content_width(area).max(1);
            let y = area.y + (area.height.saturating_sub(inner_h)) / 2;
            CardPlan {
                ring: None,
                blocks: stack(kept, heights, area.x + SIDE_PADDING, y, content_w),
                content_width: content_w,
            }
        }
    }
}

/// The rows a stack of blocks occupies, padding and gaps included. A block
/// ending the card gets a blank row under it except a button, which sits on the
/// bottom edge.
fn stack_height(kept: &[usize], blocks: &[CardBlockPlan], heights: &[u16]) -> u16 {
    let bodies: u16 = heights.iter().sum();
    let gaps = BLOCK_GAP * u16::try_from(kept.len().saturating_sub(1)).unwrap_or(0);
    let bottom = match kept.last() {
        Some(index) if blocks[*index].is_button => 0,
        _ => TOP_PADDING,
    };
    TOP_PADDING + bodies + gaps + bottom
}

/// Lay the surviving blocks out from `y` downwards.
fn stack(kept: &[usize], heights: &[u16], x: u16, y: u16, width: u16) -> Vec<PlacedBlock> {
    let mut cursor = y + TOP_PADDING;
    let mut placed = Vec::with_capacity(kept.len());
    for (position, index) in kept.iter().enumerate() {
        if position > 0 {
            cursor += BLOCK_GAP;
        }
        let height = heights[position];
        placed.push(PlacedBlock {
            index: *index,
            area: Rect::new(x, cursor, width, height),
            truncated: false,
        });
        cursor += height;
    }
    placed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A block of `rank`, whose content is `height` rows tall at any width. The
    /// width-independent measure is what lets these tests say something about
    /// PRIORITY without also restating the wrapper's arithmetic.
    fn block(rank: u16, height: u16) -> (CardBlockPlan, u16) {
        (
            CardBlockPlan {
                rank,
                is_button: false,
            },
            height,
        )
    }

    fn button(height: u16) -> (CardBlockPlan, u16) {
        (
            CardBlockPlan {
                rank: 0,
                is_button: true,
            },
            height,
        )
    }

    /// Plan the given (block, fixed height) pairs into `area`. `button` is the
    /// width the caller's button paints at, or `None` for a card without one.
    fn laid_out(
        area: Rect,
        preferred: u16,
        button: Option<u16>,
        blocks: &[(CardBlockPlan, u16)],
    ) -> Option<CardPlan> {
        let plans: Vec<CardBlockPlan> = blocks.iter().map(|(plan, _)| *plan).collect();
        plan_pane_card(area, preferred, button, &plans, |index, _width| {
            blocks[index].1
        })
    }

    #[test]
    fn the_measure_is_preferred_but_capped_by_the_pane() {
        let wide = Rect::new(0, 0, 120, 20);
        let plan = laid_out(wide, 46, None, &[block(0, 1)]).expect("fits");
        assert_eq!(
            plan.content_width, 44,
            "a wide pane gets the comfortable measure, not the whole pane"
        );
        let narrow = Rect::new(0, 0, 20, 20);
        let plan = laid_out(narrow, 46, None, &[block(0, 1)]).expect("fits");
        assert_eq!(
            plan.content_width, 16,
            "a narrow pane is capped by its own width, minus ring and padding"
        );
    }

    #[test]
    fn blocks_stack_in_order_with_one_blank_row_between_them() {
        let plan = laid_out(
            Rect::new(0, 0, 60, 24),
            46,
            Some(16),
            &[block(0, 3), block(1, 2), button(3)],
        )
        .expect("a roomy pane fits the whole card");
        let ring = plan.ring.expect("a roomy pane keeps the ring");
        assert_eq!(ring.width, 46 + 2);
        assert_eq!(
            plan.blocks.iter().map(|b| b.index).collect::<Vec<_>>(),
            vec![0, 1, 2],
            "blocks keep the caller's reading order"
        );
        assert!(
            plan.blocks.iter().all(|b| !b.truncated),
            "nothing is cut when everything fits"
        );
        let prose = plan.blocks[0].area;
        let detail = plan.blocks[1].area;
        let button = plan.blocks[2].area;
        assert_eq!(
            prose.y,
            ring.y + 1 + TOP_PADDING,
            "one blank row under the title border"
        );
        assert_eq!(detail.y, prose.y + prose.height + BLOCK_GAP);
        assert_eq!(button.y, detail.y + detail.height + BLOCK_GAP);
        assert_eq!(
            button.y + button.height,
            ring.y + ring.height - 1,
            "the button sits on the card's bottom edge"
        );
        assert_eq!(
            prose.x,
            ring.x + 1 + SIDE_PADDING,
            "one column of padding inside the ring"
        );
        assert_eq!(prose.width, 46 - SIDE_PADDING * 2);
    }

    #[test]
    fn a_card_with_no_button_keeps_a_blank_row_under_its_last_block() {
        let plan = laid_out(
            Rect::new(0, 0, 40, 20),
            30,
            None,
            &[block(0, 2), block(1, 1)],
        )
        .expect("fits");
        let ring = plan.ring.expect("ring");
        let detail = plan.blocks[1].area;
        assert_eq!(
            detail.y + detail.height,
            ring.y + ring.height - 1 - TOP_PADDING,
            "prose that ends a card is padded off the bottom border, unlike a button"
        );
    }

    /// The whole point of the rank: the CALLER says what is expendable. Here the
    /// standing explanation (rank 3) goes before the diagnosis (rank 1), which is
    /// the opposite of what a fixed "detail is secondary" order would have done.
    #[test]
    fn a_shrinking_pane_sheds_by_the_callers_rank_then_the_ring_then_the_last_block() {
        let blocks = [
            block(0, 3), // the point of the card
            block(1, 4), // the diagnosis beside it
            block(2, 1), // a key hint
            block(3, 2), // a standing explanation
            button(3),
        ];
        let survivors = |height: u16| {
            laid_out(Rect::new(0, 0, 60, height), 46, Some(16), &blocks).map(|plan| {
                (
                    plan.ring.is_some(),
                    plan.blocks.iter().map(|b| b.index).collect::<Vec<_>>(),
                )
            })
        };

        assert_eq!(
            survivors(30),
            Some((true, vec![0, 1, 2, 3, 4])),
            "a roomy pane keeps everything"
        );
        assert_eq!(
            survivors(17),
            Some((true, vec![0, 1, 2, 4])),
            "the highest rank goes first, whatever kind it is"
        );
        assert_eq!(
            survivors(16),
            Some((true, vec![0, 1, 4])),
            "then the next rank up, and the diagnosis outlives the standing \
             explanation that a fixed order by kind would have kept instead"
        );
        assert_eq!(
            survivors(14),
            Some((true, vec![0, 4])),
            "then the diagnosis, leaving the caller's most important block"
        );
        assert_eq!(
            survivors(8),
            Some((false, vec![0, 4])),
            "then the ring, because it is worth less than the sentence"
        );
        assert_eq!(
            survivors(7),
            Some((true, vec![4])),
            "then the last block, and with nothing left to hold, the ring comes back"
        );
        assert_eq!(
            survivors(4),
            Some((false, vec![4])),
            "and last of all the ring again, so the way out survives"
        );
        assert_eq!(
            survivors(2),
            None,
            "a pane with no room for the button draws nothing"
        );
    }

    /// A pane too NARROW for a ring around the button spends those columns on
    /// the button instead, and the card is bare.
    #[test]
    fn a_pane_narrower_than_the_ring_plus_button_drops_the_ring() {
        let blocks = [button(3)];
        // A 16-column button needs 16 content columns plus the padding either
        // side, so 18 columns is the narrowest pane that can paint it at all,
        // and there is nothing left over for a ring.
        let plan = laid_out(Rect::new(0, 0, 18, 10), 46, Some(16), &blocks).expect("fits");
        assert!(
            plan.ring.is_none(),
            "18 columns cannot hold the button, its padding AND a border ring"
        );
        let plan = laid_out(Rect::new(0, 0, 20, 10), 46, Some(16), &blocks).expect("fits");
        assert!(plan.ring.is_some(), "two more columns buy the ring back");
    }

    /// THE FLOOR. A lone block with no button behind it is cut to the rows it
    /// has rather than dropped: a pane that paints nothing inside its own frame
    /// tells the user nothing at all, and this was a real regression.
    #[test]
    fn a_lone_block_is_truncated_rather_than_dropped() {
        // Nine rows of content in a pane with five: too tall for the ring, too
        // tall bare, and there is no button to fall back to.
        let plan = laid_out(Rect::new(0, 0, 30, 5), 40, None, &[block(0, 9)])
            .expect("a pane with rows must show something");
        assert!(plan.ring.is_none(), "the floor gives up the ring for rows");
        assert_eq!(plan.blocks.len(), 1);
        assert!(
            plan.blocks[0].truncated,
            "the painter must be told to mark the cut"
        );
        assert_eq!(
            plan.blocks[0].area.height, 3,
            "the rows the pane has, less the padding above and below"
        );
    }

    /// The floor never fires while a button can still carry the card: dropping
    /// the block there leaves a way out on screen, which is not "nothing".
    #[test]
    fn the_floor_does_not_fire_while_a_button_survives() {
        let plan = laid_out(
            Rect::new(0, 0, 30, 5),
            40,
            Some(16),
            &[block(0, 9), button(3)],
        )
        .expect("the button fits");
        assert_eq!(
            plan.blocks.iter().map(|b| b.index).collect::<Vec<_>>(),
            vec![1]
        );
        assert!(plan.blocks.iter().all(|b| !b.truncated));
    }

    /// Blocks are re-measured at the BARE width when the ring is given up. The
    /// regression this pins: a sentence measured at the ring's narrower measure
    /// looked one row too tall for the pane it was about to be laid out bare in,
    /// so it was dropped and the frame was painted empty.
    #[test]
    fn giving_up_the_ring_re_measures_at_the_wider_bare_measure() {
        // Five rows at the ring's 26-column measure, four at the bare 28.
        let plans = [CardBlockPlan {
            rank: 0,
            is_button: false,
        }];
        let plan = plan_pane_card(Rect::new(0, 0, 30, 6), 40, None, &plans, |_index, width| {
            if width <= 26 { 5 } else { 4 }
        })
        .expect("the bare measure fits it");
        assert!(plan.ring.is_none());
        assert_eq!(plan.content_width, 28);
        assert_eq!(plan.blocks[0].area.height, 4);
        assert!(
            !plan.blocks[0].truncated,
            "it fits whole at the wider measure, so nothing is cut"
        );
    }

    /// A button is never painted CLIPPED. `Button::render` does not clip, so a
    /// layout one column too narrow produces a truncated label that looks like a
    /// real control. The button is dropped instead, and the words it sat under
    /// stay: a card with no button still says how to launch, and a card that
    /// painted nothing said nothing at all.
    #[test]
    fn a_pane_too_narrow_to_paint_the_button_whole_plans_the_card_without_it() {
        let blocks = [block(0, 2), button(3)];
        for width in 3..=18u16 {
            let plan = laid_out(Rect::new(0, 0, width, 10), 46, Some(17), &blocks)
                .expect("the sentence survives a pane the button cannot fit");
            assert_eq!(
                plan.blocks.iter().map(|b| b.index).collect::<Vec<_>>(),
                vec![0],
                "a {width}-column pane keeps the words and gives up the button"
            );
        }
        let plan =
            laid_out(Rect::new(0, 0, 19, 10), 46, Some(17), &blocks).expect("19 columns fit");
        assert_eq!(
            plan.blocks.iter().map(|b| b.index).collect::<Vec<_>>(),
            vec![0, 1],
            "and one more column buys the button back"
        );
        assert!(plan.blocks[1].area.width >= 17);
    }

    /// A card that is nothing BUT an unpaintable button plans nothing, because
    /// there is no sentence left to keep.
    #[test]
    fn a_lone_unpaintable_button_plans_nothing() {
        assert_eq!(
            laid_out(Rect::new(0, 0, 10, 10), 46, Some(17), &[button(3)]),
            None
        );
    }

    /// The ring's minimum measure counts the padding the button sits inside.
    /// Without that the ring path accepted a pane exactly two columns too narrow
    /// and painted the button clipped inside a ring that looked fine.
    #[test]
    fn the_rings_minimum_measure_includes_the_buttons_padding() {
        let blocks = [button(3)];
        let plan = laid_out(Rect::new(0, 0, 21, 10), 46, Some(17), &blocks).expect("fits");
        if let Some(ring) = plan.ring {
            assert!(
                ring.width >= 17 + SIDE_PADDING * 2 + 2,
                "a ring must be wide enough for the button plus its padding, got {ring:?}"
            );
        }
    }

    /// The floor's `.max(1)`: a pane with fewer rows than the padding still
    /// shows one row of the sentence rather than a blank frame.
    #[test]
    fn the_floor_keeps_at_least_one_row_however_short_the_pane() {
        let four = laid_out(Rect::new(0, 0, 30, 4), 40, None, &[block(0, 9)])
            .expect("four rows must show something");
        assert_eq!(four.blocks[0].area.height, 2, "four rows less the padding");
        assert!(four.blocks[0].truncated);

        let two = laid_out(Rect::new(0, 0, 30, 2), 40, None, &[block(0, 9)])
            .expect("even two rows must show something");
        assert_eq!(
            two.blocks[0].area.height, 1,
            "the padding cannot eat the last row: a card that paints zero rows is \
             the blank frame this floor exists to prevent"
        );
        assert!(two.blocks[0].truncated);
    }

    /// The precondition, enforced rather than handled: the height arithmetic
    /// puts the button on the bottom edge, so a mid-list button would silently
    /// mis-size the card.
    #[test]
    #[should_panic(expected = "must be its last block")]
    #[cfg(debug_assertions)]
    fn a_button_that_is_not_the_last_block_is_rejected() {
        let _ = laid_out(
            Rect::new(0, 0, 60, 24),
            46,
            Some(16),
            &[button(3), block(0, 1)],
        );
    }

    #[test]
    fn nothing_at_all_is_planned_for_a_zero_sized_pane() {
        assert_eq!(
            laid_out(Rect::new(0, 0, 0, 10), 46, None, &[block(0, 1)]),
            None
        );
        assert_eq!(laid_out(Rect::new(0, 0, 40, 10), 46, None, &[]), None);
    }
}
