use std::time::Instant;

use crate::{
    RecoveryOptions, delimiter::observe_delimiters, localization::localize_structure,
    parse_baseline_with_recovery_stats_and_options,
};

#[derive(Clone, Copy)]
struct CorpusCase {
    name: &'static str,
    input: &'static str,
}

const CORPUS: &[CorpusCase] = &[
    CorpusCase {
        name: "valid_simple_control",
        input: "1+2*3",
    },
    CorpusCase {
        name: "valid_nested_control",
        input: "((1+2)*3)",
    },
    CorpusCase {
        name: "missing_closing_paren",
        input: "(1+2",
    },
    CorpusCase {
        name: "missing_opening_paren",
        input: "1+2)",
    },
    CorpusCase {
        name: "mismatched_bracket",
        input: "(1+2]",
    },
    CorpusCase {
        name: "nested_unmatched_parens",
        input: "((1+2",
    },
    CorpusCase {
        name: "multiple_unmatched_delimiters",
        input: "({[1+2",
    },
    CorpusCase {
        name: "operator_error",
        input: "1+*",
    },
    CorpusCase {
        name: "delimiter_heavy_cascade",
        input: "(((1+*2",
    },
    CorpusCase {
        name: "mismatch_and_parser_error",
        input: "([1+*2}",
    },
];

const TSV_HEADER: &str = "case\tmode\tinput\tinput_size_bytes\tdelimiter_count\tmax_depth\tparser_status\tsuccess\tparse_runtime_ns\tcpct_elapsed_ns\tgenerated\tpopped\tmerged\tdiscarded\tdominance_discards\tminimum_recovery_cost\trepair_sequence_count\ttimeout\tparser_error_byte\tlocalization_data";
const SUMMARY_HEADER: &str = "case\tOFF_generated\tON_generated\tgenerated_reduction_pct\tOFF_popped\tON_popped\tpopped_reduction_pct\tON_dominance_discards\tOFF_minimum_recovery_cost\tON_minimum_recovery_cost\tOFF_repair_count\tON_repair_count\tOFF_parse_runtime_ns\tON_parse_runtime_ns\truntime_change_pct";

/// Run the fixed baseline-vs-dominance corpus and return a tab-separated report.
///
/// Wall time is measured around the existing parse-and-statistics API. Search
/// counters and CPCT+ elapsed time come directly from `RecoveryStats`.
pub fn benchmark_tsv() -> String {
    let mut output = String::from(TSV_HEADER);
    output.push('\n');
    for case in CORPUS {
        for (mode, delimiter_dominance) in [("OFF", false), ("ON", true)] {
            let options = RecoveryOptions {
                delimiter_dominance,
            };
            let started = Instant::now();
            let report = parse_baseline_with_recovery_stats_and_options(case.input, options);
            let parse_runtime_ns = started.elapsed().as_nanos();
            let delimiters = observe_delimiters(case.input);
            let localization = localize_structure(&report.baseline, &delimiters);
            let stats = &report.recovery_stats;

            let generated = sum_stat(stats, |s| s.configurations_generated);
            let popped = sum_stat(stats, |s| s.configurations_popped);
            let merged = sum_stat(stats, |s| s.configurations_merged);
            let discarded = sum_stat(stats, |s| s.configurations_discarded);
            let dominance_discards = sum_stat(stats, |s| s.configurations_discarded_dominance);
            let cpct_elapsed_ns = sum_stat(stats, |s| s.elapsed.as_nanos());
            let minimum_cost = list_stat(stats, |s| s.minimum_recovery_cost);
            let repair_sequences = list_stat(stats, |s| s.repair_sequences_after_simplification);
            let timeout = stats.iter().any(|s| s.timed_out);
            let parser_error_byte = report
                .baseline
                .error_positions
                .first()
                .map(|position| position.byte_offset.to_string())
                .unwrap_or_default();
            let localization_data = localization
                .candidates
                .iter()
                .map(|candidate| {
                    let distance = candidate
                        .distance_from_parser_error
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "NA".to_string());
                    format!(
                        "{:?}:{}:byte{}:depth{}:distance{}",
                        candidate.reason,
                        candidate.delimiter.open_char(),
                        candidate.event_position.byte_offset,
                        candidate.depth,
                        distance
                    )
                })
                .collect::<Vec<_>>()
                .join(";");

            let columns = [
                tsv_field(case.name),
                mode.to_string(),
                tsv_field(case.input),
                case.input.len().to_string(),
                (delimiters.checkpoints.len()
                    + delimiters.matches.len()
                    + delimiters.unexpected_closings.len()
                    + delimiters.mismatched_closings.len())
                .to_string(),
                delimiters.max_depth.to_string(),
                format!("{:?}", report.baseline.status),
                report.baseline.value.is_some().to_string(),
                parse_runtime_ns.to_string(),
                cpct_elapsed_ns,
                generated,
                popped,
                merged,
                discarded,
                dominance_discards,
                minimum_cost,
                repair_sequences,
                timeout.to_string(),
                parser_error_byte,
                tsv_field(&localization_data),
            ];
            output.push_str(&columns.join("\t"));
            output.push('\n');
        }
    }
    output
}

/// Summarize the exact benchmark TSV data without running additional parses.
pub fn benchmark_summary_tsv() -> String {
    summarize_benchmark_tsv(&benchmark_tsv())
}

fn summarize_benchmark_tsv(input: &str) -> String {
    let mut rows = Vec::<(String, Option<Vec<String>>, Option<Vec<String>>)>::new();
    for line in input.lines().skip(1) {
        let fields = line.split('\t').map(str::to_owned).collect::<Vec<_>>();
        if fields.len() != TSV_HEADER.split('\t').count() {
            continue;
        }
        let index = rows
            .iter()
            .position(|(case, _, _)| *case == fields[0])
            .unwrap_or_else(|| {
                rows.push((fields[0].clone(), None, None));
                rows.len() - 1
            });
        let (_, off, on) = &mut rows[index];
        match fields[1].as_str() {
            "OFF" => *off = Some(fields),
            "ON" => *on = Some(fields),
            _ => {}
        }
    }

    let mut output = String::from(SUMMARY_HEADER);
    output.push('\n');
    for (case, off, on) in rows {
        let (Some(off), Some(on)) = (off, on) else {
            continue;
        };
        let columns = [
            case,
            field_or_na(&off[10]),
            field_or_na(&on[10]),
            reduction_percent(&off[10], &on[10]),
            field_or_na(&off[11]),
            field_or_na(&on[11]),
            reduction_percent(&off[11], &on[11]),
            field_or_na(&on[14]),
            list_or_na(&off[15]),
            list_or_na(&on[15]),
            list_or_na(&off[16]),
            list_or_na(&on[16]),
            field_or_na(&off[8]),
            field_or_na(&on[8]),
            change_percent(&off[8], &on[8]),
        ];
        output.push_str(&columns.join("\t"));
        output.push('\n');
    }
    output
}

fn field_or_na(value: &str) -> String {
    if value.is_empty() {
        "N/A".to_string()
    } else {
        value.to_string()
    }
}

fn list_or_na(value: &str) -> String {
    if value.is_empty() {
        return "N/A".to_string();
    }
    value
        .split(';')
        .map(field_or_na)
        .collect::<Vec<_>>()
        .join(";")
}

fn reduction_percent(off: &str, on: &str) -> String {
    let (Ok(off), Ok(on)) = (off.parse::<u128>(), on.parse::<u128>()) else {
        return "N/A".to_string();
    };
    if off == 0 {
        return "N/A".to_string();
    }
    format!("{:.2}", (off as f64 - on as f64) / off as f64 * 100.0)
}

fn change_percent(off: &str, on: &str) -> String {
    let (Ok(off), Ok(on)) = (off.parse::<u128>(), on.parse::<u128>()) else {
        return "N/A".to_string();
    };
    if off == 0 {
        return "N/A".to_string();
    }
    format!("{:.2}", (on as f64 - off as f64) / off as f64 * 100.0)
}

fn sum_stat<T>(stats: &[lrpar::RecoveryStats], value: impl Fn(&lrpar::RecoveryStats) -> T) -> String
where
    T: Into<u128>,
{
    if stats.is_empty() {
        String::new()
    } else {
        stats
            .iter()
            .map(|stat| value(stat).into())
            .sum::<u128>()
            .to_string()
    }
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

fn tsv_field(value: &str) -> String {
    if value.contains(['\t', '\n', '\r', '"']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{CORPUS, SUMMARY_HEADER, TSV_HEADER, benchmark_tsv, summarize_benchmark_tsv};

    #[test]
    fn benchmark_output_has_stable_schema_and_ordered_off_on_pairs() {
        let output = benchmark_tsv();
        let mut lines = output.lines();
        assert_eq!(lines.next(), Some(TSV_HEADER));
        let rows = lines.collect::<Vec<_>>();
        assert_eq!(rows.len(), CORPUS.len() * 2);

        for (case, pair) in CORPUS.iter().zip(rows.chunks_exact(2)) {
            let off = pair[0].split('\t').collect::<Vec<_>>();
            let on = pair[1].split('\t').collect::<Vec<_>>();
            assert_eq!(off.len(), TSV_HEADER.split('\t').count());
            assert_eq!(on.len(), TSV_HEADER.split('\t').count());
            assert_eq!(off[0], case.name);
            assert_eq!(on[0], case.name);
            assert_eq!(off[1], "OFF");
            assert_eq!(on[1], "ON");
            assert_eq!(off[2], case.input);
            assert_eq!(on[2], case.input);
            assert_eq!(off[3], case.input.len().to_string());
            assert_eq!(on[3], case.input.len().to_string());
        }
    }

    #[test]
    fn benchmark_reports_unavailable_recovery_metrics_as_empty_cells() {
        let output = benchmark_tsv();
        let valid_control = output
            .lines()
            .find(|line| line.starts_with("valid_simple_control\tOFF\t"))
            .unwrap();
        let fields = valid_control.split('\t').collect::<Vec<_>>();
        assert_eq!(fields[8].is_empty(), false);
        assert!(fields[9].is_empty());
        assert!(fields[10].is_empty());
        assert!(fields[11].is_empty());
        assert!(fields[15].is_empty());
        assert!(fields[16].is_empty());
        assert_eq!(fields[17], "false");
    }

    #[test]
    fn summary_calculations_preserve_values_and_mark_unavailable_metrics() {
        let mut off = vec![""; 20];
        off[0] = "case_a";
        off[1] = "OFF";
        off[8] = "100";
        off[10] = "10";
        off[11] = "4";
        off[14] = "0";
        off[15] = "2";
        off[16] = "3";
        let mut on = off.clone();
        on[1] = "ON";
        on[8] = "125";
        on[10] = "7";
        on[11] = "3";
        on[14] = "2";
        on[15] = "2";
        on[16] = "2";

        let mut missing_off = vec![""; 20];
        missing_off[0] = "case_b";
        missing_off[1] = "OFF";
        let mut missing_on = missing_off.clone();
        missing_on[1] = "ON";
        let input = format!(
            "{TSV_HEADER}\n{}\n{}\n{}\n{}",
            off.join("\t"),
            on.join("\t"),
            missing_off.join("\t"),
            missing_on.join("\t")
        );
        let summary = summarize_benchmark_tsv(&input);
        let mut lines = summary.lines();
        assert_eq!(lines.next(), Some(SUMMARY_HEADER));
        assert_eq!(
            lines.next(),
            Some("case_a\t10\t7\t30.00\t4\t3\t25.00\t2\t2\t2\t3\t2\t100\t125\t25.00")
        );
        assert_eq!(
            lines.next(),
            Some("case_b\tN/A\tN/A\tN/A\tN/A\tN/A\tN/A\tN/A\tN/A\tN/A\tN/A\tN/A\tN/A\tN/A\tN/A")
        );
        assert!(lines.next().is_none());
    }
}
