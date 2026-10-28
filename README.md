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
cargo run -- --delimiter-localization --recovery-stats "({[1+2"
cargo run -- --delimiter-localization --delimiter-dominance --recovery-stats "({[1+2"
cargo run --release -- --benchmark --benchmark-trials 15 --benchmark-warmups 3 > cpct-raw.tsv
cargo run --release -- --benchmark-summary-from cpct-raw.tsv > cpct-summary.tsv
```

The program prints whether parsing succeeded, recovered through baseline CPCT+,
reported CPCT+ repair sequences without a semantic value, or failed, along with
the reported parser errors and repair sequences exposed by `lrpar`.

With `--observe-delimiters`, the program still runs the same baseline parser and
also prints the independent delimiter observation report.

With `--recovery-stats`, the selected CPCT+ mode reports per-recovery search
counters, repair-sequence counts, minimum recovery cost, timeout status, and
elapsed time. The optional `--delimiter-dominance` flag enables strict-cost
dominance, while `--delimiter-localization` independently enables
delimiter-origin frontier prioritization. The Rust API exposes both
OFF-by-default controls through `RecoveryOptions`.

### Experimental benchmark

Use `--benchmark` for machine-readable raw TSV or `--benchmark-summary` for
per-case and aggregate statistics. Both accept `--benchmark-trials N` and
`--benchmark-warmups N`; defaults are 5 measured trials and 1 warm-up, and at
least one measured trial is required. Run both commands from a release build as
shown above. The raw output records the build profile so a debug run is
identifiable, but debug timings should not be used as performance evidence.

Each measured trial is paired by input across the four modes. The fixed
pseudo-random schedule deterministically shuffles mode order per case and trial
to reduce systematic order/cache bias; warm-ups also execute all four modes.
The corpus and ordering are reproducible, while runtime measurements and
selection among equal-ranked repair ties are not. The summary command can
summarize the exact raw run with `--benchmark-summary-from`; this is
preferred so raw and aggregate values correspond to the same trials.
Configuration counts are deterministic for a fixed build/input unless a
recovery timeout makes a search incomplete.

The 30-case corpus includes 10 valid controls (all delimiter types, mixed
nests, depths 8/16/32, and expressions with 8/24 terms); 5 missing-close cases
(each delimiter kind plus nesting depths 4/6); 3 unexpected closers; 3
mismatched closers; nested/crossed delimiters; multiple unmatched openings;
a delimiter-heavy cascading syntax error; 4 ordinary syntax errors; and an
input with multiple equal-cost repairs. Corpus validation confirms lexically
valid inputs and verifies that malformed cases invoke parser recovery while
valid controls do not. Lexer failures are rejected rather than counted as
recovery experiments.

Raw rows contain input bytes and lexer token count excluding EOF, delimiter
character count and scanner maximum depth, parse outcome/value availability,
target OS/architecture, total parser-call and summed CPCT+ elapsed time, search
counters, minimum repair cost and post-simplification repair counts, timeout
state, and localization
target/origin/recovery/repair positions. Empty recovery fields mean CPCT+ was
not invoked. CPCT+ elapsed time and counters come from `RecoveryStats`; total
runtime wraps the parse-and-statistics API and includes lexer/parser setup.

Counter meanings follow `RecoveryStats`: generated counts neighbor candidates
before merging/rejection; popped counts frontier entries checked for success;
merged counts frontier-key collisions invoking the merge callback; discarded
is the sum of reason counters and is not a unique-configuration count. Duplicate
discard counts repair histories. Dominance removals count repair variants, not
unique configurations. The summary reports per-case means of work counters,
runtime median/mean/population-standard-deviation/minimum/maximum across trials,
and an `__AGGREGATE__` row summing work and total runtime by paired trial across
the corpus. Reduction is `(OFF/OFF - mode) / OFF/OFF * 100`; runtime change is
`(mode - OFF/OFF) / OFF/OFF * 100`. `search_work_effect` distinguishes less,
more, unchanged, and mixed generated/popped work; `runtime_effect` compares
median total runtime. These classifications are descriptive, not statistical
significance tests. Report search-count and runtime results separately; fewer
configurations do not imply faster runtime.

Searches that time out are marked incomplete. Their repair sets and costs must
not be treated as exhaustive equivalence evidence; runtime summaries still
include them and should be interpreted alongside timeout counts. The OFF/OFF
mode is the instrumented vendored CPCT+ implementation with delimiter-aware
features disabled, not a pristine untouched upstream lrpar baseline. A
separate pristine-upstream comparison is not part of this harness.

### Phase 6 extended corpus

Phase 6 is a separate, opt-in extension; it does not change the Phase 4
30-case corpus, defaults, case names, or TSV schemas. Run it explicitly:

```powershell
cargo run --release -- --benchmark-phase6
cargo run --release -- --benchmark-phase6 --benchmark-trials 15 --benchmark-warmups 3
cargo run --release -- --benchmark-phase6-summary
cargo run --release -- --benchmark-phase6-summary-from results\cpct-phase6-raw.tsv
```

The Phase 6 benchmark defaults to 15 measured trials and 3 warm-ups. The
existing `--benchmark-trials N` and `--benchmark-warmups N` options override
those defaults for this workflow; Phase 4 keeps its 5-trial/1-warm-up defaults.
One Phase 6 run writes `results/cpct-phase6-raw.tsv` and
`results/cpct-phase6-summary.tsv`. The summary-only options read a Phase 6 raw
TSV and write only the summary file. These paths are separate from Phase 4
outputs. The benchmark is never started implicitly.

The 23 Phase 6 inputs are generated deterministically and validated before
measurement in all four modes. Coverage includes:

- Valid nesting at depths 64, 128, and 256; missing closing parentheses at
  depths 8, 16, 32, and 64.
- Valid expressions and missing-close expressions at 48, 96, and 192 terms.
- Three mismatches at different nesting levels/positions; three crossed or
  mixed delimiter patterns; two multi-type unmatched-opening cases.
- Two long ordinary syntax errors without delimiter imbalance.

Every raw Phase 6 row adds `differential_comparison` to the Phase 4 raw
instrumentation columns. Completed paired searches are checked against OFF/OFF
for equal minimum repair costs and equal complete normalized minimum-cost
repair sets. If any mode in the paired trial times out, all four rows are
marked `incomplete_timeout`; the summary reports those trials separately and
does not claim repair-set equivalence for them. The 500 ms non-test CPCT+
recovery budget is unchanged. Timeout cases are retained as incomplete
observations rather than removed or treated as completed results.

This larger corpus is intended to expose scaling behavior, not to establish
runtime improvement by construction. Report search-work counters separately
from runtime distributions. A lower generated count is not evidence of lower
runtime, and these experiments do not expose peak memory.

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
delimiter opening origins with CPCT+ repair histories.

Example checkpoint output:

```text
Checkpoint #0: delimiter=(, byte=0, char=0, delimiter_index=0, depth=1, matched=false, closing=none, parser_state=not captured
Checkpoint #1: delimiter=(, byte=1, char=1, delimiter_index=1, depth=2, matched=true, closing=byte=7, char=7, parser_state=not captured
```

## Baseline vs Observation

The parser offers these controls:

- baseline CPCT+: `parse_baseline`
- baseline CPCT+ plus delimiter observation:
  `parse_baseline_with_delimiter_observation`
- opt-in strict-cost dominance:
  `parse_baseline_with_options` with `RecoveryOptions`
- opt-in delimiter-origin localization: `RecoveryOptions` with
  `delimiter_localization: true`

Dominance and localization are independent options. With both disabled, the
default repair semantics remain CPCT+; instrumentation and delimiter metadata
are still present in vendored `lrpar`.

## Delimiter-Origin Localization

Localization is an optional exploration policy, separate from dominance. At
each parser error it selects a relevant real delimiter origin when available:
an active opening delimiter for an unmatched or mismatched case, or the current
unexpected closing token. For unmatched openings, the implementation checks the
remaining token stream so a balanced enclosing delimiter around an unrelated
syntax error is not mistaken for an unmatched origin.

CPCT+ still starts from the parser's actual error configuration (the live LR
state stack, lookahead index, and semantic stack). It does **not** rewind the
input or restart at the opening character: the project-level checkpoint scanner
does not capture the LR and semantic-stack state required for a safe resume.
Instead, localization prioritizes already valid CPCT+ frontier configurations
that structurally resolve the selected target, but only within the same
accumulated CPCT+ cost bucket. It does not remove configurations, change costs,
or alter continuation ranking and final repair ranking. Thus CPCT+ minimum-cost
semantics and the complete repair set returned by CPCT+'s existing continuation
filter are retained when the search finishes without timeout. The final choice
among equal-ranked repairs is not guaranteed to be identical: the baseline
already leaves ties unordered, and frontier traversal can change which tied
candidate is selected. Differential tests compare the complete reported repair
sets and minimum costs for all four dominance/localization modes on a fixed
corpus of all three delimiter kinds, nested/mixed and mismatched cases, multiple
unmatched openings, and ordinary syntax errors. An earlier timeout can change
which configurations are explored before termination; timed-out runs are
incomplete and are not evidence of repair-set equivalence. The recovery start
remains the parser-error position.

When recovery statistics are requested, the report includes the selected
target/checkpoint, its delimiter and origin position/depth, the actual recovery
start, the selected repair's final operation position, origin-to-repair
distance, origin-to-recovery-start distance, start-to-final-repair distance,
and counts of prioritized/reordered frontier pops. `--benchmark` records those
fields for each of the four dominance/localization combinations.
These metrics demonstrate exploration influence, not an earlier parser restart
or a measured performance improvement.

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
incremental cost. The accumulated cost is maintained separately by the
CPCT+ cost bucket and is not part of the key; search transitions add the
existing non-negative token costs. A strictly more expensive variant cannot
contribute a minimum-cost repair; an equal-cost history can, and is never
removed. This guarantee relies on future CPCT+ transitions depending on the LR
stack, input index, the two represented continuation controls, the fixed parser
configuration, and the complete delimiter state—not semantic values or repair
history beyond those controls. Tests compare costs and complete repair sets
returned by the existing continuation filter across all four modes when no
search times out. Selection among equal-ranked repairs may vary because tie
ordering is unspecified; timeout results are partial and not covered by the
exhaustive-equivalence claim.

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
