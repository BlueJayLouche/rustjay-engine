//! Cue list order and group membership.
//!
//! The cue list is ordered by position — that is what Go walks — while a cue's
//! number and its group membership are separate fields. Three of them have to
//! agree, and nothing used to make them:
//!
//! - **A group's members are contiguous and immediately follow their header.**
//!   The engine assumes this in two different ways. Firing a group collects
//!   members by `parent`, ignoring position; advancing the standby playhead past
//!   a group walks forward while `parent` matches, by position
//!   (`next_standby_qid`). Break contiguity and the playhead lands inside a group
//!   that already fired, so Go re-fires members.
//! - **A cue's position is chosen with its number.** `choose_qid` numbers a new
//!   cue *after* some existing one; appending it to the end instead left the list
//!   reading Q1, Q2, Q3, Q1.11.
//! - **A group moves, copies and deletes as a block**, since that is how the list
//!   draws it.
//!
//! Everything here is a pure function over the cue slice so it can be tested
//! without a UI harness; the command handlers in [`crate::app`] apply the result.

use cuepool_core::Cue;
use rust_decimal::Decimal;
use std::ops::Range;

fn is_group(cue: &Cue) -> bool {
    matches!(cue, Cue::Group { .. })
}

/// The rows `idx` occupies: a Group header plus the members that follow it, or
/// just the one cue. Members past a break in contiguity are not included — this
/// reports what the list currently *is*, not what it should be.
pub fn span(cues: &[Cue], idx: usize) -> Range<usize> {
    let Some(cue) = cues.get(idx) else {
        return idx..idx;
    };
    if !is_group(cue) {
        return idx..idx + 1;
    }
    let group = cue.base().qid;
    let mut end = idx + 1;
    while cues
        .get(end)
        .is_some_and(|c| c.base().parent == Some(group))
    {
        end += 1;
    }
    idx..end
}

/// The span of the block containing `idx` — the group's span if `idx` is a
/// member, otherwise its own. Used to step over a whole group when reordering.
pub fn enclosing_span(cues: &[Cue], idx: usize) -> Range<usize> {
    if let Some(parent) = cues.get(idx).and_then(|c| c.base().parent)
        && let Some(header) = cues[..idx]
            .iter()
            .rposition(|c| is_group(c) && c.base().qid == parent)
    {
        let header_span = span(cues, header);
        if header_span.contains(&idx) {
            return header_span;
        }
    }
    span(cues, idx)
}

fn index_of(cues: &[Cue], qid: Decimal) -> Option<usize> {
    cues.iter().position(|c| c.base().qid == qid)
}

/// Where a cue added or duplicated "after `anchor`" belongs, and what group it
/// joins — so that its position agrees with the number `choose_qid` gives it.
///
/// A group header anchors *into* the group, as its first member, matching the
/// drop rule the cue list already documents ("drop on a Group cue to join it").
/// Since the caller selects whatever it inserts, adding repeatedly with a group
/// selected still lays the members out in the order they were added.
pub fn insertion(cues: &[Cue], anchor: Option<Decimal>) -> (usize, Option<Decimal>) {
    let Some(idx) = anchor.and_then(|qid| index_of(cues, qid)) else {
        return (cues.len(), None);
    };
    let cue = &cues[idx];
    if is_group(cue) {
        (idx + 1, Some(cue.base().qid))
    } else {
        (span(cues, idx).end, cue.base().parent)
    }
}

/// Move the block `from` so it starts at `to`, where `to` is an index in the
/// list as it stands *before* the move.
pub fn move_span(cues: &mut Vec<Cue>, from: Range<usize>, to: usize) {
    // Anywhere inside the block, or immediately after it, is where the block
    // already is — the same predicate the drag handler uses to decide whether a
    // drop moves anything.
    if from.is_empty() || (from.start..=from.end).contains(&to) {
        return;
    }
    let len = from.len();
    let block: Vec<Cue> = cues.drain(from.clone()).collect();
    // Removing the block shifts everything after it left by its length.
    let at = if to >= from.end { to - len } else { to };
    let at = at.min(cues.len());
    cues.splice(at..at, block);
}

/// Reorder by one step, taking a group's members with it and stepping over a
/// whole group rather than into one. Returns false when nothing moved.
///
/// A member will not step out of its group this way, and a top-level cue will
/// not step into one: membership changes are the drag gesture's job, and a
/// nudge key that silently regrouped cues would be worse than one that stops at
/// the boundary.
pub fn nudge(cues: &mut Vec<Cue>, idx: usize, down: bool) -> bool {
    if idx >= cues.len() {
        return false;
    }
    let block = span(cues, idx);
    let parent = cues[idx].base().parent;

    let neighbour = if down {
        if block.end >= cues.len() {
            return false;
        }
        block.end
    } else {
        match block.start.checked_sub(1) {
            Some(prev) => prev,
            None => return false,
        }
    };

    // A sibling under the same parent: swap with it directly.
    let sibling = cues[neighbour].base().parent == parent && !is_group(&cues[neighbour]);
    let target = if sibling {
        let other = span(cues, neighbour);
        if down { other.end } else { other.start }
    } else if parent.is_some() {
        // The neighbour is outside this group — moving would leave it.
        return false;
    } else {
        let other = enclosing_span(cues, neighbour);
        if down { other.end } else { other.start }
    };

    move_span(cues, block, target);
    true
}

/// The cue the selection should land on once `removed` is gone: the next cue
/// after the deleted block, or the last remaining one if it was at the end.
pub fn selection_after_removal(cues: &[Cue], removed: Range<usize>) -> Option<Decimal> {
    cues.get(removed.end)
        .or_else(|| cues[..removed.start].last())
        .map(|cue| cue.base().qid)
}

/// Copy a block, renumbering every cue in it and re-pointing members at the
/// copied header. Returns the copies in order; the caller inserts them.
pub fn duplicate_span(
    cues: &[Cue],
    block: Range<usize>,
    next_qid: impl Fn(&[Cue], Decimal) -> Decimal,
) -> Vec<Cue> {
    let mut taken: Vec<Cue> = cues.to_vec();
    let mut copies = Vec::with_capacity(block.len());
    let mut header_qid = None;

    for (offset, index) in block.clone().enumerate() {
        let mut copy = cues[index].clone();
        let original = copy.base().qid;
        // Number each copy against the growing list, so two copies cannot
        // collide with each other any more than with an existing cue.
        let qid = next_qid(&taken, original);
        copy.base_mut().qid = qid;
        if offset == 0 {
            copy.base_mut().name.push_str(" (copy)");
            if is_group(&copy) {
                header_qid = Some(qid);
            }
        } else if let Some(header) = header_qid {
            // Members follow the copied header, not the original.
            copy.base_mut().parent = Some(header);
        }
        taken.push(copy.clone());
        copies.push(copy);
    }
    copies
}

#[cfg(test)]
mod tests {
    use super::*;
    use cuepool_core::CueBase;

    fn group(qid: i64) -> Cue {
        Cue::Group {
            base: CueBase {
                qid: Decimal::from(qid),
                ..Default::default()
            },
        }
    }

    fn member(qid: Decimal, parent: Option<i64>) -> Cue {
        Cue::Dummy {
            base: CueBase {
                qid,
                parent: parent.map(Decimal::from),
                ..Default::default()
            },
        }
    }

    fn plain(qid: i64) -> Cue {
        member(Decimal::from(qid), None)
    }

    /// Q1 [Q1.1 Q1.2] Q2 Q3
    fn show() -> Vec<Cue> {
        vec![
            group(1),
            member(Decimal::new(11, 1), Some(1)),
            member(Decimal::new(12, 1), Some(1)),
            plain(2),
            plain(3),
        ]
    }

    fn qids(cues: &[Cue]) -> Vec<String> {
        cues.iter().map(|c| c.base().qid.to_string()).collect()
    }

    #[test]
    fn a_group_spans_its_contiguous_members() {
        let cues = show();
        assert_eq!(span(&cues, 0), 0..3);
        assert_eq!(span(&cues, 1), 1..2, "a member spans only itself");
        assert_eq!(span(&cues, 3), 3..4);
        assert_eq!(
            enclosing_span(&cues, 2),
            0..3,
            "a member's block is its group"
        );
        assert_eq!(enclosing_span(&cues, 3), 3..4);
    }

    #[test]
    fn a_span_stops_at_a_break_in_contiguity() {
        // A member stranded after an unrelated cue is not part of the span: the
        // engine walks by position too, so this reports what Go will see.
        let mut cues = show();
        cues.swap(2, 3);
        assert_eq!(span(&cues, 0), 0..2);
    }

    #[test]
    fn insertion_follows_the_anchor_rather_than_appending() {
        let cues = show();
        assert_eq!(
            insertion(&cues, Some(Decimal::new(11, 1))),
            (2, Some(Decimal::ONE)),
            "after a member, inside its group"
        );
        assert_eq!(
            insertion(&cues, Some(Decimal::ONE)),
            (1, Some(Decimal::ONE)),
            "a group anchors into itself, as the first member"
        );
        assert_eq!(
            insertion(&cues, Some(Decimal::from(2))),
            (4, None),
            "after a plain cue, top level"
        );
        assert_eq!(insertion(&cues, None), (5, None), "no selection appends");
        assert_eq!(
            insertion(&cues, Some(Decimal::from(99))),
            (5, None),
            "an unknown anchor appends"
        );
    }

    #[test]
    fn nudging_a_group_takes_its_members() {
        let mut cues = show();
        assert!(nudge(&mut cues, 0, true));
        assert_eq!(qids(&cues), ["2", "1", "1.1", "1.2", "3"]);
        assert!(nudge(&mut cues, 1, false));
        assert_eq!(qids(&cues), ["1", "1.1", "1.2", "2", "3"]);
    }

    #[test]
    fn nudging_steps_over_a_whole_group() {
        let mut cues = show();
        // Q2 sits after the group; one step up must clear all of it.
        assert!(nudge(&mut cues, 3, false));
        assert_eq!(qids(&cues), ["2", "1", "1.1", "1.2", "3"]);
    }

    #[test]
    fn members_reorder_within_their_group() {
        let mut cues = show();
        assert!(nudge(&mut cues, 1, true));
        assert_eq!(qids(&cues), ["1", "1.2", "1.1", "2", "3"]);
        assert!(
            cues[1].base().parent == Some(Decimal::ONE)
                && cues[2].base().parent == Some(Decimal::ONE),
            "reordering within a group keeps both members in it"
        );
    }

    #[test]
    fn a_member_will_not_nudge_out_of_its_group() {
        let mut cues = show();
        assert!(!nudge(&mut cues, 1, false), "first member, up");
        assert!(!nudge(&mut cues, 2, true), "last member, down");
        assert_eq!(qids(&cues), ["1", "1.1", "1.2", "2", "3"]);
    }

    #[test]
    fn nudging_stops_at_the_ends() {
        let mut cues = show();
        assert!(!nudge(&mut cues, 0, false));
        assert!(!nudge(&mut cues, 4, true));
        assert_eq!(qids(&cues), ["1", "1.1", "1.2", "2", "3"]);
    }

    #[test]
    fn moving_a_span_lands_where_asked_in_both_directions() {
        let mut cues = show();
        move_span(&mut cues, 0..3, 5);
        assert_eq!(qids(&cues), ["2", "3", "1", "1.1", "1.2"]);
        move_span(&mut cues, 2..5, 0);
        assert_eq!(qids(&cues), ["1", "1.1", "1.2", "2", "3"]);
    }

    #[test]
    fn selection_moves_to_the_next_cue_then_falls_back_to_the_previous() {
        let cues = show();
        assert_eq!(selection_after_removal(&cues, 0..3), Some(Decimal::from(2)));
        assert_eq!(selection_after_removal(&cues, 4..5), Some(Decimal::from(2)));
        assert_eq!(selection_after_removal(&cues, 0..5), None);
    }

    #[test]
    fn duplicating_a_group_renumbers_members_and_repoints_them() {
        let cues = show();
        let copies = duplicate_span(&cues, 0..3, |cues, after| {
            let show = cuepool_core::ShowFile {
                cues: cues.to_vec(),
                ..Default::default()
            };
            show.choose_qid(Some(after))
        });
        assert_eq!(copies.len(), 3);
        let header = copies[0].base().qid;
        assert!(
            !qids(&cues).contains(&header.to_string()),
            "the copied header takes a free number"
        );
        assert!(copies[0].base().name.ends_with(" (copy)"));
        for member in &copies[1..] {
            assert_eq!(
                member.base().parent,
                Some(header),
                "members follow the copied header, not the original"
            );
            assert!(!member.base().name.ends_with(" (copy)"));
        }
        let numbers: Vec<_> = copies.iter().map(|c| c.base().qid).collect();
        assert!(
            numbers
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                == numbers.len(),
            "copies must not collide with each other"
        );
    }
}
