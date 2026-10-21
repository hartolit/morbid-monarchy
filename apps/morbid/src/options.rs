//! The client's configuration: command-line options over a settings file, parsed once into a
//! validated [`ClientOptions`]. Nothing is resolved relative to the working directory: worlds
//! and settings live in the user's data and configuration directories unless an option names a
//! path, so the client behaves the same wherever it is started.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use world_runtime::checkpoint::{AUTOSAVE_TICKS, DEFAULT_AUTOSAVE_TICKS};

pub const USAGE: &str = "\
usage: morbid [options]

  --adapter ID         the GPU adapter to run the world on (as `cargo evidence inspect-adapter`
                       lists them); otherwise the settings file's, otherwise chosen on screen
  --worlds DIR         the directory of saved worlds (default: the user data directory)
  --world NAME|PATH    open a saved world at start
  --settings PATH      the settings file (default: the user configuration directory)
  --window WxH         the window's size in physical pixels
  --vsync on|off       wait for the display's refresh (default on)
  --autosave TICKS     checkpoint every TICKS ticks, 60 to 3600 (default 60: a second at 60 Hz);
                       a crash or a lost device loses at most that and the save in progress
  --help               this text
";

/// The settings file, as written by a person. Unknown keys are refused.
#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SettingsFile {
    pub adapter: Option<String>,
    pub worlds: Option<PathBuf>,
    pub vsync: Option<bool>,
    /// Ticks between autosaves, in `world_runtime::checkpoint::AUTOSAVE_TICKS`.
    pub autosave_ticks: Option<u32>,
}

/// When a world is checkpointed besides on request and at close.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Autosave {
    /// Every this many ticks: the bound on what a crash or a lost device loses (ADR 0004).
    Every(u32),
    /// Never. Only scripted and measured runs (the evidence runner's `native-client`), which
    /// save when their script says, choose this; no option or setting selects it.
    Off,
}

impl Autosave {
    pub fn ticks(&self) -> Option<u64> {
        match self {
            Self::Every(ticks) => Some(u64::from(*ticks)),
            Self::Off => None,
        }
    }
}

/// What the client runs with.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientOptions {
    pub adapter: Option<String>,
    pub worlds: PathBuf,
    pub settings: PathBuf,
    /// A world to open at start: a path, or a name in `worlds`.
    pub open: Option<PathBuf>,
    pub window_size: Option<[u32; 2]>,
    pub vsync: bool,
    pub autosave: Autosave,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptionsError {
    Help,
    Usage(String),
    Settings { path: PathBuf, reason: String },
}

impl std::fmt::Display for OptionsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Help => write!(f, "{USAGE}"),
            Self::Usage(reason) => write!(f, "{reason}\n\n{USAGE}"),
            Self::Settings { path, reason } => write!(f, "{}: {reason}", path.display()),
        }
    }
}

/// The user's configuration and data directories (XDG on Linux, `HOME` fallbacks).
fn base_dir(variable: &str, fallback: &str) -> PathBuf {
    std::env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(fallback)))
        .unwrap_or_else(std::env::temp_dir)
        .join("morbid-monarchy")
}

pub fn default_settings_path() -> PathBuf {
    base_dir("XDG_CONFIG_HOME", ".config").join("client.toml")
}

pub fn default_worlds_dir() -> PathBuf {
    base_dir("XDG_DATA_HOME", ".local/share").join("worlds")
}

/// Reads the settings file; a missing file is empty settings.
pub fn read_settings(path: &Path) -> Result<SettingsFile, OptionsError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SettingsFile::default());
        }
        Err(error) => {
            return Err(OptionsError::Settings {
                path: path.to_owned(),
                reason: error.to_string(),
            });
        }
    };
    toml::from_str(&text).map_err(|error| OptionsError::Settings {
        path: path.to_owned(),
        reason: error.to_string(),
    })
}

/// Records the chosen adapter in the settings file, keeping its other settings.
pub fn remember_adapter(path: &Path, adapter: &str) -> Result<(), String> {
    let mut settings = read_settings(path).map_err(|error| error.to_string())?;
    settings.adapter = Some(adapter.to_owned());
    let mut text = String::from("# Morbid Monarchy client settings.\n");
    text.push_str(&format!("adapter = {:?}\n", adapter));
    if let Some(worlds) = &settings.worlds {
        text.push_str(&format!("worlds = {:?}\n", worlds.display().to_string()));
    }
    if let Some(vsync) = settings.vsync {
        text.push_str(&format!("vsync = {vsync}\n"));
    }
    if let Some(ticks) = settings.autosave_ticks {
        text.push_str(&format!("autosave_ticks = {ticks}\n"));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(path, text).map_err(|error| format!("{}: {error}", path.display()))
}

fn parse_size(text: &str) -> Option<[u32; 2]> {
    let (width, height) = text.split_once(['x', 'X'])?;
    let size = [width.trim().parse().ok()?, height.trim().parse().ok()?];
    (size[0] > 0 && size[1] > 0 && size[0] <= 16_384 && size[1] <= 16_384).then_some(size)
}

/// An autosave interval a person may choose. It cannot be off: the interval bounds what a crash
/// or a lost device loses.
fn parse_autosave(text: &str) -> Option<Autosave> {
    let ticks: u32 = text.parse().ok()?;
    AUTOSAVE_TICKS
        .contains(&u64::from(ticks))
        .then_some(Autosave::Every(ticks))
}

fn autosave_range() -> String {
    format!(
        "{} to {} ticks; autosave cannot be off, it bounds what a crash or a lost device loses",
        AUTOSAVE_TICKS.start(),
        AUTOSAVE_TICKS.end()
    )
}

/// Parses the command line (without the program name) over the settings file it names.
pub fn parse(args: &[String]) -> Result<ClientOptions, OptionsError> {
    let mut adapter = None;
    let mut worlds = None;
    let mut open = None;
    let mut settings_path = None;
    let mut window_size = None;
    let mut vsync = None;
    let mut autosave = None;
    let mut index = 0;
    while index < args.len() {
        let name = args[index].as_str();
        if name == "--help" || name == "-h" {
            return Err(OptionsError::Help);
        }
        let value = || -> Result<&str, OptionsError> {
            args.get(index + 1)
                .map(String::as_str)
                .ok_or_else(|| OptionsError::Usage(format!("`{name}` needs a value")))
        };
        let usage = |what: &str| OptionsError::Usage(format!("`{name}` {what}"));
        match name {
            "--adapter" => adapter = Some(value()?.to_owned()),
            "--worlds" => worlds = Some(PathBuf::from(value()?)),
            "--world" => open = Some(PathBuf::from(value()?)),
            "--settings" => settings_path = Some(PathBuf::from(value()?)),
            "--window" => {
                window_size =
                    Some(parse_size(value()?).ok_or_else(|| usage("must be WIDTHxHEIGHT"))?)
            }
            "--vsync" => {
                vsync = Some(match value()? {
                    "on" => true,
                    "off" => false,
                    _ => return Err(usage("must be on or off")),
                })
            }
            "--autosave" => {
                autosave = Some(
                    parse_autosave(value()?)
                        .ok_or_else(|| usage(&format!("must be {}", autosave_range())))?,
                )
            }
            other => return Err(OptionsError::Usage(format!("unknown option `{other}`"))),
        }
        index += 2;
    }
    let settings_path = settings_path.unwrap_or_else(default_settings_path);
    let settings = read_settings(&settings_path)?;
    let file_autosave = match settings.autosave_ticks {
        None => None,
        Some(ticks) => {
            Some(
                parse_autosave(&ticks.to_string()).ok_or_else(|| OptionsError::Settings {
                    path: settings_path.clone(),
                    reason: format!("autosave_ticks {ticks}: must be {}", autosave_range()),
                })?,
            )
        }
    };
    Ok(ClientOptions {
        adapter: adapter.or(settings.adapter),
        worlds: worlds
            .or(settings.worlds)
            .unwrap_or_else(default_worlds_dir),
        settings: settings_path,
        open,
        window_size,
        vsync: vsync.or(settings.vsync).unwrap_or(true),
        autosave: autosave
            .or(file_autosave)
            .unwrap_or(Autosave::Every(DEFAULT_AUTOSAVE_TICKS as u32)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_owned).collect()
    }

    fn settings_in(dir: &Path, text: &str) -> PathBuf {
        let path = dir.join("client.toml");
        std::fs::write(&path, text).expect("write settings");
        path
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("morbid-options-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    #[test]
    fn options_override_the_settings_file_and_defaults_fill_the_rest() {
        let dir = scratch("override");
        let path = settings_in(
            &dir,
            "adapter = \"gl-1-2\"\nvsync = false\nautosave_ticks = 600\n",
        );
        let line = format!(
            "--settings {} --adapter vulkan-10de-2c05 --window 800x600",
            path.display()
        );
        let options = parse(&args(&line)).expect("valid");
        assert_eq!(options.adapter.as_deref(), Some("vulkan-10de-2c05"));
        assert!(!options.vsync, "the file's vsync applies");
        assert_eq!(options.autosave, Autosave::Every(600));
        assert_eq!(options.window_size, Some([800, 600]));
        assert!(options.worlds.ends_with("morbid-monarchy/worlds"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn unknown_or_malformed_input_is_refused() {
        let dir = scratch("refused");
        let path = settings_in(&dir, "adapter = \"x\"\ncolour = \"blue\"\n");
        assert!(matches!(
            parse(&args(&format!("--settings {}", path.display()))),
            Err(OptionsError::Settings { .. })
        ));
        let empty = settings_in(&dir, "");
        let base = format!("--settings {}", empty.display());
        for bad in [
            "--frobnicate 1",
            "--window 0x10",
            "--vsync maybe",
            "--autosave 10",
            "--autosave 59",
            "--autosave 3601",
            "--autosave off",
            "--adapter",
        ] {
            let line = format!("{base} {bad}");
            assert!(
                matches!(parse(&args(&line)), Err(OptionsError::Usage(_))),
                "{bad} was accepted"
            );
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn autosave_is_on_by_default_and_cannot_be_turned_off() {
        let dir = scratch("autosave");
        let empty = settings_in(&dir, "");
        let options = parse(&args(&format!("--settings {}", empty.display()))).expect("valid");
        assert_eq!(options.autosave, Autosave::Every(60));
        let bounds = format!("--settings {} --autosave 3600", empty.display());
        assert_eq!(
            parse(&args(&bounds)).expect("valid").autosave,
            Autosave::Every(3600)
        );
        for ticks in [0, 59, 3601] {
            let path = settings_in(&dir, &format!("autosave_ticks = {ticks}\n"));
            assert!(
                matches!(
                    parse(&args(&format!("--settings {}", path.display()))),
                    Err(OptionsError::Settings { .. })
                ),
                "autosave_ticks = {ticks} was accepted"
            );
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn the_chosen_adapter_is_remembered_with_the_other_settings() {
        let dir = scratch("remember");
        let path = settings_in(&dir, "vsync = false\n");
        remember_adapter(&path, "vulkan-10de-2c05").expect("written");
        let settings = read_settings(&path).expect("readable");
        assert_eq!(settings.adapter.as_deref(), Some("vulkan-10de-2c05"));
        assert_eq!(settings.vsync, Some(false));
        std::fs::remove_dir_all(dir).ok();
    }
}
