//! A request collection defines recipes, profiles, etc. that make requests
//! possible

mod cereal;
mod json;
mod models;
mod recipe_tree;

pub use cereal::HasId;
pub use json::JsonTemplate;
pub use models::*;
pub use recipe_tree::*;

use anyhow::{Context, anyhow};
use itertools::Itertools;
use std::{
    env,
    fmt::{self, Debug, Display},
    fs,
    path::{Path, PathBuf},
};
use tracing::{trace, warn};

/// The support file names to be automatically loaded as a config. We only
/// support loading from one file at a time, so if more than one of these is
/// defined, we'll take the earliest and print a warning.
const CONFIG_FILES: &[&str] = &[
    "slumber.yml",
    "slumber.yaml",
    ".slumber.yml",
    ".slumber.yaml",
];

/// A handle for a collection file. This makes it easy to load and reload
/// the collection in the file. This is just a path that we've confirmed exists.
#[derive(Clone, Debug)]
pub struct CollectionFile(PathBuf);

impl CollectionFile {
    /// Get a handle to the collection file, returning an error if none is
    /// available. This will use the override if given, otherwise it will fall
    /// back to searching the given directory for a collection.
    pub fn new(override_path: Option<PathBuf>) -> anyhow::Result<Self> {
        Self::with_dir(env::current_dir()?, override_path)
    }

    /// Get a handle to the collection file, seaching a specific directory. This
    /// is only useful for testing. Typically you just want [Self::new].
    pub fn with_dir(
        mut dir: PathBuf,
        override_path: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        // If the override is a dir, search that dir instead. If it's a file,
        // just return it
        if let Some(override_path) = override_path {
            let joined = dir.join(override_path);
            if fs::metadata(&joined)
                .with_context(|| {
                    format!("Error loading `{}`", joined.display())
                })?
                .is_dir()
            {
                dir = joined;
            } else {
                return Ok(Self(joined));
            }
        }

        detect_path(&dir)
            .ok_or_else(|| {
                anyhow!(
                "No collection file found in current or ancestor directories"
            )
            })
            .map(Self)
    }

    /// Load collection from this file. Use [Self::new] to get a handle to the
    /// file. This pattern enables the TUI to start up and watch the collection
    /// file, even if it's invalid.
    pub fn load(&self) -> anyhow::Result<Collection> {
        Collection::load(&self.0)
    }

    /// Get the path of the file that this collection was loaded from
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Display for CollectionFile {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0.display())
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
                Some(first.clone())
            }
        }
    }

    // Walk *up* the tree until we've hit the root
    search_all(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        http::{HttpMethod, content_type::ContentType},
        test_util::by_id,
    };
    use indexmap::indexmap;
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use serde_json::json;
    use slumber_util::{Factory, TempDir, assert_err, temp_dir, test_data_dir};
    use std::{fs, fs::File, time::Duration};

    /// Test various cases of [CollectionFile::with_dir]
    #[rstest]
    #[case::parent_only(None, true, false, "slumber.yml")]
    #[case::child_only(None, false, true, "child/slumber.yml")]
    #[case::parent_and_child(None, true, true, "child/slumber.yml")]
    #[case::directory(
        Some("grandchild"),
        true,
        true,
        "child/grandchild/slumber.yml"
    )]
    #[case::overriden(Some("override.yml"), true, true, "child/override.yml")]
    fn test_with_dir(
        temp_dir: TempDir,
        #[case] override_path: Option<&str>,
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
            let grandchild_dir = child_dir.join("grandchild");
            fs::create_dir(&grandchild_dir).unwrap();
            File::create(grandchild_dir.join(file)).unwrap();
        }
        File::create(child_dir.join("override.yml")).unwrap();
        let expected: PathBuf = temp_dir.join(expected);

        let actual = CollectionFile::with_dir(
            child_dir,
            override_path.map(PathBuf::from),
        )
        .unwrap();
        assert_eq!(actual.path(), expected);
    }

    /// Test that with_dir fails when no collection file is found and no
    /// override is given
    #[rstest]
    #[case::no_file(
        None,
        "No collection file found in current or ancestor directories"
    )]
    #[case::override_doesnt_exist(
        Some("./bogus/"),
        if cfg!(unix) {
            "No such file or directory"
        } else {
            "The system cannot find the file specified"
        }
    )]
    fn test_with_dir_error(
        temp_dir: TempDir,
        #[case] override_path: Option<&str>,
        #[case] expected_err: &str,
    ) {
        assert_err!(
            CollectionFile::with_dir(
                temp_dir.to_path_buf(),
                override_path.map(PathBuf::from)
            ),
            expected_err
        );
    }

    /// A catch-all regression test, to make sure we don't break anything in the
    /// collection format. This lives at the bottom because it's huge.
    #[rstest]
    #[tokio::test]
    async fn test_regression(test_data_dir: PathBuf) {
        let loaded =
            Collection::load(&test_data_dir.join("regression.yml")).unwrap();
        let expected = Collection {
            name: Some("Regression Test".to_owned()),
            profiles: by_id([
                Profile {
                    id: "profile1".into(),
                    name: Some("Profile 1".into()),
                    default: false,
                    data: indexmap! {
                        "user_guid".into() => "abc123".into(),
                        "username".into() => "xX{{chains.username}}Xx".into(),
                        "host".into() => "https://httpbin.org".into(),

                    },
                },
                Profile {
                    id: "profile2".into(),
                    name: Some("Profile 2".into()),
                    default: true,
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
                    selector_mode: SelectorMode::default(),
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
                    selector_mode: SelectorMode::default(),
                    content_type: None,
                    trim: ChainOutputTrim::None,
                },
                Chain {
                    id: "command_trim_none".into(),
                    source: ChainSource::command(["whoami"]),
                    sensitive: false,
                    selector: None,
                    selector_mode: SelectorMode::default(),
                    content_type: None,
                    trim: ChainOutputTrim::None,
                },
                Chain {
                    id: "command_trim_start".into(),
                    source: ChainSource::command(["whoami"]),
                    sensitive: false,
                    selector: None,
                    selector_mode: SelectorMode::default(),
                    content_type: None,
                    trim: ChainOutputTrim::Start,
                },
                Chain {
                    id: "command_trim_end".into(),
                    source: ChainSource::command(["whoami"]),
                    sensitive: false,
                    selector: None,
                    selector_mode: SelectorMode::default(),
                    content_type: None,
                    trim: ChainOutputTrim::End,
                },
                Chain {
                    id: "command_trim_both".into(),
                    source: ChainSource::command(["whoami"]),
                    sensitive: false,
                    selector: None,
                    selector_mode: SelectorMode::default(),
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
                    selector_mode: SelectorMode::default(),
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
                    selector_mode: SelectorMode::default(),
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
                    selector_mode: SelectorMode::default(),
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
                    selector_mode: SelectorMode::default(),
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
                    selector_mode: SelectorMode::default(),
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
                    selector_mode: SelectorMode::default(),
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
                    selector_mode: SelectorMode::default(),
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
                    selector_mode: SelectorMode::default(),
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
                    selector_mode: SelectorMode::default(),
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
                    selector_mode: SelectorMode::default(),
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
                    selector_mode: SelectorMode::default(),
                    content_type: None,
                    trim: ChainOutputTrim::None,
                },
            ]),
            recipes: by_id([
                RecipeNode::Recipe(Recipe {
                    id: "text_body".into(),
                    method: HttpMethod::Post,
                    url: "{{host}}/anything/login".into(),
                    body: Some(RecipeBody::Raw(
                        "{\"username\": \"{{username}}\", \
                        \"password\": \"{{chains.password}}\"}"
                            .into(),
                    )),
                    query: vec![
                        ("sudo".into(), "yes_please".into()),
                        ("fast".into(), "no_thanks".into()),
                    ],
                    headers: indexmap! {
                        "accept".into() => "application/json".into(),
                    },
                    ..Recipe::factory(())
                }),
                RecipeNode::Folder(Folder {
                    id: "users".into(),
                    name: Some("Users".into()),
                    children: by_id([
                        RecipeNode::Recipe(Recipe {
                            id: "simple".into(),
                            name: Some("Get User".into()),
                            method: HttpMethod::Get,
                            url: "{{host}}/anything/{{user_guid}}".into(),
                            query: vec![
                                ("value".into(), "{{field1}}".into()),
                                ("value".into(), "{{field2}}".into()),
                            ],
                            ..Recipe::factory(())
                        }),
                        RecipeNode::Recipe(Recipe {
                            id: "json_body".into(),
                            name: Some("Modify User".into()),
                            method: HttpMethod::Put,
                            url: "{{host}}/anything/{{user_guid}}".into(),
                            body: Some(
                                RecipeBody::json(
                                    json!({"username": "new username"}),
                                )
                                .unwrap(),
                            ),
                            authentication: Some(Authentication::Bearer(
                                "{{chains.auth_token}}".into(),
                            )),
                            headers: indexmap! {
                                "accept".into() => "application/json".into(),
                            },
                            ..Recipe::factory(())
                        }),
                        RecipeNode::Recipe(Recipe {
                            id: "json_body_but_not".into(),
                            name: Some("Modify User".into()),
                            method: HttpMethod::Put,
                            url: "{{host}}/anything/{{user_guid}}".into(),
                            body: Some(
                                RecipeBody::json(json!(
                                    r#"{"warning": "NOT an object"}"#
                                ))
                                .unwrap(),
                            ),
                            authentication: Some(Authentication::Basic {
                                username: "{{username}}".into(),
                                password: Some("{{password}}".into()),
                            }),
                            headers: indexmap! {
                                "accept".into() => "application/json".into(),
                            },
                            ..Recipe::factory(())
                        }),
                        RecipeNode::Recipe(Recipe {
                            id: "form_urlencoded_body".into(),
                            name: Some("Modify User".into()),
                            method: HttpMethod::Put,
                            url: "{{host}}/anything/{{user_guid}}".into(),
                            body: Some(RecipeBody::FormUrlencoded(indexmap! {
                                "username".into() => "new username".into()
                            })),
                            headers: indexmap! {
                                "accept".into() => "application/json".into(),
                            },
                            ..Recipe::factory(())
                        }),
                    ]),
                }),
            ])
            .into(),
        };
        assert_eq!(loaded, expected);
    }
}
