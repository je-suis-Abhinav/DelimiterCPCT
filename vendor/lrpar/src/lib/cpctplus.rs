use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    fmt::Debug,
    hash::{Hash, Hasher},
};

// web_time can be used on non-wasm32 but to avoid the dependency.
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
#[cfg(target_arch = "wasm32")]
use web_time::Instant;

use cactus::Cactus;
use cfgrammar::{Span, TIdx};
use lrtable::{Action, StIdx};
use num_traits::{AsPrimitive, PrimInt, Unsigned};

use super::{
    Lexeme, LexerTypes,
    dijkstra::dijkstra,
    parser::{
        AStackType, DelimiterOrigin, DelimiterOriginKind, DelimiterScoreContext, DelimiterState,
        ParseRepair, Parser, Recoverer, RecoveryStats,
        delimiter_char_from_terminal_name,
    },
};

const PARSE_AT_LEAST: usize = 3; // N in Corchuelo et al.
const TRY_PARSE_AT_MOST: usize = 250;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Repair<StorageT> {
    /// Insert a `Symbol::Token` with idx `token_idx`.
    InsertTerm(TIdx<StorageT>),
    /// Delete a symbol.
    Delete,
    /// Shift a symbol.
    Shift,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum RepairMerge<StorageT> {
    Repair(Repair<StorageT>),
    Terminator,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RepairVariant<StorageT> {
    repairs: Cactus<RepairMerge<StorageT>>,
    delimiter_state: DelimiterState,
}

impl<StorageT: PrimInt + Unsigned> RepairVariant<StorageT> {
    fn last_repair(&self) -> Option<Repair<StorageT>> {
        match *self.repairs.val().unwrap() {
            RepairMerge::Repair(r) => Some(r),
            RepairMerge::Terminator => None,
        }
    }

    fn trailing_shifts(&self) -> usize {
        self.repairs
            .vals()
            .take_while(|repair| {
                matches!(
                    **repair,
                    RepairMerge::Repair(Repair::Shift)
                )
            })
            .count()
    }

    fn with_repair(&self, repair: Option<Repair<StorageT>>, delimiter_state: DelimiterState) -> Self {
        Self {
            repairs: repair.map_or_else(
                || self.repairs.clone(),
                |repair| self.repairs.child(RepairMerge::Repair(repair)),
            ),
            delimiter_state,
        }
    }
}

#[derive(Clone, Debug)]
struct PathFNode<StorageT> {
    pstack: Cactus<StIdx<StorageT>>,
    laidx: usize,
    cf: u16,
    variants: Vec<RepairVariant<StorageT>>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct DominanceKey<StorageT: Hash> {
    pstack: Cactus<StIdx<StorageT>>,
    laidx: usize,
    ends_with_delete: bool,
    trailing_shifts: usize,
    delimiter_state: DelimiterState,
}

fn dominance_key<StorageT: Hash + PrimInt + Unsigned>(
    pstack: &Cactus<StIdx<StorageT>>,
    laidx: usize,
    variant: &RepairVariant<StorageT>,
) -> DominanceKey<StorageT> {
    DominanceKey {
        pstack: pstack.clone(),
        laidx,
        ends_with_delete: variant.last_repair() == Some(Repair::Delete),
        trailing_shifts: variant.trailing_shifts(),
        delimiter_state: variant.delimiter_state.clone(),
    }
}

fn retain_nondominated_variants<StorageT: Hash + PrimInt + Unsigned>(
    node: &mut PathFNode<StorageT>,
    cost: u16,
    best_costs: &mut HashMap<DominanceKey<StorageT>, u16>,
    stats: &mut RecoveryStats,
) -> bool {
    let pstack = node.pstack.clone();
    let laidx = node.laidx;
    node.variants.retain(|variant| {
        stats.dominance_candidates_checked += 1;
        let key = dominance_key(&pstack, laidx, variant);
        match best_costs.get_mut(&key) {
            None => {
                best_costs.insert(key, cost);
                true
            }
            Some(best_cost) => {
                stats.dominance_exact_key_matches += 1;
                if *best_cost < cost {
                    stats.configurations_discarded_dominance += 1;
                    stats.dominance_variants_discarded += 1;
                    stats.dominance_discarded_cost_difference +=
                        u64::from(cost - *best_cost);
                    stats.configurations_discarded += 1;
                    false
                } else {
                    if cost < *best_cost {
                        *best_cost = cost;
                    }
                    true
                }
            }
        }
    });
    !node.variants.is_empty()
}

struct RepairCandidate<LexemeT, StorageT>
where
    LexemeT: Lexeme<StorageT>,
    StorageT: Hash,
{
    repairs: Vec<ParseRepair<LexemeT, StorageT>>,
    delimiter_state: DelimiterState,
}

impl<StorageT: PrimInt + Unsigned> PathFNode<StorageT> {
    fn last_repair(&self) -> Option<Repair<StorageT>> {
        self.variants.first().and_then(RepairVariant::last_repair)
    }

    fn continuation_compatible(&self, other: &Self) -> bool {
        let (Some(self_variant), Some(other_variant)) =
            (self.variants.first(), other.variants.first())
        else {
            return false;
        };
        let self_delete = self_variant.last_repair() == Some(Repair::Delete);
        let other_delete = other_variant.last_repair() == Some(Repair::Delete);
        self_delete == other_delete
            && self_variant.trailing_shifts() == other_variant.trailing_shifts()
    }
}

impl<StorageT: Hash + PrimInt + Unsigned> Hash for PathFNode<StorageT> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.pstack.hash(state);
        self.laidx.hash(state);
    }
}

impl<StorageT: PrimInt + Unsigned> PartialEq for PathFNode<StorageT> {
    fn eq(&self, other: &PathFNode<StorageT>) -> bool {
        if self.laidx != other.laidx || self.pstack != other.pstack {
            return false;
        }
        // Compatible histories share their trailing-shift count and delete-ending status.
        self.continuation_compatible(other)
    }
}

impl<StorageT: PrimInt + Unsigned> Eq for PathFNode<StorageT> {}

fn merge_variants<StorageT: PrimInt + Unsigned>(
    existing: &mut Vec<RepairVariant<StorageT>>,
    incoming: Vec<RepairVariant<StorageT>>,
) -> u64 {
    let mut duplicates = 0;
    for variant in incoming {
        if existing.contains(&variant) {
            duplicates += 1;
        } else {
            existing.push(variant);
        }
    }
    duplicates
}

struct CPCTPlus<
    'a,
    'b: 'a,
    'input: 'b,
    StorageT: 'static + Eq + Hash + PrimInt + Unsigned,
    LexerTypesT: LexerTypes<StorageT = StorageT>,
    ActionT: 'a,
    ParamT: Clone,
> where
    usize: AsPrimitive<StorageT>,
{
    parser: &'a Parser<'a, 'b, 'input, StorageT, LexerTypesT, ActionT, ParamT>,
    delimiter_dominance: bool,
}

pub(super) fn recoverer<
    'a,
    StorageT: 'static + Debug + Eq + Hash + PrimInt + Unsigned,
    LexerTypesT: LexerTypes<StorageT = StorageT>,
    ActionT: 'a,
    ParamT: Clone,
>(
    parser: &'a Parser<StorageT, LexerTypesT, ActionT, ParamT>,
    delimiter_dominance: bool,
) -> Box<dyn Recoverer<StorageT, LexerTypesT, ActionT, ParamT> + 'a>
where
    usize: AsPrimitive<StorageT>,
{
    Box::new(CPCTPlus {
        parser,
        delimiter_dominance,
    })
}

impl<
    'a,
    'b: 'a,
    'input: 'b,
    StorageT: 'static + Debug + Eq + Hash + PrimInt + Unsigned,
    LexerTypesT: LexerTypes<StorageT = StorageT>,
    ActionT: 'a,
    ParamT: Clone,
> Recoverer<StorageT, LexerTypesT, ActionT, ParamT>
    for CPCTPlus<'a, 'b, 'input, StorageT, LexerTypesT, ActionT, ParamT>
where
    usize: AsPrimitive<StorageT>,
{
    fn recover(
        &self,
        finish_by: Instant,
        parser: &Parser<StorageT, LexerTypesT, ActionT, ParamT>,
        in_laidx: usize,
        in_pstack: &mut Vec<StIdx<StorageT>>,
        astack: &mut Vec<AStackType<LexerTypesT::LexemeT, ActionT>>,
        spans: &mut Vec<Span>,
        delimiter_state: &DelimiterState,
    ) -> (
        usize,
        Vec<Vec<ParseRepair<LexerTypesT::LexemeT, StorageT>>>,
        DelimiterState,
        RecoveryStats,
    ) {
        let started = Instant::now();
        let mut stats = RecoveryStats::default();
        stats.delimiter_dominance_enabled = self.delimiter_dominance;
        let error_delimiter = parser.delimiter_for_lexeme(&parser.next_lexeme(in_laidx));
        let delimiter_score_context = DelimiterScoreContext::new(delimiter_state, error_delimiter);
        // This function implements a minor variant of the algorithm from "Repairing syntax errors
        // in LR parsers" by Rafael Corchuelo, Jose A. Perez, Antonio Ruiz, and Miguel Toro.
        //
        // The major differences are: we change the shift() function (see the comment therein)
        // along the lines suggested by KimYi; and we simplify the criteria for a successful node
        // (since the numbers in the Corchuelo paper don't scale well to arbitrary grammars).
        //
        // Because we want to create a parse tree even when error recovery has happened, we can be
        // a bit clever. In our first stage, we try and find repair sequences using a cactus stack
        // to represent the parse stack, but we don't try and create/alter the parse tree. Once
        // we've found valid repairs, we select one arbitrarily (as do Corchuelo) and then replay
        // it, this time turning on parse tree creation/alteration. Thus we only pay the costs of
        // creating the parse tree for the one parse that we need it. This has a vaguely similar
        // flavour to part of the ALL(*) algorithm (where, when the LL parser gets to a point of
        // ambiguity, it fires up non-LL sub-parsers, which then tell the LL parser which path it
        // should take).
        let mut start_cactus_pstack = Cactus::new();
        for st in in_pstack.iter() {
            start_cactus_pstack = start_cactus_pstack.child(*st);
        }

        let start_node = PathFNode {
            pstack: start_cactus_pstack,
            laidx: in_laidx,
            cf: 0,
            variants: vec![RepairVariant {
                repairs: Cactus::new().child(RepairMerge::Terminator),
                delimiter_state: delimiter_state.clone(),
            }],
        };
        let mut dominance_index = HashMap::new();
        let astar_cnds = dijkstra(
            start_node,
            |explore_all, n, nbrs, stats| {
                // Calculate n's neighbours.

                if Instant::now() >= finish_by {
                    return false;
                }

                match n.last_repair() {
                    Some(Repair::Delete) => {
                        // We follow Corcheulo et al.'s suggestions and never follow Deletes with
                        // Inserts.
                    }
                    _ => {
                        if explore_all {
                            self.insert(n, nbrs, stats);
                        }
                    }
                }
                if explore_all {
                    self.delete(n, nbrs, stats);
                }
                self.shift(n, nbrs, stats);
                true
            },
            |old, new, stats| {
                // merge new_n into old_n

                let duplicate_histories = merge_variants(&mut old.variants, new.variants);
                stats.configurations_discarded_duplicate += duplicate_histories;
                stats.configurations_discarded += duplicate_histories;
            },
            |n| {
                // Is n a success node?

                // As presented in both Corchuelo et al. and Kim Yi, one type of success is if N
                // symbols are parsed in one go. Indeed, without such a check, the search space
                // quickly becomes too big. There isn't a way of encoding this check in r3s_n, so
                // we check instead for its result: if the last N ('PARSE_AT_LEAST' in this
                // library) repairs are shifts, then we've found a success node.
                if n
                    .variants
                    .first()
                    .is_some_and(|variant| ends_with_parse_at_least_shifts(&variant.repairs))
                {
                    return true;
                }

                matches!(
                    parser
                        .stable
                        .action(*n.pstack.val().unwrap(), parser.next_tidx(n.laidx)),
                    Action::Accept
                )
            },
            |n| {
                n.variants
                    .iter()
                    .map(|variant| delimiter_score_context.score(&variant.delimiter_state))
                    .max()
                    .unwrap_or_default()
            },
            |n, cost, stats| {
                if self.delimiter_dominance {
                    retain_nondominated_variants(n, cost, &mut dominance_index, stats)
                } else {
                    true
                }
            },
            &mut stats,
        );

        if astar_cnds.is_empty() {
            stats.elapsed = started.elapsed();
            return (in_laidx, vec![], delimiter_state.clone(), stats);
        }

        let Some(full_rprs) = self.collect_repairs(finish_by, in_laidx, astar_cnds) else {
            stats.timed_out = true;
            stats.elapsed = started.elapsed();
            return (in_laidx, vec![], delimiter_state.clone(), stats);
        };
        stats.repair_sequences_after_collection =
            Some(full_rprs.iter().map(|candidates| candidates.len() as u64).sum());
        let mut rnk_rprs = rank_cnds(
            parser, finish_by, in_laidx, in_pstack, full_rprs, &mut stats,
        );
        if !stats.timed_out {
            stats.repair_sequences_after_continuation = Some(rnk_rprs.len() as u64);
        }
        if rnk_rprs.is_empty() {
            stats.elapsed = started.elapsed();
            return (in_laidx, vec![], delimiter_state.clone(), stats);
        }
        simplify_repairs(parser, &mut rnk_rprs, delimiter_state, in_laidx);
        stats.repair_sequences_after_simplification = Some(rnk_rprs.len() as u64);
        let recovered_delimiter_state = rnk_rprs[0].delimiter_state.clone();
        stats.selected_repair_delimiter_score =
            Some(delimiter_score_context.score(&recovered_delimiter_state));
        let laidx = apply_repairs(
            parser,
            in_laidx,
            in_pstack,
            &mut Some(astack),
            &mut Some(spans),
            &rnk_rprs[0].repairs,
        );

        stats.elapsed = started.elapsed();
        (
            laidx,
            rnk_rprs.into_iter().map(|candidate| candidate.repairs).collect(),
            recovered_delimiter_state,
            stats,
        )
    }
}

impl<
    'a,
    'b: 'a,
    'input: 'b,
    StorageT: 'static + Debug + Eq + Hash + PrimInt + Unsigned,
    LexerTypesT: LexerTypes<StorageT = StorageT>,
    ActionT: 'a,
    ParamT: Clone,
> CPCTPlus<'a, 'b, 'input, StorageT, LexerTypesT, ActionT, ParamT>
where
    usize: AsPrimitive<StorageT>,
{
    fn insert(
        &self,
        n: &PathFNode<StorageT>,
        nbrs: &mut Vec<(u16, PathFNode<StorageT>)>,
        stats: &mut RecoveryStats,
    ) {
        let laidx = n.laidx;
        for tidx in self.parser.stable.state_actions(*n.pstack.val().unwrap()) {
            if tidx == self.parser.grm.eof_token_idx() {
                continue;
            }
            stats.insert_transitions_attempted += 1;

            let next_lexeme = self.parser.next_lexeme(n.laidx);
            let new_lexeme = Lexeme::new_faulty(
                StorageT::from(u32::from(tidx)).unwrap(),
                next_lexeme.span().start(),
                0,
            );
            let (new_laidx, n_pstack) = self.parser.lr_cactus(
                Some(new_lexeme),
                laidx,
                laidx + 1,
                n.pstack.clone(),
                &mut None,
            );
            if new_laidx > laidx {
                let nn = PathFNode {
                    pstack: n_pstack,
                    laidx: n.laidx,
                    cf: n
                        .cf
                        .checked_add(u16::from((self.parser.token_cost)(tidx)))
                        .unwrap(),
                    variants: n
                        .variants
                        .iter()
                        .map(|variant| {
                            let delimiter_state = advance_inserted_terminal(
                                self.parser,
                                &variant.delimiter_state,
                                tidx,
                                laidx,
                            );
                            variant.with_repair(
                                Some(Repair::InsertTerm(tidx)),
                                delimiter_state,
                            )
                        })
                        .collect(),
                };
                nbrs.push((nn.cf, nn));
            }
        }
    }

    fn delete(
        &self,
        n: &PathFNode<StorageT>,
        nbrs: &mut Vec<(u16, PathFNode<StorageT>)>,
        stats: &mut RecoveryStats,
    ) {
        stats.delete_transitions_attempted += 1;
        if n.laidx == self.parser.lexemes.len() {
            return;
        }

        let la_tidx = self.parser.next_tidx(n.laidx);
        let cost = (self.parser.token_cost)(la_tidx);
        let nn = PathFNode {
            pstack: n.pstack.clone(),
            laidx: n.laidx + 1,
            cf: n.cf.checked_add(u16::from(cost)).unwrap(),
            variants: n
                .variants
                .iter()
                .map(|variant| {
                    variant.with_repair(Some(Repair::Delete), variant.delimiter_state.clone())
                })
                .collect(),
        };
        nbrs.push((nn.cf, nn));
    }

    fn shift(
        &self,
        n: &PathFNode<StorageT>,
        nbrs: &mut Vec<(u16, PathFNode<StorageT>)>,
        stats: &mut RecoveryStats,
    ) {
        stats.shift_transitions_attempted += 1;
        // Forward move rule (ER3)
        //
        // Note the rule in Corchuelo et al. is confusing and, I think, wrong. It reads:
        //   (S, I) \rightarrow_{LR*} (S', I')
        //   \wedge (j = N \vee 0 < j < N \wedge f(q_r, t_{j + 1} \in {accept, error})
        // First I think the bracketing would be clearer if written as:
        //   j = N \vee (0 < j < N \wedge f(q_r, t_{j + 1} \in {accept, error})
        // And I think the condition should be:
        //   j = N \vee (0 <= j < N \wedge f(q_r, t_{j + 1} \in {accept, error})
        // because there's no reason that any symbols need to be shifted in order for an accept
        // (or, indeed an error) state to be reached.
        //
        // So the full rule should, I think, be:
        //   (S, I) \rightarrow_{LR*} (S', I')
        //   \wedge (j = N \vee (0 <= j < N \wedge f(q_r, t_{j + 1} \in {accept, error}))
        //
        // That said, as KimYi somewhat obliquely mention, generating multiple shifts in one go is
        // a bad idea: it means that we miss out on some minimal cost repairs. Instead, we should
        // only generate one shift at a time. So the adjusted rule we implement is:
        //
        //   (S, I) \rightarrow_{LR*} (S', I')
        //   \wedge 0 <= j < 1 \wedge S != S'

        let laidx = n.laidx;
        let (new_laidx, n_pstack) =
            self.parser
                .lr_cactus(None, laidx, laidx + 1, n.pstack.clone(), &mut None);
        if n.pstack != n_pstack {
            let nn = PathFNode {
                pstack: n_pstack,
                laidx: new_laidx,
                cf: n.cf,
                variants: n
                    .variants
                    .iter()
                    .map(|variant| {
                        let delimiter_state = if new_laidx > laidx {
                            advance_real_lexeme(
                                self.parser,
                                &variant.delimiter_state,
                                &self.parser.next_lexeme(laidx),
                                laidx,
                            )
                        } else {
                            variant.delimiter_state.clone()
                        };
                        variant.with_repair(
                            (new_laidx > laidx).then_some(Repair::Shift),
                            delimiter_state,
                        )
                    })
                    .collect(),
            };
            nbrs.push((nn.cf, nn));
        }
    }

    /// Convert the output from `astar_all` into something more usable. Returns `None` if it timed
    /// out while doing so.
    fn collect_repairs(
        &self,
        finish_by: Instant,
        in_laidx: usize,
        cnds: Vec<PathFNode<StorageT>>,
    ) -> Option<Vec<Vec<RepairCandidate<LexerTypesT::LexemeT, StorageT>>>> {
        fn traverse<StorageT: PrimInt>(
            finish_by: Instant,
            rm: &Cactus<RepairMerge<StorageT>>,
        ) -> Option<Vec<Vec<Repair<StorageT>>>> {
            if Instant::now() >= finish_by {
                return None;
            }
            let mut out = Vec::new();
            match *rm.val().unwrap() {
                RepairMerge::Repair(r) => {
                    let parents = traverse(finish_by, &rm.parent().unwrap())?;
                    if parents.is_empty() {
                        out.push(vec![r]);
                    } else {
                        for mut parent in parents {
                            parent.push(r);
                            out.push(parent);
                        }
                    }
                }
                RepairMerge::Terminator => (),
            }
            Some(out)
        }

        let mut all_rprs = Vec::with_capacity(cnds.len());
        for cnd in cnds {
            let mut candidates = Vec::new();
            for variant in cnd.variants {
                for history in traverse(finish_by, &variant.repairs)? {
                    candidates.push(RepairCandidate {
                        repairs: self.repair_to_parse_repair(in_laidx, &history),
                        delimiter_state: variant.delimiter_state.clone(),
                    });
                }
            }
            all_rprs.push(candidates);
        }
        Some(all_rprs)
    }

    fn repair_to_parse_repair(
        &self,
        mut laidx: usize,
        from: &[Repair<StorageT>],
    ) -> Vec<ParseRepair<LexerTypesT::LexemeT, StorageT>> {
        from.iter()
            .map(|y| match *y {
                Repair::InsertTerm(token_idx) => ParseRepair::Insert(token_idx),
                Repair::Delete => {
                    let rpr = ParseRepair::Delete(self.parser.next_lexeme(laidx));
                    laidx += 1;
                    rpr
                }
                Repair::Shift => {
                    let rpr = ParseRepair::Shift(self.parser.next_lexeme(laidx));
                    laidx += 1;
                    rpr
                }
            })
            .collect()
    }
}

fn delimiter_for_terminal<
    'a,
    'b: 'a,
    'input: 'b,
    StorageT: 'static + Debug + Eq + Hash + PrimInt + Unsigned,
    LexerTypesT: LexerTypes<StorageT = StorageT>,
    ActionT: 'a,
    ParamT: Clone,
>(
    parser: &Parser<'a, 'b, 'input, StorageT, LexerTypesT, ActionT, ParamT>,
    tidx: TIdx<StorageT>,
) -> Option<char>
where
    usize: AsPrimitive<StorageT>,
{
    [parser.grm.token_name(tidx), parser.grm.token_epp(tidx)]
        .into_iter()
        .flatten()
        .find_map(delimiter_char_from_terminal_name)
}

fn advance_inserted_terminal<
    'a,
    'b: 'a,
    'input: 'b,
    StorageT: 'static + Debug + Eq + Hash + PrimInt + Unsigned,
    LexerTypesT: LexerTypes<StorageT = StorageT>,
    ActionT: 'a,
    ParamT: Clone,
>(
    parser: &Parser<'a, 'b, 'input, StorageT, LexerTypesT, ActionT, ParamT>,
    state: &DelimiterState,
    tidx: TIdx<StorageT>,
    laidx: usize,
) -> DelimiterState
where
    usize: AsPrimitive<StorageT>,
{
    let mut state = state.clone();
    if let Some(delimiter) = delimiter_for_terminal(parser, tidx) {
        let byte_index = parser.next_lexeme(laidx).span().start();
        let char_index = parser.delimiter_char_index(byte_index);
        state.observe_inserted(delimiter, laidx, char_index, byte_index);
    }
    state
}

fn advance_real_lexeme<
    'a,
    'b: 'a,
    'input: 'b,
    StorageT: 'static + Debug + Eq + Hash + PrimInt + Unsigned,
    LexerTypesT: LexerTypes<StorageT = StorageT>,
    ActionT: 'a,
    ParamT: Clone,
>(
    parser: &Parser<'a, 'b, 'input, StorageT, LexerTypesT, ActionT, ParamT>,
    state: &DelimiterState,
    lexeme: &LexerTypesT::LexemeT,
    laidx: usize,
) -> DelimiterState
where
    usize: AsPrimitive<StorageT>,
{
    let mut state = state.clone();
    if let Some(delimiter) = parser.delimiter_for_lexeme(lexeme) {
        let byte_index = lexeme.span().start();
        let char_index = parser.delimiter_char_index(byte_index);
        state.observe_at(
            delimiter,
            DelimiterOrigin {
                input_index: laidx,
                char_index,
                byte_index,
                kind: DelimiterOriginKind::Real,
            },
        );
    }
    state
}

fn apply_delimiter_repairs<
    'a,
    'b: 'a,
    'input: 'b,
    StorageT: 'static + Debug + Eq + Hash + PrimInt + Unsigned,
    LexerTypesT: LexerTypes<StorageT = StorageT>,
    ActionT: 'a,
    ParamT: Clone,
>(
    parser: &Parser<'a, 'b, 'input, StorageT, LexerTypesT, ActionT, ParamT>,
    state: &mut DelimiterState,
    mut laidx: usize,
    repairs: &[ParseRepair<LexerTypesT::LexemeT, StorageT>],
) where
    usize: AsPrimitive<StorageT>,
{
    for repair in repairs {
        match repair {
            ParseRepair::Insert(tidx) => {
                if let Some(delimiter) = delimiter_for_terminal(parser, *tidx) {
                    let byte_index = parser.next_lexeme(laidx).span().start();
                    let char_index = parser.delimiter_char_index(byte_index);
                    state.observe_inserted(delimiter, laidx, char_index, byte_index);
                }
            }
            ParseRepair::Delete(_) => {
                laidx += 1;
            }
            ParseRepair::Shift(lexeme) => {
                let updated =
                    advance_real_lexeme(parser, state, lexeme, laidx);
                *state = updated;
                laidx += 1;
            }
        }
    }
}

/// Apply the `repairs` to `pstack` starting at position `laidx`: return the resulting parse
/// distance and a new pstack.
fn apply_repairs<
    'a,
    StorageT: 'static + Debug + Eq + Hash + PrimInt + Unsigned,
    LexerTypesT: LexerTypes<StorageT = StorageT>,
    ActionT: 'a,
    ParamT: Clone,
>(
    parser: &Parser<StorageT, LexerTypesT, ActionT, ParamT>,
    mut laidx: usize,
    pstack: &mut Vec<StIdx<StorageT>>,
    astack: &mut Option<&mut Vec<AStackType<LexerTypesT::LexemeT, ActionT>>>,
    spans: &mut Option<&mut Vec<Span>>,
    repairs: &[ParseRepair<LexerTypesT::LexemeT, StorageT>],
) -> usize
where
    usize: AsPrimitive<StorageT>,
{
    for r in repairs.iter() {
        match *r {
            ParseRepair::Insert(tidx) => {
                let next_lexeme = parser.next_lexeme(laidx);
                let new_lexeme = Lexeme::new_faulty(
                    StorageT::from(u32::from(tidx)).unwrap(),
                    next_lexeme.span().start(),
                    0,
                );
                parser.lr_upto(Some(new_lexeme), laidx, laidx + 1, pstack, astack, spans);
            }
            ParseRepair::Delete(_) => {
                laidx += 1;
            }
            ParseRepair::Shift(_) => {
                laidx = parser.lr_upto(None, laidx, laidx + 1, pstack, astack, spans);
            }
        }
    }
    laidx
}

/// Simplifies repair sequences, removes duplicates, and sorts them into order.
fn simplify_repairs<
    StorageT: 'static + Debug + Eq + Hash + PrimInt + Unsigned,
    LexerTypesT: LexerTypes<StorageT = StorageT>,
    ActionT,
    ParamT: Clone,
>(
    parser: &Parser<StorageT, LexerTypesT, ActionT, ParamT>,
    all_rprs: &mut Vec<RepairCandidate<LexerTypesT::LexemeT, StorageT>>,
    initial_delimiter_state: &DelimiterState,
    in_laidx: usize,
) where
    usize: AsPrimitive<StorageT>,
{
    for candidate in all_rprs.iter_mut() {
        // Remove shifts from the end of repairs
        while !candidate.repairs.is_empty() {
            if let ParseRepair::Shift(_) = candidate.repairs[candidate.repairs.len() - 1] {
                candidate.repairs.pop();
            } else {
                break;
            }
        }
        candidate.delimiter_state = initial_delimiter_state.clone();
        apply_delimiter_repairs(
            parser,
            &mut candidate.delimiter_state,
            in_laidx,
            &candidate.repairs,
        );
    }

    let mut delimiter_states = HashMap::with_capacity(all_rprs.len());
    let mut unique_repairs = HashSet::with_capacity(all_rprs.len());
    for candidate in all_rprs.drain(..) {
        unique_repairs.insert(candidate.repairs.clone());
        let previous = delimiter_states.insert(candidate.repairs, candidate.delimiter_state.clone());
        if let Some(previous) = previous {
            debug_assert_eq!(previous, candidate.delimiter_state);
        }
    }
    all_rprs.extend(unique_repairs.drain().map(|repairs| RepairCandidate {
        delimiter_state: delimiter_states.remove(&repairs).unwrap(),
        repairs,
    }));

    // Sort repair sequences:
    //   1) by whether they contain Inserts that are %insert_avoid
    //   2) by the number of repairs they contain
    let contains_avoid_insert = |rprs: &Vec<ParseRepair<LexerTypesT::LexemeT, StorageT>>| -> bool {
        for r in rprs.iter() {
            if let ParseRepair::Insert(tidx) = r
                && parser.grm.avoid_insert(*tidx)
            {
                return true;
            }
        }
        false
    };
    all_rprs.sort_unstable_by(|x, y| {
        let x_cai = contains_avoid_insert(&x.repairs);
        let y_cai = contains_avoid_insert(&y.repairs);
        if x_cai && !y_cai {
            Ordering::Greater
        } else if !x_cai && y_cai {
            Ordering::Less
        } else {
            x.repairs.len().cmp(&y.repairs.len())
        }
    });
}

/// Convert `PathFNode` candidates in `cnds` into vectors of `ParseRepairs`s and rank them (from
/// highest to lowest) by the distance they allow parsing to continue without error. If two or more
/// `ParseRepair`s allow the same distance of parsing, then the `ParseRepair` which requires
/// repairs over the shortest distance is preferred. Amongst `ParseRepair`s of the same rank, the
/// ordering is non-deterministic.
fn rank_cnds<
    'a,
    StorageT: 'static + Debug + Eq + Hash + PrimInt + Unsigned,
    LexerTypesT: LexerTypes<StorageT = StorageT>,
    ActionT: 'a,
    ParamT: Clone,
>(
    parser: &Parser<StorageT, LexerTypesT, ActionT, ParamT>,
    finish_by: Instant,
    in_laidx: usize,
    in_pstack: &[StIdx<StorageT>],
    in_cnds: Vec<Vec<RepairCandidate<LexerTypesT::LexemeT, StorageT>>>,
    stats: &mut RecoveryStats,
) -> Vec<RepairCandidate<LexerTypesT::LexemeT, StorageT>>
where
    usize: AsPrimitive<StorageT>,
{
    let mut cnds = Vec::new();
    let mut furthest = 0;
    for rpr_seqs in in_cnds {
        if Instant::now() >= finish_by {
            stats.timed_out = true;
            return vec![];
        }
        let mut pstack = in_pstack.to_owned();
        let mut laidx = apply_repairs(
            parser,
            in_laidx,
            &mut pstack,
            &mut None,
            &mut None,
            &rpr_seqs[0].repairs,
        );
        laidx = parser.lr_upto(
            None,
            laidx,
            in_laidx + TRY_PARSE_AT_MOST,
            &mut pstack,
            &mut None,
            &mut None,
        );
        if laidx >= furthest {
            furthest = laidx;
        }
        cnds.push((pstack, laidx, rpr_seqs));
    }

    // Remove any elements except those which parsed as far as possible.
    cnds = cnds
        .into_iter()
        .filter(|(_, x, _)| *x == furthest)
        .collect::<Vec<_>>();

    cnds.into_iter().flat_map(|(_, _, x)| x).collect::<Vec<_>>()
}

/// Do `repairs` end with enough Shift repairs to be considered a success node?
fn ends_with_parse_at_least_shifts<StorageT: PrimInt + Unsigned>(
    repairs: &Cactus<RepairMerge<StorageT>>,
) -> bool {
    let mut shfts = 0;
    for x in repairs.vals().take(PARSE_AT_LEAST) {
        match *x {
            RepairMerge::Repair(Repair::Shift) => shfts += 1,
            _ => return false,
        }
    }
    shfts == PARSE_AT_LEAST
}

#[cfg(test)]
mod test {
    use std::collections::HashMap;

    use cactus::Cactus;
    use cfgrammar::yacc::YaccGrammar;
    use num_traits::ToPrimitive;

    use crate::{
        LexParseError, Lexeme, NonStreamingLexer,
        parser::{
            DelimiterKind, DelimiterOrigin, DelimiterOriginKind, DelimiterScoreContext,
            DelimiterState, ParseRepair, RecoveryKind, RecoveryStats,
            test::{SmallLexer, do_parse},
        },
        test_utils::{TestLexeme, TestLexerTypes},
    };
    use super::{
        PathFNode, Repair, RepairMerge, RepairVariant, merge_variants,
        retain_nondominated_variants,
    };

    fn delimiter_state(kind: char, input_index: usize, byte_index: usize) -> DelimiterState {
        let mut state = DelimiterState::default();
        state.observe_at(
            kind,
            DelimiterOrigin {
                input_index,
                char_index: byte_index,
                byte_index,
                kind: DelimiterOriginKind::Real,
            },
        );
        state
    }

    fn repair_variant(
        repair: Repair<u16>,
        delimiter_state: DelimiterState,
    ) -> RepairVariant<u16> {
        RepairVariant {
            repairs: Cactus::new()
                .child(RepairMerge::Terminator)
                .child(RepairMerge::Repair(repair)),
            delimiter_state,
        }
    }

    fn dominance_node(
        variant: RepairVariant<u16>,
        cost: u16,
    ) -> PathFNode<u16> {
        PathFNode {
            pstack: Cactus::new().child(lrtable::StIdx(0u16)),
            laidx: 3,
            cf: cost,
            variants: vec![variant],
        }
    }

    #[test]
    fn strict_cost_dominance_preserves_equal_cost_histories() {
        let state = DelimiterState::default();
        let first = repair_variant(Repair::InsertTerm(cfgrammar::TIdx(1)), state.clone());
        let second = repair_variant(Repair::InsertTerm(cfgrammar::TIdx(2)), state);
        let mut index = HashMap::new();
        let mut stats = RecoveryStats::default();
        let mut first_node = dominance_node(first, 2);
        let mut second_node = dominance_node(second, 2);

        assert!(retain_nondominated_variants(
            &mut first_node,
            2,
            &mut index,
            &mut stats
        ));
        assert!(retain_nondominated_variants(
            &mut second_node,
            2,
            &mut index,
            &mut stats
        ));
        assert_eq!(second_node.variants.len(), 1);
        assert_eq!(stats.dominance_exact_key_matches, 1);
        assert_eq!(stats.dominance_variants_discarded, 0);
    }

    #[test]
    fn strict_cost_dominance_discards_only_an_exact_higher_cost_match() {
        let state = DelimiterState::default();
        let lower = repair_variant(Repair::InsertTerm(cfgrammar::TIdx(1)), state.clone());
        let higher = repair_variant(Repair::InsertTerm(cfgrammar::TIdx(2)), state);
        let mut index = HashMap::new();
        let mut stats = RecoveryStats::default();
        let mut lower_node = dominance_node(lower, 1);
        let mut higher_node = dominance_node(higher, 4);

        assert!(retain_nondominated_variants(
            &mut lower_node,
            1,
            &mut index,
            &mut stats
        ));
        assert!(!retain_nondominated_variants(
            &mut higher_node,
            4,
            &mut index,
            &mut stats
        ));
        assert!(higher_node.variants.is_empty());
        assert_eq!(stats.dominance_exact_key_matches, 1);
        assert_eq!(stats.dominance_variants_discarded, 1);
        assert_eq!(stats.dominance_discarded_cost_difference, 3);
        assert_eq!(stats.configurations_discarded_dominance, 1);
    }

    #[test]
    fn delimiter_state_and_opening_origin_are_part_of_dominance_key() {
        let first = repair_variant(
            Repair::InsertTerm(cfgrammar::TIdx(1)),
            delimiter_state('(', 1, 2),
        );
        let second = repair_variant(
            Repair::InsertTerm(cfgrammar::TIdx(2)),
            delimiter_state('(', 4, 12),
        );
        let mut index = HashMap::new();
        let mut stats = RecoveryStats::default();
        let mut first_node = dominance_node(first, 1);
        let mut second_node = dominance_node(second, 9);

        assert!(retain_nondominated_variants(
            &mut first_node,
            1,
            &mut index,
            &mut stats
        ));
        assert!(retain_nondominated_variants(
            &mut second_node,
            9,
            &mut index,
            &mut stats
        ));
        assert_eq!(second_node.variants.len(), 1);
        assert_eq!(stats.dominance_exact_key_matches, 0);
        assert_eq!(stats.dominance_variants_discarded, 0);
    }

    #[test]
    fn parser_position_and_continuation_control_are_part_of_dominance_key() {
        let state = DelimiterState::default();
        let base = repair_variant(Repair::InsertTerm(cfgrammar::TIdx(1)), state.clone());
        let base_node = dominance_node(base.clone(), 1);
        let mut index = HashMap::new();
        let mut stats = RecoveryStats::default();
        let mut first = base_node.clone();
        assert!(retain_nondominated_variants(
            &mut first,
            1,
            &mut index,
            &mut stats
        ));

        let mut different_input = dominance_node(base.clone(), 5);
        different_input.laidx += 1;
        let mut different_stack = dominance_node(base.clone(), 5);
        different_stack.pstack = Cactus::new().child(lrtable::StIdx(1u16));
        let mut delete_control = dominance_node(
            repair_variant(Repair::Delete, state.clone()),
            5,
        );
        let shifts = RepairVariant {
            repairs: base
                .repairs
                .child(RepairMerge::Repair(Repair::Shift))
                .child(RepairMerge::Repair(Repair::Shift)),
            delimiter_state: state,
        };
        let mut shift_control = dominance_node(shifts, 5);

        for candidate in [
            &mut different_input,
            &mut different_stack,
            &mut delete_control,
            &mut shift_control,
        ] {
            assert!(retain_nondominated_variants(
                candidate,
                5,
                &mut index,
                &mut stats
            ));
        }
        assert_eq!(stats.dominance_exact_key_matches, 0);
        assert_eq!(stats.dominance_variants_discarded, 0);
    }

    #[test]
    fn delimiter_score_never_participates_in_dominance() {
        let empty = DelimiterState::default();
        let opened = delimiter_state('(', 1, 2);
        let score_context = DelimiterScoreContext::new(&empty, None);
        assert_ne!(score_context.score(&empty), score_context.score(&opened));

        let lower_score = repair_variant(Repair::InsertTerm(cfgrammar::TIdx(1)), opened);
        let higher_score = repair_variant(Repair::InsertTerm(cfgrammar::TIdx(2)), empty);
        let mut index = HashMap::new();
        let mut stats = RecoveryStats::default();
        let mut lower_score_node = dominance_node(lower_score, 1);
        let mut higher_score_node = dominance_node(higher_score, 5);

        assert!(retain_nondominated_variants(
            &mut lower_score_node,
            1,
            &mut index,
            &mut stats
        ));
        assert!(retain_nondominated_variants(
            &mut higher_score_node,
            5,
            &mut index,
            &mut stats
        ));
        assert_eq!(higher_score_node.variants.len(), 1);
        assert_eq!(stats.dominance_variants_discarded, 0);
    }

    #[test]
    fn merging_compatible_histories_keeps_each_paired_delimiter_state() {
        let left = repair_variant(Repair::InsertTerm(cfgrammar::TIdx(1)), delimiter_state('(', 1, 2));
        let right = repair_variant(Repair::InsertTerm(cfgrammar::TIdx(2)), delimiter_state('[', 2, 5));
        let pstack = Cactus::new().child(lrtable::StIdx(0u16));
        let left_node = PathFNode {
            pstack: pstack.clone(),
            laidx: 3,
            cf: 1,
            variants: vec![left.clone()],
        };
        let right_node = PathFNode {
            pstack,
            laidx: 3,
            cf: 1,
            variants: vec![right.clone()],
        };

        assert_eq!(left_node, right_node);
        let mut variants = vec![left.clone()];

        assert_ne!(left.last_repair(), right.last_repair());
        assert_eq!(left.trailing_shifts(), right.trailing_shifts());
        assert_eq!(merge_variants(&mut variants, vec![right.clone()]), 0);
        assert_eq!(variants, [left.clone(), right.clone()]);
        assert_eq!(variants[0].delimiter_state.stack[0].kind, DelimiterKind::Parenthesis);
        assert_eq!(variants[1].delimiter_state.stack[0].kind, DelimiterKind::Bracket);
    }

    #[test]
    fn paired_variants_update_independently_through_insert_delete_and_shift() {
        let left = repair_variant(Repair::InsertTerm(cfgrammar::TIdx(1)), delimiter_state('(', 1, 2));
        let right = repair_variant(Repair::InsertTerm(cfgrammar::TIdx(2)), delimiter_state('[', 2, 5));

        let mut left_after_insert = left.delimiter_state.clone();
        left_after_insert.observe_inserted(')', 3, 6, 6);
        let left_after_insert =
            left.with_repair(Some(Repair::InsertTerm(cfgrammar::TIdx(3))), left_after_insert);
        let mut right_after_insert = right.delimiter_state.clone();
        right_after_insert.observe_inserted(')', 3, 6, 6);
        let right_after_insert =
            right.with_repair(Some(Repair::InsertTerm(cfgrammar::TIdx(3))), right_after_insert);
        assert_eq!(left_after_insert.delimiter_state.matched_delimiters, 1);
        assert_eq!(right_after_insert.delimiter_state.mismatched_closings, 1);

        let left_after_delete = left_after_insert.with_repair(
            Some(Repair::Delete),
            left_after_insert.delimiter_state.clone(),
        );
        let right_after_delete = right_after_insert.with_repair(
            Some(Repair::Delete),
            right_after_insert.delimiter_state.clone(),
        );
        assert_eq!(
            left_after_delete.delimiter_state,
            left_after_insert.delimiter_state
        );
        assert_eq!(
            right_after_delete.delimiter_state,
            right_after_insert.delimiter_state
        );

        let mut left_after_shift_state = left_after_delete.delimiter_state.clone();
        left_after_shift_state.observe_at(
            '{',
            DelimiterOrigin {
                input_index: 4,
                char_index: 8,
                byte_index: 9,
                kind: DelimiterOriginKind::Real,
            },
        );
        let mut right_after_shift_state = right_after_delete.delimiter_state.clone();
        right_after_shift_state.observe_at(
            '{',
            DelimiterOrigin {
                input_index: 4,
                char_index: 8,
                byte_index: 9,
                kind: DelimiterOriginKind::Real,
            },
        );
        let left_after_shift = left_after_delete.with_repair(
            Some(Repair::Shift),
            left_after_shift_state,
        );
        let right_after_shift = right_after_delete.with_repair(
            Some(Repair::Shift),
            right_after_shift_state,
        );

        assert_eq!(left_after_shift.delimiter_state.stack.len(), 1);
        assert_eq!(right_after_shift.delimiter_state.stack.len(), 1);
        assert_eq!(
            left_after_shift.delimiter_state.stack[0].kind,
            DelimiterKind::Brace
        );
        assert_eq!(
            right_after_shift.delimiter_state.stack[0].kind,
            DelimiterKind::Brace
        );
        assert_eq!(
            left_after_shift.delimiter_state.matched_delimiters,
            1
        );
        assert_eq!(
            right_after_shift.delimiter_state.mismatched_closings,
            1
        );
    }

    #[test]
    fn same_delimiter_kind_with_distinct_origins_remains_distinguishable() {
        let first = repair_variant(Repair::InsertTerm(cfgrammar::TIdx(1)), delimiter_state('(', 1, 2));
        let second = repair_variant(Repair::InsertTerm(cfgrammar::TIdx(2)), delimiter_state('(', 4, 12));
        let mut variants = vec![first.clone()];
        merge_variants(&mut variants, vec![second.clone()]);

        assert_eq!(variants[0].delimiter_state.stack[0].kind, DelimiterKind::Parenthesis);
        assert_eq!(variants[1].delimiter_state.stack[0].kind, DelimiterKind::Parenthesis);
        assert_ne!(
            variants[0].delimiter_state.stack[0].input_index,
            variants[1].delimiter_state.stack[0].input_index
        );
        assert_ne!(
            variants[0].delimiter_state.stack[0].byte_index,
            variants[1].delimiter_state.stack[0].byte_index
        );
        assert_eq!(variants[0].delimiter_state.stack[0].depth, 1);
        assert_eq!(variants[1].delimiter_state.stack[0].depth, 1);
    }

    #[test]
    fn incompatible_continuation_states_do_not_compare_equal() {
        let state = DelimiterState::default();
        let base_stack = Cactus::new().child(lrtable::StIdx(0u16));
        let make_node = |variant| PathFNode {
            pstack: base_stack.clone(),
            laidx: 3,
            cf: 1,
            variants: vec![variant],
        };
        let non_delete = make_node(repair_variant(
            Repair::InsertTerm(cfgrammar::TIdx(1)),
            state.clone(),
        ));
        let delete = make_node(repair_variant(Repair::Delete, state.clone()));
        let shift = repair_variant(Repair::Shift, state.clone());
        let two_shifts = RepairVariant {
            repairs: shift
                .repairs
                .child(RepairMerge::Repair(Repair::Shift)),
            delimiter_state: state,
        };
        let one_shift = make_node(shift);
        let two_shifts = make_node(two_shifts);

        assert_ne!(non_delete, delete);
        assert_ne!(one_shift, two_shifts);
    }

    fn pp_repairs(
        grm: &YaccGrammar<u16>,
        lex: &SmallLexer<'_>,
        repairs: &[ParseRepair<TestLexeme, u16>],
    ) -> String {
        let mut out = vec![];
        for r in repairs.iter() {
            match *r {
                ParseRepair::Insert(token_idx) => {
                    out.push(format!("Insert \"{}\"", grm.token_epp(token_idx).unwrap()))
                }
                ParseRepair::Delete(lexeme) => {
                    out.push(format!(r#"Delete "{}""#, lex.span_str(lexeme.span())))
                }
                ParseRepair::Shift(_) => out.push("Shift".to_owned()),
            }
        }
        out.join(", ")
    }

    fn check_all_repairs(
        grm: &YaccGrammar<u16>,
        lex: &SmallLexer<'_>,
        err: &LexParseError<u16, TestLexerTypes>,
        expected: &[&str],
    ) {
        match err {
            LexParseError::ParseError(e) => {
                assert_eq!(
                    e.repairs().len(),
                    expected.len(),
                    "{:?}\nhas a different number of entries to:\n{:?}",
                    e.repairs(),
                    expected
                );
                for i in 0..e.repairs().len() {
                    if !expected
                        .iter()
                        .any(|x| *x == pp_repairs(grm, lex, &e.repairs()[i]))
                    {
                        panic!(
                            "No match found for:\n  {}",
                            pp_repairs(grm, lex, &e.repairs()[i])
                        );
                    }
                }
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn corchuelo_example() {
        // The example from the Curchuelo paper
        let lexs = "\\( '('
                    \\) ')'
                    \\+ '+'
                    n 'N'";
        let grms = "%start E
%%
E : 'N'
  | E '+' 'N'
  | '(' E ')'
  ;
";

        let us = "(nn";
        let (grm, lex, pr) = do_parse(RecoveryKind::CPCTPlus, lexs, grms, us);
        let (pt, errs) = pr.unwrap_err();
        let pp = pt.unwrap().pp(&grm, us);
        // Note that:
        //   E
        //    ( (
        //    E
        //     E
        //      N n
        //     +
        //     N n
        //    )
        // is also the result of a valid minimal-cost repair, but, since the repair involves a
        // Shift, rank_cnds will always put this too low down the list for us to ever see it.
        if ![
            "E
 ( (
 E
  N n
 ) 
",
            "E
 E
  ( (
  E
   N n
  ) 
 + 
 N n
",
        ]
        .iter()
        .any(|x| *x == pp)
        {
            panic!("Can't find a match for {}", pp);
        }

        assert_eq!(errs.len(), 1);
        let err_tok_id = u32::from(grm.token_idx("N").unwrap()).to_u16().unwrap();
        match &errs[0] {
            LexParseError::ParseError(e) => {
                assert_eq!(e.lexeme(), &Lexeme::new(err_tok_id, 2, 1))
            }
            _ => unreachable!(),
        }
        check_all_repairs(
            &grm,
            &lex,
            &errs[0],
            &[
                r#"Insert ")", Insert "+""#,
                r#"Insert ")", Delete "n""#,
                r#"Insert "+", Shift, Insert ")""#,
            ],
        );

        let (grm, lex, pr) = do_parse(RecoveryKind::CPCTPlus, lexs, grms, "n)+n+n+n)");
        let (_, errs) = pr.unwrap_err();
        assert_eq!(errs.len(), 2);
        check_all_repairs(&grm, &lex, &errs[0], &[r#"Delete ")""#]);
        check_all_repairs(&grm, &lex, &errs[1], &[r#"Delete ")""#]);

        let (grm, lex, pr) = do_parse(RecoveryKind::CPCTPlus, lexs, grms, "(((+n)+n+n+n)");
        let (_, errs) = pr.unwrap_err();
        assert_eq!(errs.len(), 2);
        check_all_repairs(&grm, &lex, &errs[0], &[r#"Insert "N""#, r#"Delete "+""#]);
        check_all_repairs(&grm, &lex, &errs[1], &[r#"Insert ")""#]);
    }

    #[test]
    fn test_merge() {
        let lexs = "a 'a'
                    b 'b'
                    c 'c'
                    d 'd'";

        let grms = "%start S
%%
S: T U;
T: T1 | 'b' | T2;
T1: 'a';
T2: 'c' | 'a' 'b' 'c';
U: 'd';
";

        let us = "";
        let (grm, lex, pr) = do_parse(RecoveryKind::CPCTPlus, lexs, grms, us);
        let (_, errs) = pr.unwrap_err();
        check_all_repairs(
            &grm,
            &lex,
            &errs[0],
            &[
                "Insert \"a\", Insert \"d\"",
                "Insert \"b\", Insert \"d\"",
                "Insert \"c\", Insert \"d\"",
            ],
        );
    }

    #[test]
    fn test_unmatched() {
        let lexs = "
          blah 'BLAH'
          . 'UNMATCHED'
          ";

        let grms = "
%start S
%%
S: 'BLAH';
Unmatched:
  'UNMATCHED' { }
  ;
";

        let us = "long";
        let (grm, lex, pr) = do_parse(RecoveryKind::CPCTPlus, lexs, grms, us);
        let (_, errs) = pr.unwrap_err();
        check_all_repairs(
            &grm,
            &lex,
            &errs[0],
            &[r#"Insert "BLAH", Delete "l", Delete "o", Delete "n", Delete "g""#],
        );
    }

    fn recovery_signature(
        recovery_kind: RecoveryKind,
        input: &str,
    ) -> (Vec<Vec<String>>, Vec<Option<u16>>, Vec<RecoveryStats>) {
        let lexs = r#"[0-9]+ 'INT'
\+ '+'
\* '*'
\( '('
\) ')'"#;
        let grms = r#"%start E
%%
E: E '+' E
 | E '*' E
 | '(' E ')'
 | 'INT'
 ;"#;
        let (grm, lex, parsed) = do_parse(recovery_kind, lexs, grms, input);
        let (_, errors) = parsed.unwrap_err();
        let mut repair_sets = Vec::new();
        let mut costs = Vec::new();
        let mut stats = Vec::new();
        for error in errors {
            let LexParseError::ParseError(error) = error else {
                panic!("expected parser error for {input}");
            };
            let mut repairs = error
                .repairs()
                .iter()
                .map(|repairs| pp_repairs(&grm, &lex, repairs))
                .collect::<Vec<_>>();
            repairs.sort();
            repair_sets.push(repairs);
            costs.push(
                error
                    .recovery_stats()
                    .and_then(|recovery| recovery.minimum_recovery_cost),
            );
            if let Some(recovery) = error.recovery_stats() {
                stats.push(recovery.clone());
            }
        }
        (repair_sets, costs, stats)
    }

    #[test]
    fn dominance_preserves_minimum_cost_repair_sets_and_unexpected_closer_repair() {
        for input in ["1+*", "(1+2", "1+2)"] {
            let baseline = recovery_signature(RecoveryKind::CPCTPlus, input);
            let dominant = recovery_signature(RecoveryKind::CPCTPlusDelimiterDominance, input);
            assert_eq!(dominant.0, baseline.0, "repair set changed for {input}");
            assert_eq!(dominant.1, baseline.1, "minimum cost changed for {input}");
            assert!(
                dominant.2.iter().all(|stats| stats.delimiter_dominance_enabled),
                "{input}: {:#?}",
                dominant.2
            );
        }

        let unexpected_closer =
            recovery_signature(RecoveryKind::CPCTPlusDelimiterDominance, "1+2)");
        assert_eq!(unexpected_closer.1, [Some(1)]);
        assert_eq!(
            unexpected_closer.0,
            [vec![r#"Delete ")""#.to_owned()]]
        );

        let equal_cost_alternatives =
            recovery_signature(RecoveryKind::CPCTPlusDelimiterDominance, "1+*");
        assert_eq!(equal_cost_alternatives.0[0].len(), 2);
    }
}
