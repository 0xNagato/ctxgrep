# ctxgrep

Context-budgeted code search for AI agents. CLI and MCP server. Written in Rust.

An agent greps a codebase, gets 503 matches, burns 30k tokens on the dump, and
the line it needed was number 380. Or worse: the tool quietly returns the first
50, the agent concludes the symbol doesn't exist anywhere else, and acts on that.

`ctxgrep` returns the *most relevant* matches that fit an explicit token budget,
and always reports exactly how many it left out.

```bash
$ ctxgrep "description" ~/.claude/skills --max-tokens 400

…
/Users/x/.claude/skills/polyclaw/lib/coverage.py
     95: def get_tier_description(tier: int) -> str:

25 of 503 matches, 20 of 295 files, ~394/400 tokens.
478 matches were omitted to stay inside the budget. They are NOT absent from the
codebase — narrow the pattern, pass globs, or raise max_tokens to see them.
```

That last line is the whole point. Silent truncation is how an agent ends up
confidently wrong.

## Install

```bash
cargo install --git https://github.com/0xNagato/ctxgrep
```

## As an agent skill

[`SKILL.md`](SKILL.md) tells an LLM agent (Claude, OpenClaw and others) when to
reach for ctxgrep and what to do when matches were omitted. Copy it into your
agent's skills folder; it expects `ctxgrep` on your `PATH`.

## As an MCP server

```jsonc
{
  "mcpServers": {
    "ctxgrep": {
      "command": "ctxgrep",
      "args": ["--mcp", "--root", "/path/to/your/project"]
    }
  }
}
```

Two tools:

| Tool | Purpose |
|---|---|
| `search` | Ranked, budgeted matches. Returns what fits and reports what didn't. |
| `count` | How many matches a pattern *would* produce, costing almost nothing. Probe before you spend. |

`count` exists so an agent can check whether a pattern is too broad before
paying for the answer.

## Ranking

A budget forces a choice, and once you can only return 25 of 503 matches,
*which* 25 is the entire product. The ranking is deliberately opinionated:

- **Definitions beat call sites.** `fn`, `def`, `class`, `struct`, `impl`,
  `interface`, `export`, `func` and friends score `+40`.
- **Source beats noise.** Anything under `test/`, `vendor/`, `node_modules/`,
  `target/`, `dist/`, or matching `.min.`, scores `-35`.
- **Shallow beats deep.** `-2` per path segment.
- **Concise beats sprawling.** `-1` per 60 characters of line.
- **Filename relevance.** `+20` when the filename contains a term from the pattern.

Then each file is capped (default 4 matches) *before* global ranking, so one
dense file can't crowd out twenty others.

## Safety

The server is a file-read primitive handed to a model that reads untrusted
content. So every path argument is resolved, canonicalised, and refused if it
escapes the `--root`:

```
error: path "/etc" resolves outside the server root (/Users/x/project).
       Restart ctxgrep with --root to widen the scope.
```

Not discouraged — impossible. There are tests for both the relative (`../..`)
and absolute (`/etc`) cases.

Also: gitignore is respected, files over 2MB are skipped, binary files are
skipped (they fail UTF-8 decoding, which is the check), and single lines are
clipped at 240 characters so a minified bundle can't eat a whole budget.

## CLI

```
ctxgrep <pattern> [path] [options]
ctxgrep --mcp [--root DIR]
ctxgrep --tools                    # print the MCP tool schema as JSON

  --max-tokens N     token ceiling (default 2000)
  --max-per-file N   cap matches per file (default 4)
  --glob G           include/exclude glob, repeatable: '*.rs', '!target/**'
  --case-sensitive   default is case-insensitive
```

Exit code is `1` when there are no matches, so it composes in scripts.

## Why Rust

Traversal is the expensive part of search, and `ignore` — ripgrep's own
gitignore-aware parallel walker — does it properly. 295 files in 0.56s on a
cold cache, and the release binary is 2.7MB with no runtime to install. For a
tool an agent invokes dozens of times per session, startup cost is a feature.

## Development

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```

The tool schema is linted in CI by [skill-lint](https://github.com/0xNagato/skill-lint),
which is how the `path` parameter's server-side validation ended up documented
rather than merely implemented.

## Licence

MIT
