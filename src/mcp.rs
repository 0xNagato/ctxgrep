//! MCP server over stdio JSON-RPC, written against the wire protocol.
//!
//! Methods: initialize · notifications/* · ping · tools/list · tools/call.
//!
//! Every path argument is confined under a root chosen at startup. A search
//! server is a file-read primitive handed to a model that reads untrusted
//! content, so "search ~/.ssh" has to be impossible rather than merely
//! discouraged.

use crate::search::{render, search, Options};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

pub const PROTOCOL_VERSION: &str = "2025-06-18";
pub const SERVER_NAME: &str = "ctxgrep";
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Server {
    root: PathBuf,
}

impl Server {
    pub fn new(root: PathBuf) -> Self {
        let root = root.canonicalize().unwrap_or(root);
        Server { root }
    }

    /// Resolve a caller-supplied path inside the root, or refuse.
    fn resolve(&self, p: &str) -> Result<PathBuf, String> {
        let candidate = if p.is_empty() || p == "." {
            self.root.clone()
        } else {
            let raw = Path::new(p);
            if raw.is_absolute() {
                raw.to_path_buf()
            } else {
                self.root.join(raw)
            }
        };
        let real = candidate
            .canonicalize()
            .map_err(|e| format!("path {p:?} is not readable: {e}"))?;
        if !real.starts_with(&self.root) {
            return Err(format!(
                "path {p:?} resolves outside the server root ({}). \
                 Restart ctxgrep with --root to widen the scope.",
                self.root.display()
            ));
        }
        Ok(real)
    }

    pub fn tools(&self) -> Value {
        json!([
            {
                "name": "search",
                "description": format!(
                    "Search code under the server root ({}) and return the most \
                     relevant matches that fit an explicit token budget. Results \
                     are ranked (definitions above call sites, source above \
                     vendored and test code), capped per file so one dense file \
                     cannot crowd out the rest, and ALWAYS report how many \
                     matches were omitted. Use instead of shelling out to grep \
                     whenever a pattern might match more than a handful of lines.",
                    self.root.display()),
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string",
                            "format": "regex",
                            "description": "Rust regex syntax. Compiled and validated before any file is read; an invalid regex is rejected."
                        },
                        "path": {
                            "type": "string",
                            "description": "Subdirectory to search, relative to the server root. Validated server-side: resolved, canonicalised, and refused if it escapes the root, so traversal outside the root is not possible."
                        },
                        "max_tokens": {
                            "type": "integer", "minimum": 100, "maximum": 50000,
                            "description": "Token ceiling for the result. Default 2000."
                        },
                        "globs": {
                            "type": "array", "items": {"type": "string"},
                            "description": "Include/exclude globs, e.g. [\"*.rs\", \"!target/**\"]."
                        },
                        "max_per_file": {
                            "type": "integer", "minimum": 1, "maximum": 100,
                            "description": "Cap on matches shown per file. Default 4."
                        },
                        "case_sensitive": {
                            "type": "boolean",
                            "description": "Default false."
                        }
                    },
                    "required": ["pattern"],
                    "additionalProperties": false
                }
            },
            {
                "name": "count",
                "description":
                    "Count how many matches and files a pattern would produce, \
                     without returning any content. Costs almost no tokens. Use \
                     this first when you are unsure how broad a pattern is, then \
                     narrow it before calling search.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "pattern": {
                            "type": "string", "format": "regex",
                            "description": "Rust regex syntax, validated before use."
                        },
                        "path": {
                            "type": "string",
                            "description": "Subdirectory relative to the server root. Validated server-side and refused if it escapes the root."
                        },
                        "globs": {
                            "type": "array", "items": {"type": "string"},
                            "description": "Include/exclude globs."
                        }
                    },
                    "required": ["pattern"],
                    "additionalProperties": false
                }
            }
        ])
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        let pattern = args
            .get("pattern")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or("`pattern` is required and must be a non-empty string")?;
        let path = self.resolve(args.get("path").and_then(Value::as_str).unwrap_or("."))?;
        let globs: Vec<String> = args
            .get("globs")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        let mut opts = Options {
            pattern: pattern.to_string(),
            path: path.to_string_lossy().to_string(),
            globs,
            ..Default::default()
        };

        match name {
            "count" => {
                opts.max_tokens = 1;
                let r = search(&opts)?;
                Ok(format!(
                    "{} matches across {} files.{}",
                    r.total_matches,
                    r.files_matched,
                    if r.total_matches > 400 {
                        " That is very broad — narrow it before calling search."
                    } else {
                        ""
                    }
                ))
            }
            "search" => {
                if let Some(v) = args.get("max_tokens").and_then(Value::as_u64) {
                    opts.max_tokens = v.clamp(100, 50_000) as usize;
                }
                if let Some(v) = args.get("max_per_file").and_then(Value::as_u64) {
                    opts.max_per_file = v.clamp(1, 100) as usize;
                }
                opts.case_sensitive = args
                    .get("case_sensitive")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                Ok(render(&search(&opts)?))
            }
            other => Err(format!("unknown tool: {other}")),
        }
    }

    pub fn handle(&self, msg: &Value) -> Option<Value> {
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let id = msg.get("id").cloned();

        // Notifications carry no id and must not be answered.
        let id = id?;

        let result = match method {
            "initialize" => json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": SERVER_NAME, "version": SERVER_VERSION}
            }),
            "ping" => json!({}),
            "tools/list" => json!({"tools": self.tools()}),
            "tools/call" => {
                let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let empty = json!({});
                let args = params.get("arguments").unwrap_or(&empty);
                // Tool failures are results, not protocol errors, so the model
                // can read the message and correct its own call.
                match self.call(name, args) {
                    Ok(text) => json!({
                        "content": [{"type": "text", "text": text}],
                        "isError": false
                    }),
                    Err(e) => json!({
                        "content": [{"type": "text", "text": format!("error: {e}")}],
                        "isError": true
                    }),
                }
            }
            _ => {
                return Some(json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": {"code": -32601, "message": format!("method not found: {method}")}
                }))
            }
        };
        Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
    }

    pub fn serve<R: BufRead, W: Write>(&self, input: R, mut output: W) -> std::io::Result<()> {
        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let reply = match serde_json::from_str::<Value>(&line) {
                Ok(msg) => self.handle(&msg),
                Err(_) => Some(json!({
                    "jsonrpc": "2.0", "id": Value::Null,
                    "error": {"code": -32700, "message": "parse error"}
                })),
            };
            if let Some(r) = reply {
                writeln!(output, "{r}")?;
                output.flush()?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn srv() -> Server {
        Server::new(std::env::current_dir().unwrap())
    }

    #[test]
    fn initialize_reports_name_and_protocol() {
        let r = srv()
            .handle(&json!({"jsonrpc":"2.0","id":1,"method":"initialize"}))
            .unwrap();
        assert_eq!(r["result"]["serverInfo"]["name"], SERVER_NAME);
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL_VERSION);
    }

    #[test]
    fn notifications_get_no_reply() {
        assert!(srv()
            .handle(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .is_none());
    }

    #[test]
    fn tools_list_shape() {
        let r = srv()
            .handle(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
            .unwrap();
        let tools = r["result"]["tools"].as_array().unwrap().clone();
        assert_eq!(tools.len(), 2);
        for t in tools {
            assert!(t["name"].is_string());
            assert!(t["description"].as_str().unwrap().len() > 25);
            assert_eq!(t["inputSchema"]["additionalProperties"], json!(false));
            assert!(t["inputSchema"]["required"].is_array());
        }
    }

    #[test]
    fn unknown_method_is_a_protocol_error() {
        let r = srv()
            .handle(&json!({"jsonrpc":"2.0","id":3,"method":"nope"}))
            .unwrap();
        assert_eq!(r["error"]["code"], -32601);
    }

    #[test]
    fn unknown_tool_is_an_error_result_not_a_crash() {
        let r = srv()
            .handle(&json!({"jsonrpc":"2.0","id":4,"method":"tools/call",
                            "params":{"name":"bogus","arguments":{"pattern":"x"}}}))
            .unwrap();
        assert_eq!(r["result"]["isError"], json!(true));
    }

    #[test]
    fn missing_pattern_is_rejected() {
        let r = srv()
            .handle(&json!({"jsonrpc":"2.0","id":5,"method":"tools/call",
                            "params":{"name":"search","arguments":{}}}))
            .unwrap();
        assert_eq!(r["result"]["isError"], json!(true));
    }

    #[test]
    fn escaping_the_root_is_refused() {
        let s = Server::new(std::env::current_dir().unwrap().join("src"));
        let err = s.resolve("../..").unwrap_err();
        assert!(err.contains("outside the server root"), "got: {err}");
    }

    #[test]
    fn absolute_path_outside_root_is_refused() {
        let s = Server::new(std::env::current_dir().unwrap().join("src"));
        assert!(s.resolve("/etc").is_err());
    }

    #[test]
    fn malformed_line_yields_parse_error() {
        let mut out = Vec::new();
        srv().serve(&b"not json\n"[..], &mut out).unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["error"]["code"], -32700);
    }
}
