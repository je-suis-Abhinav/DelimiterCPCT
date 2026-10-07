# Delimiter-Aware CPCT+ Compiler Design Project

## Project Purpose

This repository implements the Compiler Design Lab research project:

"Delimiter-Aware Extension of CPCT+ for Efficient and Localized LR Parser Error Recovery"

The project extends an existing CPCT+-based LR parser with lightweight delimiter-awareness.

The implementation must remain grammar-neutral and must preserve the existing CPCT+ recovery machinery as much as possible.

The research objectives are:

1. Reduce the CPCT+ search space for delimiter-imbalance errors using delimiter-depth information.
2. Use delimiter-origin checkpoints to identify structurally plausible repair locations earlier than the eventual LR parser failure point.
3. Evaluate whether delimiter-aware recovery reduces unnecessary cascading syntax errors compared with unmodified CPCT+.
4. Measure runtime and memory overhead introduced by delimiter tracking during error-free parsing.

## Important Research Constraint

Do NOT replace CPCT+ with a completely different error-recovery algorithm.

Do NOT implement a generic parser from scratch unless required by the chosen framework.

The baseline must remain identifiable as the original/unmodified CPCT+ implementation.

The modified implementation must add delimiter-aware functionality around or within the CPCT+ recovery process.

## Technology

Current planned technology:

- Rust
- lrpar
- lrlex
- CPCT+ recovery provided by lrpar
- Cargo
- Windows/MSVC

Verify actual dependency APIs against the installed crate versions instead of assuming APIs from older versions.

## Development Strategy

Develop incrementally.

Phase 1:
- Establish a working baseline LR parser using lrpar/lrlex.
- Confirm CPCT+ recovery works on malformed input.
- Record baseline behaviour.

Phase 2:
- Implement lightweight delimiter tracking.
- Initially support:
  - ()
  - {}
  - []

Phase 3:
- Implement delimiter-origin checkpoints.
- Each opening delimiter should be associated with enough information to identify:
  - delimiter type
  - token/input position
  - nesting depth
  - relevant parser position/state information if available

Phase 4:
- Integrate delimiter-origin checkpoints with recovery candidate generation/localization.

Phase 5:
- Implement targeted search-space prioritization for delimiter-related recovery.

Phase 6:
- Build a controlled malformed-input corpus.

Phase 7:
- Benchmark baseline CPCT+ versus delimiter-aware CPCT+.

## Delimiter Tracking Requirements

Delimiter tracking must be lightweight.

Track nesting information during ordinary LR parsing.

The tracker should distinguish:

- matched delimiters
- unmatched opening delimiters
- unexpected/mismatched closing delimiters
- nesting depth
- origin position of unmatched delimiters

Do not silently discard malformed delimiter information.

## Checkpoint Requirements

A delimiter-origin checkpoint represents the structural origin of an opening delimiter.

Checkpoints should allow the system to answer:

"If the parser fails much later, which unmatched delimiter originated earlier and at what position/depth?"

The checkpoint mechanism must not require grammar-specific annotations.

## Grammar Neutrality

Do not add grammar-specific delimiter recovery productions.

Do not hard-code recovery rules for one particular grammar.

Delimiter awareness should be an independent structural layer.

## Baseline vs Modified Implementation

Maintain a clear distinction between:

1. Baseline CPCT+
2. Delimiter-aware CPCT+

The two implementations must be runnable on the same inputs.

This is required for experimental comparison.

## Metrics

Eventually collect at least:

- input size
- delimiter count
- maximum delimiter depth
- parser success/failure
- recovery runtime
- peak memory if practical
- number of recovery configurations explored, if accessible
- number of repair candidates
- repair cost
- reported recovery position
- delimiter-origin position
- distance between parser failure and delimiter origin
- number of cascading syntax errors

Do not fabricate metrics that the parser framework cannot actually expose.

If a metric cannot be measured directly, document that limitation.

## Testing

Create deterministic tests for:

### Valid input
- balanced ()
- balanced {}
- balanced []
- deeply nested delimiters
- mixed nested delimiters

### Missing delimiter
- missing )
- missing }
- missing ]

### Unexpected delimiter
- unexpected )
- unexpected }
- unexpected ]

### Mismatched delimiter
- ([)]
- {)]
- [}

### Multiple delimiter errors
- multiple missing delimiters
- nested missing delimiters
- missing delimiter followed by otherwise valid code

### Localization cases
Construct cases where:
- delimiter error occurs early
- parser failure occurs much later
- delimiter-origin checkpoint should identify the earlier structural location

## Experimental Principle

Do not claim that the new algorithm is better merely because it produces a plausible repair.

The final evaluation must compare baseline and modified CPCT+ on the same controlled corpus.

## Code Quality

Prefer:
- small modules
- deterministic behaviour
- explicit data structures
- clear names
- unit tests
- reproducible experiments

Avoid:
- unnecessary dependencies
- machine-learning components
- grammar-specific hacks
- arbitrary heuristics without an experimental justification
- silently changing baseline CPCT+ behaviour

## Research Integrity

Never fabricate experimental results.

Never report runtime, memory, configuration counts, repair counts, or accuracy values unless they were actually measured.

If an implementation detail is uncertain, inspect the installed dependency/source/API before coding.

When changing the architecture, explain why the change is consistent with the research objectives.

## Current Priority

The immediate priority is NOT the final delimiter-aware system.

First establish a working baseline CPCT+ parser.

Then add one research component at a time and test after each change.