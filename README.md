# DelimiterCPCT

This repository currently contains the baseline parser for the research project
"Delimiter-Aware Extension of CPCT+ for Efficient and Localized LR Parser Error
Recovery".

The baseline uses the original `lrpar` 0.15.0 CPCT+ algorithm to parse integer
expressions with addition, multiplication, and parentheses. The local `lrpar`
source copy adds read-only search instrumentation and per-configuration
delimiter/history metadata; neither influences CPCT+ decisions or repair
results.

- integer literals
- `+`
- `*`
- `(` and `)`

The baseline path remains unmodified. A separate delimiter-observation path now
records structural delimiter information alongside baseline parsing, but it does
not influence CPCT+ recovery.

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
```

The program prints whether parsing succeeded, recovered through baseline CPCT+,
reported CPCT+ repair sequences without a semantic value, or failed, along with
the reported parser errors and repair sequences exposed by `lrpar`.

With `--observe-delimiters`, the program still runs the same baseline parser and
also prints the independent delimiter observation report.

With `--recovery-stats`, the same CPCT+ implementation reports per-recovery
search counters, repair-sequence counts, minimum recovery cost, timeout status,
and elapsed time. This mode only observes the search; it does not change
candidate generation, merging, costs, continuation ranking, or repair selection.

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

The tracker intentionally does not depend on the expression grammar. The current
grammar only accepts parentheses, but the observation layer can still see braces
and brackets in raw input without modifying `src/expr.y`.

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

The second path only adds structural observation. It does not modify repair
costs, prune configurations, rank repairs differently, add delimiter-specific
grammar productions, or replace CPCT+.

The vendored CPCT+ search also carries experimental delimiter metadata. Each
repair-history variant remains paired with the delimiter state produced by
that history, including opening kind, token/input index, character index, byte
offset, nesting depth, and whether the opening was real or inserted. Compatible
CPCT+ configurations still use the existing frontier grouping and
continuation-control compatibility; metadata is kept paired when their repair
histories are merged. This representation adds per-history storage and does
not prune, reorder, change costs, or alter repair selection.

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
higher-cost nodes after minimum-cost discovery, and identical histories ignored
during merging. Duplicate-history events are therefore included in both the
merged counter and the discarded-reason total; the discard total itself is the
sum of disjoint discard reasons.
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
frontier ordering, pruning, merging, continuation filtering, or repair
selection. They use only the grammar-neutral delimiter stack and current
lookahead, not grammar productions or the project-level checkpoint scanner.

## API Limitations Noted

This baseline exposes the parser result and the pretty-printed `lrpar` parser
errors, including repair sequences. When CPCT+ inserts a token such as `INT`,
the semantic action may be unable to compute a numeric value because the
inserted token has no source lexeme. CPCT+ now exposes internal search counts
and recovery timing, but does not provide peak-memory measurements. Future
benchmarking must only report metrics that are actually measured or exposed by
the selected APIs.
