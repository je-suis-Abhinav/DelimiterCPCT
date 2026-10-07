use lrlex::lrlex_mod;
use lrpar::{
    LexError, LexParseError, Lexeme, NonStreamingLexer, RecoveryKind, RecoveryStats, lrpar_mod,
};

pub mod benchmark;
pub mod delimiter;
pub mod localization;

lrlex_mod!("expr.l");
lrpar_mod!("expr.y");

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaselineStatus {
    Accepted,
    Recovered,
    RecoveryReported,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineReport {
    pub input: String,
    pub status: BaselineStatus,
    pub value: Option<u64>,
    pub errors: Vec<String>,
    pub error_positions: Vec<localization::ParserErrorPosition>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedBaselineReport {
    pub baseline: BaselineReport,
    pub delimiters: delimiter::DelimiterReport,
}

pub fn parse_baseline(input: &str) -> BaselineReport {
    parse_baseline_internal(input, false, RecoveryOptions::default()).0
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaselineRecoveryStatsReport {
    pub baseline: BaselineReport,
    /// One entry for each parser error that invoked CPCT+ recovery.
    pub recovery_stats: Vec<RecoveryStats>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecoveryOptions {
    /// Enable experimental strict-cost delimiter-aware dominance. Default is disabled.
    pub delimiter_dominance: bool,
}

pub fn parse_baseline_with_recovery_stats(input: &str) -> BaselineRecoveryStatsReport {
    parse_baseline_with_recovery_stats_and_options(input, RecoveryOptions::default())
}

pub fn parse_baseline_with_recovery_stats_and_options(
    input: &str,
    options: RecoveryOptions,
) -> BaselineRecoveryStatsReport {
    let (baseline, recovery_stats) = parse_baseline_internal(input, true, options);
    BaselineRecoveryStatsReport {
        baseline,
        recovery_stats,
    }
}

pub fn parse_baseline_with_options(input: &str, options: RecoveryOptions) -> BaselineReport {
    parse_baseline_internal(input, false, options).0
}

fn parse_baseline_internal(
    input: &str,
    collect_recovery_stats: bool,
    options: RecoveryOptions,
) -> (BaselineReport, Vec<RecoveryStats>) {
    let lexerdef = expr_l::lexerdef();
    let lexer = lexerdef.lexer(input);
    let (res, errs) = if options.delimiter_dominance {
        expr_y::parse_with_recovery_kind(&lexer, RecoveryKind::CPCTPlusDelimiterDominance)
    } else {
        expr_y::parse(&lexer)
    };
    let recovery_stats = if collect_recovery_stats {
        errs.iter()
            .filter_map(|error| match error {
                LexParseError::ParseError(parse_error) => parse_error.recovery_stats().cloned(),
                LexParseError::LexError(_) => None,
            })
            .collect()
    } else {
        Vec::new()
    };
    let error_positions = errs
        .iter()
        .map(|e| parser_error_position(input, &lexer, e))
        .collect::<Vec<_>>();
    let errors = errs
        .iter()
        .map(|e| e.pp(&lexer, &expr_y::token_epp))
        .collect::<Vec<_>>();

    let value = match res {
        Some(Ok(v)) => Some(v),
        _ => None,
    };

    let recovery_reported = errors.iter().any(|e| e.contains("Repair sequences found"));

    let status = match (value, errors.is_empty(), recovery_reported) {
        (Some(_), true, _) => BaselineStatus::Accepted,
        (Some(_), false, _) => BaselineStatus::Recovered,
        (None, _, true) => BaselineStatus::RecoveryReported,
        (None, _, false) => BaselineStatus::Failed,
    };

    (
        BaselineReport {
            input: input.to_string(),
            status,
            value,
            errors,
            error_positions,
        },
        recovery_stats,
    )
}

pub fn parse_baseline_with_delimiter_observation(input: &str) -> ObservedBaselineReport {
    ObservedBaselineReport {
        baseline: parse_baseline(input),
        delimiters: delimiter::observe_delimiters(input),
    }
}

fn parser_error_position(
    input: &str,
    lexer: &lrlex::LRNonStreamingLexer<'_, '_, lrlex::defaults::DefaultLexerTypes>,
    error: &LexParseError<u32, lrlex::defaults::DefaultLexerTypes>,
) -> localization::ParserErrorPosition {
    let (kind, span) = match error {
        LexParseError::LexError(e) => (localization::ParserErrorKind::Lexing, e.span()),
        LexParseError::ParseError(e) => (localization::ParserErrorKind::Parsing, e.lexeme().span()),
    };

    let byte_offset = span.start();
    let ((line, column), _) = lexer.line_col(span);

    localization::ParserErrorPosition {
        kind,
        byte_offset,
        char_index: input[..byte_offset].chars().count(),
        line,
        column,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BaselineStatus, RecoveryOptions, parse_baseline, parse_baseline_with_delimiter_observation,
        parse_baseline_with_options, parse_baseline_with_recovery_stats,
        parse_baseline_with_recovery_stats_and_options,
    };

    #[test]
    fn valid_expressions_parse_without_recovery() {
        let cases = [("1+2", 3), ("1+2*3", 7), ("(1+2)*3", 9), ("((1+2)*3)", 9)];

        for (input, expected) in cases {
            let report = parse_baseline(input);
            assert_eq!(report.status, BaselineStatus::Accepted, "{input}");
            assert_eq!(report.value, Some(expected), "{input}");
            assert!(report.errors.is_empty(), "{input}");
        }
    }

    #[test]
    fn malformed_expressions_invoke_baseline_cpct_recovery() {
        let cases = ["1+", "1+*", "(1+2", "1+2)", "((1+2)", "1+2)"];

        for input in cases {
            let report = parse_baseline(input);
            assert_ne!(report.status, BaselineStatus::Accepted, "{input}");
            assert!(
                report
                    .errors
                    .iter()
                    .any(|e| e.contains("Repair sequences found")),
                "{input}: {report:#?}"
            );
        }
    }

    #[test]
    fn recovery_stats_are_additive_and_preserve_baseline_results() {
        let cases = [
            (
                "1+*",
                "Parsing error at line 1 column 3. Repair sequences found:\n   1: Insert INT, Delete *\n   2: Insert INT, Shift *, Insert INT",
                2,
                2,
            ),
            (
                "(1+2",
                "Parsing error at line 1 column 5. Repair sequences found:\n   1: Insert )",
                1,
                1,
            ),
            (
                "1+2)",
                "Parsing error at line 1 column 4. Repair sequences found:\n   1: Delete )",
                1,
                1,
            ),
        ];

        for (input, expected_error, repair_count, expected_cost) in cases {
            let baseline = parse_baseline(input);
            let instrumented = parse_baseline_with_recovery_stats(input);
            assert_eq!(instrumented.baseline, baseline, "{input}");
            assert_eq!(baseline.errors, [expected_error], "{input}");
            assert_eq!(instrumented.recovery_stats.len(), 1, "{input}");

            let stats = &instrumented.recovery_stats[0];
            let repeated = parse_baseline_with_recovery_stats(input);
            let repeated_stats = &repeated.recovery_stats[0];
            assert!(stats.configurations_generated > 0, "{input}: {stats:#?}");
            assert!(stats.configurations_popped > 0, "{input}: {stats:#?}");
            assert_eq!(
                stats.delimiter_score_distribution.values().sum::<u64>(),
                stats.configurations_popped,
                "{input}: {stats:#?}"
            );
            assert!(
                !stats.minimum_cost_success_delimiter_scores.is_empty(),
                "{input}: {stats:#?}"
            );
            assert!(
                stats.selected_repair_delimiter_score.is_some(),
                "{input}: {stats:#?}"
            );
            assert_eq!(
                stats.delimiter_score_distribution, repeated_stats.delimiter_score_distribution,
                "{input}"
            );
            assert_eq!(
                stats.minimum_cost_success_delimiter_scores,
                repeated_stats.minimum_cost_success_delimiter_scores,
                "{input}"
            );
            assert_eq!(
                stats.selected_repair_delimiter_score,
                repeated_stats.selected_repair_delimiter_score,
                "{input}"
            );
            assert!(
                stats.insert_transitions_attempted > 0,
                "{input}: {stats:#?}"
            );
            assert!(
                stats.delete_transitions_attempted > 0,
                "{input}: {stats:#?}"
            );
            assert!(stats.shift_transitions_attempted > 0, "{input}: {stats:#?}");
            assert_eq!(
                stats.configurations_discarded,
                stats.configurations_discarded_cost
                    + stats.configurations_discarded_timeout
                    + stats.configurations_discarded_higher_cost
                    + stats.configurations_discarded_duplicate
                    + stats.configurations_discarded_dominance,
                "{input}: {stats:#?}"
            );
            assert!(!stats.delimiter_dominance_enabled);
            assert_eq!(
                stats.repair_sequences_after_simplification,
                Some(repair_count),
                "{input}: {stats:#?}"
            );
            assert_eq!(
                stats.minimum_recovery_cost,
                Some(expected_cost),
                "{input}: {stats:#?}"
            );
        }
    }

    #[test]
    fn strict_cost_dominance_option_preserves_baseline_repairs_and_costs() {
        for input in ["1+*", "(1+2", "1+2)"] {
            let baseline =
                parse_baseline_with_recovery_stats_and_options(input, RecoveryOptions::default());
            let dominant = parse_baseline_with_recovery_stats_and_options(
                input,
                RecoveryOptions {
                    delimiter_dominance: true,
                },
            );
            assert_eq!(
                dominant.baseline.errors, baseline.baseline.errors,
                "{input}"
            );
            assert_eq!(
                dominant.baseline.status, baseline.baseline.status,
                "{input}"
            );
            assert_eq!(dominant.recovery_stats.len(), 1, "{input}");
            let stats = &dominant.recovery_stats[0];
            let baseline_stats = &baseline.recovery_stats[0];
            assert!(stats.delimiter_dominance_enabled, "{input}: {stats:#?}");
            assert_eq!(
                stats.minimum_recovery_cost, baseline_stats.minimum_recovery_cost,
                "{input}"
            );
            assert_eq!(
                stats.configurations_discarded,
                stats.configurations_discarded_cost
                    + stats.configurations_discarded_timeout
                    + stats.configurations_discarded_higher_cost
                    + stats.configurations_discarded_duplicate
                    + stats.configurations_discarded_dominance,
                "{input}: {stats:#?}"
            );
        }

        let unexpected_closer = parse_baseline_with_options(
            "1+2)",
            RecoveryOptions {
                delimiter_dominance: true,
            },
        );
        assert_eq!(
            unexpected_closer.errors,
            ["Parsing error at line 1 column 4. Repair sequences found:\n   1: Delete )"]
        );
    }

    #[test]
    fn default_recovery_options_keep_dominance_disabled() {
        let report = parse_baseline_with_recovery_stats("1+*");
        assert!(
            report
                .recovery_stats
                .iter()
                .all(|stats| !stats.delimiter_dominance_enabled)
        );
    }

    #[test]
    fn enabling_dominance_does_not_change_valid_parse_results() {
        for input in ["1+2", "(1+2)*3", "((1+2)*3)"] {
            let baseline = parse_baseline(input);
            let experimental = parse_baseline_with_options(
                input,
                RecoveryOptions {
                    delimiter_dominance: true,
                },
            );
            assert_eq!(experimental, baseline, "{input}");
        }
    }

    #[test]
    fn accepted_input_has_no_recovery_statistics() {
        let report = parse_baseline_with_recovery_stats("1+2*3");
        assert_eq!(report.baseline.status, BaselineStatus::Accepted);
        assert!(report.recovery_stats.is_empty());
    }

    #[test]
    fn delimiter_observation_does_not_change_baseline_status_or_errors() {
        let cases = ["(1+2)*3", "((1+2)*3", "1+2)", "({[1+2]})"];

        for input in cases {
            let baseline = parse_baseline(input);
            let observed = parse_baseline_with_delimiter_observation(input);
            assert_eq!(observed.baseline, baseline, "{input}");
            assert!(
                !observed.delimiters.checkpoints.is_empty()
                    || !observed.delimiters.unexpected_closings.is_empty(),
                "{input}"
            );
        }
    }

    #[test]
    fn integration_observes_expression_delimiter_origins() {
        let balanced = parse_baseline_with_delimiter_observation("(1+2)*3");
        assert_eq!(balanced.baseline.status, BaselineStatus::Accepted);
        assert_eq!(balanced.delimiters.checkpoints.len(), 1);
        assert_eq!(balanced.delimiters.matches.len(), 1);
        assert_eq!(balanced.delimiters.unmatched_openings.len(), 0);

        let missing_close = parse_baseline_with_delimiter_observation("((1+2)*3");
        assert_ne!(missing_close.baseline.status, BaselineStatus::Accepted);
        assert_eq!(missing_close.delimiters.checkpoints.len(), 2);
        assert_eq!(missing_close.delimiters.matches.len(), 1);
        assert_eq!(missing_close.delimiters.unmatched_openings.len(), 1);
        assert_eq!(missing_close.delimiters.unmatched_openings[0].depth, 1);

        let extra_close = parse_baseline_with_delimiter_observation("1+2)");
        assert_ne!(extra_close.baseline.status, BaselineStatus::Accepted);
        assert_eq!(extra_close.delimiters.checkpoints.len(), 0);
        assert_eq!(extra_close.delimiters.unexpected_closings.len(), 1);

        let grammar_unsupported = parse_baseline_with_delimiter_observation("({[1+2]})");
        assert_ne!(
            grammar_unsupported.baseline.status,
            BaselineStatus::Accepted
        );
        assert_eq!(grammar_unsupported.delimiters.checkpoints.len(), 3);
        assert_eq!(grammar_unsupported.delimiters.matches.len(), 3);
        assert_eq!(grammar_unsupported.delimiters.max_depth, 3);
    }
}
