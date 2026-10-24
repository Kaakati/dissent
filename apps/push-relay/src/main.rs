use anyhow::Result;
use clap::Parser;
use dissent_push_relay::run::{Cli, Command, run_blocking, service_command};
#[cfg(windows)]
use dissent_push_relay::run::{SPEC, run_from_args};
#[cfg(windows)]
use dissent_push_relay::service;
use std::sync::Arc;
use tokio::sync::Notify;

fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(Command::Service { action }) = cli.command {
        return service_command(action);
    }
    // Started by the Windows Service Control Manager, this runs as a service. Started from a
    // console, it returns `None` and the relay runs as an ordinary program.
    #[cfg(windows)]
    if let Some(result) = service::scm::run_if_service(&SPEC, run_from_args) {
        return result;
    }
    run_blocking(cli.run, Arc::new(Notify::new()))
}
