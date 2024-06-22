//! A request collection defines recipes, profiles, etc. that make requests
//! possible

mod cereal;
mod insomnia;
mod models;
mod recipe_tree;

pub use cereal::HasId;
pub use models::*;
pub use recipe_tree::*;

use crate::util::{parse_yaml, ResultExt};
use anyhow::{anyhow, Context};
use itertools::Itertools;
use std::{
    env,
    fmt::Debug,
    fs,
    future::Future,
    path::{Path, PathBuf},
};
use tokio::task;
use tracing::{info, trace, warn};

/// The support file names to be automatically loaded as a config. We only
/// support loading from one file at a time, so if more than one of these is
/// defined, we'll take the earliest and print a warning.
pub const CONFIG_FILES: &[&str] = &[
    "slumber.yml",
    "slumber.yaml",
    ".slumber.yml",
    ".slumber.yaml",
];

/// A wrapper around a request collection, to handle functionality around the
/// file system.
#[derive(Debug)]
pub struct CollectionFile {
    /// Path to the file that this collection was loaded from
    path: PathBuf,
    pub collection: Collection,
}

impl CollectionFile {
    /// Create a new collection file with the given path and a default
    /// collection. Useful when the collection failed to load and you want a
    /// placeholder.
    pub fn with_path(path: PathBuf) -> Self {
        Self {
            path,
            collection: Default::default(),
        }
    }

    /// Load config from the given file. The caller is responsible for using
    /// [Self::try_path] to find the file themself. This pattern enables the
    /// TUI to start up and watch the collection file, even if it's invalid.
    pub async fn load(path: PathBuf) -> anyhow::Result<Self> {
        let collection = load_collection(path.clone()).await?;
        Ok(Self { path, collection })
    }

    /// Reload a new collection from the same file used for this one.
    ///
    /// Returns `impl Future` to unlink the future from `&self`'s lifetime.
    pub fn reload(&self) -> impl Future<Output = anyhow::Result<Collection>> {
        load_collection(self.path.clone())
    }

    /// Get the path of the file that this collection was loaded from
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Get the path to the collection file, returning an error if none is
    /// available. This will use the override if given, otherwise it will fall
    /// back to searching the given directory for a collection. If the directory
    /// to search is not given, default to the current directory. This is
    /// configurable just for testing.
    pub fn try_path(
        dir: Option<PathBuf>,
        override_path: Option<PathBuf>,
    ) -> anyhow::Result<PathBuf> {
        let dir = if let Some(dir) = dir {
            dir
        } else {
            env::current_dir()?
        };
        override_path
            .map(|override_path| dir.join(override_path))
            .or_else(|| detect_path(&dir)).ok_or_else(|| {
                anyhow!("No collection file found in current or ancestor directories")
            })
    }

    /// Create a new file with a placeholder path for testing
    #[cfg(test)]
    pub fn testing(collection: Collection) -> Self {
        Self {
            path: PathBuf::default(),
            collection,
        }
    }
}

/// Search the current directory for a config file matching one of the known
/// file names, and return it if found
fn detect_path(dir: &Path) -> Option<PathBuf> {
    /// Search a directory and its parents for the collection file. Return None
    /// only if we got through the whole tree and couldn't find it
    fn search_all(dir: &Path) -> Option<PathBuf> {
        search(dir).or_else(|| {
            let parent = dir.parent()?;
            search_all(parent)
        })
    }

    /// Search a single directory for a collection file
    fn search(dir: &Path) -> Option<PathBuf> {
        trace!("Scanning for collection file in {dir:?}");

        let paths = CONFIG_FILES
            .iter()
            .map(|file| dir.join(file))
            // This could be async but I'm being lazy and skipping it for now,
            // since we only do this at startup anyway (mid-process reloading
            // reuses the detected path so we don't re-detect)
            .filter(|p| p.exists())
            .collect_vec();
        match paths.as_slice() {
            [] => None,
            [first, rest @ ..] => {
                if !rest.is_empty() {
                    warn!(
                        "Multiple collection files detected. {first:?} will be \
                            used and the following will be ignored: {rest:?}"
                    );
                }

                trace!("Found collection file at {first:?}");
                Some(first.to_path_buf())
            }
        }
    }

    // Walk *up* the tree until we've hit the root
    search_all(dir)
}

/// Load a collection from the given file. Takes an owned path because it
/// needs to be passed to a future
async fn load_collection(path: PathBuf) -> anyhow::Result<Collection> {
    info!(?path, "Loading collection file");
    // A bit pessimistic, huh... This gets around some lifetime struggles
    let error_context = format!("Error loading data from {path:?}");

    // YAML parsing is blocking so do it in a different thread. We could use
    // tokio::fs for this but that just uses std::fs underneath anyway.
    let result =
        task::spawn_blocking::<_, anyhow::Result<Collection>>(move || {
            let bytes = fs::read(path)?;
            let collection = parse_yaml(&bytes)?;
            Ok(collection)
        })
        .await;

    // Flatten the join error result into the inner task result. Result::flatten
    // is experimental :(
    // https://doc.rust-lang.org/std/result/enum.Result.html#method.flatten
    let result = match result {
        Ok(result) => result,
        Err(error) => Err(error.into()),
    };

    result.context(error_context).traced()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        http::ContentType,
        test_util::{assert_err, by_id, temp_dir, test_data_dir, TempDir},
    };
    use indexmap::indexmap;
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use serde::de::IgnoredAny;
    use serde_json::json;
    use std::{fs::File, time::Duration};

    /// Test various cases of try_path
    #[rstest]
    #[case::parent_only(None, true, false, "slumber.yml")]
    #[case::child_only(None, false, true, "child/slumber.yml")]
    #[case::parent_and_child(None, true, true, "child/slumber.yml")]
    #[case::overriden(Some("override.yml"), true, true, "child/override.yml")]
    fn test_try_path(
        temp_dir: TempDir,
        #[case] override_file: Option<&str>,
        #[case] has_parent: bool,
        #[case] has_child: bool,
        #[case] expected: &str,
    ) {
        let child_dir = temp_dir.join("child");
        fs::create_dir(&child_dir).unwrap();
        let file = "slumber.yml";
        if has_parent {
            File::create(temp_dir.join(file)).unwrap();
        }
        if has_child {
            File::create(child_dir.join(file)).unwrap();
        }
        if let Some(override_file) = override_file {
            File::create(temp_dir.join(override_file)).unwrap();
        }
        let expected: PathBuf = temp_dir.join(expected);

        let override_file = override_file.map(PathBuf::from);
        assert_eq!(
            CollectionFile::try_path(Some(child_dir), override_file).unwrap(),
            expected
        );
    }

    /// Test that try_path fails when no collection file is found and no
    /// override is given
    #[rstest]
    fn test_try_path_error(temp_dir: TempDir) {
        assert_err!(
            CollectionFile::try_path(Some(temp_dir.to_path_buf()), None),
            "No collection file found in current or ancestor directories"
        );
        drop(temp_dir); // Dropping deletes the directory
    }

    /// A catch-all regression test, to make sure we don't break anything in the
    /// collection format. This lives at the bottom because it's huge.
    #[rstest]
    #[tokio::test]
    async fn test_regression(test_data_dir: PathBuf) {
        let loaded = CollectionFile::load(test_data_dir.join("regression.yml"))
            .await
            .unwrap()
            .collection;
        let expected = Collection {
            profiles: by_id([
                Profile {
                    id: "profile1".into(),
                    name: Some("Profile 1".into()),
                    data: indexmap! {
                        "user_guid".into() => "abc123".into(),
                        "username".into() => "xX{{chains.username}}Xx".into(),
                        "host".into() => "https://httpbin.org".into(),

                    },
                },
                Profile {
                    id: "profile2".into(),
                    name: Some("Profile 2".into()),
                    data: indexmap! {
                        "host".into() => "https://httpbin.org".into(),

                    },
                },
            ]),
            chains: by_id([
                Chain {
                    id: "command".into(),
                    source: ChainSource::command(["whoami"]),
                    sensitive: false,
                    selector: None,
                    content_type: None,
                    trim: ChainOutputTrim::None,
                },
                Chain {
                    id: "command_stdin".into(),
                    source: ChainSource::Command {
                        command: vec!["head -c 1".into()],
                        stdin: Some("abcdef".into()),
                    },
                    sensitive: false,
                    selector: None,
                    content_type: None,
                    trim: ChainOutputTrim::None,
                },
                Chain {
                    id: "command_trim_none".into(),
                    source: ChainSource::command(["whoami"]),
                    sensitive: false,
                    selector: None,
                    content_type: None,
                    trim: ChainOutputTrim::None,
                },
                Chain {
                    id: "command_trim_start".into(),
                    source: ChainSource::command(["whoami"]),
                    sensitive: false,
                    selector: None,
                    content_type: None,
                    trim: ChainOutputTrim::Start,
                },
                Chain {
                    id: "command_trim_end".into(),
                    source: ChainSource::command(["whoami"]),
                    sensitive: false,
                    selector: None,
                    content_type: None,
                    trim: ChainOutputTrim::End,
                },
                Chain {
                    id: "command_trim_both".into(),
                    source: ChainSource::command(["whoami"]),
                    sensitive: false,
                    selector: None,
                    content_type: None,
                    trim: ChainOutputTrim::Both,
                },
                Chain {
                    id: "prompt_sensitive".into(),
                    source: ChainSource::Prompt {
                        message: Some("Password".into()),
                        default: None,
                    },
                    sensitive: true,
                    selector: None,
                    content_type: None,
                    trim: ChainOutputTrim::None,
                },
                Chain {
                    id: "prompt_default".into(),
                    source: ChainSource::Prompt {
                        message: Some("User GUID".into()),
                        default: Some("{{user_guid}}".into()),
                    },
                    sensitive: false,
                    selector: None,
                    content_type: None,
                    trim: ChainOutputTrim::None,
                },
                Chain {
                    id: "file".into(),
                    source: ChainSource::File {
                        path: "./README.md".into(),
                    },
                    sensitive: false,
                    selector: None,
                    content_type: None,
                    trim: ChainOutputTrim::None,
                },
                Chain {
                    id: "file_content_type".into(),
                    source: ChainSource::File {
                        path: "./data.json".into(),
                    },
                    sensitive: false,
                    selector: None,
                    content_type: Some(ContentType::Json),
                    trim: ChainOutputTrim::None,
                },
                Chain {
                    id: "request_selector".into(),
                    source: ChainSource::Request {
                        recipe: "login".into(),
                        trigger: ChainRequestTrigger::Never,
                        section: ChainRequestSection::Body,
                    },
                    sensitive: false,
                    selector: Some("$.data".parse().unwrap()),
                    content_type: None,
                    trim: ChainOutputTrim::None,
                },
                Chain {
                    id: "request_trigger_never".into(),
                    source: ChainSource::Request {
                        recipe: "login".into(),
                        trigger: ChainRequestTrigger::Never,
                        section: ChainRequestSection::Body,
                    },
                    sensitive: false,
                    selector: None,
                    content_type: None,
                    trim: ChainOutputTrim::None,
                },
                Chain {
                    id: "request_trigger_no_history".into(),
                    source: ChainSource::Request {
                        recipe: "login".into(),
                        trigger: ChainRequestTrigger::Never,
                        section: ChainRequestSection::Body,
                    },
                    sensitive: false,
                    selector: None,
                    content_type: None,
                    trim: ChainOutputTrim::None,
                },
                Chain {
                    id: "request_trigger_expire".into(),
                    source: ChainSource::Request {
                        recipe: "login".into(),
                        trigger: ChainRequestTrigger::Expire(
                            Duration::from_secs(12 * 60 * 60),
                        ),
                        section: ChainRequestSection::Body,
                    },
                    sensitive: false,
                    selector: None,
                    content_type: None,
                    trim: ChainOutputTrim::None,
                },
                Chain {
                    id: "request_trigger_always".into(),
                    source: ChainSource::Request {
                        recipe: "login".into(),
                        trigger: ChainRequestTrigger::Never,
                        section: ChainRequestSection::Body,
                    },
                    sensitive: false,
                    selector: None,
                    content_type: None,
                    trim: ChainOutputTrim::None,
                },
                Chain {
                    id: "request_section_body".into(),
                    source: ChainSource::Request {
                        recipe: "login".into(),
                        trigger: ChainRequestTrigger::Never,
                        section: ChainRequestSection::Body,
                    },
                    sensitive: false,
                    selector: None,
                    content_type: None,
                    trim: ChainOutputTrim::None,
                },
                Chain {
                    id: "request_section_header".into(),
                    source: ChainSource::Request {
                        recipe: "login".into(),
                        trigger: ChainRequestTrigger::Never,
                        section: ChainRequestSection::Header(
                            "content-type".into(),
                        ),
                    },
                    sensitive: false,
                    selector: None,
                    content_type: None,
                    trim: ChainOutputTrim::None,
                },
            ]),
            recipes: by_id([
                RecipeNode::Recipe(Recipe {
                    id: "text_body".into(),
                    name: None,
                    method: Method::Post,
                    url: "{{host}}/anything/login".into(),

                    body: Some(RecipeBody::Raw(
                        "{\"username\": \"{{username}}\", \
                        \"password\": \"{{chains.password}}\"}"
                            .into(),
                    )),
                    authentication: None,
                    query: vec![
                        ("sudo".into(), "yes_please".into()),
                        ("fast".into(), "no_thanks".into()),
                    ],
                    headers: indexmap! {
                        "Accept".into() => "application/json".into(),
                    },
                }),
                RecipeNode::Folder(Folder {
                    id: "users".into(),
                    name: Some("Users".into()),
                    children: by_id([
                        RecipeNode::Recipe(Recipe {
                            id: "simple".into(),
                            name: Some("Get User".into()),
                            method: Method::Get,
                            url: "{{host}}/anything/{{user_guid}}".into(),
                            body: None,
                            authentication: None,
                            query: vec![
                                ("value".into(), "{{field1}}".into()),
                                ("value".into(), "{{field2}}".into()),
                            ],
                            headers: indexmap! {},
                        }),
                        RecipeNode::Recipe(Recipe {
                            id: "json_body".into(),
                            name: Some("Modify User".into()),
                            method: Method::Put,
                            url: "{{host}}/anything/{{user_guid}}".into(),
                            body: Some(RecipeBody::Json(
                                json!({
                                    "username": "new username"
                                })
                                .into(),
                            )),
                            authentication: Some(Authentication::Bearer(
                                "{{chains.auth_token}}".into(),
                            )),
                            query: vec![],
                            headers: indexmap! {
                                "Accept".into() => "application/json".into(),
                            },
                        }),
                        RecipeNode::Recipe(Recipe {
                            id: "json_body_but_not".into(),
                            name: Some("Modify User".into()),
                            method: Method::Put,
                            url: "{{host}}/anything/{{user_guid}}".into(),

                            body: Some(RecipeBody::Json(
                                json!(r#"{"warning": "NOT an object"}"#).into(),
                            )),
                            authentication: Some(Authentication::Basic {
                                username: "{{username}}".into(),
                                password: Some("{{password}}".into()),
                            }),
                            query: vec![],
                            headers: indexmap! {
                                "Accept".into() => "application/json".into(),
                            },
                        }),
                        RecipeNode::Recipe(Recipe {
                            id: "form_urlencoded_body".into(),
                            name: Some("Modify User".into()),
                            method: Method::Put,
                            url: "{{host}}/anything/{{user_guid}}".into(),

                            body: Some(RecipeBody::FormUrlencoded(indexmap! {
                                "username".into() => "new username".into()
                            })),
                            authentication: None,
                            query: vec![],
                            headers: indexmap! {
                                "Accept".into() => "application/json".into(),
                            },
                        }),
                    ]),
                }),
            ])
            .into(),
            _ignore: IgnoredAny,
        };
        assert_eq!(loaded, expected);
    }
}
