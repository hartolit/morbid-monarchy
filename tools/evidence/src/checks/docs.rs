//! `check docs`: local links, canonical owners, assignment structure and package introductions.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use crate::{
    report::{Item, State},
    workspace::Workspace,
};

/// Rule files retired in favour of `AGENTS.md` and the practices; they must not return as a
/// second active rule set.
const RETIRED_RULE_PATHS: [&str; 3] = ["docs/persona.md", "docs/rules.md", "docs/knowledge"];

pub fn run(root: &Path, workspace: Option<&Workspace>) -> Vec<Item> {
    let markdown = markdown_files(root);
    vec![
        links_item(root, &markdown),
        retired_rules_item(root),
        owner_index_item(root),
        assignment_structure_item(root),
        package_readme_item(root, workspace),
    ]
}

fn links_item(root: &Path, files: &[PathBuf]) -> Item {
    let (checked, failures) = check_links(root, files);
    let state = if failures.is_empty() {
        State::Passed
    } else {
        State::Failed
    };
    Item::new(
        "docs.links",
        state,
        format!(
            "{checked} local links in {} Markdown files; {} broken",
            files.len(),
            failures.len()
        ),
    )
    .with_details(failures)
}

fn retired_rules_item(root: &Path) -> Item {
    let present: Vec<String> = RETIRED_RULE_PATHS
        .iter()
        .filter(|path| root.join(path).exists())
        .map(|path| format!("{path} exists; its rules now live in AGENTS.md and the practices"))
        .collect();
    let state = if present.is_empty() {
        State::Passed
    } else {
        State::Failed
    };
    Item::new("docs.retired-rules", state, "retired rule files are absent").with_details(present)
}

/// Every row of the owner table in `docs/README.md` must name exactly one existing document.
fn owner_index_item(root: &Path) -> Item {
    let index = root.join("docs/README.md");
    let Ok(text) = fs::read_to_string(&index) else {
        return Item::new("docs.owners", State::Failed, "docs/README.md is missing");
    };
    let mut problems = Vec::new();
    let mut rows = 0;
    for line in section_lines(&text, "## Find the owner") {
        if !line.starts_with('|') || line.starts_with("| ---") || line.starts_with("| Question") {
            continue;
        }
        rows += 1;
        let targets: Vec<String> = link_targets(line).collect();
        match targets.as_slice() {
            [target] => {
                let file = target.split('#').next().unwrap_or_default();
                if !root.join("docs").join(file).is_file() {
                    problems.push(format!("owner `{target}` does not exist"));
                }
            }
            _ => problems.push(format!("row must link exactly one owner: {line}")),
        }
    }
    if rows == 0 {
        problems.push("the `Find the owner` table has no rows".to_owned());
    }
    let state = if problems.is_empty() {
        State::Passed
    } else {
        State::Failed
    };
    Item::new(
        "docs.owners",
        state,
        format!("{rows} canonical owners indexed"),
    )
    .with_details(problems)
}

/// Each sprint in the virtual-office table has a README and a single current handoff, and the
/// whiteboard keeps its issue and discussion indexes.
fn assignment_structure_item(root: &Path) -> Item {
    let office = root.join("docs/development/virtual-office");
    let mut problems = Vec::new();
    let mut sprints = 0;
    match fs::read_to_string(office.join("README.md")) {
        Ok(text) => {
            for line in section_lines(&text, "## Active work") {
                if !line.starts_with('|')
                    || line.starts_with("| ---")
                    || line.starts_with("| Sprint")
                {
                    continue;
                }
                for target in link_targets(line) {
                    let readme = office.join(&target);
                    let Some(sprint_dir) = readme.parent() else {
                        continue;
                    };
                    sprints += 1;
                    for required in ["README.md", "handoff.md"] {
                        if !sprint_dir.join(required).is_file() {
                            problems
                                .push(format!("{} lacks {required}", relative(root, sprint_dir)));
                        }
                    }
                }
            }
        }
        Err(_) => problems.push("virtual-office README is missing".to_owned()),
    }
    for required in [
        "whiteboard/README.md",
        "whiteboard/issues/README.md",
        "whiteboard/discussions/README.md",
    ] {
        if !office.join(required).is_file() {
            problems.push(format!("virtual office lacks {required}"));
        }
    }
    let state = if problems.is_empty() {
        State::Passed
    } else {
        State::Failed
    };
    Item::new(
        "docs.assignments",
        state,
        format!("{sprints} active sprint(s) with README and handoff"),
    )
    .with_details(problems)
}

/// Every workspace package explains itself in a README beside its manifest.
fn package_readme_item(root: &Path, workspace: Option<&Workspace>) -> Item {
    let Some(workspace) = workspace else {
        return Item::new(
            "docs.package-readmes",
            State::Failed,
            "workspace metadata unavailable; package introductions not checked",
        );
    };
    let missing: Vec<String> = workspace
        .members()
        .filter(|package| !package.dir.join("README.md").is_file())
        .map(|package| {
            format!(
                "{} has no README.md in {}",
                package.name,
                relative(root, &package.dir)
            )
        })
        .collect();
    let state = if missing.is_empty() {
        State::Passed
    } else {
        State::Failed
    };
    Item::new(
        "docs.package-readmes",
        state,
        format!("{} workspace packages", workspace.members().count()),
    )
    .with_details(missing)
}

fn markdown_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if path.is_dir() {
                if name != "target" && name != ".git" {
                    pending.push(path);
                }
            } else if path.extension().is_some_and(|ext| ext == "md") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// Checks every relative link and heading anchor. Returns the number of links checked and one
/// message per failure.
pub fn check_links(root: &Path, files: &[PathBuf]) -> (usize, Vec<String>) {
    let mut anchor_cache: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    let mut failures = Vec::new();
    let mut checked = 0;
    for source in files {
        let Ok(text) = fs::read_to_string(source) else {
            failures.push(format!("{}: unreadable", relative(root, source)));
            continue;
        };
        for (line_number, target) in document_links(&text) {
            if is_external(&target) {
                continue;
            }
            checked += 1;
            let (file_part, anchor) = match target.split_once('#') {
                Some((file, anchor)) => (file, Some(anchor)),
                None => (target.as_str(), None),
            };
            let destination = if file_part.is_empty() {
                source.clone()
            } else {
                source.parent().unwrap_or(root).join(file_part)
            };
            let location = format!("{}:{line_number}: {target}", relative(root, source));
            if !destination.exists() {
                failures.push(format!("{location} -> missing file"));
                continue;
            }
            if let Some(anchor) = anchor
                && destination.extension().is_some_and(|ext| ext == "md")
            {
                let key = destination
                    .canonicalize()
                    .unwrap_or_else(|_| destination.clone());
                let anchors = anchor_cache.entry(key).or_insert_with(|| {
                    fs::read_to_string(&destination)
                        .map(|text| heading_anchors(&text))
                        .unwrap_or_default()
                });
                if !anchors.contains(anchor) {
                    failures.push(format!("{location} -> missing anchor"));
                }
            }
        }
    }
    (checked, failures)
}

fn is_external(target: &str) -> bool {
    let scheme_end = target.find(':');
    let first_separator = target.find(['/', '#']);
    match (scheme_end, first_separator) {
        (Some(colon), Some(separator)) => colon < separator,
        (Some(_), None) => true,
        _ => false,
    }
}

/// Links outside fenced code blocks and inline code spans, with 1-based line numbers.
fn document_links(text: &str) -> Vec<(usize, String)> {
    let mut links = Vec::new();
    let mut fenced = false;
    for (index, line) in text.lines().enumerate() {
        if is_fence(line) {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        links.extend(link_targets(&strip_code_spans(line)).map(|target| (index + 1, target)));
    }
    links
}

fn is_fence(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("```") || trimmed.starts_with("~~~")
}

fn strip_code_spans(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_code = false;
    for character in line.chars() {
        if character == '`' {
            in_code = !in_code;
        } else if !in_code {
            out.push(character);
        }
    }
    out
}

/// Targets of inline links `[text](target)`, excluding images.
fn link_targets(line: &str) -> impl Iterator<Item = String> + '_ {
    let bytes = line.as_bytes();
    let mut position = 0;
    std::iter::from_fn(move || {
        while let Some(offset) = line[position..].find("](") {
            let start = position + offset + 2;
            position = start;
            let opening = line[..start - 2].rfind('[');
            let is_image = opening.is_some_and(|open| open > 0 && bytes[open - 1] == b'!');
            let end = line[start..].find(')').map(|end| start + end)?;
            let target = line[start..end].trim();
            position = end + 1;
            if opening.is_none()
                || is_image
                || target.is_empty()
                || target.contains(char::is_whitespace)
            {
                continue;
            }
            return Some(target.to_owned());
        }
        None
    })
}

/// GitHub-style heading anchors, with `-1`, `-2` suffixes for repeated headings.
pub fn heading_anchors(text: &str) -> BTreeSet<String> {
    let mut anchors = BTreeSet::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut fenced = false;
    for line in text.lines() {
        if is_fence(line) {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let hashes = line.chars().take_while(|c| *c == '#').count();
        if !(1..=6).contains(&hashes) || !line[hashes..].starts_with(' ') {
            continue;
        }
        let base = slug(line[hashes..].trim().trim_end_matches('#').trim());
        let count = counts.entry(base.clone()).or_insert(0);
        anchors.insert(if *count == 0 {
            base.clone()
        } else {
            format!("{base}-{count}")
        });
        *count += 1;
    }
    anchors
}

fn slug(heading: &str) -> String {
    let mut plain = String::new();
    let mut rest = heading;
    // Link text keeps its words; the target disappears from the anchor.
    while let Some(open) = rest.find('[') {
        plain.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after
            .find("](")
            .and_then(|mid| after[mid..].find(')').map(|end| (mid, mid + end)))
        {
            Some((mid, end)) => {
                plain.push_str(&after[..mid]);
                rest = &after[end + 1..];
            }
            None => {
                plain.push('[');
                rest = after;
            }
        }
    }
    plain.push_str(rest);
    plain
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_' || *c == ' ')
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}

/// Lines from a `## Heading` up to the next heading of the same or higher level.
fn section_lines<'a>(text: &'a str, heading: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    let level = heading.chars().take_while(|c| *c == '#').count();
    text.lines()
        .skip_while(move |line| line.trim_end() != heading)
        .skip(1)
        .take_while(move |line| {
            let hashes = line.chars().take_while(|c| *c == '#').count();
            hashes == 0 || hashes > level
        })
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_follow_github_slugs() {
        let anchors = heading_anchors(
            "# 00 — First execution\n## Checks run in 00\n## Repeat\n## Repeat\n```\n# not a heading\n```\n## [Link](x.md) and `code`\n",
        );
        for expected in [
            "00--first-execution",
            "checks-run-in-00",
            "repeat",
            "repeat-1",
            "link-and-code",
        ] {
            assert!(
                anchors.contains(expected),
                "missing {expected} in {anchors:?}"
            );
        }
        assert!(!anchors.contains("not-a-heading"));
    }

    #[test]
    fn links_skip_code_images_and_external_targets() {
        let text = "[a](a.md) ![img](i.png) `[b](b.md)` [c](https://x.org) [d](#local)\n```\n[e](e.md)\n```\n";
        let targets: Vec<String> = document_links(text).into_iter().map(|(_, t)| t).collect();
        assert_eq!(targets, ["a.md", "https://x.org", "#local"]);
        assert!(is_external("https://x.org"));
        assert!(!is_external("a.md#x:y"));
    }

    #[test]
    fn broken_files_and_anchors_are_reported() {
        let dir =
            std::env::temp_dir().join(format!("monarchy-evidence-links-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch");
        fs::write(
            dir.join("a.md"),
            "# Top\n[ok](#top) [bad](#nope) [gone](missing.md) [ok2](b.md#part)\n",
        )
        .expect("write a");
        fs::write(dir.join("b.md"), "# B\n## Part\n").expect("write b");
        let files = markdown_files(&dir);
        let (checked, failures) = check_links(&dir, &files);
        assert_eq!(checked, 4);
        assert_eq!(failures.len(), 2, "{failures:?}");
        assert!(
            failures
                .iter()
                .any(|f| f.contains("#nope") && f.contains("missing anchor"))
        );
        assert!(
            failures
                .iter()
                .any(|f| f.contains("missing.md") && f.contains("missing file"))
        );
    }

    #[test]
    fn section_stops_at_the_next_peer_heading() {
        let text = "## A\nx\n### Sub\ny\n## B\nz\n";
        let lines: Vec<&str> = section_lines(text, "## A").collect();
        assert_eq!(lines, ["x", "### Sub", "y"]);
    }
}
