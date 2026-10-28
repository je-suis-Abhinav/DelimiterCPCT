use std::{collections::BTreeSet, time::Instant};

use crate::{
    BaselineStatus, RecoveryOptions, delimiter::observe_delimiters,
    parse_baseline_with_recovery_stats_and_options,
};
use lrpar::Lexer;

const DEFAULT_TRIALS: usize = 5;
const DEFAULT_WARMUPS: usize = 1;
pub const PHASE6_DEFAULT_TRIALS: usize = 15;
pub const PHASE6_DEFAULT_WARMUPS: usize = 3;
const MODE_LABELS: [(&str, &str, bool, bool); 4] = [
    ("OFF", "OFF", false, false),
    ("ON", "OFF", true, false),
    ("OFF", "ON", false, true),
    ("ON", "ON", true, true),
];

const TSV_COLUMNS: &[&str] = &[
    "case",
    "category",
    "input",
    "trial",
    "trial_count",
    "warmup_count",
    "dominance",
    "localization",
    "build_profile",
    "input_bytes",
    "token_count_excluding_eof",
    "delimiter_count",
    "max_delimiter_depth",
    "parse_outcome",
    "semantic_value_available",
    "recovery_invoked",
    "total_runtime_ns",
    "cpct_runtime_ns",
    "generated_candidates",
    "frontier_pops",
    "frontier_merges",
    "discard_events",
    "discard_cost",
    "discard_timeout",
    "discard_higher_cost",
    "discard_duplicate_histories",
    "discard_dominance_variants",
    "dominance_candidates_checked",
    "dominance_exact_key_matches",
    "dominance_variants_discarded",
    "dominance_discarded_cost_difference",
    "minimum_recovery_costs",
    "minimum_repair_sequence_counts",
    "timed_out",
    "parser_error_byte",
    "localization_targets",
    "localization_origin_input_indices",
    "localization_origin_byte_indices",
    "localization_origin_char_indices",
    "localization_origin_depths",
    "recovery_start_input_indices",
    "recovery_start_byte_indices",
    "final_repair_byte_indices",
    "localization_priority_pops",
    "localization_priority_reorders",
    "localization_delimiters",
    "localization_checkpoint_ids",
    "target_platform",
];

const SUMMARY_COLUMNS: &[&str] = &[
    "case",
    "dominance",
    "localization",
    "trial_count",
    "warmup_count",
    "build_profile",
    "target_platform",
    "timeout_trials",
    "generated_mean",
    "popped_mean",
    "merged_mean",
    "discard_events_mean",
    "dominance_candidates_mean",
    "dominance_discards_mean",
    "minimum_cost_values",
    "repair_sequence_count_values",
    "total_runtime_median_ns",
    "total_runtime_mean_ns",
    "total_runtime_stddev_ns",
    "total_runtime_min_ns",
    "total_runtime_max_ns",
    "cpct_runtime_median_ns",
    "cpct_runtime_mean_ns",
    "cpct_runtime_stddev_ns",
    "cpct_runtime_min_ns",
    "cpct_runtime_max_ns",
    "generated_reduction_vs_off_off_pct",
    "popped_reduction_vs_off_off_pct",
    "total_runtime_change_vs_off_off_pct",
    "cpct_runtime_change_vs_off_off_pct",
    "localization_priority_pops_mean",
    "localization_priority_reorders_mean",
    "localization_order_changed",
    "search_work_effect",
    "runtime_effect",
];
const PHASE6_RAW_EXTRA_COLUMN: &str = "differential_comparison";
const PHASE6_SUMMARY_EXTRA_COLUMNS: &[&str] =
    &["equivalence_checked_trials", "incomplete_comparison_trials"];

#[derive(Clone, Debug)]
struct CorpusCase {
    name: String,
    category: &'static str,
    input: String,
    expects_recovery: bool,
}

fn corpus() -> Vec<CorpusCase> {
    let mut cases = vec![
        ("valid_simple", "valid", "1+2*3", false),
        ("valid_parentheses", "valid", "(1+2)*3", false),
        ("valid_braces", "valid", "{1+2}*3", false),
        ("valid_brackets", "valid", "[1+2]*3", false),
        ("valid_mixed_nesting", "valid", "({[1+2]}*3)", false),
        ("missing_close_paren", "missing_close", "(1+2", true),
        ("missing_close_brace", "missing_close", "{1+2", true),
        ("missing_close_bracket", "missing_close", "[1+2", true),
        ("unexpected_close_paren", "unexpected_close", "1+2)", true),
        ("unexpected_close_brace", "unexpected_close", "1+2}", true),
        ("unexpected_close_bracket", "unexpected_close", "1+2]", true),
        ("mismatched_close_paren", "mismatched", "{1+2)", true),
        ("mismatched_close_brace", "mismatched", "[1+2}", true),
        ("mismatched_close_bracket", "mismatched", "(1+2]", true),
        ("nested_missing_mixed", "nested_mixed", "({[1+2", true),
        ("nested_crossed_delimiters", "nested_mixed", "([1+2)]", true),
        (
            "multiple_unmatched_openings",
            "multiple_unmatched",
            "(({1+2})",
            true,
        ),
        (
            "delimiter_heavy_cascade",
            "delimiter_cascade",
            "({[(1+*2]})",
            true,
        ),
        ("ordinary_missing_operand", "ordinary_syntax", "1+*", true),
        ("ordinary_extra_operator", "ordinary_syntax", "1++2", true),
        (
            "ordinary_error_in_balanced_group",
            "ordinary_syntax",
            "(1+*)",
            true,
        ),
        (
            "multiple_equal_cost_repairs",
            "equal_cost_repairs",
            "1+*",
            true,
        ),
        ("ordinary_missing_lhs", "ordinary_syntax", "*1+2", true),
    ]
    .into_iter()
    .map(|(name, category, input, expects_recovery)| CorpusCase {
        name: name.to_string(),
        category,
        input: input.to_string(),
        expects_recovery,
    })
    .collect::<Vec<_>>();

    for depth in [8, 16, 32] {
        cases.push(CorpusCase {
            name: format!("valid_nested_depth_{depth}"),
            category: "increasing_nesting",
            input: format!("{}1+2{}", "(".repeat(depth), ")".repeat(depth)),
            expects_recovery: false,
        });
    }
    for depth in [4, 6] {
        cases.push(CorpusCase {
            name: format!("missing_close_depth_{depth}"),
            category: "increasing_nesting",
            input: format!("{}1+2", "(".repeat(depth)),
            expects_recovery: true,
        });
    }

    for terms in [8, 24] {
        let input = (1..=terms)
            .map(|n| (n % 9 + 1).to_string())
            .collect::<Vec<_>>()
            .join("+");
        cases.push(CorpusCase {
            name: format!("valid_expression_terms_{terms}"),
            category: "increasing_input_size",
            input,
            expects_recovery: false,
        });
    }

    cases
}

fn phase6_corpus() -> Vec<CorpusCase> {
    let mut cases = Vec::new();
    for depth in [64, 128, 256] {
        cases.push(CorpusCase {
            name: format!("phase6_valid_nesting_depth_{depth}"),
            category: "phase6_valid_nesting",
            input: format!("{}1+2{}", "(".repeat(depth), ")".repeat(depth)),
            expects_recovery: false,
        });
    }
    for depth in [8, 16, 32, 64] {
        cases.push(CorpusCase {
            name: format!("phase6_missing_close_depth_{depth}"),
            category: "phase6_missing_close_nesting",
            input: format!("{}1+2", "(".repeat(depth)),
            expects_recovery: true,
        });
    }
    for terms in [48, 96, 192] {
        let expression = expression_with_terms(terms);
        cases.push(CorpusCase {
            name: format!("phase6_valid_expression_terms_{terms}"),
            category: "phase6_valid_expression_size",
            input: expression.clone(),
            expects_recovery: false,
        });
        cases.push(CorpusCase {
            name: format!("phase6_missing_close_expression_terms_{terms}"),
            category: "phase6_missing_close_expression_size",
            input: format!("({expression}"),
            expects_recovery: true,
        });
    }
    cases.extend(
        [
            (
                "phase6_mismatch_outer_close",
                "phase6_mismatched",
                "({[1+2)}]",
            ),
            (
                "phase6_mismatch_inner_close",
                "phase6_mismatched",
                "(({1+2)]}",
            ),
            (
                "phase6_mismatch_later_position",
                "phase6_mismatched",
                "{([1+2]})",
            ),
            (
                "phase6_crossed_paren_bracket",
                "phase6_crossed_mixed",
                "([1+{2*3)]}",
            ),
            (
                "phase6_crossed_brace_paren",
                "phase6_crossed_mixed",
                "{(1+[2*3})]",
            ),
            (
                "phase6_mixed_unexpected_close",
                "phase6_crossed_mixed",
                "1+({2*([3+4)]}",
            ),
            (
                "phase6_multiple_unmatched_three_types",
                "phase6_multiple_unmatched",
                "({[1+2",
            ),
            (
                "phase6_multiple_unmatched_mixed_depth",
                "phase6_multiple_unmatched",
                "({[({1+2})",
            ),
        ]
        .into_iter()
        .map(|(name, category, input)| CorpusCase {
            name: name.to_string(),
            category,
            input: input.to_string(),
            expects_recovery: true,
        }),
    );
    for (name, suffix) in [
        ("phase6_ordinary_error_long_missing_operand", "+*7"),
        ("phase6_ordinary_error_long_extra_operator", "++7"),
    ] {
        cases.push(CorpusCase {
            name: name.to_string(),
            category: "phase6_ordinary_syntax",
            input: format!("{}{}", expression_with_terms(96), suffix),
            expects_recovery: true,
        });
    }
    cases
}

fn expression_with_terms(terms: usize) -> String {
    (1..=terms)
        .map(|number| (number % 9 + 1).to_string())
        .collect::<Vec<_>>()
        .join("+")
}

#[derive(Clone)]
struct TrialRecord {
    case_index: usize,
    mode_index: usize,
    trial: usize,
    fields: Vec<String>,
    build_profile: String,
    target_platform: String,
    parse_ns: u128,
    cpct_ns: Option<u128>,
    generated: Option<u64>,
    popped: Option<u64>,
    merged: Option<u64>,
    discarded: Option<u64>,
    dominance_candidates: Option<u64>,
    dominance_discards: Option<u64>,
    localization_priority_pops: Option<u64>,
    localization_priority_reorders: Option<u64>,
    minimum_costs: String,
    repair_counts: String,
    timeout: bool,
    differential_status: Option<String>,
}

#[derive(Clone)]
struct DifferentialObservation {
    minimum_costs: Vec<Option<u16>>,
    repair_sets: Vec<Vec<String>>,
    timed_out: bool,
}

impl DifferentialObservation {
    fn from_report(report: &crate::BaselineRecoveryStatsReport) -> Self {
        Self {
            minimum_costs: report
                .recovery_stats
                .iter()
                .map(|stats| stats.minimum_recovery_cost)
                .collect(),
            repair_sets: normalized_repair_sets(&report.baseline.errors),
            timed_out: report.recovery_stats.iter().any(|stats| stats.timed_out),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DifferentialResult {
    Equivalent,
    IncompleteTimeout,
}

/// Run paired measurements in a deterministic, varied mode order.
///
/// The returned rows contain measured observations; timings are inherently
/// machine- and load-dependent. Use a release build for performance results.
pub fn benchmark_tsv() -> String {
    benchmark_tsv_with_trials(DEFAULT_TRIALS, DEFAULT_WARMUPS)
        .expect("the built-in benchmark settings are valid")
}

pub fn benchmark_tsv_with_trials(trials: usize, warmups: usize) -> Result<String, String> {
    benchmark_tsv_for_cases(&corpus(), trials, warmups, false)
}

pub fn benchmark_phase6_tsv_with_trials(trials: usize, warmups: usize) -> Result<String, String> {
    benchmark_tsv_for_cases(&phase6_corpus(), trials, warmups, true)
}

pub fn benchmark_phase6_tsv() -> String {
    benchmark_phase6_tsv_with_trials(PHASE6_DEFAULT_TRIALS, PHASE6_DEFAULT_WARMUPS)
        .expect("the built-in Phase 6 benchmark settings are valid")
}

fn benchmark_tsv_for_cases(
    cases: &[CorpusCase],
    trials: usize,
    warmups: usize,
    compare_modes: bool,
) -> Result<String, String> {
    if trials == 0 {
        return Err("benchmark trial count must be greater than zero".to_string());
    }
    let mut records = Vec::with_capacity(cases.len() * MODE_LABELS.len() * trials);

    for (case_index, case) in cases.iter().enumerate() {
        let token_count = input_token_count(&case.input)
            .ok_or_else(|| format!("benchmark case {:?} has a lexical error", case.name))?;
        let delimiter_report = observe_delimiters(&case.input);
        for warmup in 0..warmups {
            for mode_index in mode_order(case_index, warmup, true) {
                let options = mode_options(mode_index);
                let report = parse_baseline_with_recovery_stats_and_options(&case.input, options);
                validate_case(case, &report.baseline.status, &report.recovery_stats.len())?;
            }
        }
        for trial in 0..trials {
            let mut differential_observations = vec![None; MODE_LABELS.len()];
            for mode_index in mode_order(case_index, trial, false) {
                let options = mode_options(mode_index);
                let started = Instant::now();
                let report = parse_baseline_with_recovery_stats_and_options(&case.input, options);
                let parse_ns = started.elapsed().as_nanos();
                validate_case(case, &report.baseline.status, &report.recovery_stats.len())?;
                if compare_modes {
                    differential_observations[mode_index] =
                        Some(DifferentialObservation::from_report(&report));
                }
                records.push(make_record(
                    case_index,
                    case,
                    mode_index,
                    trial,
                    trials,
                    warmups,
                    token_count,
                    &delimiter_report,
                    parse_ns,
                    report,
                ));
            }
            if compare_modes {
                let comparison = check_differential_equivalence(case, &differential_observations)?;
                let status = match comparison {
                    DifferentialResult::Equivalent => "equivalent",
                    DifferentialResult::IncompleteTimeout => "incomplete_timeout",
                };
                let mut rows_updated = 0;
                for record in records
                    .iter_mut()
                    .filter(|record| record.case_index == case_index && record.trial == trial)
                {
                    record.fields.push(status.to_string());
                    record.differential_status = Some(status.to_string());
                    rows_updated += 1;
                }
                if rows_updated != MODE_LABELS.len() {
                    return Err(format!(
                        "Phase 6 case {:?} has an incomplete paired trial",
                        case.name
                    ));
                }
            }
        }
    }

    records_to_tsv(&records, compare_modes)
}

fn records_to_tsv(records: &[TrialRecord], phase6: bool) -> Result<String, String> {
    let mut output = raw_tsv_header(phase6);
    output.push('\n');
    let mut unique_rows = BTreeSet::new();
    for record in records {
        if !unique_rows.insert((record.case_index, record.mode_index, record.trial)) {
            return Err("benchmark run generated duplicate paired trial rows".to_string());
        }
        let expected_len = TSV_COLUMNS.len() + if phase6 { 1 } else { 0 };
        if record.fields.len() != expected_len {
            return Err("benchmark run generated a row with an invalid schema".to_string());
        }
        output.push_str(&record.fields.join("\t"));
        output.push('\n');
    }
    Ok(output)
}

fn raw_tsv_header(phase6: bool) -> String {
    if phase6 {
        format!("{}\t{}", TSV_COLUMNS.join("\t"), PHASE6_RAW_EXTRA_COLUMN)
    } else {
        TSV_COLUMNS.join("\t")
    }
}

fn summary_tsv_header(phase6: bool) -> String {
    if phase6 {
        format!(
            "{}\t{}",
            SUMMARY_COLUMNS.join("\t"),
            PHASE6_SUMMARY_EXTRA_COLUMNS.join("\t")
        )
    } else {
        SUMMARY_COLUMNS.join("\t")
    }
}

pub fn benchmark_summary_tsv() -> String {
    benchmark_summary_tsv_with_trials(DEFAULT_TRIALS, DEFAULT_WARMUPS)
        .expect("the built-in benchmark settings are valid")
}

pub fn benchmark_summary_tsv_with_trials(trials: usize, warmups: usize) -> Result<String, String> {
    let raw = benchmark_tsv_with_trials(trials, warmups)?;
    benchmark_summary_from_tsv(&raw)
}

pub fn benchmark_summary_from_tsv(raw: &str) -> Result<String, String> {
    benchmark_summary_for_tsv(raw, &corpus(), false)
}

pub fn benchmark_phase6_summary_from_tsv(raw: &str) -> Result<String, String> {
    benchmark_summary_for_tsv(raw, &phase6_corpus(), true)
}

pub fn benchmark_phase6_summary_tsv_with_trials(
    trials: usize,
    warmups: usize,
) -> Result<String, String> {
    let raw = benchmark_phase6_tsv_with_trials(trials, warmups)?;
    benchmark_phase6_summary_from_tsv(&raw)
}

fn benchmark_summary_for_tsv(
    raw: &str,
    cases: &[CorpusCase],
    phase6: bool,
) -> Result<String, String> {
    if raw.lines().next() != Some(raw_tsv_header(phase6).as_str()) {
        return Err("benchmark TSV header did not match the expected schema".to_string());
    }
    let first_row = raw
        .lines()
        .nth(1)
        .ok_or_else(|| "benchmark TSV contains no measured rows".to_string())?;
    let metadata = first_row.split('\t').collect::<Vec<_>>();
    let expected_columns = TSV_COLUMNS.len() + usize::from(phase6);
    if metadata.len() != expected_columns {
        return Err("benchmark TSV row has an unexpected number of columns".to_string());
    }
    let trials = metadata[4]
        .parse::<usize>()
        .map_err(|_| "invalid trial count in benchmark TSV".to_string())?;
    let warmups = metadata[5]
        .parse::<usize>()
        .map_err(|_| "invalid warm-up count in benchmark TSV".to_string())?;
    if trials == 0 {
        return Err("benchmark TSV must contain at least one measured trial".to_string());
    }
    let mut records = parse_raw_records(raw, cases, phase6)?;
    let expected_rows = cases.len() * MODE_LABELS.len() * trials;
    let unique_rows = records
        .iter()
        .map(|record| (record.case_index, record.mode_index, record.trial))
        .collect::<BTreeSet<_>>();
    if records.len() != expected_rows || unique_rows.len() != expected_rows {
        return Err("benchmark TSV is missing or duplicates paired trial rows".to_string());
    }
    if phase6 {
        validate_phase6_comparison_metadata(&records)?;
    }
    if records.iter().any(|record| {
        record.fields[4] != trials.to_string()
            || record.fields[5] != warmups.to_string()
            || record.trial >= trials
            || record.fields[8] != metadata[8]
            || record.fields[47] != metadata[47]
            || (phase6 && record.fields[48].is_empty())
    }) {
        return Err("benchmark TSV has inconsistent trial or build metadata".to_string());
    }
    summarize_records(&mut records, trials, warmups, cases, phase6)
}

fn validate_phase6_comparison_metadata(records: &[TrialRecord]) -> Result<(), String> {
    let mut pairs = std::collections::BTreeMap::<(usize, usize), Vec<&TrialRecord>>::new();
    for record in records {
        pairs
            .entry((record.case_index, record.trial))
            .or_default()
            .push(record);
    }
    for rows in pairs.values() {
        if rows.len() != MODE_LABELS.len() {
            return Err("Phase 6 TSV has an incomplete mode pair".to_string());
        }
        let timed_out = rows.iter().any(|record| record.timeout);
        let expected_status = if timed_out {
            "incomplete_timeout"
        } else {
            "equivalent"
        };
        if rows
            .iter()
            .any(|record| record.differential_status.as_deref() != Some(expected_status))
        {
            return Err(
                "Phase 6 TSV differential status conflicts with its timeout observations"
                    .to_string(),
            );
        }
    }
    Ok(())
}

fn validate_case(
    case: &CorpusCase,
    status: &BaselineStatus,
    recovery_count: &usize,
) -> Result<(), String> {
    if matches!(status, BaselineStatus::Failed) && !case.expects_recovery {
        return Err(format!("benchmark case {:?} failed parsing", case.name));
    }
    if case.expects_recovery && *recovery_count == 0 {
        return Err(format!(
            "benchmark case {:?} did not reach CPCT+ recovery",
            case.name
        ));
    }
    if !case.expects_recovery && *recovery_count != 0 {
        return Err(format!(
            "valid benchmark case {:?} unexpectedly invoked CPCT+",
            case.name
        ));
    }
    Ok(())
}

fn check_differential_equivalence(
    case: &CorpusCase,
    observations: &[Option<DifferentialObservation>],
) -> Result<DifferentialResult, String> {
    let observations = observations
        .iter()
        .enumerate()
        .map(|(index, observation)| {
            observation.as_ref().ok_or_else(|| {
                format!(
                    "Phase 6 case {:?} is missing mode {} for differential validation",
                    case.name, index
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if observations.iter().any(|observation| observation.timed_out) {
        return Ok(DifferentialResult::IncompleteTimeout);
    }

    let baseline = observations[0];
    if case.expects_recovery
        && observations.iter().any(|observation| {
            observation.minimum_costs.is_empty()
                || observation.minimum_costs.iter().any(Option::is_none)
                || observation.repair_sets.is_empty()
                || observation.repair_sets.iter().any(Vec::is_empty)
        })
    {
        return Err(format!(
            "completed Phase 6 case {:?} is missing minimum-cost repair data",
            case.name
        ));
    }
    for (mode_index, observation) in observations.iter().enumerate().skip(1) {
        if observation.minimum_costs != baseline.minimum_costs {
            return Err(format!(
                "Phase 6 case {:?} has a minimum recovery cost discrepancy in mode {}",
                case.name, mode_index
            ));
        }
        if observation.repair_sets != baseline.repair_sets {
            return Err(format!(
                "Phase 6 case {:?} has a minimum-cost repair-set discrepancy in mode {}",
                case.name, mode_index
            ));
        }
    }
    Ok(DifferentialResult::Equivalent)
}

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
                .map(str::to_string)
                .collect::<Vec<_>>();
            repairs.sort();
            repairs
        })
        .collect::<Vec<_>>();
    repair_sets.sort();
    repair_sets
}

fn mode_options(mode_index: usize) -> RecoveryOptions {
    let (_, _, delimiter_dominance, delimiter_localization) = MODE_LABELS[mode_index];
    RecoveryOptions {
        delimiter_dominance,
        delimiter_localization,
    }
}

fn mode_order(case_index: usize, trial: usize, warmup: bool) -> [usize; 4] {
    let mut order = [0, 1, 2, 3];
    let mut state = 0x9E37_79B9_u64
        ^ (case_index as u64).wrapping_mul(0x85EB_CA6B)
        ^ (trial as u64).wrapping_mul(0xC2B2_AE35)
        ^ u64::from(warmup);
    for index in (1..order.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let swap_index = (state as usize) % (index + 1);
        order.swap(index, swap_index);
    }
    order
}

fn input_token_count(input: &str) -> Option<usize> {
    let lexerdef = crate::expr_l::lexerdef();
    let lexer = lexerdef.lexer(input);
    lexer
        .iter()
        .collect::<Result<Vec<_>, _>>()
        .ok()
        .map(|tokens| tokens.len())
}

#[allow(clippy::too_many_arguments)]
fn make_record(
    case_index: usize,
    case: &CorpusCase,
    mode_index: usize,
    trial: usize,
    trial_count: usize,
    warmup_count: usize,
    token_count: usize,
    delimiters: &crate::delimiter::DelimiterReport,
    parse_ns: u128,
    report: crate::BaselineRecoveryStatsReport,
) -> TrialRecord {
    let stats = &report.recovery_stats;
    let sum = |value: fn(&lrpar::RecoveryStats) -> u64| {
        if stats.is_empty() {
            String::new()
        } else {
            stats.iter().map(value).sum::<u64>().to_string()
        }
    };
    let list = |value: &dyn Fn(&lrpar::RecoveryStats) -> Option<String>| {
        stats
            .iter()
            .map(|stat| value(stat).unwrap_or_default())
            .collect::<Vec<_>>()
            .join(";")
    };
    let (dominance, localization, _, _) = MODE_LABELS[mode_index];
    let cpct_ns = (!stats.is_empty()).then(|| {
        stats
            .iter()
            .map(|stat| stat.elapsed.as_nanos())
            .sum::<u128>()
    });
    let generated: Option<u64> =
        (!stats.is_empty()).then(|| stats.iter().map(|stat| stat.configurations_generated).sum());
    let popped: Option<u64> =
        (!stats.is_empty()).then(|| stats.iter().map(|stat| stat.configurations_popped).sum());
    let merged: Option<u64> =
        (!stats.is_empty()).then(|| stats.iter().map(|stat| stat.configurations_merged).sum());
    let discarded: Option<u64> =
        (!stats.is_empty()).then(|| stats.iter().map(|stat| stat.configurations_discarded).sum());
    let dominance_candidates: Option<u64> = (!stats.is_empty()).then(|| {
        stats
            .iter()
            .map(|stat| stat.dominance_candidates_checked)
            .sum()
    });
    let dominance_discards: Option<u64> = (!stats.is_empty()).then(|| {
        stats
            .iter()
            .map(|stat| stat.dominance_variants_discarded)
            .sum()
    });
    let localization_priority_pops: Option<u64> = (!stats.is_empty()).then(|| {
        stats
            .iter()
            .map(|stat| stat.localization_priority_pops)
            .sum()
    });
    let localization_priority_reorders: Option<u64> = (!stats.is_empty()).then(|| {
        stats
            .iter()
            .map(|stat| stat.localization_priority_reorders)
            .sum()
    });
    let min_costs = list(&|stat: &lrpar::RecoveryStats| {
        stat.minimum_recovery_cost.map(|value| value.to_string())
    });
    let repair_counts = list(&|stat: &lrpar::RecoveryStats| {
        stat.repair_sequences_after_simplification
            .map(|value| value.to_string())
    });
    let timeout = stats.iter().any(|stat| stat.timed_out);
    let build_profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let target_platform = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
    let parser_error_byte = report
        .baseline
        .error_positions
        .first()
        .map(|position| position.byte_offset.to_string())
        .unwrap_or_default();
    let delimiter_count = case
        .input
        .chars()
        .filter(|ch| matches!(ch, '(' | ')' | '{' | '}' | '[' | ']'))
        .count();

    let fields = vec![
        tsv_field(&case.name),
        case.category.to_string(),
        tsv_field(&case.input),
        trial.to_string(),
        trial_count.to_string(),
        warmup_count.to_string(),
        dominance.to_string(),
        localization.to_string(),
        build_profile.to_string(),
        case.input.len().to_string(),
        token_count.to_string(),
        delimiter_count.to_string(),
        delimiters.max_depth.to_string(),
        format!("{:?}", report.baseline.status),
        report.baseline.value.is_some().to_string(),
        (!stats.is_empty()).to_string(),
        parse_ns.to_string(),
        cpct_ns.map(|value| value.to_string()).unwrap_or_default(),
        generated.map(|value| value.to_string()).unwrap_or_default(),
        popped.map(|value| value.to_string()).unwrap_or_default(),
        merged.map(|value| value.to_string()).unwrap_or_default(),
        discarded.map(|value| value.to_string()).unwrap_or_default(),
        sum(|stat| stat.configurations_discarded_cost),
        sum(|stat| stat.configurations_discarded_timeout),
        sum(|stat| stat.configurations_discarded_higher_cost),
        sum(|stat| stat.configurations_discarded_duplicate),
        sum(|stat| stat.configurations_discarded_dominance),
        sum(|stat| stat.dominance_candidates_checked),
        sum(|stat| stat.dominance_exact_key_matches),
        sum(|stat| stat.dominance_variants_discarded),
        sum(|stat| stat.dominance_discarded_cost_difference),
        list(&|stat| stat.minimum_recovery_cost.map(|value| value.to_string())),
        list(&|stat| {
            stat.repair_sequences_after_simplification
                .map(|value| value.to_string())
        }),
        timeout.to_string(),
        parser_error_byte,
        list_string_stat(stats, |stat| stat.localization_target.clone()),
        list_stat(stats, |stat| stat.localization_origin_input_index),
        list_stat(stats, |stat| stat.localization_origin_byte_index),
        list_stat(stats, |stat| stat.localization_origin_char_index),
        list_stat(stats, |stat| stat.localization_origin_depth),
        list_stat(stats, |stat| stat.recovery_start_input_index),
        list_stat(stats, |stat| stat.recovery_start_byte_index),
        list_stat(stats, |stat| stat.final_repair_byte_index),
        sum(|stat| stat.localization_priority_pops),
        sum(|stat| stat.localization_priority_reorders),
        list_stat(stats, |stat| stat.localization_delimiter),
        list_stat(stats, |stat| stat.localization_checkpoint_id),
        target_platform.clone(),
    ];
    debug_assert_eq!(fields.len(), TSV_COLUMNS.len());

    TrialRecord {
        case_index,
        mode_index,
        trial,
        fields,
        build_profile: build_profile.to_string(),
        target_platform,
        parse_ns,
        cpct_ns,
        generated,
        popped,
        merged,
        discarded,
        dominance_candidates,
        dominance_discards,
        localization_priority_pops,
        localization_priority_reorders,
        minimum_costs: min_costs,
        repair_counts,
        timeout,
        differential_status: None,
    }
}

fn list_string_stat(
    stats: &[lrpar::RecoveryStats],
    value: impl Fn(&lrpar::RecoveryStats) -> Option<String>,
) -> String {
    stats
        .iter()
        .map(|stat| value(stat).unwrap_or_default())
        .collect::<Vec<_>>()
        .join(";")
}

fn list_stat<T: std::fmt::Display>(
    stats: &[lrpar::RecoveryStats],
    value: impl Fn(&lrpar::RecoveryStats) -> Option<T>,
) -> String {
    stats
        .iter()
        .map(|stat| {
            value(stat)
                .map(|value| value.to_string())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn summarize_records(
    records: &mut [TrialRecord],
    trials: usize,
    warmups: usize,
    cases: &[CorpusCase],
    phase6: bool,
) -> Result<String, String> {
    let mut output = summary_tsv_header(phase6);
    output.push('\n');
    for case_index in 0..cases.len() {
        for mode_index in 0..MODE_LABELS.len() {
            let group = records
                .iter()
                .filter(|record| record.case_index == case_index && record.mode_index == mode_index)
                .collect::<Vec<_>>();
            output.push_str(&summary_row(
                &cases[case_index].name,
                mode_index,
                &group,
                records,
                case_index,
                trials,
                warmups,
                phase6,
            )?);
            output.push('\n');
        }
    }

    let mut aggregate = Vec::new();
    for mode_index in 0..MODE_LABELS.len() {
        for trial in 0..trials {
            let selected = records
                .iter()
                .filter(|record| record.mode_index == mode_index && record.trial == trial)
                .collect::<Vec<_>>();
            if selected.len() != cases.len() {
                return Err("benchmark aggregate is missing case/trial records".to_string());
            }
            aggregate.push(aggregate_record(mode_index, trial, &selected));
        }
    }
    for mode_index in 0..MODE_LABELS.len() {
        let group = aggregate
            .iter()
            .filter(|record| record.mode_index == mode_index)
            .collect::<Vec<_>>();
        output.push_str(&summary_row(
            "__AGGREGATE__",
            mode_index,
            &group,
            &aggregate,
            usize::MAX,
            trials,
            warmups,
            phase6,
        )?);
        output.push('\n');
    }
    Ok(output)
}

fn aggregate_record(mode_index: usize, trial: usize, selected: &[&TrialRecord]) -> TrialRecord {
    let sum_counter = |get: fn(&TrialRecord) -> Option<u64>| {
        selected
            .iter()
            .map(|record| get(record).unwrap_or_default())
            .sum::<u64>()
    };
    let costs = selected
        .iter()
        .flat_map(|record| record.minimum_costs.split(';'))
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(";");
    let repairs = selected
        .iter()
        .flat_map(|record| record.repair_counts.split(';'))
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(";");
    TrialRecord {
        case_index: usize::MAX,
        mode_index,
        trial,
        fields: Vec::new(),
        build_profile: selected
            .first()
            .map(|record| record.build_profile.clone())
            .unwrap_or_else(|| "unknown".to_string()),
        target_platform: selected
            .first()
            .map(|record| record.target_platform.clone())
            .unwrap_or_else(|| "unknown".to_string()),
        parse_ns: selected.iter().map(|record| record.parse_ns).sum(),
        cpct_ns: Some(
            selected
                .iter()
                .map(|record| record.cpct_ns.unwrap_or_default())
                .sum(),
        ),
        generated: Some(sum_counter(|record| record.generated)),
        popped: Some(sum_counter(|record| record.popped)),
        merged: Some(sum_counter(|record| record.merged)),
        discarded: Some(sum_counter(|record| record.discarded)),
        dominance_candidates: Some(sum_counter(|record| record.dominance_candidates)),
        dominance_discards: Some(sum_counter(|record| record.dominance_discards)),
        localization_priority_pops: Some(sum_counter(|record| record.localization_priority_pops)),
        localization_priority_reorders: Some(sum_counter(|record| {
            record.localization_priority_reorders
        })),
        minimum_costs: costs,
        repair_counts: repairs,
        timeout: selected.iter().any(|record| record.timeout),
        differential_status: if selected
            .iter()
            .any(|record| record.differential_status.as_deref() == Some("incomplete_timeout"))
        {
            Some("incomplete_timeout".to_string())
        } else if selected
            .iter()
            .all(|record| record.differential_status.as_deref() == Some("equivalent"))
        {
            Some("equivalent".to_string())
        } else {
            None
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn summary_row(
    case_name: &str,
    mode_index: usize,
    group: &[&TrialRecord],
    all_records: &[TrialRecord],
    case_index: usize,
    trials: usize,
    warmups: usize,
    phase6: bool,
) -> Result<String, String> {
    if group.len() != trials {
        return Err(format!("incomplete trial group for {case_name}"));
    }
    let (dominance, localization, _, _) = MODE_LABELS[mode_index];
    let baseline_mode = 0;
    let baseline = all_records
        .iter()
        .filter(|record| {
            record.mode_index == baseline_mode
                && record.trial < trials
                && (case_index == usize::MAX || record.case_index == case_index)
        })
        .collect::<Vec<_>>();
    if baseline.len() != trials {
        return Err(format!("missing OFF/OFF paired trials for {case_name}"));
    }

    let generated = counter_mean(group, |record| record.generated);
    let popped = counter_mean(group, |record| record.popped);
    let merged = counter_mean(group, |record| record.merged);
    let discarded = counter_mean(group, |record| record.discarded);
    let dominance_candidates = counter_mean(group, |record| record.dominance_candidates);
    let dominance_discards = counter_mean(group, |record| record.dominance_discards);
    let localization_priority_pops =
        counter_mean(group, |record| record.localization_priority_pops);
    let localization_priority_reorders =
        counter_mean(group, |record| record.localization_priority_reorders);
    let baseline_generated = counter_mean(&baseline, |record| record.generated);
    let baseline_popped = counter_mean(&baseline, |record| record.popped);
    let total_values = group
        .iter()
        .map(|record| record.parse_ns)
        .collect::<Vec<_>>();
    let cpct_values = group
        .iter()
        .map(|record| record.cpct_ns)
        .collect::<Option<Vec<_>>>();
    let baseline_total = baseline
        .iter()
        .map(|record| record.parse_ns)
        .collect::<Vec<_>>();
    let baseline_cpct = baseline
        .iter()
        .map(|record| record.cpct_ns)
        .collect::<Option<Vec<_>>>();
    let search_effect = work_effect(generated, popped, baseline_generated, baseline_popped);
    let runtime_effect = compare_effect(median(&baseline_total), median(&total_values));
    let profile = group[0].build_profile.as_str();
    let timeouts = group.iter().filter(|record| record.timeout).count();
    let costs = unique_join(group.iter().map(|record| record.minimum_costs.as_str()));
    let repairs = unique_join(group.iter().map(|record| record.repair_counts.as_str()));

    let mut fields = vec![
        tsv_field(case_name),
        dominance.to_string(),
        localization.to_string(),
        trials.to_string(),
        warmups.to_string(),
        profile.to_string(),
        group[0].target_platform.clone(),
        timeouts.to_string(),
        display_optional(generated),
        display_optional(popped),
        display_optional(merged),
        display_optional(discarded),
        display_optional(dominance_candidates),
        display_optional(dominance_discards),
        costs,
        repairs,
        display_stat(median(&total_values)),
        display_stat(mean(&total_values)),
        display_stat(stddev(&total_values)),
        display_integer(*total_values.iter().min().unwrap()),
        display_integer(*total_values.iter().max().unwrap()),
        cpct_values
            .as_ref()
            .map(|values| display_stat(median(values)))
            .unwrap_or_else(|| "N/A".to_string()),
        cpct_values
            .as_ref()
            .map(|values| display_stat(mean(values)))
            .unwrap_or_else(|| "N/A".to_string()),
        cpct_values
            .as_ref()
            .map(|values| display_stat(stddev(values)))
            .unwrap_or_else(|| "N/A".to_string()),
        cpct_values
            .as_ref()
            .map(|values| display_integer(*values.iter().min().unwrap()))
            .unwrap_or_else(|| "N/A".to_string()),
        cpct_values
            .as_ref()
            .map(|values| display_integer(*values.iter().max().unwrap()))
            .unwrap_or_else(|| "N/A".to_string()),
        reduction_percent(baseline_generated, generated),
        reduction_percent(baseline_popped, popped),
        change_percent(median(&baseline_total), median(&total_values)),
        match (baseline_cpct.as_ref(), cpct_values.as_ref()) {
            (Some(base), Some(current)) => change_percent(median(base), median(current)),
            _ => "N/A".to_string(),
        },
        display_optional(localization_priority_pops),
        display_optional(localization_priority_reorders),
        group
            .iter()
            .filter_map(|record| record.localization_priority_reorders)
            .next()
            .map(|_| {
                group.iter().any(|record| {
                    record
                        .localization_priority_reorders
                        .is_some_and(|count| count > 0)
                })
            })
            .map(|changed| changed.to_string())
            .unwrap_or_else(|| "N/A".to_string()),
        search_effect,
        runtime_effect,
    ];
    if phase6 {
        let equivalent_trials = group
            .iter()
            .filter(|record| record.differential_status.as_deref() == Some("equivalent"))
            .count();
        let incomplete_trials = group
            .iter()
            .filter(|record| record.differential_status.as_deref() == Some("incomplete_timeout"))
            .count();
        fields.extend([equivalent_trials.to_string(), incomplete_trials.to_string()]);
    }
    debug_assert_eq!(
        fields.len(),
        SUMMARY_COLUMNS.len()
            + if phase6 {
                PHASE6_SUMMARY_EXTRA_COLUMNS.len()
            } else {
                0
            }
    );
    Ok(fields.join("\t"))
}

fn parse_raw_records(
    raw: &str,
    cases: &[CorpusCase],
    phase6: bool,
) -> Result<Vec<TrialRecord>, String> {
    let mut lines = raw.lines();
    if lines.next() != Some(raw_tsv_header(phase6).as_str()) {
        return Err("benchmark TSV header did not match the expected schema".to_string());
    }
    let mut records = Vec::new();
    for line in lines {
        let fields = line.split('\t').map(str::to_string).collect::<Vec<_>>();
        if fields.len() != TSV_COLUMNS.len() + if phase6 { 1 } else { 0 } {
            return Err("benchmark TSV row has an unexpected number of columns".to_string());
        }
        let case_index = cases
            .iter()
            .position(|case| case.name == fields[0])
            .ok_or_else(|| format!("unknown benchmark case {:?}", fields[0]))?;
        let mode_index = MODE_LABELS
            .iter()
            .position(|(dominance, localization, _, _)| {
                *dominance == fields[6] && *localization == fields[7]
            })
            .ok_or_else(|| "unknown benchmark mode in TSV".to_string())?;
        let trial = fields[3]
            .parse()
            .map_err(|_| "invalid trial index in benchmark TSV".to_string())?;
        let differential_status = if phase6 {
            let status = fields[TSV_COLUMNS.len()].clone();
            if status != "equivalent" && status != "incomplete_timeout" {
                return Err("invalid Phase 6 differential status in benchmark TSV".to_string());
            }
            Some(status)
        } else {
            None
        };
        records.push(TrialRecord {
            case_index,
            mode_index,
            trial,
            build_profile: fields[8].clone(),
            target_platform: fields[47].clone(),
            parse_ns: parse_u128(&fields[16])?,
            cpct_ns: optional_u128(&fields[17])?,
            generated: optional_u64(&fields[18])?,
            popped: optional_u64(&fields[19])?,
            merged: optional_u64(&fields[20])?,
            discarded: optional_u64(&fields[21])?,
            dominance_candidates: optional_u64(&fields[27])?,
            dominance_discards: optional_u64(&fields[29])?,
            localization_priority_pops: optional_u64(&fields[43])?,
            localization_priority_reorders: optional_u64(&fields[44])?,
            minimum_costs: fields[31].clone(),
            repair_counts: fields[32].clone(),
            timeout: fields[33] == "true",
            differential_status,
            fields,
        });
    }
    Ok(records)
}

fn counter_mean(group: &[&TrialRecord], get: fn(&TrialRecord) -> Option<u64>) -> Option<f64> {
    let values = group
        .iter()
        .map(|record| get(record))
        .collect::<Option<Vec<_>>>()?;
    Some(values.iter().map(|value| *value as f64).sum::<f64>() / values.len() as f64)
}

fn unique_join<'a>(values: impl Iterator<Item = &'a str>) -> String {
    values
        .flat_map(|value| value.split(';'))
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(";")
}

fn mean(values: &[u128]) -> f64 {
    values.iter().map(|value| *value as f64).sum::<f64>() / values.len() as f64
}

fn median(values: &[u128]) -> f64 {
    let mut ordered = values.to_vec();
    ordered.sort_unstable();
    let middle = ordered.len() / 2;
    if ordered.len() % 2 == 0 {
        (ordered[middle - 1] as f64 + ordered[middle] as f64) / 2.0
    } else {
        ordered[middle] as f64
    }
}

fn stddev(values: &[u128]) -> f64 {
    let average = mean(values);
    (values
        .iter()
        .map(|value| (*value as f64 - average).powi(2))
        .sum::<f64>()
        / values.len() as f64)
        .sqrt()
}

fn display_optional(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.2}"))
        .unwrap_or_else(|| "N/A".to_string())
}

fn display_stat(value: f64) -> String {
    format!("{value:.2}")
}

fn display_integer(value: u128) -> String {
    value.to_string()
}

fn reduction_percent(off: Option<f64>, on: Option<f64>) -> String {
    match (off, on) {
        (Some(off), Some(on)) if off > 0.0 => format!("{:.2}", (off - on) / off * 100.0),
        _ => "N/A".to_string(),
    }
}

fn change_percent(off: f64, on: f64) -> String {
    if off <= 0.0 {
        "N/A".to_string()
    } else {
        format!("{:.2}", (on - off) / off * 100.0)
    }
}

fn work_effect(
    generated: Option<f64>,
    popped: Option<f64>,
    base_generated: Option<f64>,
    base_popped: Option<f64>,
) -> String {
    match (generated, popped, base_generated, base_popped) {
        (Some(generated), Some(popped), Some(base_generated), Some(base_popped)) => match (
            generated.total_cmp(&base_generated),
            popped.total_cmp(&base_popped),
        ) {
            (std::cmp::Ordering::Less, std::cmp::Ordering::Less)
            | (std::cmp::Ordering::Less, std::cmp::Ordering::Equal)
            | (std::cmp::Ordering::Equal, std::cmp::Ordering::Less) => "less_work",
            (std::cmp::Ordering::Equal, std::cmp::Ordering::Equal) => "no_work_change",
            (std::cmp::Ordering::Greater, std::cmp::Ordering::Greater)
            | (std::cmp::Ordering::Greater, std::cmp::Ordering::Equal)
            | (std::cmp::Ordering::Equal, std::cmp::Ordering::Greater) => "more_work",
            _ => "mixed_work",
        }
        .to_string(),
        _ => "N/A".to_string(),
    }
}

fn compare_effect(base: f64, current: f64) -> String {
    match current.total_cmp(&base) {
        std::cmp::Ordering::Less => "faster",
        std::cmp::Ordering::Equal => "no_runtime_change",
        std::cmp::Ordering::Greater => "slower",
    }
    .to_string()
}

fn parse_u128(value: &str) -> Result<u128, String> {
    value
        .parse()
        .map_err(|_| format!("invalid numeric benchmark value {value:?}"))
}

fn optional_u128(value: &str) -> Result<Option<u128>, String> {
    if value.is_empty() {
        Ok(None)
    } else {
        parse_u128(value).map(Some)
    }
}

fn optional_u64(value: &str) -> Result<Option<u64>, String> {
    if value.is_empty() {
        Ok(None)
    } else {
        value
            .parse()
            .map(Some)
            .map_err(|_| format!("invalid numeric benchmark value {value:?}"))
    }
}

fn tsv_field(value: &str) -> String {
    if value.contains(['\t', '\n', '\r', '"']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::{BaselineStatus, RecoveryOptions, parse_baseline_with_recovery_stats_and_options};

    use super::{
        CorpusCase, DifferentialObservation, DifferentialResult, MODE_LABELS,
        PHASE6_DEFAULT_TRIALS, PHASE6_DEFAULT_WARMUPS, SUMMARY_COLUMNS, TSV_COLUMNS,
        benchmark_phase6_summary_from_tsv, benchmark_phase6_tsv_with_trials,
        benchmark_summary_from_tsv, benchmark_tsv_with_trials, change_percent,
        check_differential_equivalence, compare_effect, corpus, input_token_count, mean, median,
        mode_options, mode_order, normalized_repair_sets, phase6_corpus, raw_tsv_header,
        reduction_percent, stddev, summary_tsv_header,
    };

    #[test]
    fn corpus_is_fixed_lexically_valid_and_reaches_expected_parser_paths() {
        let cases = corpus();
        let categories = cases
            .iter()
            .map(|case| case.category)
            .collect::<BTreeSet<_>>();
        for category in [
            "valid",
            "missing_close",
            "unexpected_close",
            "mismatched",
            "nested_mixed",
            "multiple_unmatched",
            "delimiter_cascade",
            "ordinary_syntax",
            "equal_cost_repairs",
            "increasing_nesting",
            "increasing_input_size",
        ] {
            assert!(categories.contains(category), "missing category {category}");
        }
        for case in cases {
            for (_, _, dominance, localization) in MODE_LABELS {
                let report = parse_baseline_with_recovery_stats_and_options(
                    &case.input,
                    RecoveryOptions {
                        delimiter_dominance: dominance,
                        delimiter_localization: localization,
                    },
                );
                if !case.expects_recovery {
                    assert_ne!(report.baseline.status, BaselineStatus::Failed, "{case:?}");
                }
                assert_eq!(
                    !report.recovery_stats.is_empty(),
                    case.expects_recovery,
                    "wrong parser path for {case:?}"
                );
            }
        }
    }

    #[test]
    fn exhaustive_corpus_modes_match_costs_and_complete_reported_repair_sets() {
        let mut exhaustive_case_count = 0;
        for case in corpus().into_iter().filter(|case| case.expects_recovery) {
            let reports = MODE_LABELS.map(|(_, _, dominance, localization)| {
                parse_baseline_with_recovery_stats_and_options(
                    &case.input,
                    RecoveryOptions {
                        delimiter_dominance: dominance,
                        delimiter_localization: localization,
                    },
                )
            });
            if reports
                .iter()
                .any(|report| report.recovery_stats.iter().any(|stats| stats.timed_out))
            {
                continue;
            }

            let baseline_costs = reports[0]
                .recovery_stats
                .iter()
                .map(|stats| stats.minimum_recovery_cost)
                .collect::<Vec<_>>();
            let baseline_repairs = normalized_repair_sets(&reports[0].baseline.errors);
            assert!(
                !baseline_costs.is_empty(),
                "no recovery cost for {:?}",
                case.name
            );
            assert!(
                baseline_repairs.iter().all(|repairs| !repairs.is_empty()),
                "incomplete baseline repair report for {:?}",
                case.name
            );
            for report in reports.iter().skip(1) {
                assert_eq!(
                    report
                        .recovery_stats
                        .iter()
                        .map(|stats| stats.minimum_recovery_cost)
                        .collect::<Vec<_>>(),
                    baseline_costs,
                    "minimum cost differs for {:?}",
                    case.name
                );
                assert_eq!(
                    normalized_repair_sets(&report.baseline.errors),
                    baseline_repairs,
                    "complete reported repair set differs for {:?}",
                    case.name
                );
            }
            exhaustive_case_count += 1;
        }
        assert!(exhaustive_case_count > 0);
    }

    #[test]
    fn benchmark_schedule_is_repeatable_and_varies_paired_mode_order() {
        for case_index in 0..corpus().len() {
            for trial in 0..12 {
                assert_eq!(
                    mode_order(case_index, trial, false),
                    mode_order(case_index, trial, false)
                );
            }
        }
        let schedules = (0..12)
            .map(|trial| mode_order(0, trial, false))
            .collect::<BTreeSet<_>>();
        assert!(schedules.len() > 1);
        for (index, (_, _, dominance, localization)) in MODE_LABELS.iter().enumerate() {
            assert_eq!(
                mode_options(index),
                RecoveryOptions {
                    delimiter_dominance: *dominance,
                    delimiter_localization: *localization,
                }
            );
        }
    }

    #[test]
    fn benchmark_emits_complete_trial_rows_and_summary_statistics() {
        let raw = benchmark_tsv_with_trials(2, 1).unwrap();
        let lines = raw.lines().collect::<Vec<_>>();
        assert_eq!(lines[0], TSV_COLUMNS.join("\t"));
        let cases = corpus();
        assert_eq!(lines.len() - 1, cases.len() * MODE_LABELS.len() * 2);
        for row in lines.iter().skip(1) {
            let fields = row.split('\t').collect::<Vec<_>>();
            assert_eq!(fields.len(), TSV_COLUMNS.len());
            assert_eq!(fields[4], "2");
            assert_eq!(fields[5], "1");
            assert!(!fields[8].is_empty());
            assert!(!fields[9].is_empty());
            assert!(!fields[10].is_empty());
            assert!(!fields[16].is_empty());
        }
        for case in &cases {
            for trial in 0..2 {
                let modes = lines
                    .iter()
                    .skip(1)
                    .filter_map(|row| {
                        let fields = row.split('\t').collect::<Vec<_>>();
                        (fields[0] == case.name && fields[3] == trial.to_string())
                            .then(|| (fields[6], fields[7]))
                    })
                    .collect::<BTreeSet<_>>();
                assert_eq!(
                    modes,
                    [("OFF", "OFF"), ("ON", "OFF"), ("OFF", "ON"), ("ON", "ON")]
                        .into_iter()
                        .collect()
                );
            }
        }
        let valid_control = lines
            .iter()
            .skip(1)
            .find(|row| row.starts_with("valid_simple\t"))
            .unwrap()
            .split('\t')
            .collect::<Vec<_>>();
        assert_eq!(valid_control[15], "false");
        assert!(valid_control[17..33].iter().all(|field| field.is_empty()));
        assert_eq!(valid_control[33], "false");

        let summary = benchmark_summary_from_tsv(&raw).unwrap();
        let summary_lines = summary.lines().collect::<Vec<_>>();
        assert_eq!(summary_lines[0], SUMMARY_COLUMNS.join("\t"));
        assert_eq!(
            summary_lines.len() - 1,
            (cases.len() + 1) * MODE_LABELS.len()
        );
        assert!(
            summary_lines
                .last()
                .is_some_and(|line| line.starts_with("__AGGREGATE__\tON\tON\t"))
        );
    }

    #[test]
    fn benchmark_rejects_zero_measured_trials() {
        assert!(benchmark_tsv_with_trials(0, 1).is_err());
    }

    #[test]
    fn phase6_corpus_is_lexically_valid_unique_and_scales_as_declared() {
        assert_eq!(PHASE6_DEFAULT_TRIALS, 15);
        assert_eq!(PHASE6_DEFAULT_WARMUPS, 3);
        let cases = phase6_corpus();
        let names = cases
            .iter()
            .map(|case| case.name.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(names.len(), cases.len());

        for case in &cases {
            assert!(
                input_token_count(&case.input).is_some(),
                "lexically invalid Phase 6 case {:?}",
                case.name
            );
            assert!(case.name.starts_with("phase6_"));
        }

        for depth in [64, 128, 256] {
            let case = cases
                .iter()
                .find(|case| case.name == format!("phase6_valid_nesting_depth_{depth}"))
                .unwrap();
            assert_eq!(case.input.matches('(').count(), depth);
            assert_eq!(case.input.matches(')').count(), depth);
            assert!(!case.expects_recovery);
        }
        for depth in [8, 16, 32, 64] {
            let case = cases
                .iter()
                .find(|case| case.name == format!("phase6_missing_close_depth_{depth}"))
                .unwrap();
            assert_eq!(case.input.matches('(').count(), depth);
            assert_eq!(case.input.matches(')').count(), 0);
            assert!(case.expects_recovery);
        }
        for terms in [48, 96, 192] {
            assert!(cases.iter().any(|case| {
                case.name == format!("phase6_valid_expression_terms_{terms}")
                    && !case.expects_recovery
            }));
            assert!(cases.iter().any(|case| {
                case.name == format!("phase6_missing_close_expression_terms_{terms}")
                    && case.expects_recovery
            }));
        }
        assert!(
            cases
                .iter()
                .all(|case| { case.name.starts_with("phase6_valid_") || case.expects_recovery })
        );
    }

    #[test]
    fn phase6_benchmark_emits_paired_rows_and_round_trips_summary() {
        let raw = benchmark_phase6_tsv_with_trials(1, 0).unwrap();
        let lines = raw.lines().collect::<Vec<_>>();
        assert_eq!(lines[0], raw_tsv_header(true));
        assert_eq!(lines[1].split('\t').count(), TSV_COLUMNS.len() + 1);
        let cases = phase6_corpus();
        assert_eq!(lines.len() - 1, cases.len() * MODE_LABELS.len());
        for case in &cases {
            let modes = lines
                .iter()
                .skip(1)
                .filter_map(|row| {
                    let fields = row.split('\t').collect::<Vec<_>>();
                    (fields[0] == case.name).then(|| (fields[6], fields[7]))
                })
                .collect::<BTreeSet<_>>();
            assert_eq!(
                modes,
                [("OFF", "OFF"), ("ON", "OFF"), ("OFF", "ON"), ("ON", "ON")]
                    .into_iter()
                    .collect()
            );
        }

        let summary = benchmark_phase6_summary_from_tsv(&raw).unwrap();
        let summary_lines = summary.lines().collect::<Vec<_>>();
        assert_eq!(summary_lines[0], summary_tsv_header(true));
        assert_eq!(
            summary_lines.len() - 1,
            (cases.len() + 1) * MODE_LABELS.len()
        );
        assert!(
            benchmark_summary_from_tsv(&raw).is_err(),
            "Phase 4 summary parser must reject Phase 6 corpus rows"
        );

        let mut incomplete_rows = raw
            .lines()
            .map(|line| line.split('\t').map(str::to_string).collect::<Vec<_>>())
            .collect::<Vec<_>>();
        let incomplete_start = 1 + 3 * MODE_LABELS.len();
        for row in incomplete_rows
            .iter_mut()
            .skip(incomplete_start)
            .take(MODE_LABELS.len())
        {
            row[48] = "incomplete_timeout".to_string();
        }
        incomplete_rows[incomplete_start][33] = "true".to_string();
        let incomplete_raw = incomplete_rows
            .iter()
            .map(|row| row.join("\t"))
            .collect::<Vec<_>>()
            .join("\n");
        let incomplete_summary = benchmark_phase6_summary_from_tsv(&incomplete_raw).unwrap();
        let incomplete_case_rows = incomplete_summary
            .lines()
            .skip(1)
            .filter(|line| line.starts_with("phase6_missing_close_depth_8\t"))
            .map(|line| line.split('\t').collect::<Vec<_>>())
            .collect::<Vec<_>>();
        assert_eq!(incomplete_case_rows.len(), MODE_LABELS.len());
        assert!(incomplete_case_rows.iter().all(|row| row[36] == "1"));
    }

    #[test]
    fn phase6_differential_validation_marks_timeout_incomplete() {
        let case = CorpusCase {
            name: "phase6_timeout_test".to_string(),
            category: "phase6_test",
            input: "1+*".to_string(),
            expects_recovery: true,
        };
        let completed = DifferentialObservation {
            minimum_costs: vec![Some(1)],
            repair_sets: vec![vec!["delete *".to_string()]],
            timed_out: false,
        };
        let mut observations = vec![Some(completed.clone()); MODE_LABELS.len()];
        assert_eq!(
            check_differential_equivalence(&case, &observations).unwrap(),
            DifferentialResult::Equivalent
        );

        observations[2] = Some(DifferentialObservation {
            minimum_costs: vec![Some(2)],
            repair_sets: vec![vec!["different repair".to_string()]],
            timed_out: true,
        });
        assert_eq!(
            check_differential_equivalence(&case, &observations).unwrap(),
            DifferentialResult::IncompleteTimeout
        );

        observations[2] = Some(DifferentialObservation {
            minimum_costs: vec![Some(2)],
            repair_sets: vec![vec!["different repair".to_string()]],
            timed_out: false,
        });
        assert!(check_differential_equivalence(&case, &observations).is_err());
    }

    #[test]
    fn runtime_distribution_uses_median_mean_and_population_standard_deviation() {
        let samples = [1, 2, 3];
        assert_eq!(mean(&samples), 2.0);
        assert_eq!(median(&samples), 2.0);
        assert!((stddev(&samples) - (2.0_f64 / 3.0).sqrt()).abs() < f64::EPSILON);
        assert_eq!(median(&[1, 3]), 2.0);
    }

    #[test]
    fn summary_percentage_calculations_preserve_values_and_mark_unavailable() {
        assert_eq!(reduction_percent(Some(10.0), Some(7.0)), "30.00");
        assert_eq!(reduction_percent(Some(0.0), Some(0.0)), "N/A");
        assert_eq!(reduction_percent(None, Some(1.0)), "N/A");
        assert_eq!(change_percent(100.0, 125.0), "25.00");
        assert_eq!(change_percent(0.0, 1.0), "N/A");
        assert_eq!(compare_effect(100.0, 125.0), "slower");
        assert_eq!(compare_effect(100.0, 75.0), "faster");
        assert_eq!(compare_effect(100.0, 100.0), "no_runtime_change");
    }
}
