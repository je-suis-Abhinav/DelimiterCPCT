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
pub(crate) fn dijkstra<N, FM, FN, FS, FScore>(
    start_node: N,
    neighbours: FN,
    merge: FM,
    success: FS,
    delimiter_score: FScore,
    stats: &mut RecoveryStats,
) -> Vec<N>
where
    N: Debug + Clone + Hash + Eq + PartialEq,
    FN: Fn(bool, &N, &mut Vec<(u16, N)>, &mut RecoveryStats) -> bool,
    FM: Fn(&mut N, N, &mut RecoveryStats),
    FS: Fn(&N) -> bool,
    FScore: Fn(&N) -> i64,
{
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

        let (_, n) = todo[usize::from(c)].pop().unwrap();
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
    while let Some((_, n)) = scs_todo.pop() {
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
