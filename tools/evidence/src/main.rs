use std::process::ExitCode;

use monarchy_evidence::{EXIT_USAGE, cli, execute, find_root};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let invocation = match cli::parse(args.iter().cloned()) {
        Ok(invocation) => invocation,
        Err(cli::UsageError(message)) => {
            eprintln!("error: {message}\n\n{}", cli::USAGE);
            return ExitCode::from(EXIT_USAGE as u8);
        }
    };
    let status = execute(&invocation, &args, &find_root());
    ExitCode::from(u8::try_from(status).unwrap_or(1))
}
