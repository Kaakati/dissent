use anyhow::Result;
use clap::Parser;
use dissent_seeder::{Cli, Command, SPEC, run_blocking, run_from_args, service, service_command};
use std::sync::Arc;
use tokio::sync::Notify;

fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(Command::Service { action }) = cli.command {
        return service_command(action);
    }
    // Started by the Windows Service Control Manager, this runs as a service. Started from a
    // console, it returns `None` and the seeder runs as an ordinary program.
    #[cfg(windows)]
    if let Some(result) = service::scm::run_if_service(&SPEC, run_from_args) {
        return result;
    }
    run_blocking(cli.run, Arc::new(Notify::new()))
}
