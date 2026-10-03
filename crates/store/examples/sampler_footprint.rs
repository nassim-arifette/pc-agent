//! Offline sampler benchmark. Only footprint/cadence are printed; the database stays local.
use pcagent_store::sampler::Sampler;
use std::{path::PathBuf, time::Duration};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(
        args.next()
            .ok_or("Provide a local benchmark database path")?,
    );
    let seconds = args
        .next()
        .map(|s| s.parse::<u64>())
        .transpose()?
        .unwrap_or(60)
        .clamp(10, 3600);
    let sampler = Sampler::start(&path)?;
    std::thread::sleep(Duration::from_secs(seconds));
    sampler.stop();
    println!("{}", serde_json::to_string_pretty(&sampler.status())?);
    Ok(())
}
