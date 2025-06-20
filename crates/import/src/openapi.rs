//! Import request collections from an OpenAPI v3.0 or v3.1 specification.
//!
//! - Servers are mapped to profiles
//!     - URL of the server is stored in the `host` field
//! - Operations (i.e. path-method pairs) are mapped to recipes
//! - Tags are mapped to folders
//!     - Since tags are m2m but folders are o2m, we only take the first tag
//! - References are resolved within the same file. We don't support resolving
//!   from other files.
//!
//! OpenAPI is not semver compliant (a change they helpfully made in in a minor
//! version), and 3.1 is not backward compatible with 3.0. We have two separate
//! importers because each we use one library that only supports 3.0 and one
//! that only supports 3.1.

mod resolve;
mod v3_0;
mod v3_1;

use crate::ImportInput;
use anyhow::{Context, anyhow};
use slumber_core::collection::Collection;
use slumber_util::NEW_ISSUE_LINK;
use tracing::warn;

/// Loads a collection from an OpenAPI v3 specification file
pub async fn from_openapi(input: &ImportInput) -> anyhow::Result<Collection> {
    warn!(
        "The OpenAPI importer is approximate. Some features are missing \
            and it may not give you an equivalent or fulling functional
            collection. If you encounter a bug or would like to request support
            for a particular OpenAPI feature, please open an issue:
            {NEW_ISSUE_LINK}"
    );

    // Read the spec into YAML and use the `version` field to determine which
    // importer to use. The format can be YAML or JSON, so we can just treat it
    // all as YAML
    let content = input.load().await?;
    let yaml = serde_yaml::from_str(&content)
        .context("Error deserializing OpenAPI collection")?;

    let version =
        get_version(&yaml).ok_or_else(|| anyhow!("Missing OpenAPI version"))?;
    if version.starts_with("3.0.") {
        v3_0::from_openapi_v3_0(yaml)
    } else if version.starts_with("3.1.") {
        v3_1::from_openapi_v3_1(yaml)
    } else {
        Err(anyhow!(
            "Unsupported OpenAPI version. Supported versions are: 3.0, 3.1"
        ))
    }
}

fn get_version(yaml: &serde_yaml::Value) -> Option<&str> {
    if let serde_yaml::Value::Mapping(mapping) = yaml {
        mapping.get("openapi").and_then(|v| v.as_str())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use slumber_core::collection::Collection;
    use slumber_util::test_data_dir;
    use std::path::PathBuf;

    const OPENAPI_V3_0_FILE: &str = "openapi_v3_0_petstore.yml";
    const OPENAPI_V3_0_IMPORTED_FILE: &str =
        "openapi_v3_0_petstore_imported.yml";

    const OPENAPI_V3_1_FILE: &str = "openapi_v3_1_petstore.yml";
    const OPENAPI_V3_1_IMPORTED_FILE: &str =
        "openapi_v3_1_petstore_imported.yml";

    /// Catch-all test for OpenAPI v3.0 import
    #[rstest]
    #[tokio::test]
    async fn test_openapiv3_0_import(test_data_dir: PathBuf) {
        let input = ImportInput::Path(test_data_dir.join(OPENAPI_V3_0_FILE));
        let imported = from_openapi(&input).await.unwrap();
        let expected =
            Collection::load(&test_data_dir.join(OPENAPI_V3_0_IMPORTED_FILE))
                .unwrap();
        assert_eq!(imported, expected);
    }

    /// Catch-all test for OpenAPI v3.1 import
    #[rstest]
    #[tokio::test]
    async fn test_openapiv3_1_import(test_data_dir: PathBuf) {
        let input = ImportInput::Path(test_data_dir.join(OPENAPI_V3_1_FILE));
        let imported = from_openapi(&input).await.unwrap();
        let expected =
            Collection::load(&test_data_dir.join(OPENAPI_V3_1_IMPORTED_FILE))
                .unwrap();
        assert_eq!(imported, expected);
    }
}
