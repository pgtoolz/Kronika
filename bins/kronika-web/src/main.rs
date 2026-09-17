//! Kronika HTTP server entry point.

mod api;
mod auth;
mod body;
mod budget;
mod config;
mod encoding;
mod export;
mod help;
mod mcp;
mod query_adapter;
mod request;
mod response;
mod route;
mod server;
mod streaming;
mod ui;

use anyhow::Result;

fn main() -> Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    if let Some(argument) = arguments.next() {
        anyhow::ensure!(
            arguments.next().is_none(),
            "unexpected arguments; use kronika-web --help"
        );
        if argument == "--help" || argument == "-h" {
            print!("{}", help::HELP);
            return Ok(());
        }
        if argument == "--version" {
            println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        anyhow::bail!(
            "unexpected argument {}; use kronika-web --help",
            argument.display()
        );
    }
    server::run()
}

#[cfg(test)]
mod tests;
