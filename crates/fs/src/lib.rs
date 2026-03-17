#![forbid(unsafe_code)]
#![deny(clippy::all)]

use slumber_core::collection::{Collection, CollectionFile};
use std::path::PathBuf;

/// TODO
/// TODO better name
pub struct SlumberFs {
    /// TODO
    collection: Collection,
    /// TODO
    mount_path: PathBuf,
}

impl SlumberFs {
    /// TODO
    pub fn new(
        collection_path: Option<PathBuf>,
        mount_path: PathBuf,
    ) -> anyhow::Result<Self> {
        let collection_file = CollectionFile::new(None)?;
        let collection = collection_file.load()?;
        Ok(Self {
            collection,
            mount_path,
        })
    }

    /// TODO
    pub async fn run(self) -> anyhow::Result<()> {
        todo!()
    }
}
