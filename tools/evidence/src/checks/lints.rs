//! Clippy and compiler warnings compared against the finite baseline exception list.
//!
//! New and migrated code must be warning-free. The legacy crates had warnings when the runner
//! was introduced; each is listed with its owner and removal stage in `lint-baseline.toml`.
//! Matching is exact per file and lint: a new warning fails, and so does an exception whose
//! warning has been fixed, so the list cannot silently shelter new code or outlive its reason.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LintBaseline {
    #[serde(default, rename = "exception")]
    pub exceptions: Vec<LintException>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LintException {
    pub file: String,
    pub lint: String,
    pub count: usize,
    pub owner: String,
    pub removal: String,
}

impl LintBaseline {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        let baseline: Self =
            toml::from_str(&text).map_err(|error| format!("parse {}: {error}", path.display()))?;
        let mut seen = BTreeSet::new();
        for exception in &baseline.exceptions {
            if exception.count == 0
                || exception.owner.trim().is_empty()
                || exception.removal.trim().is_empty()
            {
                return Err(format!(
                    "exception {} {} needs a positive count, an owner and a removal stage",
                    exception.file, exception.lint
                ));
            }
            if !seen.insert((exception.file.as_str(), exception.lint.as_str())) {
                return Err(format!(
                    "duplicate exception {} {}",
                    exception.file, exception.lint
                ));
            }
        }
        Ok(baseline)
    }
}

/// One deduplicated diagnostic: the same warning reported for a library and its test build
/// counts once.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Diagnostic {
    pub level: String,
    pub lint: String,
    pub file: String,
    pub line: u64,
    pub column: u64,
    pub message: String,
}

/// Extracts diagnostics from `cargo --message-format=json` output. Paths are made relative to
/// `root`; diagnostics outside the workspace (dependencies) are ignored.
pub fn parse_diagnostics(output: &str, root: &Path) -> BTreeSet<Diagnostic> {
    let root_text = root.display().to_string();
    let mut diagnostics = BTreeSet::new();
    for line in output.lines() {
        let Ok(json) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if json["reason"] != "compiler-message" {
            continue;
        }
        let message = &json["message"];
        let level = message["level"].as_str().unwrap_or_default();
        if level != "warning" && level != "error" {
            continue;
        }
        let text = message["message"].as_str().unwrap_or_default();
        // Summary lines such as "3 warnings emitted" carry no span.
        let Some(span) = message["spans"]
            .as_array()
            .and_then(|spans| spans.iter().find(|span| span["is_primary"] == true))
        else {
            if level == "error" {
                diagnostics.insert(Diagnostic {
                    level: level.to_owned(),
                    lint: "error".to_owned(),
                    file: String::new(),
                    line: 0,
                    column: 0,
                    message: text.to_owned(),
                });
            }
            continue;
        };
        let raw_file = span["file_name"].as_str().unwrap_or_default();
        let file = raw_file
            .strip_prefix(&root_text)
            .map(|rest| rest.trim_start_matches('/'))
            .unwrap_or(raw_file);
        if Path::new(file).is_absolute() || file.starts_with("..") {
            continue;
        }
        diagnostics.insert(Diagnostic {
            level: level.to_owned(),
            lint: message["code"]["code"]
                .as_str()
                .unwrap_or("warning")
                .to_owned(),
            file: file.to_owned(),
            line: span["line_start"].as_u64().unwrap_or(0),
            column: span["column_start"].as_u64().unwrap_or(0),
            message: text.to_owned(),
        });
    }
    diagnostics
}

/// Compares diagnostics with the exceptions. Returns problems; empty means the lint gate holds.
pub fn compare(diagnostics: &BTreeSet<Diagnostic>, baseline: &LintBaseline) -> Vec<String> {
    let mut problems = Vec::new();
    let mut counts: BTreeMap<(&str, &str), Vec<&Diagnostic>> = BTreeMap::new();
    for diagnostic in diagnostics {
        if diagnostic.level == "error" {
            problems.push(format!(
                "error {}:{}: {}",
                diagnostic.file, diagnostic.line, diagnostic.message
            ));
            continue;
        }
        counts
            .entry((diagnostic.file.as_str(), diagnostic.lint.as_str()))
            .or_default()
            .push(diagnostic);
    }
    let allowed: BTreeMap<(&str, &str), usize> = baseline
        .exceptions
        .iter()
        .map(|exception| {
            (
                (exception.file.as_str(), exception.lint.as_str()),
                exception.count,
            )
        })
        .collect();
    for (key, found) in &counts {
        let permitted = allowed.get(key).copied().unwrap_or(0);
        if found.len() > permitted {
            for diagnostic in found {
                problems.push(format!(
                    "new {} at {}:{}:{}: {}",
                    diagnostic.lint,
                    diagnostic.file,
                    diagnostic.line,
                    diagnostic.column,
                    diagnostic.message
                ));
            }
        }
    }
    for (key, permitted) in &allowed {
        let found = counts.get(key).map_or(0, Vec::len);
        if found < *permitted {
            problems.push(format!(
                "stale exception {} {}: {permitted} recorded, {found} found; lower or remove it",
                key.0, key.1
            ));
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    fn warning(file: &str, line: u64, lint: &str) -> String {
        serde_json::json!({
            "reason": "compiler-message",
            "message": {
                "level": "warning",
                "message": "something",
                "code": {"code": lint},
                "spans": [{"is_primary": true, "file_name": file, "line_start": line, "column_start": 1}]
            }
        })
        .to_string()
    }

    fn baseline(entries: &[(&str, &str, usize)]) -> LintBaseline {
        LintBaseline {
            exceptions: entries
                .iter()
                .map(|(file, lint, count)| LintException {
                    file: (*file).to_owned(),
                    lint: (*lint).to_owned(),
                    count: *count,
                    owner: "legacy".to_owned(),
                    removal: "06".to_owned(),
                })
                .collect(),
        }
    }

    #[test]
    fn duplicate_lib_and_test_reports_count_once() {
        let line = warning("/w/a.rs", 3, "clippy::collapsible_if");
        let output = format!("{line}\n{line}\nnot json\n");
        assert_eq!(parse_diagnostics(&output, Path::new("/w")).len(), 1);
    }

    #[test]
    fn exact_exceptions_pass_and_new_or_fixed_warnings_fail() {
        let output = [
            warning("/w/a.rs", 3, "clippy::x"),
            warning("/w/a.rs", 9, "clippy::x"),
        ]
        .join("\n");
        let diagnostics = parse_diagnostics(&output, Path::new("/w"));
        assert!(compare(&diagnostics, &baseline(&[("a.rs", "clippy::x", 2)])).is_empty());

        let new = compare(&diagnostics, &baseline(&[("a.rs", "clippy::x", 1)]));
        assert!(
            new.iter()
                .any(|problem| problem.starts_with("new clippy::x")),
            "{new:?}"
        );

        let stale = compare(
            &diagnostics,
            &baseline(&[("a.rs", "clippy::x", 2), ("b.rs", "clippy::y", 1)]),
        );
        assert!(
            stale
                .iter()
                .any(|problem| problem.starts_with("stale exception b.rs")),
            "{stale:?}"
        );

        let unlisted = compare(&diagnostics, &baseline(&[]));
        assert_eq!(unlisted.len(), 2);
    }

    #[test]
    fn dependency_diagnostics_are_ignored() {
        let output = warning("/home/u/.cargo/registry/src/x/lib.rs", 1, "unused");
        assert!(parse_diagnostics(&output, Path::new("/w")).is_empty());
    }
}
