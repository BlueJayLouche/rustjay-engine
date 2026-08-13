/// After firing the cue at `start_idx`, return the next cue to put on standby:
/// skip the cues that auto-fired alongside it — a Group's members (everything up
/// to the next Group), or a `WithLast`/`AfterLast` continuation chain — and land
/// on the next manually-triggered cue. `None` at the end of the list.
pub(crate) fn next_standby_qid(
    cues: &[cuepool_core::Cue],
    start_idx: usize,
) -> Option<rust_decimal::Decimal> {
    let mut i = start_idx + 1;
    if matches!(cues.get(start_idx), Some(cuepool_core::Cue::Group { .. })) {
        // A group fired all its members (cues whose parent is this group) — skip
        // past them to the next standby.
        let gid = cues[start_idx].base().qid;
        while i < cues.len() && cues[i].base().parent == Some(gid) {
            i += 1;
        }
    } else {
        while i < cues.len()
            && matches!(
                cues[i].base().trigger,
                cuepool_core::TriggerMode::WithLast | cuepool_core::TriggerMode::AfterLast
            )
        {
            i += 1;
        }
    }
    cues.get(i).map(|c| c.base().qid)
}

/// The first enabled `AfterLast` cue directly following `finished_qid` — the
/// next link of a completion chain. Disabled followers are skipped; anything
/// else (or an unknown qid) ends the chain.
pub(crate) fn next_after_last(
    cues: &[cuepool_core::Cue],
    finished_qid: rust_decimal::Decimal,
) -> Option<&cuepool_core::Cue> {
    let idx = cues.iter().position(|c| c.base().qid == finished_qid)?;
    cues[idx + 1..]
        .iter()
        .take_while(|c| c.base().trigger == cuepool_core::TriggerMode::AfterLast)
        .find(|c| c.enabled())
}

/// Follow a goto chain from `first_target` to the first non-goto cue. Returns
/// `None` on a cycle (including a self-target) or a dead end, so the caller never
/// recurses into `play_cue` indefinitely. `goto_qid` is the originating goto cue.
pub(crate) fn resolve_goto_target(
    cues: &[cuepool_core::Cue],
    goto_qid: rust_decimal::Decimal,
    first_target: rust_decimal::Decimal,
) -> Option<rust_decimal::Decimal> {
    let mut current = first_target;
    let mut visited = std::collections::HashSet::from([goto_qid]);
    loop {
        if !visited.insert(current) {
            return None; // cycle
        }
        match cues.iter().find(|c| c.base().qid == current) {
            Some(cuepool_core::Cue::Goto { target_qid, .. }) => current = *target_qid,
            Some(_) => return Some(current),
            None => return None, // dead end
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn after_last_chain_fires_one_link_at_a_time() {
        use cuepool_core::TriggerMode::{AfterLast, Go};
        let cues = vec![dummy(1, Go), dummy(2, AfterLast), dummy(3, AfterLast), dummy(4, Go)];
        // Each completion fires exactly the next link; a non-AfterLast cue ends it.
        let q = |n: i64| rust_decimal::Decimal::from(n);
        assert_eq!(next_after_last(&cues, q(1)).map(|c| c.base().qid), Some(q(2)));
        assert_eq!(next_after_last(&cues, q(2)).map(|c| c.base().qid), Some(q(3)));
        assert_eq!(next_after_last(&cues, q(3)), None);
        assert_eq!(next_after_last(&cues, q(99)), None);

        // A disabled link is skipped over, not a dead end.
        let mut cues = cues;
        cues[1].base_mut().enabled = false;
        assert_eq!(next_after_last(&cues, q(1)).map(|c| c.base().qid), Some(q(3)));
    }

    fn dummy(qid: i64, trigger: cuepool_core::TriggerMode) -> cuepool_core::Cue {
        cuepool_core::Cue::Dummy {
            base: cuepool_core::CueBase {
                qid: rust_decimal::Decimal::from(qid),
                trigger,
                ..Default::default()
            },
        }
    }
    fn group(qid: i64) -> cuepool_core::Cue {
        cuepool_core::Cue::Group {
            base: cuepool_core::CueBase {
                qid: rust_decimal::Decimal::from(qid),
                ..Default::default()
            },
        }
    }
    fn member(qid: i64, group: i64) -> cuepool_core::Cue {
        cuepool_core::Cue::Dummy {
            base: cuepool_core::CueBase {
                qid: rust_decimal::Decimal::from(qid),
                parent: Some(rust_decimal::Decimal::from(group)),
                ..Default::default()
            },
        }
    }

    #[test]
    fn step_skips_withlast_and_afterlast_chain() {
        use cuepool_core::TriggerMode::*;
        let cues = vec![dummy(1, Go), dummy(2, WithLast), dummy(3, AfterLast), dummy(4, Go)];
        // Going Q1 also auto-fires Q2/Q3 -> next standby is Q4.
        assert_eq!(next_standby_qid(&cues, 0), Some(rust_decimal::Decimal::from(4)));
    }

    #[test]
    fn step_plain_go_advances_by_one() {
        use cuepool_core::TriggerMode::Go;
        let cues = vec![dummy(1, Go), dummy(2, Go), dummy(3, Go)];
        assert_eq!(next_standby_qid(&cues, 0), Some(rust_decimal::Decimal::from(2)));
    }

    #[test]
    fn step_over_group_skips_members() {
        use cuepool_core::TriggerMode::Go;
        // Group Q10 owns Q11/Q12; Q30 is a free cue after the group.
        let cues = vec![group(10), member(11, 10), member(12, 10), dummy(30, Go)];
        // Going group Q10 fires its members -> next standby is the free cue Q30.
        assert_eq!(next_standby_qid(&cues, 0), Some(rust_decimal::Decimal::from(30)));
    }

    #[test]
    fn step_at_end_returns_none() {
        let cues = vec![dummy(1, cuepool_core::TriggerMode::Go)];
        assert_eq!(next_standby_qid(&cues, 0), None);
    }

    fn goto(qid: i64, target: i64) -> cuepool_core::Cue {
        cuepool_core::Cue::Goto {
            base: cuepool_core::CueBase {
                qid: rust_decimal::Decimal::from(qid),
                ..Default::default()
            },
            target_qid: rust_decimal::Decimal::from(target),
        }
    }
    fn dec(n: i64) -> rust_decimal::Decimal {
        rust_decimal::Decimal::from(n)
    }

    #[test]
    fn goto_resolves_direct_target() {
        let cues = vec![goto(1, 2), dummy(2, cuepool_core::TriggerMode::Go)];
        assert_eq!(resolve_goto_target(&cues, dec(1), dec(2)), Some(dec(2)));
    }

    #[test]
    fn goto_resolves_through_chain() {
        // Q1 -> Q2(goto) -> Q3(real)
        let cues = vec![goto(1, 2), goto(2, 3), dummy(3, cuepool_core::TriggerMode::Go)];
        assert_eq!(resolve_goto_target(&cues, dec(1), dec(2)), Some(dec(3)));
    }

    #[test]
    fn goto_self_target_is_none() {
        let cues = vec![goto(1, 1)];
        assert_eq!(resolve_goto_target(&cues, dec(1), dec(1)), None);
    }

    #[test]
    fn goto_cycle_is_none() {
        // Q1 -> Q2 -> Q1 : the bug that crashed (stack overflow); must be None.
        let cues = vec![goto(1, 2), goto(2, 1)];
        assert_eq!(resolve_goto_target(&cues, dec(1), dec(2)), None);
    }

    #[test]
    fn goto_dead_end_is_none() {
        let cues = vec![goto(1, 99)];
        assert_eq!(resolve_goto_target(&cues, dec(1), dec(99)), None);
    }
}
