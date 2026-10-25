# DelimiterCPCT

This repository currently contains the baseline parser for the research project
"Delimiter-Aware Extension of CPCT+ for Efficient and Localized LR Parser Error
Recovery".

With delimiter dominance disabled, the default parser preserves the original
`lrpar` 0.15.0 CPCT+ repair semantics for integer expressions with addition,
multiplication, and `()`, `{}`, and `[]` grouping. The vendored `lrpar` source
still contains search instrumentation and per-configuration delimiter/history
metadata. An explicit experimental option enables strict-cost
delimiter-aware dominance.

- integer literals
- `+`
- `*`
- `(`, `)`, `{`, `}`, `[`, and `]` as equivalent grouping delimiters

With dominance disabled, delimiter metadata and instrumentation do not alter
CPCT+ repair behavior. A separate delimiter-observation path records structural
delimiter information alongside parsing, without influencing CPCT+ recovery.

## Versions

- Rust edition: 2024
- `cfgrammar`: 0.15.0
- `lrlex`: 0.15.0
- `lrpar`: 0.15.0

The selected grmtools crates are pinned to the same stable release so that the
lexer, parser generator, and shared grammar/table crates stay mutually
compatible. `lrpar`'s default recovery mode is CPCT+.

## Generated Parser

`build.rs` uses `lrlex::CTLexerBuilder` to compile:

- `src/expr.l`
- `src/expr.y`

The generated modules are included with:

- `lrlex_mod!("expr.l")`
- `lrpar_mod!("expr.y")`

## Running

```powershell
cargo run -- "1+2*3"
cargo run -- "1+*"
cargo run -- --observe-delimiters "((1+2)*3"
cargo run -- --recovery-stats "1+*"
cargo run -- --delimiter-dominance --recovery-stats "1+*"
cargo run --release -- --benchmark > cpct-benchmark.tsv
cargo run --release -- --benchmark-summary
```

The program prints whether parsing succeeded, recovered through baseline CPCT+,
reported CPCT+ repair sequences without a semantic value, or failed, along with
the reported parser errors and repair sequences exposed by `lrpar`.

With `--observe-delimiters`, the program still runs the same baseline parser and
also prints the independent delimiter observation report.

With `--recovery-stats`, the selected CPCT+ mode reports per-recovery search
counters, repair-sequence counts, minimum recovery cost, timeout status, and
elapsed time. By default, this mode only observes the search. The optional
`--delimiter-dominance` flag enables the experimental rule documented below.
The Rust API exposes the same OFF-by-default control with `RecoveryOptions`.

`--benchmark` runs a fixed ten-input corpus once with dominance OFF and once
with dominance ON per input, then writes TSV to standard output. The corpus
includes valid controls; missing/unexpected delimiters; mismatched, nested, and
multiple unmatched delimiters; operator errors; and delimiter-heavy errors.
`parse_runtime_ns` measures the complete parse-and-statistics call;
`cpct_elapsed_ns` and search counters come directly from `RecoveryStats`.
Per-recovery costs and post-simplification repair counts are semicolon-separated
when one input triggers multiple recovery attempts. Empty fields mean the parser
did not expose a recovery value for that input (for example, a valid control
does not invoke CPCT+). Localization data is produced independently by the
existing delimiter scanner/localizer. All six grouping-delimiter characters
are lexer tokens, so malformed grouping cases in the corpus reach parser-level
CPCT+ recovery; inputs containing other unsupported characters may still fail
lexing. Runtime fields are measured values,
not deterministic test fixtures; use the same build profile and machine when
comparing runs.

`--benchmark-summary` runs the same corpus/modes and prints a compact TSV
comparison. Reduction percentages are `(OFF - ON) / OFF * 100`; runtime change
is `(ON - OFF) / OFF * 100` (positive means ON took longer). Exact OFF/ON
generated, popped, runtime, cost, repair-count, and dominance-discard values
are included alongside the percentages. `N/A` marks absent measurements or a
zero OFF denominator, for which a percentage is undefined.

## Delimiter Observation

The delimiter tracker is a grammar-neutral structural scanner over the raw input
text. It supports:

- `(` and `)`
- `{` and `}`
- `[` and `]`

It records:

- current and maximum nesting depth
- matched delimiter pairs
- unmatched opening delimiters
- unexpected closing delimiters
- mismatched closing delimiters
- delimiter-origin checkpoints for every opening delimiter

The tracker intentionally does not depend on the expression grammar. The
expression grammar treats all three pairs as equivalent grouping delimiters,
while retaining the existing arithmetic precedence and associativity.

The parser remains an integer-expression grammar: it does not support
identifiers, unary operators, division, strings, or comments. The structural
scanner observes delimiter-looking characters in raw text and does not
distinguish characters inside strings or comments; those constructs are outside
this grammar.

## Checkpoints

Each opening delimiter creates a `DelimiterCheckpoint` containing:

- checkpoint id
- delimiter type
- opening byte offset
- opening character index
- delimiter-token index among observed delimiters
- nesting depth
- matched/unmatched status
- optional closing position
- parser state, currently `None`

The separate checkpoint scanner does not capture LR parser state, so its
`parser_state` field remains `None`. The vendored parser separately carries
delimiter opening origins with CPCT+ repair histories; this metadata does not
alter parse or recovery decisions.

Example checkpoint output:

```text
Checkpoint #0: delimiter=(, byte=0, char=0, delimiter_index=0, depth=1, matched=false, closing=none, parser_state=not captured
Checkpoint #1: delimiter=(, byte=1, char=1, delimiter_index=1, depth=2, matched=true, closing=byte=7, char=7, parser_state=not captured
```

## Baseline vs Observation

There are now two execution paths:

- baseline CPCT+: `parse_baseline`
- baseline CPCT+ plus delimiter observation:
  `parse_baseline_with_delimiter_observation`
- opt-in strict-cost dominance:
  `parse_baseline_with_options` with `RecoveryOptions`

The second path only adds structural observation. It does not modify repair
costs, prune configurations, rank repairs differently, add delimiter-specific
grammar productions, or replace CPCT+.

The vendored CPCT+ search also carries experimental delimiter metadata. Each
repair-history variant remains paired with the delimiter state produced by
that history, including opening kind, token/input index, character index, byte
offset, nesting depth, and whether the opening was real or inserted. Compatible
CPCT+ configurations still use the existing frontier grouping and
continuation-control compatibility; metadata is kept paired when their repair
histories are merged. With dominance disabled, this representation does not
prune, reorder, change costs, or alter repair selection. The opt-in strict-cost
mode is described separately below.

## Testing

```powershell
cargo test
```

The deterministic baseline tests cover these valid inputs:

- `1+2`
- `1+2*3`
- `(1+2)*3`
- `((1+2)*3)`

Malformed inputs used to verify CPCT+ recovery:

- `1+`
- `1+*`
- `(1+2`
- `1+2)`

Delimiter tracker tests cover balanced delimiters, unmatched openings,
unexpected closings, mismatches, deeply nested delimiters, multiple independent
pairs, multiple unmatched openings, deterministic checkpoint positions, and
parser integration with observation enabled.
- `((1+2)`
- `1+2)`

## CPCT+ Recovery Statistics

The local `lrpar` 0.15.0 source copy adds `RecoveryStats` and
`ParseError::recovery_stats()`. The project-level additive API is
`parse_baseline_with_recovery_stats`, which returns the unchanged
`BaselineReport` together with one statistics record for each parser error that
invoked CPCT+ recovery. Accepted inputs have no recovery records.

Counters are collected inside CPCT+ and its cost-bucket search. “Generated”
counts neighbor candidates offered to the frontier (not the initial node);
“popped” counts unique frontier entries removed and checked for success.
“Merged” counts frontier key collisions (including exact duplicate histories).
Discard counters distinguish same-cost-closure cost rejects, deadline leftovers,
higher-cost nodes after minimum-cost discovery, identical histories ignored
during merging, and strict-cost dominance removals. Duplicate-history events
are therefore included in both the
merged counter and the discarded-reason total. The aggregate discard counter
is a sum of discard events, not a count of unique frontier nodes: dominance and
duplicate reasons count repair variants/histories, while other reasons count
candidate or queued configurations.
Transition-attempt counters count each non-EOF insertion terminal considered,
each delete transition call (including calls at EOF), and each shift/reduction
transition call. Repair-sequence counts are measured after collection, after
continuation filtering, and after simplification/deduplication. These counts are
`None` when recovery does not reach the corresponding stage, such as when the
search finds no success node or times out during collection.

Elapsed time is the wall-clock time within a single recovery attempt. Timeout
status is set when the recovery algorithm explicitly stops because its
deadline has been reached. These measurements include instrumentation overhead;
they are descriptive search metrics, not evidence of a performance improvement.

The stats API and `--recovery-stats` CLI also report delimiter scores. The
score is computed observationally for each popped configuration from its
carried delimiter-state alternatives and a context snapshot at the parser-error
lookahead. Higher scores indicate structural progress:

- `+3` for each opening delimiter present at the error that is no longer
  outstanding, and `-3` for each newly outstanding opening;
- `+2` for each newly matched pair;
- `-4` for each newly unexpected or mismatched closing delimiter;
- `+2`, `+3`, or `+4` when the parser-error closing delimiter has a matching
  opening on top of the stack, for matching, mismatched, or unexpected-close
  contexts respectively.

For merged configurations, the reported configuration score is the maximum
score among its carried delimiter states. `delimiter_score_distribution` is a
histogram over popped configurations; `minimum_cost_success_delimiter_scores`
records the scores of successful configurations at the minimum CPCT+ cost; and
`selected_repair_delimiter_score` scores the repair selected by the existing
continuation filtering and simplification. Scores do not participate in
frontier ordering, dominance, merging, continuation filtering, or repair
selection. They use only the grammar-neutral delimiter stack and current
lookahead, not grammar productions or the project-level checkpoint scanner.

## Experimental Strict-Cost Delimiter Dominance

Dominance is OFF for existing parser entry points. Enable it explicitly with
`--delimiter-dominance` (optionally combined with `--recovery-stats`) or with
`RecoveryOptions { delimiter_dominance: true }`. The generated parser also
exposes `parse_with_recovery_kind`, and vendored lrpar provides
`RecoveryKind::CPCTPlusDelimiterDominance`.

Variant A is discarded only when a previous variant has a **strictly lower
accumulated CPCT+ cost** and an exactly equal dominance key. The key is:

- the complete LR parser state stack;
- the current input/lexeme index;
- the CPCT+ continuation state: delete-ending status and trailing-shift count;
- the complete delimiter state, including delimiter kinds, opening origins,
  real-versus-inserted identity, depth, and delimiter event counters.

Equal-cost variants are retained, including distinct repair histories. Different
delimiter stacks or opening origins, parser stacks, input positions, or
continuation states do not match. The index only filters variants; it does not
alter transition costs, cost-bucket ordering, continuation ranking, repair
simplification, or repair selection. The delimiter score is never consulted.

The correctness rationale is that equal keys have identical future LR behavior,
the same legal CPCT+ continuation controls, and identical deterministic
delimiter-state updates. Therefore every continuation available to a
higher-cost variant is also available to the lower-cost variant at the same
incremental cost. A strictly more expensive variant cannot contribute a
minimum-cost repair; an equal-cost history can, and is never removed. Tests
compare minimum costs and complete reported minimum-cost repair sets with
dominance OFF and ON.

`RecoveryStats` adds `dominance_candidates_checked`,
`dominance_exact_key_matches`, `dominance_variants_discarded`, and
`dominance_discarded_cost_difference`; `configurations_discarded_dominance`
feeds the aggregate discarded count. Equal-cost key matches are counted but
not discarded. No performance improvement is claimed unless measured runs
show reduced generated or popped work.

## API Limitations Noted

This baseline exposes the parser result and the pretty-printed `lrpar` parser
errors, including repair sequences. When CPCT+ inserts a token such as `INT`,
the semantic action may be unable to compute a numeric value because the
inserted token has no source lexeme. CPCT+ now exposes internal search counts
and recovery timing, but does not provide peak-memory measurements. Future
benchmarking must only report metrics that are actually measured or exposed by
the selected APIs.
