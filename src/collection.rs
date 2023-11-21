//! A request collection defines recipes, profiles, etc. that make requests
//! possible

mod insomnia;

use crate::template::Template;
use anyhow::Context;
use derive_more::{Deref, Display, From};
use indexmap::IndexMap;
use serde::{
    de::{EnumAccess, VariantAccess},
    Deserialize, Deserializer, Serialize,
};
use serde_json_path::JsonPath;
use std::{
    fmt,
    fmt::Debug,
    future::Future,
    marker::PhantomData,
    path::{Path, PathBuf},
};
use tokio::fs;
use tracing::{info, warn};

/// The support file names to be automatically loaded as a config. We only
/// support loading from one file at a time, so if more than one of these is
/// defined, we'll take the earliest and print a warning.
pub const CONFIG_FILES: &[&str] = &[
    "slumber.yml",
    "slumber.yaml",
    ".slumber.yml",
    ".slumber.yaml",
];

/// A collection of requests
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RequestCollection<S = PathBuf> {
    /// The source of the collection, typically a path to the file it was
    /// loaded from
    #[serde(skip)]
    source: S,

    /// Unique ID for this collection. This should be unique for across all
    /// collections used on one computer.
    pub id: CollectionId,
    #[serde(default)]
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub chains: Vec<Chain>,
    /// Internally we call these recipes, but to a user `requests` is more
    /// intuitive
    #[serde(default, rename = "requests")]
    pub recipes: Vec<RequestRecipe>,
}

/// A unique ID for a collection. This is necessary to differentiate between
/// responses from different collections in the repository.
#[derive(Clone, Debug, Default, Display, From, Serialize, Deserialize)]
pub struct CollectionId(String);

/// Mutually exclusive hot-swappable config group
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    pub id: ProfileId,
    pub name: Option<String>,
    pub data: IndexMap<String, ProfileValue>,
}

#[derive(
    Clone,
    Debug,
    Deref,
    Default,
    Display,
    Eq,
    From,
    Hash,
    PartialEq,
    Serialize,
    Deserialize,
)]
pub struct ProfileId(String);

/// The value type of a profile's data mapping
#[derive(Clone, Debug, Serialize)]
#[cfg_attr(test, derive(PartialEq))]
#[serde(rename_all = "snake_case")]
pub enum ProfileValue {
    /// A raw text string
    Raw(String),
    /// A nested template, which allows for recursion. By requiring the user to
    /// declare this up front, we can parse the template during collection
    /// deserialization. It also keeps a cap on the complexity of nested
    /// templates, which is a balance between usability and simplicity (both
    /// for the user and the code).
    Template(Template),
}

/// A definition of how to make a request. This is *not* called `Request` in
/// order to distinguish it from a single instance of an HTTP request. And it's
/// not called `RequestTemplate` because the word "template" has a specific
/// meaning related to string interpolation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequestRecipe {
    pub id: RequestRecipeId,
    pub name: Option<String>,
    /// *Not* a template string because the usefulness doesn't justify the
    /// complexity
    pub method: String,
    pub url: Template,
    pub body: Option<Template>,
    #[serde(default)]
    pub query: IndexMap<String, Template>,
    #[serde(default)]
    pub headers: IndexMap<String, Template>,
}

#[derive(
    Clone,
    Debug,
    Deref,
    Default,
    Display,
    Eq,
    From,
    Hash,
    PartialEq,
    Serialize,
    Deserialize,
)]
pub struct RequestRecipeId(String);

/// A chain is a means to data from one response in another request. The chain
/// is the middleman: it defines where and how to pull the value, then recipes
/// can use it in a template via `{{chains.<chain_id>}}`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Chain {
    pub id: ChainId,
    pub source: ChainSource,
    /// Mask chained value in the UI
    #[serde(default)]
    pub sensitive: bool,
    /// JSONpath to extract a value from the response. For JSON data only.
    pub selector: Option<JsonPath>,
}

#[derive(
    Clone,
    Debug,
    Deref,
    Default,
    Display,
    Eq,
    From,
    Hash,
    PartialEq,
    Serialize,
    Deserialize,
)]
pub struct ChainId(String);

impl From<&str> for ChainId {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}

/// The source of data for a chain
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainSource {
    /// Load data from the most recent response of a particular request recipe
    Request(RequestRecipeId),
    /// Run an external command to get a result
    Command(Vec<String>),
    /// Load data from a file
    File(PathBuf),
    /// Prompt the user for a value, with an optional label
    Prompt(Option<String>),
}

impl<S> RequestCollection<S> {
    /// Replace the source value on this collection
    pub fn with_source<T>(self, source: T) -> RequestCollection<T> {
        RequestCollection {
            source,
            id: self.id,
            profiles: self.profiles,
            chains: self.chains,
            recipes: self.recipes,
        }
    }
}

impl RequestCollection<PathBuf> {
    /// Load config from the given file. The caller is responsible for using
    /// [Self::detect_path] to find the file themself. This pattern enables the
    /// TUI to start up and watch the collection file, even if it's invalid.
    pub async fn load(path: PathBuf) -> Result<Self, anyhow::Error> {
        // Figure out which file we want to load from
        info!(?path, "Loading collection file");

        // First, parse the file to raw YAML values, so we can apply
        // anchor/alias merging. Then parse that to our config type
        let future = async {
            let content = fs::read(&path).await?;
            let mut yaml_value =
                serde_yaml::from_slice::<serde_yaml::Value>(&content)?;
            yaml_value.apply_merge()?;
            Ok::<RequestCollection, anyhow::Error>(serde_yaml::from_value(
                yaml_value,
            )?)
        };

        Ok(future
            .await
            .with_context(|| format!("Error loading collection from {path:?}"))?
            .with_source(path))
    }

    /// Reload a new collection from the same file used for this one.
    ///
    /// Returns `impl Future` to unlink the future from `&self`'s lifetime.
    pub fn reload(&self) -> impl Future<Output = Result<Self, anyhow::Error>> {
        Self::load(self.source.clone())
    }

    /// Get the path of the file that this collection was loaded from
    pub fn path(&self) -> &Path {
        &self.source
    }

    /// Search the current directory for a config file matching one of the known
    /// file names, and return it if found
    pub fn detect_path() -> Option<PathBuf> {
        let paths: Vec<&Path> = CONFIG_FILES
            .iter()
            .map(Path::new)
            // This could be async but I'm being lazy and skipping it for now,
            // since we only do this at startup anyway (mid-process reloading
            // reuses the detected path so we don't re-detect)
            .filter(|p| p.exists())
            .collect();
        match paths.as_slice() {
            [] => None,
            [path] => Some(path.to_path_buf()),
            [first, rest @ ..] => {
                // Print a warning, but don't actually fail
                warn!(
                    "Multiple config files detected. {first:?} will be used \
                    and the following will be ignored: {rest:?}"
                );
                Some(first.to_path_buf())
            }
        }
    }
}

impl Profile {
    /// Get a presentable name for this profile
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

impl From<String> for ProfileValue {
    fn from(value: String) -> Self {
        Self::Raw(value)
    }
}

impl From<&str> for ProfileValue {
    fn from(value: &str) -> Self {
        Self::Raw(value.into())
    }
}

impl RequestRecipe {
    /// Get a presentable name for this recipe
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

/// Deserialize a string OR enum into a ProfileValue. This is based on the
/// generated derive code, with extra logic to default to !raw for a string.
impl<'de> Deserialize<'de> for ProfileValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        const VARIANTS: &[&str] = &["raw", "template"];

        enum Field {
            Raw,
            Template,
        }

        struct FieldVisitor;
        impl<'de> serde::de::Visitor<'de> for FieldVisitor {
            type Value = Field;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "variant identifier")
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    0u64 => Ok(Field::Raw),
                    1u64 => Ok(Field::Template),
                    _ => Err(serde::de::Error::invalid_value(
                        serde::de::Unexpected::Unsigned(value),
                        &"variant index 0 <= i < 2",
                    )),
                }
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "raw" => Ok(Field::Raw),
                    "template" => Ok(Field::Template),
                    _ => {
                        Err(serde::de::Error::unknown_variant(value, VARIANTS))
                    }
                }
            }

            fn visit_bytes<E>(self, value: &[u8]) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value {
                    b"raw" => Ok(Field::Raw),
                    b"template" => Ok(Field::Template),
                    _ => {
                        let value = String::from_utf8_lossy(value);
                        Err(serde::de::Error::unknown_variant(&value, VARIANTS))
                    }
                }
            }
        }

        impl<'de> serde::Deserialize<'de> for Field {
            #[inline]
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                serde::Deserializer::deserialize_identifier(
                    deserializer,
                    FieldVisitor,
                )
            }
        }

        struct Visitor<'de> {
            lifetime: PhantomData<&'de ()>,
        }

        impl<'de> serde::de::Visitor<'de> for Visitor<'de> {
            type Value = ProfileValue;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "enum ProfileValue or string",)
            }

            fn visit_enum<A>(self, data: A) -> Result<Self::Value, A::Error>
            where
                A: EnumAccess<'de>,
            {
                match EnumAccess::variant(data)? {
                    (Field::Raw, variant) => Result::map(
                        VariantAccess::newtype_variant::<String>(variant),
                        ProfileValue::Raw,
                    ),
                    (Field::Template, variant) => Result::map(
                        VariantAccess::newtype_variant::<Template>(variant),
                        ProfileValue::Template,
                    ),
                }
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(ProfileValue::Raw(value.into()))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(ProfileValue::Raw(value))
            }
        }

        deserializer.deserialize_any(Visitor {
            lifetime: PhantomData,
        })
    }
}
