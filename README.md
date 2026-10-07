# DelimiterCPCT

This repository currently contains the baseline parser for the research project
"Delimiter-Aware Extension of CPCT+ for Efficient and Localized LR Parser Error
Recovery".

The baseline is intentionally small and unmodified. It uses `lrlex` and `lrpar`
to parse integer expressions with addition, multiplication, and parentheses:

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
```

The program prints whether parsing succeeded, recovered through baseline CPCT+,
reported CPCT+ repair sequences without a semantic value, or failed, along with
the reported parser errors and repair sequences exposed by `lrpar`.

With `--observe-delimiters`, the program still runs the same baseline parser and
also prints the independent delimiter observation report.

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

Parser state is not fabricated. This phase does not modify `lrpar` or CPCT+, so
no internal parser state is recorded yet.

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

## API Limitations Noted

This baseline exposes the parser result and the pretty-printed `lrpar` parser
errors, including repair sequences. When CPCT+ inserts a token such as `INT`,
the semantic action may be unable to compute a numeric value because the
inserted token has no source lexeme. It does not expose internal CPCT+ search
configuration counts, memory usage, or recovery runtime metrics through this
entry point. Future benchmarking must only report metrics that are actually
measured or exposed by the selected APIs.
