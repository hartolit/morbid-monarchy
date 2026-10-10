//! Workspace packages and their resolved dependency graph, read from `cargo metadata`.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::Value;

#[derive(Debug, Clone)]
pub struct Package {
    pub id: String,
    pub name: String,
    pub dir: PathBuf,
    pub is_member: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DependencyKind {
    Normal,
    Build,
    Dev,
}

#[derive(Debug, Clone)]
pub struct Workspace {
    packages: BTreeMap<String, Package>,
    /// Resolved edges: package id -> (dependency id, kinds through which it is used).
    edges: BTreeMap<String, Vec<(String, BTreeSet<DependencyKind>)>>,
}

impl Workspace {
    /// Runs `cargo metadata --locked`. With `all_features`, optional edges are visible; without
    /// it, the graph is what a default build compiles.
    pub fn load(root: &Path, all_features: bool) -> Result<Self, String> {
        let mut args = vec!["metadata", "--format-version", "1", "--locked"];
        if all_features {
            args.push("--all-features");
        }
        let output = Command::new("cargo")
            .current_dir(root)
            .args(&args)
            .output()
            .map_err(|error| format!("could not start cargo metadata: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "cargo metadata failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let json: Value = serde_json::from_slice(&output.stdout)
            .map_err(|error| format!("cargo metadata output is not JSON: {error}"))?;
        Self::from_metadata(&json)
    }

    pub fn from_metadata(json: &Value) -> Result<Self, String> {
        let members: BTreeSet<&str> = json["workspace_members"]
            .as_array()
            .ok_or("metadata lacks workspace_members")?
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let mut packages = BTreeMap::new();
        for package in json["packages"]
            .as_array()
            .ok_or("metadata lacks packages")?
        {
            let id = package["id"]
                .as_str()
                .ok_or("package without id")?
                .to_owned();
            let manifest = package["manifest_path"]
                .as_str()
                .ok_or("package without manifest")?;
            let dir = Path::new(manifest)
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            packages.insert(
                id.clone(),
                Package {
                    is_member: members.contains(id.as_str()),
                    name: package["name"].as_str().unwrap_or_default().to_owned(),
                    id,
                    dir,
                },
            );
        }
        let mut edges = BTreeMap::new();
        for node in json["resolve"]["nodes"]
            .as_array()
            .ok_or("metadata lacks resolve")?
        {
            let id = node["id"].as_str().unwrap_or_default().to_owned();
            let mut deps = Vec::new();
            for dep in node["deps"].as_array().into_iter().flatten() {
                let target = dep["pkg"].as_str().unwrap_or_default().to_owned();
                let kinds = dep["dep_kinds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|kind| match kind["kind"].as_str() {
                        Some("dev") => DependencyKind::Dev,
                        Some("build") => DependencyKind::Build,
                        _ => DependencyKind::Normal,
                    })
                    .collect();
                deps.push((target, kinds));
            }
            edges.insert(id, deps);
        }
        Ok(Self { packages, edges })
    }

    pub fn members(&self) -> impl Iterator<Item = &Package> {
        self.packages.values().filter(|package| package.is_member)
    }

    pub fn member(&self, name: &str) -> Option<&Package> {
        self.members().find(|package| package.name == name)
    }

    pub fn package(&self, id: &str) -> Option<&Package> {
        self.packages.get(id)
    }

    /// Direct dependencies of `id` used through any of `kinds`.
    pub fn direct(&self, id: &str, kinds: &[DependencyKind]) -> Vec<&Package> {
        self.edges
            .get(id)
            .into_iter()
            .flatten()
            .filter(|(_, used)| used.iter().any(|kind| kinds.contains(kind)))
            .filter_map(|(target, _)| self.packages.get(target))
            .collect()
    }

    /// Every package reachable from `id` through normal and build dependencies, each with one
    /// dependency path from `id` for diagnostics.
    pub fn production_closure(&self, id: &str) -> BTreeMap<String, Vec<String>> {
        let production = [DependencyKind::Normal, DependencyKind::Build];
        let mut reached: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut pending: Vec<(String, Vec<String>)> = vec![(id.to_owned(), Vec::new())];
        while let Some((current, path)) = pending.pop() {
            for dep in self.direct(&current, &production) {
                if dep.id == id || reached.contains_key(&dep.id) {
                    continue;
                }
                let mut dep_path = path.clone();
                dep_path.push(dep.name.clone());
                reached.insert(dep.id.clone(), dep_path.clone());
                pending.push((dep.id.clone(), dep_path));
            }
        }
        reached
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Workspace {
        let json = serde_json::json!({
            "workspace_members": ["app", "model"],
            "packages": [
                {"id": "app", "name": "app", "manifest_path": "/w/app/Cargo.toml"},
                {"id": "model", "name": "model", "manifest_path": "/w/model/Cargo.toml"},
                {"id": "engine", "name": "engine-core", "manifest_path": "/r/engine/Cargo.toml"},
                {"id": "tester", "name": "tester", "manifest_path": "/r/tester/Cargo.toml"}
            ],
            "resolve": {"nodes": [
                {"id": "app", "deps": [
                    {"pkg": "model", "dep_kinds": [{"kind": null}]},
                    {"pkg": "tester", "dep_kinds": [{"kind": "dev"}]}
                ]},
                {"id": "model", "deps": [{"pkg": "engine", "dep_kinds": [{"kind": null}]}]},
                {"id": "engine", "deps": []},
                {"id": "tester", "deps": []}
            ]}
        });
        Workspace::from_metadata(&json).expect("metadata")
    }

    #[test]
    fn closure_follows_production_edges_only() {
        let workspace = sample();
        let closure = workspace.production_closure("app");
        assert_eq!(
            closure.get("engine"),
            Some(&vec!["model".to_owned(), "engine-core".to_owned()])
        );
        assert!(
            !closure.contains_key("tester"),
            "dev dependencies are not production"
        );
        assert_eq!(workspace.members().count(), 2);
    }
}
