use std::{fmt::Debug, hash::Hash};

use indexmap::{
    indexmap,
    map::{Entry, IndexMap},
};

use super::parser::RecoveryStats;

/// Starting at `start_node`, return, in arbitrary order, all least-cost success nodes.
///
/// * `neighbours` takes a node `n` and returns an iterator consisting of all `n`'s neighbouring
///   nodes.
/// * `success` takes a node `n` and returns `true` if it is a success node or `false` otherwise.
///
/// The name of this function isn't entirely accurate: this isn't Dijkstra's original algorithm or
/// one of its well-known variants. However, unlike the astar_all function it doesn't expect a
/// heuristic and it also filters out some duplicates.
pub(crate) fn dijkstra<N, FM, FN, FS, FScore, FPriority, FD>(
    mut start_node: N,
    neighbours: FN,
    merge: FM,
    success: FS,
    delimiter_score: FScore,
    prioritize_frontier: bool,
    localization_priority: FPriority,
    mut retain_nondominated: FD,
    stats: &mut RecoveryStats,
) -> Vec<N>
where
    N: Debug + Clone + Hash + Eq + PartialEq,
    FN: Fn(bool, &N, &mut Vec<(u16, N)>, &mut RecoveryStats) -> bool,
    FM: Fn(&mut N, N, &mut RecoveryStats),
    FS: Fn(&N) -> bool,
    FScore: Fn(&N) -> i64,
    FPriority: Fn(&N) -> i64,
    FD: FnMut(&mut N, u16, &mut RecoveryStats) -> bool,
{
    retain_nondominated(&mut start_node, 0, stats);
    let mut scs_nodes = Vec::new();
    let mut todo: Vec<IndexMap<N, N>> = vec![indexmap![start_node.clone() => start_node]];
    let mut c: u16 = 0;
    let mut next = Vec::new();
    loop {
        if todo[usize::from(c)].is_empty() {
            c = c.checked_add(1).unwrap();
            if usize::from(c) == todo.len() {
                return Vec::new();
            }
            continue;
        }

        let (n, prioritized, reordered) = pop_preferred(
            &mut todo[usize::from(c)],
            prioritize_frontier,
            &localization_priority,
        )
        .unwrap();
        if prioritized {
            stats.localization_priority_pops += 1;
        }
        if reordered {
            stats.localization_priority_reorders += 1;
        }
        stats.configurations_popped += 1;
        let score = delimiter_score(&n);
        *stats.delimiter_score_distribution.entry(score).or_default() += 1;
        if success(&n) {
            stats.minimum_recovery_cost = Some(c);
            stats.minimum_cost_success_delimiter_scores.push(score);
            scs_nodes.push(n);
            break;
        }

        if !neighbours(true, &n, &mut next, stats) {
            stats.timed_out = true;
            let discarded = todo.iter().map(|bucket| bucket.len() as u64).sum::<u64>();
            stats.configurations_discarded_timeout += discarded;
            stats.configurations_discarded += discarded;
            return Vec::new();
        }
        for (nbr_cost, nbr) in next.drain(..) {
            stats.configurations_generated += 1;
            let mut nbr = nbr;
            if !retain_nondominated(&mut nbr, nbr_cost, stats) {
                continue;
            }
            let off = usize::from(nbr_cost);
            todo.resize(todo.len() + off + 1, IndexMap::new());
            match todo[off].entry(nbr.clone()) {
                Entry::Vacant(e) => {
                    e.insert(nbr);
                }
                Entry::Occupied(mut e) => {
                    stats.configurations_merged += 1;
                    merge(e.get_mut(), nbr, stats);
                }
            }
        }
    }

    let higher_cost = todo
        .iter()
        .enumerate()
        .filter(|(cost, _)| *cost > usize::from(c))
        .map(|(_, bucket)| bucket.len() as u64)
        .sum::<u64>();
    stats.configurations_discarded_higher_cost += higher_cost;
    stats.configurations_discarded += higher_cost;
    let mut scs_todo = todo
        .drain(usize::from(c)..usize::from(c) + 1)
        .next()
        .unwrap();
    while let Some((n, prioritized, reordered)) =
        pop_preferred(&mut scs_todo, prioritize_frontier, &localization_priority)
    {
        if prioritized {
            stats.localization_priority_pops += 1;
        }
        if reordered {
            stats.localization_priority_reorders += 1;
        }
        stats.configurations_popped += 1;
        let score = delimiter_score(&n);
        *stats.delimiter_score_distribution.entry(score).or_default() += 1;
        if success(&n) {
            stats.minimum_cost_success_delimiter_scores.push(score);
            scs_nodes.push(n);
            continue;
        }
        if !neighbours(false, &n, &mut next, stats) {
            stats.timed_out = true;
            let discarded = scs_todo.len() as u64;
            stats.configurations_discarded_timeout += discarded;
            stats.configurations_discarded += discarded;
            return Vec::new();
        }
        for (nbr_cost, nbr) in next.drain(..) {
            stats.configurations_generated += 1;
            let mut nbr = nbr;
            if !retain_nondominated(&mut nbr, nbr_cost, stats) {
                continue;
            }
            if nbr_cost == c {
                match scs_todo.entry(nbr.clone()) {
                    Entry::Vacant(e) => {
                        e.insert(nbr);
                    }
                    Entry::Occupied(mut e) => {
                        stats.configurations_merged += 1;
                        merge(e.get_mut(), nbr, stats);
                    }
                }
            } else {
                stats.configurations_discarded_cost += 1;
                stats.configurations_discarded += 1;
            }
        }
    }

    scs_nodes
}

fn pop_preferred<N, FPriority>(
    bucket: &mut IndexMap<N, N>,
    prioritize_frontier: bool,
    localization_priority: &FPriority,
) -> Option<(N, bool, bool)>
where
    N: Hash + Eq,
    FPriority: Fn(&N) -> i64,
{
    if !prioritize_frontier {
        let (_, node) = bucket.pop()?;
        return Some((node, false, false));
    }
    let mut best_index = None;
    let mut best_priority = i64::MIN;
    for (index, node) in bucket.values().enumerate() {
        let priority = localization_priority(node);
        if priority >= best_priority {
            best_priority = priority;
            best_index = Some(index);
        }
    }
    let best_index = best_index?;
    let reordered = best_index + 1 != bucket.len();
    let (_, node) = bucket.shift_remove_index(best_index)?;
    Some((node, best_priority > 0, reordered))
}

#[cfg(test)]
mod test {
    use std::cell::RefCell;

    use super::dijkstra;
    use crate::parser::RecoveryStats;

    #[test]
    fn localization_reorders_only_within_the_same_cost_bucket() {
        for prioritize_frontier in [false, true] {
            let popped = RefCell::new(Vec::new());
            let result = dijkstra(
                0u8,
                |_, node, next, _| {
                    if *node == 0 {
                        next.push((1, 2));
                        next.push((1, 1));
                        next.push((2, 3));
                    }
                    true
                },
                |_, _, _| {},
                |node| {
                    popped.borrow_mut().push(*node);
                    false
                },
                |_| 0,
                prioritize_frontier,
                |node| i64::from(*node == 2 || *node == 3),
                |_, _, _| true,
                &mut RecoveryStats::default(),
            );

            assert!(result.is_empty());
            assert_eq!(
                *popped.borrow(),
                if prioritize_frontier {
                    vec![0, 2, 1, 3]
                } else {
                    vec![0, 1, 2, 3]
                }
            );
        }
    }

    #[test]
    fn timeout_is_reported_as_incomplete_for_all_dominance_localization_modes() {
        for (dominance, localization) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let mut stats = RecoveryStats::default();
            let result = dijkstra(
                0u8,
                |_, node, next, _| {
                    if *node == 0 {
                        next.push((1, 2));
                        next.push((1, 1));
                        true
                    } else {
                        false
                    }
                },
                |_, _, _| {},
                |_| false,
                |_| 0,
                localization,
                |node| i64::from(*node == 2),
                |_, _, stats| {
                    if dominance {
                        stats.dominance_candidates_checked += 1;
                    }
                    true
                },
                &mut stats,
            );

            assert!(result.is_empty());
            assert!(stats.timed_out);
            assert_eq!(stats.minimum_recovery_cost, None);
            assert_eq!(stats.configurations_generated, 2);
            assert_eq!(stats.configurations_popped, 2);
            assert_eq!(stats.configurations_discarded_timeout, 1);
            assert_eq!(stats.localization_priority_pops > 0, localization);
            assert_eq!(stats.dominance_candidates_checked > 0, dominance);
        }
    }

    #[test]
    fn timeout_inside_minimum_cost_bucket_discards_partial_successes() {
        let mut stats = RecoveryStats::default();
        let result = dijkstra(
            0u8,
            |_, node, next, _| {
                if *node == 0 {
                    next.push((1, 1));
                    next.push((1, 2));
                    next.push((1, 3));
                    true
                } else {
                    false
                }
            },
            |_, _, _| {},
            |node| *node == 1 || *node == 3,
            |_| 0,
            false,
            |_| 0,
            |_, _, _| true,
            &mut stats,
        );

        assert!(result.is_empty());
        assert!(stats.timed_out);
        assert_eq!(stats.minimum_recovery_cost, Some(1));
        assert_eq!(stats.configurations_discarded_timeout, 1);
    }
}
