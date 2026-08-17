//! ctxgrep — context-budgeted code search for AI agents.
//!
//!   ctxgrep <pattern> [path] [--max-tokens N] [--glob G]...
//!   ctxgrep --mcp [--root DIR]
//!   ctxgrep --tools            # print the tool schema as JSON

mod budget;
mod mcp;
mod search;

use search::Options;
use std::io::{self, BufReader};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
ctxgrep — context-budgeted code search for AI agents

  ctxgrep <pattern> [path] [options]     search and print
  ctxgrep --mcp [--root DIR]             run as an MCP server over stdio
  ctxgrep --tools                        print the MCP tool schema as JSON

Options:
  --max-tokens N     token ceiling for results (default 2000)
  --max-per-file N   cap matches shown per file (default 4)
  --glob G           include/exclude glob, repeatable (e.g. '*.rs', '!target/**')
  --case-sensitive   default is case-insensitive
  -h, --help         this text
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|a| a == "-h" || a == "--help") || args.is_empty() {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    let root = || {
        flag_value(&args, "--root")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    };

    if args.iter().any(|a| a == "--tools") {
        let server = mcp::Server::new(root());
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({"tools": server.tools()}))
                .unwrap_or_default()
        );
        return ExitCode::SUCCESS;
    }

    if args.iter().any(|a| a == "--mcp") {
        let server = mcp::Server::new(root());
        if let Err(e) = server.serve(BufReader::new(io::stdin()), io::stdout()) {
            eprintln!("ctxgrep: {e}");
            return ExitCode::FAILURE;
        }
        return ExitCode::SUCCESS;
    }

    // CLI mode. Positional args are anything not a flag and not a flag's value.
    const VALUED: [&str; 4] = ["--max-tokens", "--max-per-file", "--glob", "--root"];
    let positional: Vec<&String> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| {
            if a.starts_with('-') {
                return false;
            }
            let prev = i.checked_sub(1).and_then(|p| args.get(p)).map(String::as_str);
            !matches!(prev, Some(p) if VALUED.contains(&p))
        })
        .map(|(_, a)| a)
        .collect();

    let Some(pattern) = positional.first() else {
        eprintln!("ctxgrep: no pattern given\n\n{USAGE}");
        return ExitCode::FAILURE;
    };

    let opts = Options {
        pattern: (*pattern).clone(),
        path: positional
            .get(1)
            .map(|s| (*s).clone())
            .unwrap_or_else(|| ".".into()),
        max_tokens: flag_value(&args, "--max-tokens")
            .and_then(|v| v.parse().ok())
            .unwrap_or(2000),
        max_per_file: flag_value(&args, "--max-per-file")
            .and_then(|v| v.parse().ok())
            .unwrap_or(4),
        case_sensitive: args.iter().any(|a| a == "--case-sensitive"),
        globs: flag_values(&args, "--glob"),
        ..Default::default()
    };

    match search::search(&opts) {
        Ok(r) => {
            println!("{}", search::render(&r));
            if r.hits.is_empty() {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(e) => {
            eprintln!("ctxgrep: {e}");
            ExitCode::FAILURE
        }
    }
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn flag_values(args: &[String], flag: &str) -> Vec<String> {
    args.iter()
        .enumerate()
        .filter(|(_, a)| a.as_str() == flag)
        .filter_map(|(i, _)| args.get(i + 1).cloned())
        .collect()
}
