//! Gitignore-aware parallel search that fits an explicit token budget.
//!
//! The ranking exists because a budget forces a choice. Once you can only
//! return twenty of four hundred matches, *which* twenty is the entire product.
//! A definition beats a call site; source beats vendored code; a shallow path
//! beats one buried six directories down.

use crate::budget::{clip, est_tokens, Budget};
use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};
use std::sync::mpsc;

const MAX_LINE_CHARS: usize = 240;

pub struct Options {
    pub pattern: String,
    pub path: String,
    pub max_tokens: usize,
    pub max_per_file: usize,
    pub case_sensitive: bool,
    pub globs: Vec<String>,
    pub max_filesize: u64,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            pattern: String::new(),
            path: ".".into(),
            max_tokens: 2000,
            max_per_file: 4,
            case_sensitive: false,
            globs: Vec::new(),
            max_filesize: 2 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub path: String,
    pub line_no: u64,
    pub text: String,
    pub score: i64,
}

#[derive(Debug, Default)]
pub struct SearchResult {
    pub hits: Vec<Hit>,
    pub total_matches: usize,
    pub files_matched: usize,
    pub files_shown: usize,
    pub tokens_used: usize,
    pub budget: usize,
    pub truncated: bool,
}

/// Lines that declare something. Worth far more than a line that uses it.
fn is_definition(line: &str) -> bool {
    let t = line.trim_start();
    const KW: [&str; 22] = [
        "fn ", "pub fn", "async fn", "def ", "class ", "struct ", "enum ",
        "impl ", "trait ", "interface ", "type ", "func ", "export ",
        "public ", "private ", "protected ", "const ", "let ", "var ",
        "function ", "module ", "package ",
    ];
    KW.iter().any(|k| t.starts_with(k))
}

/// Paths whose contents are rarely the answer.
fn deprioritised(path: &str) -> bool {
    const NOISE: [&str; 12] = [
        "/test/", "/tests/", "/vendor/", "/node_modules/", "/target/",
        "/__pycache__/", "/dist/", "/build/", ".min.", ".lock", "/fixtures/",
        "/.git/",
    ];
    let p = path.to_lowercase();
    NOISE.iter().any(|n| p.contains(n))
}

fn score(path: &str, line: &str, terms: &[String]) -> i64 {
    let mut s: i64 = 0;
    if is_definition(line) {
        s += 40;
    }
    if deprioritised(path) {
        s -= 35;
    }
    let stem = path.rsplit('/').next().unwrap_or("").to_lowercase();
    if terms.iter().any(|t| !t.is_empty() && stem.contains(t)) {
        s += 20;
    }
    s -= 2 * path.matches('/').count() as i64; // prefer shallow
    s -= (line.trim().len() / 60) as i64; // prefer concise
    s
}

/// Alphanumeric fragments of the pattern, used for filename bonuses.
fn terms_of(pattern: &str) -> Vec<String> {
    pattern
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| t.len() > 2)
        .map(|t| t.to_lowercase())
        .collect()
}

pub fn search(opts: &Options) -> Result<SearchResult, String> {
    let re: Regex = RegexBuilder::new(&opts.pattern)
        .case_insensitive(!opts.case_sensitive)
        .build()
        .map_err(|e| format!("invalid regex: {e}"))?;
    let terms = terms_of(&opts.pattern);

    let mut wb = WalkBuilder::new(&opts.path);
    wb.hidden(true).git_ignore(true).git_global(false);
    if !opts.globs.is_empty() {
        let mut ob = OverrideBuilder::new(&opts.path);
        for g in &opts.globs {
            ob.add(g).map_err(|e| format!("bad glob {g:?}: {e}"))?;
        }
        wb.overrides(ob.build().map_err(|e| format!("bad globs: {e}"))?);
    }

    let (tx, rx) = mpsc::channel::<Vec<Hit>>();
    let max_filesize = opts.max_filesize;
    wb.build_parallel().run(|| {
        let tx = tx.clone();
        let re = re.clone();
        let terms = terms.clone();
        Box::new(move |entry| {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => return ignore::WalkState::Continue,
            };
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                return ignore::WalkState::Continue;
            }
            if entry.metadata().map(|m| m.len() > max_filesize).unwrap_or(false) {
                return ignore::WalkState::Continue;
            }
            let path = entry.path().to_string_lossy().to_string();
            // Binary files fail utf8 decoding; that is our binary check.
            let content = match std::fs::read_to_string(entry.path()) {
                Ok(c) => c,
                Err(_) => return ignore::WalkState::Continue,
            };
            let mut hits = Vec::new();
            for (i, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    hits.push(Hit {
                        path: path.clone(),
                        line_no: (i + 1) as u64,
                        text: clip(line, MAX_LINE_CHARS),
                        score: score(&path, line, &terms),
                    });
                }
            }
            if !hits.is_empty() {
                let _ = tx.send(hits);
            }
            ignore::WalkState::Continue
        })
    });
    drop(tx);

    let per_file: Vec<Vec<Hit>> = rx.iter().collect();
    let files_matched = per_file.len();
    let total_matches: usize = per_file.iter().map(|v| v.len()).sum();

    // Cap each file's contribution before ranking globally, so one dense file
    // cannot crowd out every other file in the result.
    let mut candidates: Vec<Hit> = Vec::new();
    for mut file_hits in per_file {
        file_hits.sort_by(|a, b| b.score.cmp(&a.score).then(a.line_no.cmp(&b.line_no)));
        file_hits.truncate(opts.max_per_file);
        candidates.extend(file_hits);
    }
    candidates.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(a.path.cmp(&b.path))
            .then(a.line_no.cmp(&b.line_no))
    });

    let mut budget = Budget::new(opts.max_tokens);
    let mut kept: Vec<Hit> = Vec::new();
    for h in candidates {
        // Roughly what the rendered line will cost, including its path header.
        let cost = est_tokens(&h.text) + 8;
        if !budget.try_spend(cost) {
            break;
        }
        kept.push(h);
    }

    kept.sort_by(|a, b| a.path.cmp(&b.path).then(a.line_no.cmp(&b.line_no)));
    let mut shown_files: Vec<&String> = kept.iter().map(|h| &h.path).collect();
    shown_files.dedup();
    let files_shown = shown_files.len();

    Ok(SearchResult {
        truncated: kept.len() < total_matches,
        tokens_used: budget.used(),
        budget: budget.limit(),
        files_shown,
        files_matched,
        total_matches,
        hits: kept,
    })
}

/// Render grouped by file, and always state what was left out.
///
/// Silent truncation is the failure this whole tool exists to prevent: a model
/// that believes it has seen every match will confidently conclude something
/// does not exist.
pub fn render(r: &SearchResult) -> String {
    if r.hits.is_empty() {
        return format!(
            "No matches.{}",
            if r.total_matches > 0 {
                format!(
                    " ({} matches found but none fit the {}-token budget — raise max_tokens.)",
                    r.total_matches, r.budget
                )
            } else {
                String::new()
            }
        );
    }
    let mut out = String::new();
    let mut current = "";
    for h in &r.hits {
        if h.path != current {
            if !current.is_empty() {
                out.push('\n');
            }
            out.push_str(&h.path);
            out.push('\n');
            current = &h.path;
        }
        out.push_str(&format!("  {:>5}: {}\n", h.line_no, h.text));
    }
    out.push_str(&format!(
        "\n{} of {} matches, {} of {} files, ~{}/{} tokens.",
        r.hits.len(),
        r.total_matches,
        r.files_shown,
        r.files_matched,
        r.tokens_used,
        r.budget
    ));
    if r.truncated {
        out.push_str(&format!(
            "\n{} matches were omitted to stay inside the budget. \
             They are NOT absent from the codebase — narrow the pattern, pass \
             globs, or raise max_tokens to see them.",
            r.total_matches - r.hits.len()
        ));
    }
    out
}
