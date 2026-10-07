use std::{env, io};

use DelimiterCPCT::{
    BaselineStatus, delimiter::DelimiterReport, parse_baseline,
    parse_baseline_with_delimiter_observation,
};

fn main() {
    let mut observe = false;
    let input = env::args()
        .skip(1)
        .filter(|arg| {
            if arg == "--observe-delimiters" {
                observe = true;
                false
            } else {
                true
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
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
        eprintln!("Usage: cargo run -- [--observe-delimiters] \"1+2*3\"");
        return;
    }

    if observe {
        let report = parse_baseline_with_delimiter_observation(&input);
        print_baseline(&report.baseline);
        print_delimiters(&report.delimiters);
    } else {
        let report = parse_baseline(&input);
        print_baseline(&report);
    }
}

fn print_baseline(report: &DelimiterCPCT::BaselineReport) {
    println!("Baseline: unmodified lrpar CPCT+");
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
