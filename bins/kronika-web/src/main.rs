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
    let config = config::parse_from(std::env::args_os()).unwrap_or_else(|error| error.exit());
    server::run(config)
}

#[cfg(test)]
mod tests;
