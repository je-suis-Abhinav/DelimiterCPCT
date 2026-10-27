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
    /// Enable delimiter-origin prioritization within equal-cost CPCT+ frontier buckets.
    pub delimiter_localization: bool,
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
    let recovery_kind = match (options.delimiter_dominance, options.delimiter_localization) {
        (false, false) => RecoveryKind::CPCTPlus,
        (true, false) => RecoveryKind::CPCTPlusDelimiterDominance,
        (false, true) => RecoveryKind::CPCTPlusDelimiterLocalization,
        (true, true) => RecoveryKind::CPCTPlusDelimiterDominanceAndLocalization,
    };
    let (res, errs) = if matches!(recovery_kind, RecoveryKind::CPCTPlus) {
        expr_y::parse(&lexer)
    } else {
        expr_y::parse_with_recovery_kind(&lexer, recovery_kind)
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

    fn normalized_repair_sets(errors: &[String]) -> Vec<Vec<String>> {
        let mut repair_sets = errors
            .iter()
            .map(|error| {
                let Some((_, repairs)) = error.split_once("Repair sequences found:\n") else {
                    return Vec::new();
                };
                let mut repairs = repairs
                    .lines()
                    .filter_map(|line| line.trim().split_once(": ").map(|(_, repair)| repair))
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                repairs.sort();
                repairs
            })
            .collect::<Vec<_>>();
        repair_sets.sort();
        repair_sets
    }

    #[test]
    fn valid_expressions_parse_without_recovery() {
        let cases = [
            ("1+2", 3),
            ("1+2*3", 7),
            ("(1+2)*3", 9),
            ("{1+2}*3", 9),
            ("[1+2]*3", 9),
            ("((1+2)*3)", 9),
            ("{{1+2}*[3+4]}", 21),
            ("({[1+2]}*3)", 9),
            ("[(1+2)*{3+4}]", 21),
        ];

        for (input, expected) in cases {
            let report = parse_baseline(input);
            assert_eq!(report.status, BaselineStatus::Accepted, "{input}");
            assert_eq!(report.value, Some(expected), "{input}");
            assert!(report.errors.is_empty(), "{input}");
        }
    }

    #[test]
    fn malformed_expressions_invoke_baseline_cpct_recovery() {
        let cases = [
            "1+", "1+*", "(1+2", "{1+2", "[1+2", "1+2)", "1+2}", "1+2]", "([1+2)]", "{(1+2}]",
            "[{1+2)]", "({[1+2", "(({1+2})", "1+2)+3",
        ];

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
                    delimiter_localization: false,
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
                delimiter_localization: false,
            },
        );
        assert_eq!(
            unexpected_closer.errors,
            ["Parsing error at line 1 column 4. Repair sequences found:\n   1: Delete )"]
        );
    }

    #[test]
    fn all_four_modes_preserve_exhaustive_minimum_cost_repair_sets() {
        let cases = [
            "(1+2",       // missing )
            "{1+2",       // missing }
            "[1+2",       // missing ]
            "1+2)",       // unexpected )
            "1+2}",       // unexpected }
            "1+2]",       // unexpected ]
            "([1+2)]",    // crossed delimiters
            "{(1+2}]",    // mismatched close and extra close
            "[{1+2)]",    // mismatched close
            "({[1+2",     // nested missing delimiters
            "(({1+2})",   // multiple nested delimiters with a missing close
            "({[1+*2]})", // delimiter imbalance with an operator error
            "1+*",        // multiple equal-cost repair candidates
            "1++2",       // ordinary operator error
            "*1+2",       // ordinary prefix syntax error
            "(1+*)",      // ordinary syntax error inside balanced delimiters
        ];
        let modes = [
            RecoveryOptions::default(),
            RecoveryOptions {
                delimiter_dominance: true,
                delimiter_localization: false,
            },
            RecoveryOptions {
                delimiter_dominance: false,
                delimiter_localization: true,
            },
            RecoveryOptions {
                delimiter_dominance: true,
                delimiter_localization: true,
            },
        ];

        for input in cases {
            let reports =
                modes.map(|options| parse_baseline_with_recovery_stats_and_options(input, options));
            let off = &reports[0];
            assert!(
                !off.recovery_stats.is_empty(),
                "CPCT+ was not invoked for {input:?}"
            );
            assert!(
                off.recovery_stats.iter().all(|stats| !stats.timed_out),
                "cannot compare a timed-out OFF search for {input:?}: {:#?}",
                off.recovery_stats
            );
            assert!(off.recovery_stats.iter().all(|stats| {
                !stats.delimiter_dominance_enabled && !stats.delimiter_localization_enabled
            }));
            let off_repairs = normalized_repair_sets(&off.baseline.errors);
            assert!(
                off_repairs.iter().all(|repairs| !repairs.is_empty()),
                "{input:?}"
            );

            for (options, experimental) in modes.into_iter().zip(reports.iter()).skip(1) {
                assert!(
                    experimental
                        .recovery_stats
                        .iter()
                        .all(|stats| !stats.timed_out),
                    "cannot claim exhaustive equivalence after timeout for {input:?}, \
                     options={options:?}: {:#?}",
                    experimental.recovery_stats
                );
                assert_eq!(
                    experimental.recovery_stats.len(),
                    off.recovery_stats.len(),
                    "recovery attempts differ for {input:?}, options={options:?}"
                );
                assert_eq!(
                    normalized_repair_sets(&experimental.baseline.errors),
                    off_repairs,
                    "minimum-cost repair sets differ for {input:?}, options={options:?}"
                );
                assert_eq!(
                    experimental
                        .recovery_stats
                        .iter()
                        .map(|stats| stats.minimum_recovery_cost)
                        .collect::<Vec<_>>(),
                    off.recovery_stats
                        .iter()
                        .map(|stats| stats.minimum_recovery_cost)
                        .collect::<Vec<_>>(),
                    "minimum recovery costs differ for {input:?}, options={options:?}"
                );
                assert!(experimental.recovery_stats.iter().all(|stats| {
                    stats.delimiter_dominance_enabled == options.delimiter_dominance
                        && stats.delimiter_localization_enabled == options.delimiter_localization
                }));
            }

            if input == "1+*" {
                assert_eq!(off_repairs.len(), 1);
                assert_eq!(off_repairs[0].len(), 2);
                for experimental in reports.iter().skip(1) {
                    assert_eq!(
                        normalized_repair_sets(&experimental.baseline.errors)[0].len(),
                        2,
                        "equal-cost alternatives were lost for {input:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn delimiter_localization_prioritizes_valid_cpct_configurations_and_records_positions() {
        let input = "({[1+2";
        let report = parse_baseline_with_recovery_stats_and_options(
            input,
            RecoveryOptions {
                delimiter_dominance: false,
                delimiter_localization: true,
            },
        );
        let stats = &report.recovery_stats[0];
        assert!(stats.delimiter_localization_enabled);
        assert_eq!(
            stats.localization_target.as_deref(),
            Some("unmatched_opening")
        );
        assert_eq!(stats.localization_checkpoint_id, Some(2));
        assert_eq!(stats.localization_delimiter, Some('['));
        assert_eq!(stats.localization_origin_input_index, Some(2));
        assert_eq!(stats.localization_origin_byte_index, Some(2));
        assert_eq!(stats.localization_origin_char_index, Some(2));
        assert_eq!(stats.localization_origin_depth, Some(3));
        assert_eq!(stats.recovery_start_input_index, Some(6));
        assert_eq!(stats.recovery_start_byte_index, Some(6));
        assert!(stats.localization_priority_pops > 0, "{stats:#?}");
        assert!(stats.localization_priority_reorders > 0, "{stats:#?}");
        assert!(stats.final_repair_byte_index.is_some());
        assert_eq!(stats.localization_to_recovery_start_distance, Some(4));
        assert!(stats.localization_to_repair_distance.is_some());
        assert!(stats.recovery_start_to_repair_distance.is_some());
    }

    #[test]
    fn localization_does_not_target_balanced_delimiters_or_unrelated_syntax_errors() {
        for input in ["(1+*)", "1+*"] {
            let off =
                parse_baseline_with_recovery_stats_and_options(input, RecoveryOptions::default());
            let localized = parse_baseline_with_recovery_stats_and_options(
                input,
                RecoveryOptions {
                    delimiter_dominance: false,
                    delimiter_localization: true,
                },
            );
            assert_eq!(localized.baseline.errors, off.baseline.errors, "{input}");
            assert_eq!(
                normalized_repair_sets(&localized.baseline.errors),
                normalized_repair_sets(&off.baseline.errors),
                "{input}"
            );
            assert!(
                localized
                    .recovery_stats
                    .iter()
                    .all(|stats| stats.localization_target.is_none()),
                "{input}: {:#?}",
                localized.recovery_stats
            );
        }
    }

    #[test]
    fn localization_identifies_unexpected_and_mismatched_delimiter_origins() {
        let unexpected = parse_baseline_with_recovery_stats_and_options(
            "1+2}",
            RecoveryOptions {
                delimiter_dominance: false,
                delimiter_localization: true,
            },
        );
        let unexpected_stats = &unexpected.recovery_stats[0];
        assert_eq!(
            unexpected_stats.localization_target.as_deref(),
            Some("unexpected_closing")
        );
        assert_eq!(unexpected_stats.localization_checkpoint_id, None);
        assert_eq!(unexpected_stats.localization_delimiter, Some('}'));
        assert_eq!(unexpected_stats.localization_origin_input_index, Some(3));
        assert_eq!(unexpected_stats.localization_origin_byte_index, Some(3));
        assert_eq!(unexpected_stats.localization_origin_char_index, Some(3));
        assert_eq!(unexpected_stats.localization_origin_depth, Some(0));
        assert_eq!(unexpected_stats.recovery_start_byte_index, Some(3));

        let mismatched = parse_baseline_with_recovery_stats_and_options(
            "([1+2)",
            RecoveryOptions {
                delimiter_dominance: false,
                delimiter_localization: true,
            },
        );
        let mismatch_stats = &mismatched.recovery_stats[0];
        assert_eq!(
            mismatch_stats.localization_target.as_deref(),
            Some("mismatched_closing_opening")
        );
        assert_eq!(mismatch_stats.localization_checkpoint_id, Some(1));
        assert_eq!(mismatch_stats.localization_delimiter, Some('['));
        assert_eq!(mismatch_stats.localization_origin_input_index, Some(1));
        assert_eq!(mismatch_stats.localization_origin_byte_index, Some(1));
        assert_eq!(mismatch_stats.localization_origin_char_index, Some(1));
        assert_eq!(mismatch_stats.localization_origin_depth, Some(2));
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
        for input in [
            "1+2",
            "(1+2)*3",
            "{1+2}*3",
            "[1+2]*3",
            "((1+2)*3)",
            "({[1+2]}*3)",
        ] {
            let baseline = parse_baseline(input);
            let experimental = parse_baseline_with_options(
                input,
                RecoveryOptions {
                    delimiter_dominance: true,
                    delimiter_localization: false,
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
        let cases = [
            "(1+2)*3",
            "{1+2}*3",
            "[1+2]*3",
            "((1+2)*3",
            "1+2)",
            "({[1+2]})",
        ];

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

        let mixed = parse_baseline_with_delimiter_observation("({[1+2]})");
        assert_eq!(mixed.baseline.status, BaselineStatus::Accepted);
        assert_eq!(mixed.baseline.value, Some(3));
        assert_eq!(mixed.delimiters.checkpoints.len(), 3);
        assert_eq!(mixed.delimiters.matches.len(), 3);
        assert_eq!(mixed.delimiters.max_depth, 3);
    }

    #[test]
    fn delimiter_tracking_and_localization_preserve_kind_origin_and_depth() {
        use crate::{
            delimiter::DelimiterKind,
            localization::{LocalizationReason, localize_structure},
        };

        for (input, kind, depth) in [
            ("(1+2", DelimiterKind::Parenthesis, 1),
            ("{1+2", DelimiterKind::Brace, 1),
            ("[1+2", DelimiterKind::Bracket, 1),
            ("({[1+2", DelimiterKind::Bracket, 3),
        ] {
            let observed = parse_baseline_with_delimiter_observation(input);
            assert_eq!(
                observed.baseline.status,
                BaselineStatus::Recovered,
                "{input}"
            );
            let candidate =
                &localize_structure(&observed.baseline, &observed.delimiters).candidates[0];
            assert_eq!(
                candidate.reason,
                LocalizationReason::UnmatchedOpening,
                "{input}"
            );
            assert_eq!(candidate.delimiter, kind, "{input}");
            assert_eq!(candidate.depth, depth, "{input}");
            assert_eq!(
                candidate.event_position.byte_offset,
                input.find(kind.open_char()).unwrap()
            );
            assert_eq!(
                candidate
                    .checkpoint
                    .as_ref()
                    .map(|checkpoint| checkpoint.kind),
                Some(kind),
                "{input}"
            );
        }

        for (input, kind) in [
            ("1+2)", DelimiterKind::Parenthesis),
            ("1+2}", DelimiterKind::Brace),
            ("1+2]", DelimiterKind::Bracket),
        ] {
            let observed = parse_baseline_with_delimiter_observation(input);
            assert_eq!(
                observed.baseline.status,
                BaselineStatus::Recovered,
                "{input}"
            );
            let candidate =
                &localize_structure(&observed.baseline, &observed.delimiters).candidates[0];
            assert_eq!(
                candidate.reason,
                LocalizationReason::UnexpectedClosing,
                "{input}"
            );
            assert_eq!(candidate.delimiter, kind, "{input}");
            assert_eq!(
                candidate.event_position.byte_offset,
                input.len() - 1,
                "{input}"
            );
        }

        let mismatched = parse_baseline_with_delimiter_observation("{(1+2]");
        let mismatch = &mismatched.delimiters.mismatched_closings[0];
        assert_eq!(mismatch.opening_kind, DelimiterKind::Parenthesis);
        assert_eq!(mismatch.opening_position.byte_offset, 1);
        assert_eq!(mismatch.closing_kind, DelimiterKind::Bracket);
        assert_eq!(mismatch.closing_position.byte_offset, 5);
        assert_eq!(mismatch.depth, 2);
    }
}
