//! rust-analyzer backed symbol lookup.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

pub fn is_available() -> bool {
    Command::new("rust-analyzer")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct WorkspaceSymbol {
    pub name: String,
    pub path: String,
    pub uri: String,
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Default)]
pub struct WorkspaceAnalysis {
    pub symbols: Vec<WorkspaceSymbol>,
    pub reference_paths: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SymbolSeed {
    pub chunk_id: String,
    pub symbol: String,
    pub path: String,
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone)]
pub struct SymbolReference {
    pub from_chunk_id: String,
    pub symbol: String,
    pub path: String,
    pub line: u32,
}

pub fn analyze_workspace_symbols(
    root: &Path,
    query: &str,
    limit: usize,
) -> Result<WorkspaceAnalysis> {
    if !is_available() {
        anyhow::bail!("rust-analyzer is not available in PATH");
    }

    let mut conn = LspConnection::spawn("rust-analyzer")?;

    let root_uri = file_uri(root)?;
    let init = json!({
        "processId": null,
        "rootUri": root_uri,
        "capabilities": {},
        "trace": "off",
    });
    let _ = conn.request("initialize", init)?;
    conn.notify("initialized", json!({}))?;

    let response = conn.request("workspace/symbol", json!({"query": query}))?;
    let mut symbols = BTreeSet::new();

    if let Some(items) = response.as_array() {
        for item in items.iter().take(limit.max(1) * 8) {
            let Some(name) = item.get("name").and_then(Value::as_str) else {
                continue;
            };
            let uri = item
                .get("location")
                .and_then(|l| l.get("uri"))
                .and_then(Value::as_str)
                .or_else(|| {
                    item.get("location").and_then(|l| l.get("targetUri")).and_then(Value::as_str)
                });
            let Some(uri) = uri else {
                continue;
            };
            let location = item.get("location");
            let line = location
                .and_then(|l| l.get("range"))
                .and_then(|r| r.get("start"))
                .and_then(|s| s.get("line"))
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            let character = location
                .and_then(|l| l.get("range"))
                .and_then(|r| r.get("start"))
                .and_then(|s| s.get("character"))
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;

            let Some(path) = file_uri_to_path(uri) else {
                continue;
            };
            if let Ok(rel) = path.strip_prefix(root) {
                let rel_s = rel.to_string_lossy().replace('\\', "/");
                if rel_s.ends_with(".rs") {
                    symbols.insert(WorkspaceSymbol {
                        name: name.to_string(),
                        path: rel_s,
                        uri: uri.to_string(),
                        line,
                        character,
                    });
                }
            }
        }
    }

    let symbols: Vec<WorkspaceSymbol> = symbols.into_iter().take(limit.max(1) * 3).collect();
    let reference_paths = reference_paths(&mut conn, root, &symbols, limit.max(1) * 4)?;

    let _ = conn.request("shutdown", json!(null));
    let _ = conn.notify("exit", json!(null));

    Ok(WorkspaceAnalysis { symbols, reference_paths })
}

pub fn analyze_symbol_references(
    root: &Path,
    seeds: &[SymbolSeed],
    max_symbols: usize,
    max_refs_per_symbol: usize,
) -> Result<Vec<SymbolReference>> {
    if !is_available() {
        anyhow::bail!("rust-analyzer is not available in PATH");
    }

    if seeds.is_empty() {
        return Ok(Vec::new());
    }

    let mut conn = LspConnection::spawn("rust-analyzer")?;

    let root_uri = file_uri(root)?;
    let init = json!({
        "processId": null,
        "rootUri": root_uri,
        "capabilities": {},
        "trace": "off",
    });
    let _ = conn.request("initialize", init)?;
    conn.notify("initialized", json!({}))?;

    let mut refs = BTreeSet::new();
    for seed in seeds.iter().take(max_symbols.max(1)) {
        let uri = match file_uri(&root.join(&seed.path)) {
            Ok(uri) => uri,
            Err(_) => continue,
        };
        let params = json!({
            "textDocument": { "uri": uri },
            "position": { "line": seed.line, "character": seed.character },
            "context": { "includeDeclaration": true }
        });

        let response = conn.request("textDocument/references", params)?;
        let Some(items) = response.as_array() else {
            continue;
        };

        for item in items.iter().take(max_refs_per_symbol.max(1)) {
            let Some(uri) = item.get("uri").and_then(Value::as_str) else {
                continue;
            };
            let Some(path) = file_uri_to_path(uri) else {
                continue;
            };
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            let rel_s = rel.to_string_lossy().replace('\\', "/");
            if !rel_s.ends_with(".rs") {
                continue;
            }

            let line = item
                .get("range")
                .and_then(|r| r.get("start"))
                .and_then(|s| s.get("line"))
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            refs.insert((seed.chunk_id.clone(), seed.symbol.to_ascii_lowercase(), rel_s, line));
        }
    }

    let _ = conn.request("shutdown", json!(null));
    let _ = conn.notify("exit", json!(null));

    Ok(refs
        .into_iter()
        .map(|(from_chunk_id, symbol, path, line)| SymbolReference {
            from_chunk_id,
            symbol,
            path,
            line,
        })
        .collect())
}

fn reference_paths(
    conn: &mut LspConnection,
    root: &Path,
    symbols: &[WorkspaceSymbol],
    limit: usize,
) -> Result<Vec<String>> {
    let mut paths = BTreeSet::new();
    for symbol in symbols.iter().take(limit.max(1)) {
        let params = json!({
            "textDocument": { "uri": symbol.uri },
            "position": { "line": symbol.line, "character": symbol.character },
            "context": { "includeDeclaration": true }
        });

        let response = conn.request("textDocument/references", params)?;
        if let Some(items) = response.as_array() {
            for item in items {
                let Some(uri) = item.get("uri").and_then(Value::as_str) else {
                    continue;
                };
                let Some(path) = file_uri_to_path(uri) else {
                    continue;
                };
                if let Ok(rel) = path.strip_prefix(root) {
                    let rel_s = rel.to_string_lossy().replace('\\', "/");
                    if rel_s.ends_with(".rs") {
                        paths.insert(rel_s);
                    }
                }
            }
        }
    }

    Ok(paths.into_iter().take(limit.max(1) * 5).collect())
}

fn file_uri(path: &Path) -> Result<String> {
    let absolute = path
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize {}", path.display()))?;
    let normalized = absolute.to_string_lossy().replace('\\', "/");
    if normalized.starts_with('/') {
        Ok(format!("file://{normalized}"))
    } else {
        Ok(format!("file:///{normalized}"))
    }
}

fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let raw = uri.strip_prefix("file://")?;
    let decoded = raw
        .replace("%20", " ")
        .replace("%23", "#")
        .replace("%25", "%")
        .replace("%5B", "[")
        .replace("%5D", "]");

    #[cfg(windows)]
    {
        let trimmed = if is_windows_drive_path(&decoded) {
            decoded.strip_prefix('/').unwrap_or(&decoded)
        } else {
            &decoded
        };
        Some(PathBuf::from(trimmed))
    }

    #[cfg(not(windows))]
    {
        Some(PathBuf::from(decoded))
    }
}

#[cfg(windows)]
fn is_windows_drive_path(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(
        (chars.next(), chars.next(), chars.next()),
        (Some('/'), Some(drive), Some(':')) if drive.is_ascii_alphabetic()
    )
}

struct LspConnection {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl LspConnection {
    fn spawn(binary: &str) -> Result<Self> {
        let mut child = Command::new(binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("Failed to spawn {binary}"))?;

        let stdin = child.stdin.take().context("Failed to open LSP stdin")?;
        let stdout = child.stdout.take().context("Failed to open LSP stdout")?;
        Ok(Self { child, stdin, stdout: BufReader::new(stdout), next_id: 1 })
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        let msg = json!({"jsonrpc": "2.0", "method": method, "params": params});
        self.send_message(&msg)
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.send_message(&msg)?;

        loop {
            let incoming = self.read_message()?;
            let msg_id = incoming.get("id").and_then(Value::as_i64);
            if msg_id != Some(id) {
                continue;
            }

            if let Some(err) = incoming.get("error") {
                anyhow::bail!("LSP error for {method}: {err}");
            }
            return Ok(incoming.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    fn send_message(&mut self, msg: &Value) -> Result<()> {
        let payload = serde_json::to_vec(msg)?;
        write!(self.stdin, "Content-Length: {}\r\n\r\n", payload.len())?;
        self.stdin.write_all(&payload)?;
        self.stdin.flush()?;
        Ok(())
    }

    fn read_message(&mut self) -> Result<Value> {
        let mut content_length = None::<usize>;

        loop {
            let mut line = String::new();
            let read = self.stdout.read_line(&mut line)?;
            if read == 0 {
                anyhow::bail!("LSP process ended unexpectedly");
            }
            let trimmed = line.trim_end();
            if trimmed.is_empty() {
                break;
            }
            let lower = trimmed.to_ascii_lowercase();
            if let Some(rest) = lower.strip_prefix("content-length:") {
                content_length = rest.trim().parse::<usize>().ok();
            }
        }

        let len = content_length.context("Missing Content-Length in LSP response")?;
        let mut buf = vec![0u8; len];
        self.stdout.read_exact(&mut buf)?;
        let value: Value = serde_json::from_slice(&buf)?;
        Ok(value)
    }
}

impl Drop for LspConnection {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::file_uri_to_path;

    #[test]
    fn parses_file_uri_to_path() {
        let path = file_uri_to_path("file:///tmp/my%20repo/src/main.rs").expect("path");
        let normalized = path.to_string_lossy().replace('\\', "/");
        assert!(normalized.ends_with("/tmp/my repo/src/main.rs"));
    }
}
