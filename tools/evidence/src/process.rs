//! Nested command execution with captured logs and preserved exit status.
//!
//! Every external step (cargo, git, a test binary) runs through [`run_logged`]. A nonzero exit
//! becomes a failed item, and a program that cannot be started becomes unavailable. Neither
//! case can be reported as passed, so a failure inside a nested Cargo invocation always reaches
//! the runner's own exit status.

use std::{
    ffi::OsString,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::report::State;

/// A command to execute: program, arguments, working directory and extra environment.
#[derive(Debug, Clone)]
pub struct Step {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    pub env: Vec<(OsString, OsString)>,
}

impl Step {
    pub fn new(program: impl Into<OsString>, cwd: &Path) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: cwd.to_path_buf(),
            env: Vec::new(),
        }
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// The command line as it would be typed, for logs and reports.
    pub fn display(&self) -> String {
        let mut parts: Vec<String> = self
            .env
            .iter()
            .map(|(key, value)| format!("{}={}", key.to_string_lossy(), value.to_string_lossy()))
            .collect();
        parts.push(self.program.to_string_lossy().into_owned());
        parts.extend(
            self.args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned()),
        );
        parts.join(" ")
    }
}

/// The outcome of one executed step.
#[derive(Debug)]
pub struct Outcome {
    pub state: State,
    /// Exit code when the process ran and exited normally.
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    /// Log file relative to the run directory.
    pub log: Option<String>,
    /// Why the step is not `Passed`, when it is not.
    pub reason: Option<String>,
}

/// Runs `step`, writing its command line, exit status and both output streams to
/// `run_dir/logs/<log_name>.log`.
pub fn run_logged(step: &Step, run_dir: &Path, log_name: &str) -> Outcome {
    let mut command = Command::new(&step.program);
    command
        .args(&step.args)
        .current_dir(&step.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in &step.env {
        command.env(key, value);
    }

    let result = command.output();
    let (state, exit_code, stdout, stderr, reason) = match result {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            let code = output.status.code();
            if output.status.success() {
                (State::Passed, code, stdout, stderr, None)
            } else {
                let reason = match code {
                    Some(code) => format!("exited with status {code}"),
                    None => "terminated by a signal".to_owned(),
                };
                (State::Failed, code, stdout, stderr, Some(reason))
            }
        }
        Err(error) => (
            State::Unavailable,
            None,
            String::new(),
            String::new(),
            Some(format!(
                "could not start `{}`: {error}",
                step.program.to_string_lossy()
            )),
        ),
    };

    let log = write_log(
        run_dir,
        log_name,
        step,
        exit_code,
        reason.as_deref(),
        &stdout,
        &stderr,
    );
    Outcome {
        state,
        exit_code,
        stdout,
        stderr,
        log,
        reason,
    }
}

fn write_log(
    run_dir: &Path,
    log_name: &str,
    step: &Step,
    exit_code: Option<i32>,
    reason: Option<&str>,
    stdout: &str,
    stderr: &str,
) -> Option<String> {
    let relative = format!("logs/{log_name}.log");
    let path = run_dir.join(&relative);
    let parent = path.parent()?;
    fs::create_dir_all(parent).ok()?;
    let mut file = File::create(&path).ok()?;
    let status = match (exit_code, reason) {
        (Some(code), _) => format!("exit status {code}"),
        (None, Some(reason)) => reason.to_owned(),
        (None, None) => "unknown status".to_owned(),
    };
    writeln!(
        file,
        "$ {}\n# cwd: {}\n# {status}\n\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        step.display(),
        step.cwd.display()
    )
    .ok()?;
    Some(relative)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "monarchy-evidence-process-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create scratch directory");
        dir
    }

    #[test]
    fn nonzero_exit_fails_and_is_logged() {
        let dir = scratch("nonzero");
        let step = Step::new("sh", &dir).args(["-c", "echo visible; exit 3"]);
        let outcome = run_logged(&step, &dir, "failing");
        assert_eq!(outcome.state, State::Failed);
        assert_eq!(outcome.exit_code, Some(3));
        let log = fs::read_to_string(dir.join(outcome.log.expect("log path"))).expect("log");
        assert!(log.contains("exit status 3") && log.contains("visible"));
    }

    #[test]
    fn success_passes() {
        let dir = scratch("success");
        let outcome = run_logged(&Step::new("true", &dir), &dir, "ok");
        assert_eq!(outcome.state, State::Passed);
    }

    #[test]
    fn missing_program_is_unavailable_not_passed() {
        let dir = scratch("missing");
        let step = Step::new("monarchy-evidence-no-such-program", &dir);
        let outcome = run_logged(&step, &dir, "missing");
        assert_eq!(outcome.state, State::Unavailable);
        assert!(outcome.reason.expect("reason").contains("could not start"));
    }
}
