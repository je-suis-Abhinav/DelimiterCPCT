#![allow(clippy::derive_partial_eq_without_eq)]
use std::{
    collections::BTreeMap,
    error::Error,
    fmt::{self, Debug, Display, Write as _},
    hash::Hash,
    marker::PhantomData,
    vec,
};

// Can be used on non-wasm32 but to avoid the dependency.
#[cfg(not(target_arch = "wasm32"))]
use std::time::{Duration, Instant};
#[cfg(target_arch = "wasm32")]
use web_time::{Duration, Instant};

use cactus::Cactus;
use cfgrammar::{RIdx, Span, TIdx, header::Value, span::Location, yacc::YaccGrammar};
use lrtable::{Action, StIdx, StateTable};
use num_traits::{AsPrimitive, PrimInt, Unsigned};
use proc_macro2::TokenStream;
use quote::quote;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::{LexError, Lexeme, LexerTypes, NonStreamingLexer, cpctplus};

#[cfg(test)]
const RECOVERY_TIME_BUDGET: u64 = 60_000; // milliseconds
#[cfg(not(test))]
const RECOVERY_TIME_BUDGET: u64 = 500; // milliseconds

#[deprecated(
    since = "0.14.0",
    note = "Use the version of `Node` exported from your `lrpar_mod!`"
)]
pub type Node<T, S> = _deprecated_moved_::Node<T, S>;

#[doc(hidden)]
pub mod _deprecated_moved_ {
    use super::*;
    /// A generic parse tree.
    #[derive(Debug, Clone, PartialEq)]
    pub enum Node<LexemeT: Lexeme<StorageT>, StorageT> {
        /// Terminals store a single lexeme.
        Term { lexeme: LexemeT },
        /// Nonterminals reference a rule and have zero or more `Node`s as children.
        Nonterm {
            ridx: RIdx<StorageT>,
            nodes: Vec<Node<LexemeT, StorageT>>,
        },
    }
}

#[allow(deprecated)]
impl<LexemeT: Lexeme<StorageT>, StorageT: 'static + PrimInt + Unsigned> Node<LexemeT, StorageT>
where
    usize: AsPrimitive<StorageT>,
{
    /// Return a pretty-printed version of this node.
    pub fn pp(&self, grm: &YaccGrammar<StorageT>, input: &str) -> String {
        let mut st = vec![(0, self)]; // Stack of (indent level, node) pairs
        let mut s = String::new();
        while let Some((indent, e)) = st.pop() {
            for _ in 0..indent {
                s.push(' ');
            }
            match *e {
                Node::Term { lexeme } => {
                    let tidx = TIdx(lexeme.tok_id());
                    let tn = grm.token_name(tidx).unwrap();
                    let lt = &input[lexeme.span().start()..lexeme.span().end()];
                    writeln!(s, "{} {}", tn, lt).ok();
                }
                Node::Nonterm { ridx, ref nodes } => {
                    writeln!(s, "{}", grm.rule_name_str(ridx)).ok();
                    for x in nodes.iter().rev() {
                        st.push((indent + 1, x));
                    }
                }
            }
        }
        s
    }
}

type PStack<StorageT> = Vec<StIdx<StorageT>>; // Parse stack
type TokenCostFn<'a, StorageT> = &'a (dyn Fn(TIdx<StorageT>) -> u8 + 'a);
type ActionFn<'a, 'b, 'input, StorageT, LexerTypesT, ActionT, ParamT> = &'a dyn Fn(
    RIdx<StorageT>,
    &'b dyn NonStreamingLexer<'input, LexerTypesT>,
    Span,
    vec::Drain<AStackType<<LexerTypesT as LexerTypes>::LexemeT, ActionT>>,
    ParamT,
) -> ActionT;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) enum DelimiterKind {
    Parenthesis,
    Brace,
    Bracket,
}

impl DelimiterKind {
    pub(super) fn from_open(delimiter: char) -> Option<Self> {
        match delimiter {
            '(' => Some(Self::Parenthesis),
            '{' => Some(Self::Brace),
            '[' => Some(Self::Bracket),
            _ => None,
        }
    }

    pub(super) fn from_close(delimiter: char) -> Option<Self> {
        match delimiter {
            ')' => Some(Self::Parenthesis),
            '}' => Some(Self::Brace),
            ']' => Some(Self::Bracket),
            _ => None,
        }
    }

    pub(super) fn open_char(self) -> char {
        match self {
            Self::Parenthesis => '(',
            Self::Brace => '{',
            Self::Bracket => '[',
        }
    }

    pub(super) fn close_char(self) -> char {
        match self {
            Self::Parenthesis => ')',
            Self::Brace => '}',
            Self::Bracket => ']',
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DelimiterCloseContext {
    MatchingOpening,
    MismatchedOpening,
    Unexpected,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) enum DelimiterOriginKind {
    Real,
    Inserted { ordinal: u64 },
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) struct DelimiterOrigin {
    pub(super) input_index: usize,
    pub(super) char_index: usize,
    pub(super) byte_index: usize,
    pub(super) kind: DelimiterOriginKind,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) struct DelimiterFrame {
    pub(super) kind: DelimiterKind,
    pub(super) input_index: usize,
    pub(super) char_index: usize,
    pub(super) byte_index: usize,
    pub(super) depth: usize,
    pub(super) origin_kind: DelimiterOriginKind,
}

#[derive(Clone, Debug, Default, Hash, PartialEq, Eq)]
pub(super) struct DelimiterState {
    pub(super) stack: Vec<DelimiterFrame>,
    pub(super) matched_delimiters: u64,
    pub(super) unexpected_closings: u64,
    pub(super) mismatched_closings: u64,
    max_depth: usize,
    inserted_origin_count: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct DelimiterScoreContext {
    initial_stack: Vec<DelimiterKind>,
    initial_matched_delimiters: u64,
    initial_unexpected_closings: u64,
    initial_mismatched_closings: u64,
    error_closing: Option<(DelimiterKind, DelimiterCloseContext)>,
}

impl DelimiterScoreContext {
    pub(super) fn new(state: &DelimiterState, error_delimiter: Option<char>) -> Self {
        let error_closing = error_delimiter.and_then(|delimiter| {
            let closing = DelimiterKind::from_close(delimiter)?;
            let context = match state.stack.last().map(|frame| frame.kind) {
                Some(opening) if opening == closing => DelimiterCloseContext::MatchingOpening,
                Some(_) => DelimiterCloseContext::MismatchedOpening,
                None => DelimiterCloseContext::Unexpected,
            };
            Some((closing, context))
        });

        Self {
            initial_stack: state.stack.iter().map(|frame| frame.kind).collect(),
            initial_matched_delimiters: state.matched_delimiters,
            initial_unexpected_closings: state.unexpected_closings,
            initial_mismatched_closings: state.mismatched_closings,
            error_closing,
        }
    }

    /// Higher scores indicate more structural progress relative to the parser-error context:
    /// +3 per original opening removed, -3 per new unmatched opening, +2 per new match,
    /// -4 per new unexpected/mismatched close, and +2 when the error's closing delimiter
    /// currently has a matching opening on top of the stack (bonus: +2, +3, or +4 for
    /// matching, mismatched, or unexpected parser-error context, respectively).
    pub(super) fn score(&self, state: &DelimiterState) -> i64 {
        let shared_prefix = self
            .initial_stack
            .iter()
            .zip(state.stack.iter().map(|frame| frame.kind))
            .take_while(|(initial, current)| *initial == current)
            .count();
        let resolved_openings = self.initial_stack.len().saturating_sub(shared_prefix);
        let added_openings = state.stack.len().saturating_sub(shared_prefix);
        let matches = state
            .matched_delimiters
            .saturating_sub(self.initial_matched_delimiters);
        let unexpected = state
            .unexpected_closings
            .saturating_sub(self.initial_unexpected_closings);
        let mismatched = state
            .mismatched_closings
            .saturating_sub(self.initial_mismatched_closings);

        let to_score = |value: usize| i64::try_from(value).unwrap_or(i64::MAX);
        let to_score_count = |value: u64| i64::try_from(value).unwrap_or(i64::MAX);
        let mut score = to_score(resolved_openings)
            .saturating_mul(3)
            .saturating_sub(to_score(added_openings).saturating_mul(3))
            .saturating_add(to_score_count(matches).saturating_mul(2))
            .saturating_sub(to_score_count(unexpected).saturating_mul(4))
            .saturating_sub(to_score_count(mismatched).saturating_mul(4));
        if let Some((closing, context)) = self.error_closing
            && state.stack.last().map(|frame| frame.kind) == Some(closing)
        {
            let context_bonus = match context {
                DelimiterCloseContext::MatchingOpening => 2,
                DelimiterCloseContext::MismatchedOpening => 3,
                DelimiterCloseContext::Unexpected => 4,
            };
            score = score.saturating_add(context_bonus);
        }
        score
    }
}

impl DelimiterState {
    #[cfg(test)]
    pub(super) fn observe(&mut self, delimiter: char) {
        self.observe_at(
            delimiter,
            DelimiterOrigin {
                input_index: 0,
                char_index: 0,
                byte_index: 0,
                kind: DelimiterOriginKind::Real,
            },
        );
    }

    pub(super) fn observe_at(&mut self, delimiter: char, mut origin: DelimiterOrigin) {
        if let Some(kind) = DelimiterKind::from_open(delimiter) {
            if matches!(origin.kind, DelimiterOriginKind::Inserted { .. }) {
                origin.kind = DelimiterOriginKind::Inserted {
                    ordinal: self.inserted_origin_count,
                };
                self.inserted_origin_count = self.inserted_origin_count.saturating_add(1);
            }
            let depth = self.stack.len() + 1;
            self.stack.push(DelimiterFrame {
                kind,
                input_index: origin.input_index,
                char_index: origin.char_index,
                byte_index: origin.byte_index,
                depth,
                origin_kind: origin.kind,
            });
            self.max_depth = self.max_depth.max(self.stack.len());
        } else if let Some(kind) = DelimiterKind::from_close(delimiter) {
            match self.stack.pop() {
                Some(opening) if opening.kind == kind => self.matched_delimiters += 1,
                Some(_) => self.mismatched_closings += 1,
                None => self.unexpected_closings += 1,
            }
        }
    }

    pub(super) fn observe_inserted(
        &mut self,
        delimiter: char,
        input_index: usize,
        char_index: usize,
        byte_index: usize,
    ) {
        self.observe_at(
            delimiter,
            DelimiterOrigin {
                input_index,
                char_index,
                byte_index,
                kind: DelimiterOriginKind::Inserted { ordinal: 0 },
            },
        );
    }
}

pub(super) fn delimiter_char_from_text(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let delimiter = chars.next()?;
    if chars.next().is_some()
        || !matches!(delimiter, '(' | ')' | '{' | '}' | '[' | ']')
    {
        return None;
    }
    Some(delimiter)
}

pub(super) fn delimiter_char_from_terminal_name(name: &str) -> Option<char> {
    delimiter_char_from_text(name).or_else(|| {
        let quote = name.chars().next()?;
        if !matches!(quote, '\'' | '"') || !name.ends_with(quote) {
            return None;
        }
        delimiter_char_from_text(&name[quote.len_utf8()..name.len() - quote.len_utf8()])
    })
}

#[cfg(test)]
mod delimiter_state_tests {
    use super::{
        DelimiterKind, DelimiterOrigin, DelimiterOriginKind, DelimiterScoreContext, DelimiterState,
        delimiter_char_from_terminal_name, delimiter_char_from_text,
    };

    #[test]
    fn tracks_matched_delimiters_and_nested_depth() {
        let mut state = DelimiterState::default();
        for delimiter in ['(', '[', '{', '}', ']', ')'] {
            state.observe(delimiter);
        }

        assert!(state.stack.is_empty());
        assert_eq!(state.matched_delimiters, 3);
        assert_eq!(state.max_depth, 3);
        assert_eq!(state.unexpected_closings, 0);
        assert_eq!(state.mismatched_closings, 0);
    }

    #[test]
    fn tracks_unmatched_openings() {
        let mut state = DelimiterState::default();
        state.observe('(');
        state.observe('[');

        assert_eq!(
            state.stack.iter().map(|frame| frame.kind).collect::<Vec<_>>(),
            [DelimiterKind::Parenthesis, DelimiterKind::Bracket]
        );
        assert_eq!(state.max_depth, 2);
    }

    #[test]
    fn tracks_unexpected_and_mismatched_closings() {
        let mut unexpected = DelimiterState::default();
        unexpected.observe(')');
        assert_eq!(unexpected.unexpected_closings, 1);
        assert!(unexpected.stack.is_empty());

        let mut mismatched = DelimiterState::default();
        mismatched.observe('{');
        mismatched.observe(')');
        assert_eq!(mismatched.mismatched_closings, 1);
        assert!(mismatched.stack.is_empty());
    }

    #[test]
    fn real_shift_updates_delimiter_state_but_non_delimiters_do_not() {
        let mut state = DelimiterState::default();
        state.observe_at(
            '(',
            DelimiterOrigin {
                input_index: 0,
                char_index: 0,
                byte_index: 0,
                kind: DelimiterOriginKind::Real,
            },
        );
        let after_shift = state.clone();
        if delimiter_char_from_text("identifier").is_some() {
            state.observe_at(
                '(',
                DelimiterOrigin {
                    input_index: 1,
                    char_index: 1,
                    byte_index: 1,
                    kind: DelimiterOriginKind::Real,
                },
            );
        }
        assert_eq!(state, after_shift);
        assert_eq!(state.stack[0].kind, DelimiterKind::Parenthesis);
    }

    #[test]
    fn deleting_a_delimiter_leaves_the_configuration_state_unchanged() {
        let mut before_delete = DelimiterState::default();
        before_delete.observe('[');
        let after_delete = before_delete.clone();

        assert_eq!(after_delete, before_delete);
        assert_eq!(after_delete.stack[0].kind, DelimiterKind::Bracket);
    }

    #[test]
    fn delimiter_frames_keep_source_and_inserted_origin_metadata() {
        let mut state = DelimiterState::default();
        state.observe_inserted('(', 4, 8, 10);
        let inserted = state.stack[0];
        assert_eq!(inserted.input_index, 4);
        assert_eq!(inserted.char_index, 8);
        assert_eq!(inserted.byte_index, 10);
        assert_eq!(inserted.depth, 1);
        assert_eq!(
            inserted.origin_kind,
            DelimiterOriginKind::Inserted { ordinal: 0 }
        );

        state.observe(')');
        state.observe_inserted('(', 4, 8, 10);
        assert_eq!(
            state.stack[0].origin_kind,
            DelimiterOriginKind::Inserted { ordinal: 1 }
        );
    }

    #[test]
    fn inserted_terminal_mapping_accepts_only_reliable_spellings() {
        for (name, expected) in [
            ("(", Some('(')),
            ("')'", Some(')')),
            ("\"[\"", Some('[')),
            ("left_paren", None),
            ("'not-a-delimiter'", None),
            ("'(", None),
        ] {
            assert_eq!(delimiter_char_from_terminal_name(name), expected, "{name}");
        }
    }

    #[test]
    fn delimiter_scores_are_deterministic_and_reflect_error_context() {
        let mut start = DelimiterState::default();
        start.observe('(');
        let context = DelimiterScoreContext::new(&start, Some(')'));

        let mut resolved = start.clone();
        resolved.observe(')');
        let resolved_score = context.score(&resolved);
        assert_eq!(context.score(&resolved), resolved_score);

        let still_open = start.clone();
        assert!(resolved_score > context.score(&still_open));

        let unexpected_context = DelimiterScoreContext::new(&DelimiterState::default(), Some(']'));
        let mut structurally_consistent = DelimiterState::default();
        structurally_consistent.observe('[');
        assert!(
            unexpected_context.score(&structurally_consistent)
                > unexpected_context.score(&DelimiterState::default())
        );

        let mut mismatched_start = DelimiterState::default();
        mismatched_start.observe('{');
        let mismatched_context = DelimiterScoreContext::new(&mismatched_start, Some(')'));
        let mut resolved_mismatch = mismatched_start.clone();
        resolved_mismatch.observe('}');
        assert!(
            mismatched_context.score(&resolved_mismatch)
                > mismatched_context.score(&mismatched_start)
        );

        let mut new_unexpected_close = DelimiterState::default();
        new_unexpected_close.observe(')');
        assert!(
            unexpected_context.score(&new_unexpected_close)
                < unexpected_context.score(&DelimiterState::default())
        );
    }
}

#[derive(Debug)]
pub enum AStackType<LexemeT, ActionT> {
    ActionType(ActionT),
    Lexeme(LexemeT),
}

pub(super) struct Parser<
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
    rcvry_kind: RecoveryKind,
    pub(super) grm: &'a YaccGrammar<StorageT>,
    pub(super) token_cost: TokenCostFn<'a, StorageT>,
    pub(super) stable: &'a StateTable<StorageT>,
    lexer: &'b dyn NonStreamingLexer<'input, LexerTypesT>,
    // In the long term, we should remove the `lexemes` field entirely, as the `NonStreamingLexer` API is
    // powerful enough to allow us to incrementally obtain lexemes and buffer them when necessary.
    pub(super) lexemes: Vec<LexerTypesT::LexemeT>,
    actions: &'a [ActionFn<'a, 'b, 'input, LexerTypesT::StorageT, LexerTypesT, ActionT, ParamT>],
    param: ParamT,
}

impl<
    'a,
    'b: 'a,
    'input: 'b,
    StorageT: 'static + Debug + Hash + PrimInt + Unsigned,
    LexerTypesT: LexerTypes<StorageT = StorageT>,
    Node,
>
    Parser<
        'a,
        'b,
        'input,
        StorageT,
        LexerTypesT,
        Node,
        (
            &dyn Fn(LexerTypesT::LexemeT) -> Node,
            &dyn Fn(RIdx<StorageT>, Vec<Node>) -> Node,
        ),
    >
where
    usize: AsPrimitive<StorageT>,
{
    fn parse_map(
        rcvry_kind: RecoveryKind,
        grm: &YaccGrammar<StorageT>,
        token_cost: TokenCostFn<'a, StorageT>,
        stable: &StateTable<StorageT>,
        lexer: &'b dyn NonStreamingLexer<'input, LexerTypesT>,
        lexemes: Vec<LexerTypesT::LexemeT>,
        fterm: &'a dyn Fn(LexerTypesT::LexemeT) -> Node,
        fnonterm: &'a dyn Fn(RIdx<StorageT>, Vec<Node>) -> Node,
    ) -> (Option<Node>, Vec<LexParseError<StorageT, LexerTypesT>>) {
        for tidx in grm.iter_tidxs() {
            assert!(token_cost(tidx) > 0);
        }
        let mut actions: Vec<
            ActionFn<
                'a,
                'b,
                'input,
                StorageT,
                LexerTypesT,
                Node,
                (
                    &'a dyn Fn(LexerTypesT::LexemeT) -> Node,
                    &'a dyn Fn(RIdx<StorageT>, Vec<Node>) -> Node,
                ),
            >,
        > = Vec::new();
        actions.resize(usize::from(grm.prods_len()), &action_map);
        let psr = Parser {
            rcvry_kind,
            grm,
            token_cost,
            stable,
            lexer,
            lexemes,
            actions: actions.as_slice(),
            param: (fterm, fnonterm),
        };
        let mut pstack = vec![stable.start_state()];
        let mut astack = Vec::new();
        let mut errors = Vec::new();
        let mut spans = Vec::new();
        let accpt = psr.lr(0, &mut pstack, &mut astack, &mut errors, &mut spans);
        (accpt, errors)
    }
}

fn action_map<StorageT, LexerTypesT: LexerTypes, Node>(
    ridx: RIdx<StorageT>,
    _lexer: &dyn NonStreamingLexer<LexerTypesT>,
    _span: Span,
    astack: vec::Drain<AStackType<LexerTypesT::LexemeT, Node>>,
    param: (
        &dyn Fn(LexerTypesT::LexemeT) -> Node,
        &dyn Fn(RIdx<StorageT>, Vec<Node>) -> Node,
    ),
) -> Node
where
    usize: AsPrimitive<LexerTypesT::StorageT>,
    LexerTypesT::LexemeT: Lexeme<StorageT>,
{
    let (fterm, fnonterm) = param;
    let mut nodes = Vec::with_capacity(astack.len());
    for a in astack {
        nodes.push(match a {
            AStackType::ActionType(n) => n,
            AStackType::Lexeme(lexeme) => fterm(lexeme),
        });
    }
    fnonterm(ridx, nodes)
}

#[deprecated(
    since = "0.14.0",
    note = "Deprecated with `parse_generictree` there is no direct replacement, besides a custom action"
)]
#[allow(deprecated)]
/// The action which implements [`cfgrammar::yacc::YaccOriginalActionKind::GenericParseTree`].
/// Usually you should just use the action kind directly. But you can also call this from
/// within a custom action to return a generic parse tree with custom behavior.
pub fn action_generictree<StorageT, LexerTypesT: LexerTypes>(
    ridx: RIdx<StorageT>,
    _lexer: &dyn NonStreamingLexer<LexerTypesT>,
    _span: Span,
    astack: vec::Drain<AStackType<LexerTypesT::LexemeT, Node<LexerTypesT::LexemeT, StorageT>>>,
    _param: (),
) -> Node<LexerTypesT::LexemeT, StorageT>
where
    usize: AsPrimitive<LexerTypesT::StorageT>,
    LexerTypesT::LexemeT: Lexeme<StorageT>,
{
    let mut nodes = Vec::with_capacity(astack.len());
    for a in astack {
        nodes.push(match a {
            AStackType::ActionType(n) => n,
            AStackType::Lexeme(lexeme) => Node::Term { lexeme },
        });
    }
    Node::Nonterm { ridx, nodes }
}

impl<
    'a,
    'b: 'a,
    'input: 'b,
    StorageT: 'static + Debug + Eq + Hash + PrimInt + Unsigned,
    LexerTypesT: LexerTypes<StorageT = StorageT>,
    ActionT: 'a,
    ParamT: Clone,
> Parser<'a, 'b, 'input, StorageT, LexerTypesT, ActionT, ParamT>
where
    usize: AsPrimitive<StorageT>,
{
    fn parse_actions(
        rcvry_kind: RecoveryKind,
        grm: &'a YaccGrammar<StorageT>,
        token_cost: TokenCostFn<'a, StorageT>,
        stable: &'a StateTable<StorageT>,
        lexer: &'b dyn NonStreamingLexer<'input, LexerTypesT>,
        lexemes: Vec<LexerTypesT::LexemeT>,
        actions: &'a [ActionFn<'a, 'b, 'input, StorageT, LexerTypesT, ActionT, ParamT>],
        param: ParamT,
    ) -> (Option<ActionT>, Vec<LexParseError<StorageT, LexerTypesT>>) {
        for tidx in grm.iter_tidxs() {
            assert!(token_cost(tidx) > 0);
        }
        let psr = Parser {
            rcvry_kind,
            grm,
            token_cost,
            stable,
            lexer,
            lexemes,
            actions,
            param,
        };
        let mut pstack = vec![stable.start_state()];
        let mut astack = Vec::new();
        let mut errors = Vec::new();
        let mut spans = Vec::new();
        let accpt = psr.lr(0, &mut pstack, &mut astack, &mut errors, &mut spans);
        (accpt, errors)
    }

    /// Start parsing text at `laidx` (using the lexeme in `lexeme_prefix`, if it is not `None`,
    /// as the first lexeme) up to (but excluding) `end_laidx` (if it's specified). Parsing
    /// continues as long as possible (assuming that any errors encountered can be recovered from)
    /// unless `end_laidx` is `None`, at which point this function returns as soon as it
    /// encounters an error.
    ///
    /// Note that if `lexeme_prefix` is specified, `laidx` will still be incremented, and thus
    /// `end_laidx` *must* be set to `laidx + 1` in order that the parser doesn't skip the real
    /// lexeme at position `laidx`.
    ///
    /// Return `Some(value)` if the parse reached an accept state (i.e. all the input was consumed,
    /// possibly after making repairs) or `None` (i.e. some of the input was not consumed, even
    /// after possibly making repairs) otherwise.
    fn lr(
        &self,
        mut laidx: usize,
        pstack: &mut PStack<StorageT>,
        astack: &mut Vec<AStackType<LexerTypesT::LexemeT, ActionT>>,
        errors: &mut Vec<LexParseError<StorageT, LexerTypesT>>,
        spans: &mut Vec<Span>,
    ) -> Option<ActionT> {
        let mut recoverer = None;
        let mut recovery_budget = Duration::from_millis(RECOVERY_TIME_BUDGET);
        let mut delimiter_state = DelimiterState::default();
        loop {
            debug_assert_eq!(astack.len(), spans.len());
            let stidx = *pstack.last().unwrap();
            let la_tidx = self.next_tidx(laidx);

            match self.stable.action(stidx, la_tidx) {
                Action::Reduce(pidx) => {
                    let ridx = self.grm.prod_to_rule(pidx);
                    let pop_idx = pstack.len() - self.grm.prod(pidx).len();

                    pstack.drain(pop_idx..);
                    let prior = *pstack.last().unwrap();
                    pstack.push(self.stable.goto(prior, ridx).unwrap());

                    let span = if spans.is_empty() {
                        Span::new(0, 0)
                    } else if pop_idx - 1 < spans.len() {
                        Span::new(spans[pop_idx - 1].start(), spans[spans.len() - 1].end())
                    } else {
                        Span::new(spans[spans.len() - 1].start(), spans[spans.len() - 1].end())
                    };
                    spans.truncate(pop_idx - 1);
                    spans.push(span);

                    let v = AStackType::ActionType(self.actions[usize::from(pidx)](
                        ridx,
                        self.lexer,
                        span,
                        astack.drain(pop_idx - 1..),
                        self.param.clone(),
                    ));
                    astack.push(v);
                }
                Action::Shift(state_id) => {
                    let la_lexeme = self.next_lexeme(laidx);
                    let byte_index = la_lexeme.span().start();
                    if let Some(delimiter) = self.delimiter_for_lexeme(&la_lexeme) {
                        let char_index = self.delimiter_char_index(byte_index);
                        delimiter_state.observe_at(
                            delimiter,
                            DelimiterOrigin {
                                input_index: laidx,
                                char_index,
                                byte_index,
                                kind: DelimiterOriginKind::Real,
                            },
                        );
                    }
                    pstack.push(state_id);
                    astack.push(AStackType::Lexeme(la_lexeme));

                    spans.push(la_lexeme.span());
                    laidx += 1;
                }
                Action::Accept => {
                    debug_assert_eq!(la_tidx, self.grm.eof_token_idx());
                    debug_assert_eq!(astack.len(), 1);
                    match astack.drain(..).next().unwrap() {
                        AStackType::ActionType(v) => return Some(v),
                        _ => unreachable!(),
                    }
                }
                Action::Error => {
                    if recoverer.is_none() {
                        recoverer = Some(match self.rcvry_kind {
                            RecoveryKind::CPCTPlus => cpctplus::recoverer(self, false, false),
                            RecoveryKind::CPCTPlusDelimiterDominance => {
                                cpctplus::recoverer(self, true, false)
                            }
                            RecoveryKind::CPCTPlusDelimiterLocalization => {
                                cpctplus::recoverer(self, false, true)
                            }
                            RecoveryKind::CPCTPlusDelimiterDominanceAndLocalization => {
                                cpctplus::recoverer(self, true, true)
                            }
                            RecoveryKind::None => {
                                let la_lexeme = self.next_lexeme(laidx);
                                errors.push(
                                    ParseError {
                                        stidx,
                                        lexeme: la_lexeme,
                                        repairs: vec![],
                                        recovery_stats: None,
                                    }
                                    .into(),
                                );
                                return None;
                            }
                        });
                    }

                    let before = Instant::now();
                    let finish_by = before + recovery_budget;
                    let (new_laidx, repairs, new_delimiter_state, recovery_stats) = recoverer
                        .as_ref()
                        .unwrap()
                        .as_ref()
                        .recover(
                            finish_by,
                            self,
                            laidx,
                            pstack,
                            astack,
                            spans,
                            &delimiter_state,
                        );
                    let after = Instant::now();
                    recovery_budget = recovery_budget
                        .checked_sub(after - before)
                        .unwrap_or_else(|| Duration::new(0, 0));
                    let keep_going = !repairs.is_empty();
                    let la_lexeme = self.next_lexeme(laidx);
                    errors.push(
                        ParseError {
                            stidx,
                            lexeme: la_lexeme,
                            repairs,
                            recovery_stats: Some(recovery_stats),
                        }
                        .into(),
                    );
                    if !keep_going {
                        return None;
                    }
                    laidx = new_laidx;
                    delimiter_state = new_delimiter_state;
                }
            }
        }
    }

    /// Parse from `laidx` up to (but excluding) `end_laidx` mutating `pstack` as parsing occurs.
    /// Returns the index of the token it parsed up to (by definition <= end_laidx: can be less if
    /// the input is < end_laidx, or if an error is encountered). Does not do any form of error
    /// recovery.
    pub(super) fn lr_upto(
        &self,
        lexeme_prefix: Option<LexerTypesT::LexemeT>,
        mut laidx: usize,
        end_laidx: usize,
        pstack: &mut PStack<StorageT>,
        astack: &mut Option<&mut Vec<AStackType<LexerTypesT::LexemeT, ActionT>>>,
        spans: &mut Option<&mut Vec<Span>>,
    ) -> usize {
        assert!(lexeme_prefix.is_none() || end_laidx == laidx + 1);
        while laidx != end_laidx && laidx <= self.lexemes.len() {
            let stidx = *pstack.last().unwrap();
            let la_tidx = if let Some(l) = lexeme_prefix {
                TIdx(l.tok_id())
            } else {
                self.next_tidx(laidx)
            };

            match self.stable.action(stidx, la_tidx) {
                Action::Reduce(pidx) => {
                    let ridx = self.grm.prod_to_rule(pidx);
                    let pop_idx = pstack.len() - self.grm.prod(pidx).len();
                    if let Some(ref mut astack_uw) = *astack {
                        if let Some(ref mut spans_uw) = *spans {
                            let span = if spans_uw.is_empty() {
                                Span::new(0, 0)
                            } else if pop_idx - 1 < spans_uw.len() {
                                Span::new(
                                    spans_uw[pop_idx - 1].start(),
                                    spans_uw[spans_uw.len() - 1].end(),
                                )
                            } else {
                                Span::new(
                                    spans_uw[spans_uw.len() - 1].start(),
                                    spans_uw[spans_uw.len() - 1].end(),
                                )
                            };
                            spans_uw.truncate(pop_idx - 1);
                            spans_uw.push(span);

                            let v = AStackType::ActionType(self.actions[usize::from(pidx)](
                                ridx,
                                self.lexer,
                                span,
                                astack_uw.drain(pop_idx - 1..),
                                self.param.clone(),
                            ));
                            astack_uw.push(v);
                        } else {
                            unreachable!();
                        }
                    }

                    pstack.drain(pop_idx..);
                    let prior = *pstack.last().unwrap();
                    pstack.push(self.stable.goto(prior, ridx).unwrap());
                }
                Action::Shift(state_id) => {
                    if let Some(ref mut astack_uw) = *astack
                        && let Some(spans_uw) = spans
                    {
                        let la_lexeme = if let Some(l) = lexeme_prefix {
                            l
                        } else {
                            self.next_lexeme(laidx)
                        };
                        astack_uw.push(AStackType::Lexeme(la_lexeme));
                        spans_uw.push(la_lexeme.span());
                    }
                    pstack.push(state_id);
                    laidx += 1;
                }
                Action::Accept => {
                    break;
                }
                Action::Error => {
                    break;
                }
            }
        }
        laidx
    }

    /// Return a `Lexeme` for the next lemexe (if `laidx` == `self.lexemes.len()` this will be
    /// a lexeme constructed to look as if contains the EOF token).
    pub(super) fn next_lexeme(&self, laidx: usize) -> LexerTypesT::LexemeT {
        let llen = self.lexemes.len();
        debug_assert!(laidx <= llen);
        if laidx < llen {
            self.lexemes[laidx]
        } else {
            // We have to artificially construct a Lexeme for the EOF lexeme.
            let last_la_end = if llen == 0 {
                0
            } else {
                debug_assert!(laidx > 0);
                let last_la = self.lexemes[laidx - 1];
                last_la.span().end()
            };

            Lexeme::new_faulty(
                StorageT::from(u32::from(self.grm.eof_token_idx())).unwrap(),
                last_la_end,
                0,
            )
        }
    }

    /// Return the `TIdx` of the next lexeme (if `laidx` == `self.lexemes.len()` this will be the
    /// EOF `TIdx`).
    pub(super) fn next_tidx(&self, laidx: usize) -> TIdx<StorageT> {
        let ll = self.lexemes.len();
        debug_assert!(laidx <= ll);
        if laidx < ll {
            TIdx(self.lexemes[laidx].tok_id())
        } else {
            self.grm.eof_token_idx()
        }
    }

    pub(super) fn delimiter_for_lexeme(
        &self,
        lexeme: &LexerTypesT::LexemeT,
    ) -> Option<char> {
        delimiter_char_from_text(self.lexer.span_str(lexeme.span()))
    }

    pub(super) fn delimiter_char_index(&self, byte_index: usize) -> usize {
        self.lexer
            .span_str(Span::new(0, byte_index))
            .chars()
            .count()
    }

    /// Start parsing text at `laidx` (using the lexeme in `lexeme_prefix`, if it is not `None`,
    /// as the first lexeme) up to (but excluding) `end_laidx`. If an error is encountered, parsing
    /// immediately terminates (without recovery).
    ///
    /// Note that if `lexeme_prefix` is specified, `laidx` will still be incremented, and thus
    /// `end_laidx` *must* be set to `laidx + 1` in order that the parser doesn't skip the real
    /// lexeme at position `laidx`.
    #[allow(deprecated)]
    pub(super) fn lr_cactus(
        &self,
        lexeme_prefix: Option<LexerTypesT::LexemeT>,
        mut laidx: usize,
        end_laidx: usize,
        mut pstack: Cactus<StIdx<StorageT>>,
        tstack: &mut Option<&mut Vec<Node<LexerTypesT::LexemeT, StorageT>>>,
    ) -> (usize, Cactus<StIdx<StorageT>>) {
        assert!(lexeme_prefix.is_none() || end_laidx == laidx + 1);
        while laidx != end_laidx {
            let stidx = *pstack.val().unwrap();
            let la_tidx = if let Some(l) = lexeme_prefix {
                TIdx(l.tok_id())
            } else {
                self.next_tidx(laidx)
            };

            match self.stable.action(stidx, la_tidx) {
                Action::Reduce(pidx) => {
                    let ridx = self.grm.prod_to_rule(pidx);
                    let pop_num = self.grm.prod(pidx).len();
                    if let Some(ref mut tstack_uw) = *tstack {
                        let nodes = tstack_uw
                            .drain(pstack.len() - pop_num - 1..)
                            .collect::<Vec<Node<LexerTypesT::LexemeT, StorageT>>>();
                        tstack_uw.push(Node::Nonterm { ridx, nodes });
                    }

                    for _ in 0..pop_num {
                        pstack = pstack.parent().unwrap();
                    }
                    let prior = *pstack.val().unwrap();
                    pstack = pstack.child(self.stable.goto(prior, ridx).unwrap());
                }
                Action::Shift(state_id) => {
                    if let Some(ref mut tstack_uw) = *tstack {
                        let la_lexeme = if let Some(l) = lexeme_prefix {
                            l
                        } else {
                            self.next_lexeme(laidx)
                        };
                        tstack_uw.push(Node::Term { lexeme: la_lexeme });
                    }
                    pstack = pstack.child(state_id);
                    laidx += 1;
                }
                Action::Accept => {
                    debug_assert_eq!(la_tidx, self.grm.eof_token_idx());
                    if let Some(ref tstack_uw) = *tstack {
                        debug_assert_eq!(tstack_uw.len(), 1);
                    }
                    break;
                }
                Action::Error => {
                    break;
                }
            }
        }
        (laidx, pstack)
    }
}

pub(super) trait Recoverer<
    StorageT: 'static + Debug + Hash + PrimInt + Unsigned,
    LexerTypesT: LexerTypes<StorageT = StorageT>,
    ActionT,
    ParamT: Clone,
> where
    usize: AsPrimitive<StorageT>,
{
    fn recover(
        &self,
        finish_by: Instant,
        parser: &Parser<StorageT, LexerTypesT, ActionT, ParamT>,
        in_laidx: usize,
        in_pstack: &mut PStack<StorageT>,
        astack: &mut Vec<AStackType<LexerTypesT::LexemeT, ActionT>>,
        spans: &mut Vec<Span>,
        delimiter_state: &DelimiterState,
    ) -> (
        usize,
        Vec<Vec<ParseRepair<LexerTypesT::LexemeT, StorageT>>>,
        DelimiterState,
        RecoveryStats,
    );
}

/// Search statistics collected during one CPCT+ recovery attempt.
///
/// Counters observe each recovery search. Strict-cost delimiter dominance changes search only
/// when explicitly enabled through [`RecoveryKind::CPCTPlusDelimiterDominance`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecoveryStats {
    /// Neighbor candidates offered to the frontier, including ones later merged or rejected.
    pub configurations_generated: u64,
    /// Unique frontier entries removed and checked for success.
    pub configurations_popped: u64,
    /// Frontier key collisions for which the merge callback was invoked, including duplicates.
    pub configurations_merged: u64,
    /// Discard events summed across reason counters; a reason may count frontier nodes or variants.
    pub configurations_discarded: u64,
    /// Same-cost-closure neighbors rejected because they exceed the minimum repair cost.
    pub configurations_discarded_cost: u64,
    /// Queued configurations left unexplored when the recovery deadline is reached.
    pub configurations_discarded_timeout: u64,
    /// Higher-cost frontier configurations no longer needed after finding minimum-cost repairs.
    pub configurations_discarded_higher_cost: u64,
    /// Duplicate repair histories ignored during a merge; these are also counted as merged.
    pub configurations_discarded_duplicate: u64,
    /// Repair variants discarded because an exact delimiter-aware state was already reached
    /// with strictly lower CPCT+ cost.
    pub configurations_discarded_dominance: u64,
    /// Number of repair variants considered by the strict-cost dominance index.
    pub dominance_candidates_checked: u64,
    /// Candidates whose complete dominance key was already present, including equal-cost cases.
    pub dominance_exact_key_matches: u64,
    /// Number of repair-history variants removed by strict-cost dominance.
    pub dominance_variants_discarded: u64,
    /// Sum of (discarded cost - dominating cost) over discarded variants.
    pub dominance_discarded_cost_difference: u64,
    /// Whether strict-cost delimiter-aware dominance was enabled for this recovery attempt.
    pub delimiter_dominance_enabled: bool,
    /// Whether delimiter-origin localization influenced CPCT+ frontier exploration.
    pub delimiter_localization_enabled: bool,
    /// Structural event associated with the selected localization target.
    pub localization_target: Option<String>,
    /// Opening checkpoint id, when the selected target is an opening delimiter.
    pub localization_checkpoint_id: Option<usize>,
    /// Delimiter character associated with the selected localization target.
    pub localization_delimiter: Option<char>,
    /// Input token index for the selected origin or unexpected closing event.
    pub localization_origin_input_index: Option<usize>,
    /// Byte offset for the selected origin or unexpected closing event.
    pub localization_origin_byte_index: Option<usize>,
    /// Character index for the selected origin or unexpected closing event.
    pub localization_origin_char_index: Option<usize>,
    /// Nesting depth of the selected target.
    pub localization_origin_depth: Option<usize>,
    /// Input token index where CPCT+ actually began recovery.
    pub recovery_start_input_index: Option<usize>,
    /// Byte offset where CPCT+ actually began recovery.
    pub recovery_start_byte_index: Option<usize>,
    /// Absolute byte distance between the selected origin/event and recovery start.
    pub localization_to_recovery_start_distance: Option<usize>,
    /// Byte offset of the final operation in the selected repair sequence.
    pub final_repair_byte_index: Option<usize>,
    /// Absolute byte distance from the selected origin/event to the final repair.
    pub localization_to_repair_distance: Option<usize>,
    /// Absolute byte distance between the recovery start and final repair operation.
    pub recovery_start_to_repair_distance: Option<usize>,
    /// Popped configurations prioritized because they resolved the selected target.
    pub localization_priority_pops: u64,
    /// Pops whose localization priority changed the legacy LIFO order within a cost bucket.
    pub localization_priority_reorders: u64,
    /// Candidate insertion transitions attempted (one per non-EOF terminal considered).
    pub insert_transitions_attempted: u64,
    /// Delete transition calls, including calls at EOF where no transition can be emitted.
    pub delete_transitions_attempted: u64,
    /// Shift/reduction transition calls.
    pub shift_transitions_attempted: u64,
    /// Concrete repair sequences represented after collection of merged histories.
    pub repair_sequences_after_collection: Option<u64>,
    /// Repair sequences retained by continuation-distance filtering.
    pub repair_sequences_after_continuation: Option<u64>,
    /// Repair sequences remaining after simplification, deduplication, and sorting.
    pub repair_sequences_after_simplification: Option<u64>,
    /// Minimum successful CPCT+ repair cost found, if a success node was reached.
    pub minimum_recovery_cost: Option<u16>,
    /// Number of popped configurations observed at each deterministic delimiter score.
    pub delimiter_score_distribution: BTreeMap<i64, u64>,
    /// Scores of successful configurations at the minimum CPCT+ repair cost.
    pub minimum_cost_success_delimiter_scores: Vec<i64>,
    /// Structural score of the repair sequence selected by the unchanged CPCT+ ranking.
    pub selected_repair_delimiter_score: Option<i64>,
    /// Whether recovery stopped because it reached its deadline.
    pub timed_out: bool,
    /// Wall-clock duration spent in this recovery attempt.
    pub elapsed: Duration,
}

/// What recovery algorithm should be used when a syntax error is encountered?
#[derive(Clone, Copy, Debug)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[non_exhaustive]
pub enum RecoveryKind {
    /// The CPCT+ algorithm from Diekmann/Tratt "Don't Panic! Better, Fewer, Syntax Errors for LR
    /// Parsers".
    CPCTPlus,
    /// Experimental CPCT+ with strict-cost dominance over identical parser, continuation, and
    /// complete delimiter states. Equal-cost histories are retained.
    CPCTPlusDelimiterDominance,
    /// CPCT+ with delimiter-origin prioritization inside equal-cost frontier buckets.
    CPCTPlusDelimiterLocalization,
    /// CPCT+ with both delimiter localization and strict-cost delimiter-aware dominance.
    CPCTPlusDelimiterDominanceAndLocalization,
    /// Don't use error recovery: return as soon as the first syntax error is encountered.
    None,
}

impl TryFrom<RecoveryKind> for Value<Location> {
    type Error = cfgrammar::header::HeaderError<Location>;
    fn try_from(rk: RecoveryKind) -> Result<Value<Location>, Self::Error> {
        use cfgrammar::{
            Location,
            header::{Namespaced, Setting},
        };
        let from_loc = Location::Other("From<RecoveryKind>".to_string());
        Ok(match rk {
            RecoveryKind::CPCTPlus => Value::Setting(Setting::Unitary(Namespaced {
                namespace: Some(("RecoveryKind".to_string(), from_loc.clone())),
                member: ("CPCTPlus".to_string(), from_loc.clone()),
            })),
            RecoveryKind::CPCTPlusDelimiterDominance => Value::Setting(Setting::Unitary(Namespaced {
                namespace: Some(("RecoveryKind".to_string(), from_loc.clone())),
                member: ("CPCTPlusDelimiterDominance".to_string(), from_loc.clone()),
            })),
            RecoveryKind::CPCTPlusDelimiterLocalization => {
                Value::Setting(Setting::Unitary(Namespaced {
                    namespace: Some(("RecoveryKind".to_string(), from_loc.clone())),
                    member: ("CPCTPlusDelimiterLocalization".to_string(), from_loc.clone()),
                }))
            }
            RecoveryKind::CPCTPlusDelimiterDominanceAndLocalization => {
                Value::Setting(Setting::Unitary(Namespaced {
                    namespace: Some(("RecoveryKind".to_string(), from_loc.clone())),
                    member: (
                        "CPCTPlusDelimiterDominanceAndLocalization".to_string(),
                        from_loc.clone(),
                    ),
                }))
            }
            RecoveryKind::None => Value::Setting(Setting::Unitary(Namespaced {
                namespace: Some(("RecoveryKind".to_string(), from_loc.clone())),
                member: ("None".to_string(), from_loc.clone()),
            })),
        })
    }
}

impl TryFrom<&Value<Location>> for RecoveryKind {
    type Error = cfgrammar::header::HeaderError<Location>;
    fn try_from(rk: &Value<Location>) -> Result<RecoveryKind, Self::Error> {
        use cfgrammar::header::{HeaderError, HeaderErrorKind, Namespaced, Setting};

        match rk {
            Value::Setting(Setting::Unitary(Namespaced {
                namespace,
                member: (kind, kind_loc),
            })) => {
                match namespace {
                    Some((ns, loc)) if ns.to_lowercase() != "recoverykind" => {
                        return Err(HeaderError {
                            kind: HeaderErrorKind::ConversionError(
                                "RecoveryKind",
                                "Unknown namespace",
                            ),
                            locations: vec![loc.clone()],
                        });
                    }
                    _ => {}
                }
                match kind.to_lowercase().as_ref() {
                    "cpctplus" => Ok(RecoveryKind::CPCTPlus),
                    "cpctplusdelimiterdominance" => {
                        Ok(RecoveryKind::CPCTPlusDelimiterDominance)
                    }
                    "cpctplusdelimiterlocalization" => {
                        Ok(RecoveryKind::CPCTPlusDelimiterLocalization)
                    }
                    "cpctplusdelimiterdominanceandlocalization" => {
                        Ok(RecoveryKind::CPCTPlusDelimiterDominanceAndLocalization)
                    }
                    "none" => Ok(RecoveryKind::None),
                    _ => Err(HeaderError {
                        kind: HeaderErrorKind::ConversionError("RecoveryKind", "Unknown variant"),
                        locations: vec![kind_loc.clone()],
                    }),
                }
            }
            value => Err(HeaderError {
                kind: HeaderErrorKind::ConversionError(
                    "RecoveryKind",
                    "Cannot convert to RecoveryKind",
                ),
                locations: vec![value.primary_location().clone()],
            }),
        }
    }
}

impl quote::ToTokens for RecoveryKind {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        tokens.extend(match *self {
            RecoveryKind::CPCTPlus => quote!(::lrpar::RecoveryKind::CPCTPlus),
            RecoveryKind::CPCTPlusDelimiterDominance => {
                quote!(::lrpar::RecoveryKind::CPCTPlusDelimiterDominance)
            }
            RecoveryKind::CPCTPlusDelimiterLocalization => {
                quote!(::lrpar::RecoveryKind::CPCTPlusDelimiterLocalization)
            }
            RecoveryKind::CPCTPlusDelimiterDominanceAndLocalization => {
                quote!(::lrpar::RecoveryKind::CPCTPlusDelimiterDominanceAndLocalization)
            }
            RecoveryKind::None => quote!(::lrpar::RecoveryKind::None),
        })
    }
}

/// A lexing or parsing error. Although the two are quite distinct in terms of what can be reported
/// to users, both can (at least conceptually) occur at any point of the intertwined lexing/parsing
/// process.
#[derive(Debug)]
pub enum LexParseError<
    StorageT: 'static + Debug + Hash + PrimInt + Unsigned,
    LexerTypesT: LexerTypes<StorageT = StorageT>,
> where
    usize: AsPrimitive<StorageT>,
{
    LexError(LexerTypesT::LexErrorT),
    ParseError(ParseError<LexerTypesT::LexemeT, StorageT>),
}

impl<StorageT: Debug + Hash + PrimInt + Unsigned, LexerTypesT: LexerTypes<StorageT = StorageT>>
    LexParseError<StorageT, LexerTypesT>
where
    usize: AsPrimitive<StorageT>,
{
    /// A pretty-printer of a lexer/parser error. This isn't suitable for all purposes, but it's
    /// often good enough. The output format is not guaranteed to be stable but is likely to be of
    /// the form:
    ///
    /// ```text
    /// Parsing error at line 3 column 8. Repair sequences found:
    ///   1: Insert ID
    ///   2: Delete +, Shift 3
    /// ```
    ///
    /// If you are using the compile-time parse system, your `grm_y` module exposes a suitable
    /// [`epp`](../ctbuilder/struct.CTParserBuilder.html#method.process_file) function; if you are
    /// using the run-time system,
    /// [`YaccGrammar`](../../cfgrammar/yacc/grammar/struct.YaccGrammar.html) exposes a suitable
    /// [`epp`](../../cfgrammar/yacc/grammar/struct.YaccGrammar.html#method.token_epp) function,
    /// though you will have to wrap it in a closure e.g. `&|t| grm.token_epp(t)`.
    pub fn pp<'a>(
        &self,
        lexer: &dyn NonStreamingLexer<LexerTypesT>,
        epp: &dyn Fn(TIdx<StorageT>) -> Option<&'a str>,
    ) -> String {
        match self {
            LexParseError::LexError(e) => {
                let ((line, col), _) = lexer.line_col(e.span());
                format!("Lexing error at line {} column {}.", line, col)
            }
            LexParseError::ParseError(e) => {
                let ((line, col), _) = lexer.line_col(e.lexeme().span());
                let mut out = format!("Parsing error at line {} column {}.", line, col);
                let repairs_len = e.repairs().len();
                if repairs_len == 0 {
                    out.push_str(" No repair sequences found.");
                } else {
                    out.push_str(" Repair sequences found:");
                    for (i, rs) in e.repairs().iter().enumerate() {
                        let padding = ((repairs_len as f64).log10() as usize)
                            - (((i + 1) as f64).log10() as usize)
                            + 1;
                        write!(out, "\n  {}{}: ", " ".repeat(padding), i + 1).ok();
                        let mut rs_out = Vec::new();

                        // Merge together Deletes iff they are consecutive (if they are separated
                        // by even a single character, they will not be merged).
                        let mut i = 0;
                        while i < rs.len() {
                            match rs[i] {
                                ParseRepair::Delete(l) => {
                                    let mut j = i + 1;
                                    let mut last_end = l.span().end();
                                    while j < rs.len() {
                                        if let ParseRepair::Delete(next_l) = rs[j]
                                            && next_l.span().start() == last_end
                                        {
                                            last_end = next_l.span().end();
                                            j += 1;
                                            continue;
                                        }
                                        break;
                                    }
                                    let t = &lexer
                                        .span_str(Span::new(l.span().start(), last_end))
                                        .replace('\n', "\\n");
                                    rs_out.push(format!("Delete {}", t));
                                    i = j;
                                }
                                ParseRepair::Insert(tidx) => {
                                    rs_out.push(format!("Insert {}", epp(tidx).unwrap()));
                                    i += 1;
                                }
                                ParseRepair::Shift(l) => {
                                    let t = &lexer.span_str(l.span()).replace('\n', "\\n");
                                    rs_out.push(format!("Shift {}", t));
                                    i += 1;
                                }
                            }
                        }

                        out.push_str(&rs_out.join(", "));
                    }
                }
                out
            }
        }
    }
}

impl<
    StorageT: 'static + Debug + Hash + PrimInt + Unsigned,
    LexerTypesT: LexerTypes<StorageT = StorageT>,
> fmt::Display for LexParseError<StorageT, LexerTypesT>
where
    usize: AsPrimitive<StorageT>,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            LexParseError::LexError(ref e) => Display::fmt(e, f),
            LexParseError::ParseError(ref e) => Display::fmt(e, f),
        }
    }
}

impl<
    StorageT: 'static + Debug + Hash + PrimInt + Unsigned,
    LexerTypesT: LexerTypes<StorageT = StorageT>,
> Error for LexParseError<StorageT, LexerTypesT>
where
    usize: AsPrimitive<StorageT>,
{
}

impl<
    StorageT: 'static + Debug + Hash + PrimInt + Unsigned,
    LexerTypesT: LexerTypes<StorageT = StorageT, LexErrorT = T>,
    T: LexError,
> From<T> for LexParseError<StorageT, LexerTypesT>
where
    usize: AsPrimitive<StorageT>,
{
    fn from(err: T) -> LexParseError<StorageT, LexerTypesT> {
        LexParseError::LexError(err)
    }
}

impl<
    StorageT: 'static + Debug + Hash + PrimInt + Unsigned,
    LexerTypesT: LexerTypes<StorageT = StorageT>,
> From<ParseError<LexerTypesT::LexemeT, StorageT>> for LexParseError<StorageT, LexerTypesT>
where
    usize: AsPrimitive<StorageT>,
{
    fn from(
        err: ParseError<LexerTypesT::LexemeT, StorageT>,
    ) -> LexParseError<StorageT, LexerTypesT> {
        LexParseError::ParseError(err)
    }
}

/// A run-time parser builder.
pub struct RTParserBuilder<
    'a,
    StorageT: 'static + Eq + Debug + Hash + PrimInt + Unsigned,
    LexerTypesT: LexerTypes<StorageT = StorageT>,
> where
    usize: AsPrimitive<StorageT>,
{
    grm: &'a YaccGrammar<StorageT>,
    stable: &'a StateTable<StorageT>,
    recoverer: RecoveryKind,
    term_costs: &'a dyn Fn(TIdx<StorageT>) -> u8,
    phantom: PhantomData<(StorageT, LexerTypesT)>,
}

impl<
    'a,
    StorageT: 'static + Debug + Hash + PrimInt + Unsigned,
    LexerTypesT: LexerTypes<StorageT = StorageT>,
> RTParserBuilder<'a, StorageT, LexerTypesT>
where
    usize: AsPrimitive<StorageT>,
{
    /// Create a new run-time parser from a `YaccGrammar`, and a `StateTable`.
    pub fn new(grm: &'a YaccGrammar<StorageT>, stable: &'a StateTable<StorageT>) -> Self {
        RTParserBuilder {
            grm,
            stable,
            recoverer: RecoveryKind::CPCTPlus,
            term_costs: &|_| 1,
            phantom: PhantomData,
        }
    }

    /// Set the recoverer for this parser to `rk`.
    pub fn recoverer(mut self, rk: RecoveryKind) -> Self {
        self.recoverer = rk;
        self
    }

    pub fn term_costs(mut self, f: &'a dyn Fn(TIdx<StorageT>) -> u8) -> Self {
        self.term_costs = f;
        self
    }

    #[deprecated(
        since = "0.14.0",
        note = "Use `parse_map` to return a `Node` from your `lrpar_mod!` instead"
    )]
    #[allow(deprecated)]
    /// Parse input, and (if possible) return a generic parse tree. See the arguments for
    /// [`parse_actions`](#method.parse_actions) for more details about the return value.
    pub fn parse_generictree(
        &self,
        lexer: &dyn NonStreamingLexer<LexerTypesT>,
    ) -> (
        Option<Node<LexerTypesT::LexemeT, StorageT>>,
        Vec<LexParseError<StorageT, LexerTypesT>>,
    ) {
        self.parse_map(lexer, &|lexeme| Node::Term { lexeme }, &|ridx, nodes| {
            Node::Nonterm { ridx, nodes }
        })
    }

    /// Parse input, and (if possible) return a generic parse tree. See the arguments for
    /// [`parse_actions`](#method.parse_actions) for more details about the return value.
    pub fn parse_map<Node>(
        &self,
        lexer: &dyn NonStreamingLexer<LexerTypesT>,
        fterm: &dyn Fn(LexerTypesT::LexemeT) -> Node,
        fnonterm: &dyn Fn(RIdx<StorageT>, Vec<Node>) -> Node,
    ) -> (Option<Node>, Vec<LexParseError<StorageT, LexerTypesT>>) {
        let lexemes = match lexer.iter().collect() {
            Ok(lexemes) => lexemes,
            Err(e) => return (None, vec![e.into()]),
        };
        Parser::<
            StorageT,
            LexerTypesT,
            Node,
            (
                &dyn Fn(LexerTypesT::LexemeT) -> Node,
                &dyn Fn(RIdx<StorageT>, Vec<Node>) -> Node,
            ),
        >::parse_map(
            self.recoverer,
            self.grm,
            self.term_costs,
            self.stable,
            lexer,
            lexemes,
            fterm,
            fnonterm,
        )
    }

    #[deprecated(since = "0.14.0", note = "Use `parse_map` instead")]
    /// Parse input, returning any errors found. See the arguments for
    /// [`parse_actions`](#method.parse_actions) for more details about the return value.
    pub fn parse_noaction(
        &self,
        lexer: &dyn NonStreamingLexer<LexerTypesT>,
    ) -> Vec<LexParseError<StorageT, LexerTypesT>> {
        self.parse_map(lexer, &|_| (), &|_, _| ()).1
    }

    /// Parse input, execute actions, and return the associated value (if possible) and/or any
    /// lexing/parsing errors encountered. Note that the two parts of the (value, errors) return
    /// pair are entirely independent: one can encounter errors without a value being produced
    /// (`None, [...]`), errors and a value (`Some(...), [...]`), as well as a value and no errors
    /// (`Some(...), []`). Errors are sorted by the position they were found in the input and can
    /// be a mix of lexing and parsing errors.
    pub fn parse_actions<'b: 'a, 'input: 'b, ActionT: 'a, ParamT: Clone>(
        &self,
        lexer: &'b dyn NonStreamingLexer<'input, LexerTypesT>,
        actions: &'a [ActionFn<'a, 'b, 'input, StorageT, LexerTypesT, ActionT, ParamT>],
        param: ParamT,
    ) -> (Option<ActionT>, Vec<LexParseError<StorageT, LexerTypesT>>) {
        let lexemes = match lexer.iter().collect() {
            Ok(lexemes) => lexemes,
            Err(e) => return (None, vec![e.into()]),
        };
        Parser::parse_actions(
            self.recoverer,
            self.grm,
            self.term_costs,
            self.stable,
            lexer,
            lexemes,
            actions,
            param,
        )
    }

    pub fn grammar(&self) -> &YaccGrammar<StorageT> {
        self.grm
    }
}

/// After a parse error is encountered, the parser attempts to find a way of recovering. Each entry
/// in the sequence of repairs is represented by a `ParseRepair`.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ParseRepair<LexemeT: Lexeme<StorageT>, StorageT: Hash> {
    /// Insert a `Symbol::Token`.
    Insert(TIdx<StorageT>),
    /// Delete a symbol.
    Delete(LexemeT),
    /// Shift a symbol.
    Shift(LexemeT),
}

/// Records a single parse error.
#[derive(Clone)]
pub struct ParseError<LexemeT: Lexeme<StorageT>, StorageT: Hash> {
    stidx: StIdx<StorageT>,
    lexeme: LexemeT,
    repairs: Vec<Vec<ParseRepair<LexemeT, StorageT>>>,
    recovery_stats: Option<RecoveryStats>,
}

impl<LexemeT: Lexeme<StorageT> + Debug, StorageT: Debug + Hash> Debug
    for ParseError<LexemeT, StorageT>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParseError")
            .field("stidx", &self.stidx)
            .field("lexeme", &self.lexeme)
            .field("repairs", &self.repairs)
            .finish()
    }
}

impl<LexemeT: Lexeme<StorageT>, StorageT: Hash + PartialEq> PartialEq
    for ParseError<LexemeT, StorageT>
where
    LexemeT: PartialEq,
    StIdx<StorageT>: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.stidx == other.stidx && self.lexeme == other.lexeme && self.repairs == other.repairs
    }
}

impl<LexemeT: Lexeme<StorageT>, StorageT: Debug + Hash> Display for ParseError<LexemeT, StorageT> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Parse error at lexeme {:?}", self.lexeme)
    }
}

impl<LexemeT: Lexeme<StorageT>, StorageT: Debug + Hash> Error for ParseError<LexemeT, StorageT> {}

impl<LexemeT: Lexeme<StorageT>, StorageT: Hash + PrimInt + Unsigned> ParseError<LexemeT, StorageT> {
    /// Return the state table index where this error was detected.
    pub fn stidx(&self) -> StIdx<StorageT> {
        self.stidx
    }

    /// Return the lexeme where this error was detected.
    pub fn lexeme(&self) -> &LexemeT {
        &self.lexeme
    }

    /// Return the repairs found that would fix this error. Note that there are infinite number of
    /// possible repairs for any error, so this is by definition a (finite) subset.
    pub fn repairs(&self) -> &Vec<Vec<ParseRepair<LexemeT, StorageT>>> {
        &self.repairs
    }

    /// Return statistics for the CPCT+ recovery attempt that produced this error, if any.
    ///
    /// Parse errors produced with recovery disabled have no recovery statistics.
    pub fn recovery_stats(&self) -> Option<&RecoveryStats> {
        self.recovery_stats.as_ref()
    }
}

#[cfg(test)]
pub(crate) mod test {
    use std::collections::HashMap;

    use cfgrammar::{
        Span,
        yacc::{YaccGrammar, YaccKind, YaccOriginalActionKind},
    };
    use lrtable::{Minimiser, from_yacc};
    use num_traits::ToPrimitive;
    use regex::Regex;

    use super::*;
    use crate::{
        Lexeme, Lexer,
        test_utils::{TestLexError, TestLexeme, TestLexerTypes},
    };

    #[allow(deprecated)]
    pub(crate) fn do_parse<'input>(
        rcvry_kind: RecoveryKind,
        lexs: &str,
        grms: &str,
        input: &'input str,
    ) -> (
        YaccGrammar<u16>,
        SmallLexer<'input>,
        Result<
            Node<TestLexeme, u16>,
            (
                Option<Node<TestLexeme, u16>>,
                Vec<LexParseError<u16, TestLexerTypes>>,
            ),
        >,
    ) {
        do_parse_with_costs(rcvry_kind, lexs, grms, input, &HashMap::new())
    }

    #[allow(deprecated)]
    fn do_parse_with_costs<'input>(
        rcvry_kind: RecoveryKind,
        lexs: &str,
        grms: &str,
        input: &'input str,
        costs: &HashMap<&str, u8>,
    ) -> (
        YaccGrammar<u16>,
        SmallLexer<'input>,
        Result<
            Node<TestLexeme, u16>,
            (
                Option<Node<TestLexeme, u16>>,
                Vec<LexParseError<u16, TestLexerTypes>>,
            ),
        >,
    ) {
        let grm = YaccGrammar::<u16>::new_with_storaget(
            YaccKind::Original(YaccOriginalActionKind::GenericParseTree),
            grms,
        )
        .unwrap();
        let (_, stable) = from_yacc(&grm, Minimiser::Pager).unwrap();
        let rule_ids = grm
            .tokens_map()
            .iter()
            .map(|(&n, &i)| (n.to_owned(), u32::from(i).to_u16().unwrap()))
            .collect();
        let lexer_rules = small_lexer(lexs, rule_ids);
        let lexemes = small_lex(lexer_rules, input);
        let lexer = SmallLexer { lexemes, s: input };
        let costs_tidx = costs
            .iter()
            .map(|(k, v)| (grm.token_idx(k).unwrap(), v))
            .collect::<HashMap<_, _>>();
        let (r, errs) = RTParserBuilder::new(&grm, &stable)
            .recoverer(rcvry_kind)
            .term_costs(&|tidx| **costs_tidx.get(&tidx).unwrap_or(&&1))
            .parse_generictree(&lexer);
        if let Some(node) = r {
            if errs.is_empty() {
                (grm, lexer, Ok(node))
            } else {
                (grm, lexer, Err((Some(node), errs)))
            }
        } else {
            (grm, lexer, Err((None, errs)))
        }
    }

    fn check_parse_output(lexs: &str, grms: &str, input: &str, expected: &str) {
        let (grm, _, pt) = do_parse(RecoveryKind::CPCTPlus, lexs, grms, input);
        assert_eq!(expected, pt.unwrap().pp(&grm, input));
    }

    // SmallLexer is our highly simplified version of lrlex (allowing us to avoid having to have
    // lrlex as a dependency of lrpar). The format is the same as lrlex *except*:
    //   * The initial "%%" isn't needed, and only "'" is valid as a rule name delimiter.
    //   * "Unnamed" rules aren't allowed (e.g. you can't have a rule which discards whitespaces).
    pub(crate) struct SmallLexer<'input> {
        lexemes: Vec<TestLexeme>,
        s: &'input str,
    }

    impl Lexer<TestLexerTypes> for SmallLexer<'_> {
        fn iter<'a>(&'a self) -> Box<dyn Iterator<Item = Result<TestLexeme, TestLexError>> + 'a> {
            Box::new(self.lexemes.iter().map(|x| Ok(*x)))
        }
    }

    impl<'input> NonStreamingLexer<'input, TestLexerTypes> for SmallLexer<'input> {
        fn span_str(&self, span: Span) -> &'input str {
            &self.s[span.start()..span.end()]
        }

        fn span_lines_str(&self, _: Span) -> &'input str {
            unreachable!();
        }

        fn line_col(&self, _: Span) -> ((usize, usize), (usize, usize)) {
            unreachable!();
        }
    }

    fn small_lexer(lexs: &str, ids_map: HashMap<String, u16>) -> Vec<(u16, Regex)> {
        let mut rules = Vec::new();
        for l in lexs.split('\n').map(|x| x.trim()).filter(|x| !x.is_empty()) {
            assert!(l.rfind('\'') == Some(l.len() - 1));
            let i = l[..l.len() - 1].rfind('\'').unwrap();
            let name = &l[i + 1..l.len() - 1];
            let re = &l[..i - 1].trim();
            rules.push((
                ids_map[name],
                Regex::new(&format!("\\A(?:{})", re)).unwrap(),
            ));
        }
        rules
    }

    fn small_lex(rules: Vec<(u16, Regex)>, input: &str) -> Vec<TestLexeme> {
        let mut lexemes = vec![];
        let mut i = 0;
        while i < input.len() {
            let mut longest = 0; // Length of the longest match
            let mut longest_tok_id = 0; // This is only valid iff longest != 0
            for (tok_id, r) in rules.iter() {
                if let Some(m) = r.find(&input[i..]) {
                    let len = m.end();
                    if len > longest {
                        longest = len;
                        longest_tok_id = *tok_id;
                    }
                }
            }
            assert!(longest > 0);
            lexemes.push(Lexeme::new(longest_tok_id, i, longest));
            i += longest;
        }
        lexemes
    }

    #[test]
    fn simple_parse() {
        // From p4 of https://www.cs.umd.edu/class/spring2014/cmsc430/lectures/lec07.pdf
        check_parse_output(
            "[a-zA-Z_] 'ID'
             \\+ '+'",
            "
%start E
%%
E: T '+' E
 | T;
T: 'ID';
",
            "a+b",
            "E
 T
  ID a
 + +
 E
  T
   ID b
",
        );
    }

    #[test]
    fn parse_empty_rules() {
        let lexs = "[a-zA-Z_] 'ID'";
        let grms = "%start S
%%
S: L;
L: 'ID'
 | ;
";
        check_parse_output(
            lexs, grms, "", "S
 L
",
        );

        check_parse_output(
            lexs,
            grms,
            "x",
            "S
 L
  ID x
",
        );
    }

    #[test]
    fn recursive_parse() {
        let lexs = "\\+ '+'
                    \\* '*'
                    [0-9]+ 'INT'";
        let grms = "%start Expr
%%
Expr : Expr '+' Term | Term;
Term : Term '*' Factor | Factor;
Factor : 'INT';";

        check_parse_output(
            lexs,
            grms,
            "2+3*4",
            "Expr
 Expr
  Term
   Factor
    INT 2
 + +
 Term
  Term
   Factor
    INT 3
  * *
  Factor
   INT 4
",
        );
        check_parse_output(
            lexs,
            grms,
            "2*3+4",
            "Expr
 Expr
  Term
   Term
    Factor
     INT 2
   * *
   Factor
    INT 3
 + +
 Term
  Factor
   INT 4
",
        );
    }

    #[test]
    fn parse_error() {
        let lexs = "\\( '('
                    \\) ')'
                    [a-zA-Z_][a-zA-Z_0-9]* 'ID'";
        let grms = "%start Call
%%
Call: 'ID' '(' ')';";

        check_parse_output(
            lexs,
            grms,
            "f()",
            "Call
 ID f
 ( (
 ) )
",
        );

        let (grm, _, pr) = do_parse(RecoveryKind::CPCTPlus, lexs, grms, "f(");
        let (_, errs) = pr.unwrap_err();
        assert_eq!(errs.len(), 1);
        let err_tok_id = usize::from(grm.eof_token_idx()).to_u16().unwrap();
        match &errs[0] {
            LexParseError::ParseError(e) => {
                assert_eq!(e.lexeme(), &Lexeme::new_faulty(err_tok_id, 2, 0));
                assert!(e.lexeme().faulty());
            }
            _ => unreachable!(),
        }

        let (grm, _, pr) = do_parse(RecoveryKind::CPCTPlus, lexs, grms, "f(f(");
        let (_, errs) = pr.unwrap_err();
        assert_eq!(errs.len(), 1);
        let err_tok_id = usize::from(grm.token_idx("ID").unwrap()).to_u16().unwrap();
        match &errs[0] {
            LexParseError::ParseError(e) => {
                assert_eq!(e.lexeme(), &Lexeme::new(err_tok_id, 2, 1));
                assert!(!e.lexeme().faulty());
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn test_parse_map() {
        #[derive(PartialEq, Debug)]
        enum TestParseMap<'a> {
            Term(&'a str, &'a str),
            NonTerm(&'a str, Vec<TestParseMap<'a>>),
        }
        let lex_src = r#"[0-9]+ 'INT'
\+ '+'
\* '*'
"#;
        let grammar_src = "
%grmtools{YaccKind: Original(NoAction)}
%start Expr
%%
Expr : Expr '+' Term | Term;
Term : Term '*' Factor | Factor;
Factor : 'INT';";
        let input_src = "2*3+4";
        let grm = str::parse::<YaccGrammar<u16>>(grammar_src).unwrap();
        let (_, stable) = lrtable::from_yacc(&grm, lrtable::Minimiser::Pager).unwrap();
        let rt_parser = RTParserBuilder::new(&grm, &stable);
        let rule_ids = grm
            .tokens_map()
            .iter()
            .map(|(&n, &i)| (n.to_owned(), u32::from(i).to_u16().unwrap()))
            .collect();
        let lexer_rules = small_lexer(lex_src, rule_ids);
        let lexemes = small_lex(lexer_rules, input_src);
        let lexer = SmallLexer {
            lexemes,
            s: input_src,
        };

        let (parse_map, errs) = rt_parser.parse_map(
            &lexer,
            &|lexeme: TestLexeme| {
                let tidx = TIdx(lexeme.tok_id());
                let tn = &grm.token_name(tidx).unwrap();
                let lt = &input_src[lexeme.span().start()..lexeme.span().end()];
                TestParseMap::Term(tn, lt)
            },
            &|ridx, nodes| {
                let rule_name = &grm.rule_name_str(ridx);
                TestParseMap::NonTerm(rule_name, nodes)
            },
        );
        assert!(parse_map.is_some() && errs.is_empty());
        // Sanity check the `parse_generictree` output.
        check_parse_output(
            lex_src,
            grammar_src,
            input_src,
            "Expr
 Expr
  Term
   Term
    Factor
     INT 2
   * *
   Factor
    INT 3
 + +
 Term
  Factor
   INT 4
",
        );

        let expected_parse_map = {
            use TestParseMap::*;
            NonTerm(
                "Expr",
                vec![
                    NonTerm(
                        "Expr",
                        vec![NonTerm(
                            "Term",
                            vec![
                                NonTerm("Term", vec![NonTerm("Factor", vec![Term("INT", "2")])]),
                                Term("*", "*"),
                                NonTerm("Factor", vec![Term("INT", "3")]),
                            ],
                        )],
                    ),
                    Term("+", "+"),
                    NonTerm("Term", vec![NonTerm("Factor", vec![Term("INT", "4")])]),
                ],
            )
        };
        assert_eq!(parse_map, Some(expected_parse_map));
    }
}
