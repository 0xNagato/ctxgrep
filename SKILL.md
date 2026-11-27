---
name: ctxgrep
description: Search notes, memory, logs, documents and code for a word or pattern and get only the most relevant matches that fit a size limit, with a count of what was left out. Use when you need to find where something is, recall what was decided earlier, or a folder or file is too big to read whole.
---

# ctxgrep

Context-budgeted search. Returns the best matches that fit a token budget and always says how many it left out, so a cut-off list is never mistaken for the whole picture.

```
ctxgrep "<regex>" <folder-or-file> --max-tokens 800
```

Not installed: `cargo install --git https://github.com/0xNagato/ctxgrep`.

- Options: `--max-per-file N` (default 4), `--glob '*.md'` or `--glob '!node_modules/**'` (repeatable), `--case-sensitive`. Output is ranked: definitions first, tests and vendored code last.
- Search only folders you are allowed to read for the current task. It is read-only and respects `.gitignore`.
- Start with one specific word, not a sentence. Probe narrow, then widen.
- If matches were omitted, narrow the pattern or folder, or raise `--max-tokens`, and rerun before you say something is absent.
- Run as an MCP server (`ctxgrep --mcp --root <dir>`), it offers `search` and `count`; call `count` first to see whether a pattern is too broad.
- Open only the files the matches point to, and read just the surrounding part. Report what you found with its path; if nothing matched, say what you searched for and where.
