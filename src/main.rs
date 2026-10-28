use std::{env, fs, io};

use DelimiterCPCT::{
    BaselineStatus, RecoveryOptions, delimiter::DelimiterReport, parse_baseline_with_options,
    parse_baseline_with_recovery_stats_and_options,
};

fn main() {
    let mut run_benchmark = false;
    let mut run_benchmark_summary = false;
    let mut run_phase6_benchmark = false;
    let mut run_phase6_summary = false;
    let mut observe = false;
    let mut show_recovery_stats = false;
    let mut delimiter_dominance = false;
    let mut delimiter_localization = false;
    let mut benchmark_summary_input = None;
    let mut phase6_summary_input = None;
    let mut benchmark_trials = None;
    let mut benchmark_warmups = None;
    let args = env::args().skip(1).collect::<Vec<_>>();
    let mut input_args = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        match arg.as_str() {
            "--benchmark" => run_benchmark = true,
            "--benchmark-summary" => run_benchmark_summary = true,
            "--benchmark-phase6" => run_phase6_benchmark = true,
            "--benchmark-phase6-summary" => run_phase6_summary = true,
            "--observe-delimiters" => observe = true,
            "--recovery-stats" => show_recovery_stats = true,
            "--delimiter-dominance" => delimiter_dominance = true,
            "--delimiter-localization" => delimiter_localization = true,
            "--benchmark-trials" | "--benchmark-warmups" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("Missing numeric value after {arg}");
                    std::process::exit(2);
                };
                let Ok(value) = value.parse::<usize>() else {
                    eprintln!("Invalid count for {arg}: {value:?}");
                    std::process::exit(2);
                };
                if arg == "--benchmark-trials" {
                    benchmark_trials = Some(value);
                } else {
                    benchmark_warmups = Some(value);
                }
                index += 1;
            }
            "--benchmark-summary-from" => {
                let Some(path) = args.get(index + 1) else {
                    eprintln!("Missing path after {arg}");
                    std::process::exit(2);
                };
                run_benchmark_summary = true;
                benchmark_summary_input = Some(path.clone());
                index += 1;
            }
            "--benchmark-phase6-summary-from" => {
                let Some(path) = args.get(index + 1) else {
                    eprintln!("Missing path after {arg}");
                    std::process::exit(2);
                };
                run_phase6_summary = true;
                phase6_summary_input = Some(path.clone());
                index += 1;
            }
            _ => input_args.push(arg.clone()),
        }
        index += 1;
    }
    let input = input_args.join(" ");
    if run_phase6_benchmark {
        let trials = benchmark_trials.unwrap_or(DelimiterCPCT::benchmark::PHASE6_DEFAULT_TRIALS);
        let warmups = benchmark_warmups.unwrap_or(DelimiterCPCT::benchmark::PHASE6_DEFAULT_WARMUPS);
        let result = DelimiterCPCT::benchmark::benchmark_phase6_tsv_with_trials(trials, warmups)
            .and_then(|raw| {
                let summary = DelimiterCPCT::benchmark::benchmark_phase6_summary_from_tsv(&raw)?;
                fs::create_dir_all("results")
                    .map_err(|error| format!("could not create results directory: {error}"))?;
                fs::write("results/cpct-phase6-raw.tsv", raw)
                    .map_err(|error| format!("could not write Phase 6 raw TSV: {error}"))?;
                fs::write("results/cpct-phase6-summary.tsv", summary)
                    .map_err(|error| format!("could not write Phase 6 summary TSV: {error}"))?;
                Ok(())
            });
        if let Err(error) = result {
            eprintln!("Phase 6 benchmark failed: {error}");
            std::process::exit(1);
        }
        eprintln!("Wrote results/cpct-phase6-raw.tsv and results/cpct-phase6-summary.tsv");
        return;
    }
    if run_phase6_summary {
        let input_path =
            phase6_summary_input.unwrap_or_else(|| "results/cpct-phase6-raw.tsv".to_string());
        let result = fs::read_to_string(&input_path)
            .map_err(|error| format!("could not read Phase 6 raw TSV {input_path:?}: {error}"))
            .and_then(|raw| DelimiterCPCT::benchmark::benchmark_phase6_summary_from_tsv(&raw))
            .and_then(|summary| {
                fs::create_dir_all("results")
                    .map_err(|error| format!("could not create results directory: {error}"))?;
                fs::write("results/cpct-phase6-summary.tsv", summary)
                    .map_err(|error| format!("could not write Phase 6 summary TSV: {error}"))
            });
        if let Err(error) = result {
            eprintln!("Phase 6 summary failed: {error}");
            std::process::exit(1);
        }
        eprintln!("Wrote results/cpct-phase6-summary.tsv");
        return;
    }
    if run_benchmark_summary {
        let summary = if let Some(path) = benchmark_summary_input {
            fs::read_to_string(&path)
                .map_err(|error| format!("could not read benchmark TSV {path:?}: {error}"))
                .and_then(|raw| DelimiterCPCT::benchmark::benchmark_summary_from_tsv(&raw))
        } else {
            DelimiterCPCT::benchmark::benchmark_summary_tsv_with_trials(
                benchmark_trials.unwrap_or(5),
                benchmark_warmups.unwrap_or(1),
            )
        };
        match summary {
            Ok(output) => print!("{output}"),
            Err(error) => {
                eprintln!("Benchmark failed: {error}");
                std::process::exit(1);
            }
        }
        return;
    }
    if run_benchmark {
        match DelimiterCPCT::benchmark::benchmark_tsv_with_trials(
            benchmark_trials.unwrap_or(5),
            benchmark_warmups.unwrap_or(1),
        ) {
            Ok(output) => print!("{output}"),
            Err(error) => {
                eprintln!("Benchmark failed: {error}");
                std::process::exit(1);
            }
        }
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
            "Usage: cargo run -- [--observe-delimiters] [--recovery-stats] [--delimiter-dominance] [--delimiter-localization] \"1+2*3\"\n\
             Benchmark: cargo run --release -- --benchmark [--benchmark-trials N] [--benchmark-warmups N]\n\
             Phase 6 benchmark: cargo run --release -- --benchmark-phase6 [--benchmark-trials N] [--benchmark-warmups N]\n\
             Phase 6 summary: cargo run --release -- --benchmark-phase6-summary [--benchmark-phase6-summary-from cpct-phase6-raw.tsv]\n\
             Summary: cargo run --release -- --benchmark-summary-from cpct-raw.tsv"
        );
        return;
    }

    let options = RecoveryOptions {
        delimiter_dominance,
        delimiter_localization,
    };
    if show_recovery_stats {
        let report = parse_baseline_with_recovery_stats_and_options(&input, options);
        print_baseline(
            &report.baseline,
            delimiter_dominance,
            delimiter_localization,
        );
        print_recovery_stats(&report.recovery_stats);
        if observe {
            print_delimiters(&DelimiterCPCT::delimiter::observe_delimiters(&input));
        }
    } else if observe {
        let report = parse_baseline_with_options(&input, options);
        print_baseline(&report, delimiter_dominance, delimiter_localization);
        print_delimiters(&DelimiterCPCT::delimiter::observe_delimiters(&input));
    } else {
        let report = parse_baseline_with_options(&input, options);
        print_baseline(&report, delimiter_dominance, delimiter_localization);
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
        println!(
            "  delimiter localization: enabled={}, target={:?}, checkpoint_id={:?}, delimiter={:?}, origin_input={:?}, origin_byte={:?}, origin_char={:?}, origin_depth={:?}",
            recovery.delimiter_localization_enabled,
            recovery.localization_target,
            recovery.localization_checkpoint_id,
            recovery.localization_delimiter,
            recovery.localization_origin_input_index,
            recovery.localization_origin_byte_index,
            recovery.localization_origin_char_index,
            recovery.localization_origin_depth
        );
        println!(
            "  recovery positions: start_input={:?}, start_byte={:?}, final_repair_byte={:?}, origin_to_start_distance={:?}, origin_to_repair_distance={:?}, start_to_repair_distance={:?}, prioritized_pops={}, reorders={}",
            recovery.recovery_start_input_index,
            recovery.recovery_start_byte_index,
            recovery.final_repair_byte_index,
            recovery.localization_to_recovery_start_distance,
            recovery.localization_to_repair_distance,
            recovery.recovery_start_to_repair_distance,
            recovery.localization_priority_pops,
            recovery.localization_priority_reorders
        );
    }
}

fn print_baseline(
    report: &DelimiterCPCT::BaselineReport,
    delimiter_dominance: bool,
    delimiter_localization: bool,
) {
    println!(
        "Parser mode: CPCT+ (dominance={}, localization={})",
        if delimiter_dominance { "ON" } else { "OFF" },
        if delimiter_localization { "ON" } else { "OFF" }
    );
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
