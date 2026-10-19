//! Command-line grammar of `cargo evidence`.
//!
//! The grammar is small and fixed, so it is parsed by hand rather than through an argument
//! framework. Unknown commands, suites and options are usage errors; a suite that is known but
//! not implemented yet parses successfully and then fails explicitly when executed.

use std::path::PathBuf;

use world_model::command::EditTool;

pub const USAGE: &str = "\
usage: cargo evidence <command> [options]

commands:
  check docs                         local links, owners, assignment and package structure
  check static                       formatting, compilation, Clippy, rustdoc, repository contracts
  check all --adapter ID             every lane, including GPU suites and the connected scenario
  test cpu                           workspace tests against the coverage registry
  test gpu --suite NAME --adapter ID GPU suite on an explicit adapter
                                     (suites: cells, transfers, bodies, streaming, render, all)
  inspect-adapter                    enumerate adapters with their identifiers and limits
  run --scenario NAME --adapter ID   connected scenario through the production runtime
  session --store PATH --adapter ID  open a saved world headless, run it and close it in order
        [--ticks N] [--center X,Y]   (saving its last tick); run again to reopen it. --ticks
        [--radius R] [--edit T,X,Y]... (default 600), a new window, edits after it settles
        [--autosave TICKS|off]       (T: water, lava, oil, ice, fire, sand, raise, lower)
  bench --profile NAME --adapter ID  repeated measurements of a frozen profile
        [--case NAME]... [--runs N]  (profiles: qualification, scale, capacity)

options:
  --output DIR                       run directory (default: target/evidence/<time>-<command>)
  -h, --help                         show this text
";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub command: Command,
    pub output: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Help,
    CheckDocs,
    CheckStatic,
    CheckAll { adapter: String },
    TestCpu,
    TestGpu { suite: GpuSuite, adapter: String },
    InspectAdapter,
    Run { scenario: String, adapter: String },
    Session(SessionRequest),
    Bench(BenchRequest),
}

/// A headless session over a saved world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRequest {
    /// As given; relative paths are resolved against the invoking directory.
    pub store: PathBuf,
    pub adapter: String,
    pub ticks: u32,
    /// A window to move to after opening: centre chunk and radius.
    pub window: Option<((i32, i32), u32)>,
    pub edits: Vec<SessionEdit>,
    pub autosave: Autosave,
}

/// An edit applied once the window has settled: a tool at a cell, radius 2, strength 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionEdit {
    pub tool: EditTool,
    pub cell: (i32, i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Autosave {
    /// The session owner's default interval.
    Default,
    Off,
    Every(u64),
}

/// The ticks a session runs when `--ticks` is not given, and the most it accepts.
pub const SESSION_TICKS: u32 = 600;
pub const SESSION_MAX_TICKS: u32 = 216_000;

pub const EDIT_TOOLS: [(&str, EditTool); 8] = [
    ("water", EditTool::Water),
    ("lava", EditTool::Lava),
    ("oil", EditTool::Oil),
    ("ice", EditTool::Ice),
    ("fire", EditTool::Fire),
    ("sand", EditTool::Sand),
    ("raise", EditTool::Raise),
    ("lower", EditTool::Lower),
];

impl SessionEdit {
    pub fn name(self) -> &'static str {
        EDIT_TOOLS
            .iter()
            .find(|(_, tool)| *tool == self.tool)
            .map_or("?", |(name, _)| name)
    }
}

fn session_request(options: &mut Options) -> Result<SessionRequest, UsageError> {
    let store = PathBuf::from(options.require("store")?);
    let adapter = options.require("adapter")?;
    let ticks = match options.take_single("ticks")? {
        Some(text) => text
            .parse::<u32>()
            .ok()
            .filter(|ticks| (1..=SESSION_MAX_TICKS).contains(ticks))
            .ok_or_else(|| {
                UsageError(format!(
                    "--ticks must be 1 to {SESSION_MAX_TICKS}, got `{text}`"
                ))
            })?,
        None => SESSION_TICKS,
    };
    let center = options
        .take_single("center")?
        .map(|text| {
            let parsed = text.split_once(',').and_then(|(x, y)| {
                Some((x.trim().parse::<i32>().ok()?, y.trim().parse::<i32>().ok()?))
            });
            parsed.ok_or_else(|| UsageError(format!("--center must be X,Y, got `{text}`")))
        })
        .transpose()?;
    let radius = options
        .take_single("radius")?
        .map(|text| {
            text.parse::<u32>()
                .ok()
                .filter(|radius| world_model::config::ACTIVE_RADIUS_CHUNKS.contains(radius))
                .ok_or_else(|| {
                    UsageError(format!(
                        "--radius must be {} to {}, got `{text}`",
                        world_model::config::ACTIVE_RADIUS_CHUNKS.start(),
                        world_model::config::ACTIVE_RADIUS_CHUNKS.end()
                    ))
                })
        })
        .transpose()?;
    let window = match (center, radius) {
        (None, None) => None,
        (Some(center), Some(radius)) => Some((center, radius)),
        _ => {
            return Err(UsageError(
                "--center and --radius move the window together".to_owned(),
            ));
        }
    };
    let edits = options
        .take_all("edit")
        .into_iter()
        .map(|text| {
            let mut parts = text.split(',');
            let edit = (|| {
                let name = parts.next()?.trim();
                let tool = EDIT_TOOLS.iter().find(|(tool, _)| *tool == name)?.1;
                let x = parts.next()?.trim().parse::<i32>().ok()?;
                let y = parts.next()?.trim().parse::<i32>().ok()?;
                parts.next().is_none().then_some(SessionEdit { tool, cell: (x, y) })
            })();
            edit.ok_or_else(|| {
                UsageError(format!(
                    "--edit must be TOOL,X,Y with TOOL one of water, lava, oil, ice, fire, sand, raise, lower; got `{text}`"
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let autosave = match options.take_single("autosave")?.as_deref() {
        None => Autosave::Default,
        Some("off") => Autosave::Off,
        Some(text) => text
            .parse::<u64>()
            .ok()
            .filter(|ticks| *ticks > 0)
            .map(Autosave::Every)
            .ok_or_else(|| {
                UsageError(format!(
                    "--autosave must be a positive tick count or `off`, got `{text}`"
                ))
            })?,
    };
    Ok(SessionRequest {
        store,
        adapter,
        ticks,
        window,
        edits,
        autosave,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BenchRequest {
    pub profile: String,
    pub adapter: String,
    /// Empty means every case the profile defines.
    pub cases: Vec<String>,
    /// `None` uses the profile's frozen run count.
    pub runs: Option<u32>,
}

/// GPU suites named by the sprint. Each owner stage registers its implementation; until then
/// executing a suite fails rather than passing vacuously.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuSuite {
    Cells,
    Transfers,
    Bodies,
    Streaming,
    Render,
    All,
}

impl GpuSuite {
    pub const NAMED: [Self; 5] = [
        Self::Cells,
        Self::Transfers,
        Self::Bodies,
        Self::Streaming,
        Self::Render,
    ];

    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "cells" => Self::Cells,
            "transfers" => Self::Transfers,
            "bodies" => Self::Bodies,
            "streaming" => Self::Streaming,
            "render" => Self::Render,
            "all" => Self::All,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Cells => "cells",
            Self::Transfers => "transfers",
            Self::Bodies => "bodies",
            Self::Streaming => "streaming",
            Self::Render => "render",
            Self::All => "all",
        }
    }

    /// The sprint prompt that delivers the suite.
    pub fn owner_stage(self) -> &'static str {
        match self {
            Self::Transfers => "02",
            Self::Cells => "03",
            Self::Bodies => "04",
            Self::Streaming => "05",
            Self::Render => "06",
            Self::All => "02–06",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageError(pub String);

pub fn parse<I, S>(args: I) -> Result<Invocation, UsageError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut words: Vec<String> = Vec::new();
    let mut options: Vec<(String, String)> = Vec::new();
    let mut tokens = args.into_iter().map(Into::into);
    while let Some(token) = tokens.next() {
        if token == "-h" || token == "--help" {
            return Ok(Invocation {
                command: Command::Help,
                output: None,
            });
        }
        if let Some(name) = token.strip_prefix("--") {
            let value = tokens
                .next()
                .filter(|value| !value.starts_with("--"))
                .ok_or_else(|| UsageError(format!("option --{name} needs a value")))?;
            options.push((name.to_owned(), value));
        } else if options.is_empty() {
            words.push(token);
        } else {
            return Err(UsageError(format!(
                "unexpected argument `{token}` after options"
            )));
        }
    }

    let mut options = Options(options);
    let output = options.take_single("output")?.map(PathBuf::from);
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let command = match words.as_slice() {
        [] => return Err(UsageError("missing command".to_owned())),
        ["help"] => Command::Help,
        ["check", "docs"] => Command::CheckDocs,
        ["check", "static"] => Command::CheckStatic,
        ["check", "all"] => Command::CheckAll {
            adapter: options.require("adapter")?,
        },
        ["check", lane] => return Err(UsageError(format!("unknown check lane `{lane}`"))),
        ["test", "cpu"] => Command::TestCpu,
        ["test", "gpu"] => {
            let name = options.require("suite")?;
            let suite = GpuSuite::parse(&name)
                .ok_or_else(|| UsageError(format!("unknown GPU suite `{name}`")))?;
            Command::TestGpu {
                suite,
                adapter: options.require("adapter")?,
            }
        }
        ["test", lane] => return Err(UsageError(format!("unknown test lane `{lane}`"))),
        ["inspect-adapter"] => Command::InspectAdapter,
        ["run"] => Command::Run {
            scenario: options.require("scenario")?,
            adapter: options.require("adapter")?,
        },
        ["session"] => Command::Session(session_request(&mut options)?),
        ["bench"] => {
            let runs = match options.take_single("runs")? {
                Some(text) => Some(
                    text.parse::<u32>()
                        .ok()
                        .filter(|runs| *runs > 0)
                        .ok_or_else(|| {
                            UsageError(format!("--runs must be a positive integer, got `{text}`"))
                        })?,
                ),
                None => None,
            };
            Command::Bench(BenchRequest {
                profile: options.require("profile")?,
                adapter: options.require("adapter")?,
                cases: options.take_all("case"),
                runs,
            })
        }
        other => return Err(UsageError(format!("unknown command `{}`", other.join(" ")))),
    };
    options.reject_remaining()?;
    Ok(Invocation { command, output })
}

struct Options(Vec<(String, String)>);

impl Options {
    fn take_all(&mut self, name: &str) -> Vec<String> {
        let (taken, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.0)
            .into_iter()
            .partition(|(key, _)| key == name);
        self.0 = kept;
        taken.into_iter().map(|(_, value)| value).collect()
    }

    fn take_single(&mut self, name: &str) -> Result<Option<String>, UsageError> {
        let mut values = self.take_all(name);
        match values.len() {
            0 => Ok(None),
            1 => Ok(values.pop()),
            _ => Err(UsageError(format!("option --{name} given more than once"))),
        }
    }

    fn require(&mut self, name: &str) -> Result<String, UsageError> {
        self.take_single(name)?
            .ok_or_else(|| UsageError(format!("missing required option --{name}")))
    }

    fn reject_remaining(self) -> Result<(), UsageError> {
        match self.0.first() {
            Some((name, _)) => Err(UsageError(format!("unknown or misplaced option --{name}"))),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(args: &[&str]) -> Result<Command, UsageError> {
        parse(args.iter().copied()).map(|invocation| invocation.command)
    }

    #[test]
    fn parses_every_documented_command() {
        assert_eq!(command(&["check", "docs"]), Ok(Command::CheckDocs));
        assert_eq!(command(&["check", "static"]), Ok(Command::CheckStatic));
        assert_eq!(
            command(&["check", "all", "--adapter", "a"]),
            Ok(Command::CheckAll {
                adapter: "a".into()
            })
        );
        assert_eq!(command(&["test", "cpu"]), Ok(Command::TestCpu));
        assert_eq!(
            command(&["test", "gpu", "--suite", "cells", "--adapter", "a"]),
            Ok(Command::TestGpu {
                suite: GpuSuite::Cells,
                adapter: "a".into()
            })
        );
        assert_eq!(command(&["inspect-adapter"]), Ok(Command::InspectAdapter));
        assert_eq!(
            command(&["run", "--scenario", "resident-world", "--adapter", "a"]),
            Ok(Command::Run {
                scenario: "resident-world".into(),
                adapter: "a".into()
            })
        );
        assert_eq!(
            command(&[
                "bench",
                "--profile",
                "scale",
                "--adapter",
                "a",
                "--case",
                "idle",
                "--case",
                "flow",
                "--runs",
                "2"
            ]),
            Ok(Command::Bench(BenchRequest {
                profile: "scale".into(),
                adapter: "a".into(),
                cases: vec!["idle".into(), "flow".into()],
                runs: Some(2),
            }))
        );
    }

    #[test]
    fn output_option_applies_to_any_command() {
        let invocation = parse(["check", "docs", "--output", "target/x"]).expect("parse");
        assert_eq!(invocation.output, Some(PathBuf::from("target/x")));
    }

    #[test]
    fn rejects_unknown_or_incomplete_requests() {
        for args in [
            &[][..],
            &["check"][..],
            &["check", "everything"][..],
            &["test", "gpu", "--adapter", "a"][..],
            &["test", "gpu", "--suite", "physics", "--adapter", "a"][..],
            &["test", "gpu", "--suite", "cells"][..],
            &["check", "all"][..],
            &["bench", "--profile", "scale"][..],
            &[
                "bench",
                "--profile",
                "scale",
                "--adapter",
                "a",
                "--runs",
                "0",
            ][..],
            &["check", "docs", "--colour", "always"][..],
            &["check", "docs", "--output"][..],
            &["check", "docs", "--output", "a", "--output", "b"][..],
            &["deploy"][..],
        ] {
            assert!(command(args).is_err(), "accepted {args:?}");
        }
    }

    #[test]
    fn session_options_are_parsed_and_checked() {
        assert_eq!(
            command(&["session", "--store", "w.redb", "--adapter", "a"]),
            Ok(Command::Session(SessionRequest {
                store: "w.redb".into(),
                adapter: "a".into(),
                ticks: SESSION_TICKS,
                window: None,
                edits: Vec::new(),
                autosave: Autosave::Default,
            }))
        );
        assert_eq!(
            command(&[
                "session",
                "--store",
                "/tmp/w.redb",
                "--adapter",
                "a",
                "--ticks",
                "120",
                "--center",
                "-3,4",
                "--radius",
                "2",
                "--edit",
                "raise,10,-20",
                "--edit",
                "water,0,0",
                "--autosave",
                "off",
            ]),
            Ok(Command::Session(SessionRequest {
                store: "/tmp/w.redb".into(),
                adapter: "a".into(),
                ticks: 120,
                window: Some(((-3, 4), 2)),
                edits: vec![
                    SessionEdit {
                        tool: EditTool::Raise,
                        cell: (10, -20),
                    },
                    SessionEdit {
                        tool: EditTool::Water,
                        cell: (0, 0),
                    },
                ],
                autosave: Autosave::Off,
            }))
        );
        assert!(matches!(
            command(&[
                "session",
                "--store",
                "w",
                "--adapter",
                "a",
                "--autosave",
                "90"
            ]),
            Ok(Command::Session(SessionRequest {
                autosave: Autosave::Every(90),
                ..
            }))
        ));
        for args in [
            &["session", "--adapter", "a"][..],
            &["session", "--store", "w"][..],
            &["session", "--store", "w", "--adapter", "a", "--ticks", "0"][..],
            &[
                "session",
                "--store",
                "w",
                "--adapter",
                "a",
                "--ticks",
                "many",
            ][..],
            &[
                "session",
                "--store",
                "w",
                "--adapter",
                "a",
                "--center",
                "1,2",
            ][..],
            &["session", "--store", "w", "--adapter", "a", "--radius", "2"][..],
            &[
                "session",
                "--store",
                "w",
                "--adapter",
                "a",
                "--center",
                "1",
                "--radius",
                "2",
            ][..],
            &[
                "session",
                "--store",
                "w",
                "--adapter",
                "a",
                "--center",
                "1,2",
                "--radius",
                "33",
            ][..],
            &[
                "session",
                "--store",
                "w",
                "--adapter",
                "a",
                "--edit",
                "paint,1,2",
            ][..],
            &[
                "session",
                "--store",
                "w",
                "--adapter",
                "a",
                "--edit",
                "raise,1",
            ][..],
            &[
                "session",
                "--store",
                "w",
                "--adapter",
                "a",
                "--edit",
                "raise,1,2,3",
            ][..],
            &[
                "session",
                "--store",
                "w",
                "--adapter",
                "a",
                "--autosave",
                "0",
            ][..],
            &["session", "--store", "w", "--store", "v", "--adapter", "a"][..],
            &["session", "--store", "w", "--adapter", "a", "--seed", "1"][..],
        ] {
            assert!(command(args).is_err(), "accepted {args:?}");
        }
    }

    #[test]
    fn every_named_suite_has_an_owner_stage() {
        for suite in GpuSuite::NAMED {
            assert_eq!(GpuSuite::parse(suite.name()), Some(suite));
            assert!(!suite.owner_stage().is_empty());
        }
    }
}
