mod auth;
mod cli;
mod daemon;
mod filters;
mod ipc;
mod jobs;
mod model;
mod providers;
mod rc;
mod storage;
fn main() {
    if let Err(error) = cli::main() {
        eprintln!("omarchy-cloudkit: {}", storage::safe(&error));
        std::process::exit(1);
    }
}
