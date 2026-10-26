#![forbid(unsafe_code)]

fn main() {
    if let Err(error) = pcagent_elevation::worker_main(std::env::args_os().skip(1)) {
        // The error contains a code only; no job path, nonce, file contents or user identity.
        eprintln!("pcagent-elevated: {error}");
        std::process::exit(match error {
            pcagent_elevation::ElevationError::TimedOut => 124,
            pcagent_elevation::ElevationError::Cancelled => 125,
            _ => 1,
        });
    }
}
