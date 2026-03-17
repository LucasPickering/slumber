use anyhow::anyhow;
use slumber_fs::SlumberFs;
use slumber_util::initialize_tracing;
use std::{env, path::PathBuf};
use tracing::level_filters::LevelFilter;

/// DEVELOPMENT ONLY: Run the filesystem frontend
///
/// We don't have clap in this crate, so arguments are passed as environment
/// variables:
/// - `COLLECTION_PATH`: Collection file path
/// - `MOUNT_PATH`: Directory to mount the filesystem to
/// - `LOG`: Log level
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let collection_path: Option<PathBuf> =
        env::var("COLLECTION_PATH").ok().map(PathBuf::from);
    let mount_path: PathBuf = env::var("MOUNT_PATH")
        .map_err(|_| anyhow!("Missing MOUNT_PATH"))?
        .into();
    let log_level: LevelFilter = env::var("LOG")
        .map(|value| value.parse().unwrap())
        .unwrap_or(LevelFilter::INFO);

    initialize_tracing(log_level, true);

    let fs = SlumberFs::new(collection_path, mount_path)?;
    fs.run().await
}
