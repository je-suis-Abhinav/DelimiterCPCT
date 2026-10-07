use std::{env, io};

use DelimiterCPCT::{
    BaselineStatus, RecoveryOptions, delimiter::DelimiterReport, parse_baseline_with_options,
    parse_baseline_with_recovery_stats_and_options,
};

fn main() {
    let mut run_benchmark = false;
    let mut observe = false;
    let mut show_recovery_stats = false;
    let mut delimiter_dominance = false;
    let input = env::args()
        .skip(1)
        .filter(|arg| match arg.as_str() {
            "--benchmark" => {
                run_benchmark = true;
                false
            }
            "--observe-delimiters" => {
                observe = true;
                false
            }
            "--recovery-stats" => {
                show_recovery_stats = true;
                false
            }
            "--delimiter-dominance" => {
                delimiter_dominance = true;
                false
            }
            _ => true,
        })
        .collect::<Vec<_>>()
        .join(" ");
    if run_benchmark {
        print!("{}", DelimiterCPCT::benchmark::benchmark_tsv());
        return;
    }
    let input = if input.is_empty() {
        let mut buf = String::new();
        io::stdin()
            .read_line(&mut buf)
            .expect("failed to read expression from stdin");
        buf.trim_end().to_string()
    } else {
        input
    };

    if input.trim().is_empty() {
        eprintln!(
            "Usage: cargo run -- [--observe-delimiters] [--recovery-stats] [--delimiter-dominance] \"1+2*3\""
        );
        return;
    }

    let options = RecoveryOptions {
        delimiter_dominance,
    };
    if show_recovery_stats {
        let report = parse_baseline_with_recovery_stats_and_options(&input, options);
        print_baseline(&report.baseline, delimiter_dominance);
        print_recovery_stats(&report.recovery_stats);
        if observe {
            print_delimiters(&DelimiterCPCT::delimiter::observe_delimiters(&input));
        }
    } else if observe {
        let report = parse_baseline_with_options(&input, options);
        print_baseline(&report, delimiter_dominance);
        print_delimiters(&DelimiterCPCT::delimiter::observe_delimiters(&input));
    } else {
        let report = parse_baseline_with_options(&input, options);
        print_baseline(&report, delimiter_dominance);
    }
}

fn print_recovery_stats(stats: &[lrpar::RecoveryStats]) {
    if stats.is_empty() {
        println!("CPCT+ recovery statistics: none");
        return;
    }

    for (index, recovery) in stats.iter().enumerate() {
        println!("CPCT+ recovery statistics #{}:", index + 1);
        println!(
            "  configurations: generated={}, popped={}, merged={}, discarded={} (cost={}, timeout={}, higher_cost={}, duplicate={}, dominance={})",
            recovery.configurations_generated,
            recovery.configurations_popped,
            recovery.configurations_merged,
            recovery.configurations_discarded,
            recovery.configurations_discarded_cost,
            recovery.configurations_discarded_timeout,
            recovery.configurations_discarded_higher_cost,
            recovery.configurations_discarded_duplicate,
            recovery.configurations_discarded_dominance
        );
        println!(
            "  transition attempts: insert={}, delete={}, shift/reduction={}",
            recovery.insert_transitions_attempted,
            recovery.delete_transitions_attempted,
            recovery.shift_transitions_attempted
        );
        println!(
            "  repair sequences: collected={:?}, after_continuation={:?}, after_simplification={:?}",
            recovery.repair_sequences_after_collection,
            recovery.repair_sequences_after_continuation,
            recovery.repair_sequences_after_simplification
        );
        println!(
            "  minimum_recovery_cost={:?}, timed_out={}, elapsed_ns={}",
            recovery.minimum_recovery_cost,
            recovery.timed_out,
            recovery.elapsed.as_nanos()
        );
        println!(
            "  delimiter scores: popped={:?}, minimum_cost_success={:?}, selected_repair={:?}",
            recovery.delimiter_score_distribution,
            recovery.minimum_cost_success_delimiter_scores,
            recovery.selected_repair_delimiter_score
        );
        println!(
            "  strict-cost delimiter dominance: enabled={}, checked={}, exact_key_matches={}, variants_discarded={}, discarded_cost_difference={}",
            recovery.delimiter_dominance_enabled,
            recovery.dominance_candidates_checked,
            recovery.dominance_exact_key_matches,
            recovery.dominance_variants_discarded,
            recovery.dominance_discarded_cost_difference
        );
    }
}

fn print_baseline(report: &DelimiterCPCT::BaselineReport, delimiter_dominance: bool) {
    if delimiter_dominance {
        println!("Parser mode: experimental strict-cost delimiter-dominance CPCT+");
    } else {
        println!("Baseline: unmodified lrpar CPCT+");
    }
    println!("Input: {}", report.input);
    match &report.status {
        BaselineStatus::Accepted => println!("Status: successful parse"),
        BaselineStatus::Recovered => println!("Status: parse recovered by CPCT+"),
        BaselineStatus::RecoveryReported => {
            println!("Status: CPCT+ repair sequence reported; semantic value unavailable")
        }
        BaselineStatus::Failed => println!("Status: parse failed"),
    }

    match &report.value {
        Some(value) => println!("Value: {value}"),
        None => println!("Value: <unavailable>"),
    }

    if report.errors.is_empty() {
        println!("Parser errors: none");
    } else {
        println!("Parser errors:");
        for error in &report.errors {
            println!("{error}");
        }
    }
}

fn print_delimiters(report: &DelimiterReport) {
    println!("Delimiter observation: enabled");
    println!("Delimiter checkpoints: {}", report.checkpoints.len());
    println!("Matched pairs: {}", report.matches.len());
    println!("Unmatched openings: {}", report.unmatched_openings.len());
    println!("Unexpected closings: {}", report.unexpected_closings.len());
    println!("Mismatched closings: {}", report.mismatched_closings.len());
    println!("Final depth: {}", report.final_depth);
    println!("Max depth: {}", report.max_depth);

    if report.checkpoints.is_empty() {
        println!("Checkpoints: none");
    } else {
        println!("Checkpoints:");
        for checkpoint in report.checkpoint_summaries() {
            println!("{checkpoint}");
        }
    }
}
