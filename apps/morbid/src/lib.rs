//! The native client of Morbid Monarchy: a window, the world's GPU owner and renderer on one
//! device, the runtime's session over the saved world, and the tools that turn input into
//! commands.
//!
//! - [`options`]: command line and settings file, validated once;
//! - [`catalog`]: the saved worlds (list, create, import a baseline save);
//! - [`live`]: a world's session over its store and the GPU owner, close, loss and recovery;
//! - [`present`]: the window's surface, the overlay, and the menu device used while no world
//!   is open;
//! - [`input`], [`observer`], [`play`]: input, the viewpoint, and the tools as commands;
//! - [`ui`]: the overlay's screens and panels;
//! - [`client`]: the window's event loop and the phases of an open world;
//! - [`driver`]: scripted runs of the real client for evidence;
//! - [`viewer`]: the window showing a session a script drives (the visible scenarios).

pub mod catalog;
pub mod client;
pub mod driver;
pub mod input;
pub mod live;
pub mod observer;
pub mod options;
pub mod play;
pub mod present;
pub mod ui;
pub mod viewer;

use std::process::ExitCode;

/// The `morbid` executable: parses the command line and runs the client.
pub fn main_with_args(args: &[String]) -> ExitCode {
    let options = match options::parse(args) {
        Ok(options) => options,
        Err(options::OptionsError::Help) => {
            print!("{}", options::USAGE);
            return ExitCode::SUCCESS;
        }
        Err(error @ options::OptionsError::Usage(_)) => {
            eprintln!("morbid: {error}");
            return ExitCode::from(2);
        }
        Err(error) => {
            eprintln!("morbid: {error}");
            return ExitCode::FAILURE;
        }
    };
    match client::run(options, None) {
        Ok(summary) if summary.errors.is_empty() => ExitCode::SUCCESS,
        Ok(summary) => {
            for error in &summary.errors {
                eprintln!("morbid: {error}");
            }
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("morbid: {error}");
            ExitCode::FAILURE
        }
    }
}
