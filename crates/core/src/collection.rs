//! A request collection defines recipes, profiles, etc. that make requests
//! possible

mod cereal;
mod models;
mod recipe_tree;

pub use models::*;
pub use recipe_tree::*;

use crate::ps::PetitEngine;
use anyhow::{Context as _, anyhow};
use itertools::Itertools;
use petitscript::Process;
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
const CONFIG_FILES: &[&str] = &["slumber.js", ".slumber.js"];

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
                .with_context(|| format!("Error loading {joined:?}"))?
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

    /// Load a collection from this file, using the given PS engine to execute
    /// the file. The returned value will contain the loaded collection as well
    /// as the PS process that loaded it. The process can be used to invoke
    /// render functions.
    pub fn load(
        &self,
        engine: &PetitEngine,
    ) -> anyhow::Result<LoadedCollection> {
        engine.load_collection(self.0.clone())
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

/// Create a new file with a placeholder path for testing
/// TODO delete this?
#[cfg(any(test, feature = "test"))]
impl slumber_util::Factory<()> for CollectionFile {
    fn factory(_: ()) -> Self {
        Self(PathBuf::default())
    }
}

/// TODO better name
#[derive(Debug)]
pub struct LoadedCollection {
    pub collection: Collection,
    pub process: Process,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{http::HttpMethod, render::Procedure, test_util::by_id};
    use indexmap::indexmap;
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use serde_json::json;
    use slumber_util::{Factory, TempDir, assert_err, temp_dir, test_data_dir};
    use std::{fs, fs::File};

    /// Test various cases of try_path
    #[rstest]
    #[case::parent_only(None, true, false, "slumber.js")]
    #[case::child_only(None, false, true, "child/slumber.js")]
    #[case::parent_and_child(None, true, true, "child/slumber.js")]
    #[case::directory(
        Some("grandchild"),
        true,
        true,
        "child/grandchild/slumber.js"
    )]
    #[case::overriden(Some("override.js"), true, true, "child/override.js")]
    fn test_try_path(
        temp_dir: TempDir,
        #[case] override_path: Option<&str>,
        #[case] has_parent: bool,
        #[case] has_child: bool,
        #[case] expected: &str,
    ) {
        let child_dir = temp_dir.join("child");
        fs::create_dir(&child_dir).unwrap();
        let file = "slumber.js";
        if has_parent {
            File::create(temp_dir.join(file)).unwrap();
        }
        if has_child {
            File::create(child_dir.join(file)).unwrap();
            let grandchild_dir = child_dir.join("grandchild");
            fs::create_dir(&grandchild_dir).unwrap();
            File::create(grandchild_dir.join(file)).unwrap();
        }
        File::create(child_dir.join("override.js")).unwrap();
        let expected: PathBuf = temp_dir.join(expected);

        let actual = CollectionFile::with_dir(
            child_dir,
            override_path.map(PathBuf::from),
        )
        .unwrap();
        assert_eq!(actual.path(), expected);
    }

    /// Test that try_path fails when no collection file is found and no
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
    fn test_try_path_error(
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
        let LoadedCollection {
            process,
            collection: actual,
        } = PetitEngine::new()
            .load_collection(test_data_dir.join("regression.js"))
            .unwrap();

        // Define some common procedures that are used several times
        let url = Procedure::parse(
            &process,
            r#"`${profile("host")}/anything/${profile("userGuid")}`"#,
        );
        let password = Procedure::parse(
            &process,
            r#"prompt({ message: "Password", sensitive: true })"#,
        );

        let expected = Collection {
            profiles: by_id([
                Profile {
                    id: "profile1".into(),
                    name: Some("Profile 1".into()),
                    default: false,
                    data: indexmap! {
                        "userGuid".into() => "abc123".into(),
                        "username".into() => Procedure::parse(
                            &process,
                            r#"`xX${username()}Xx`"#,
                        ),
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
            recipes: by_id([
                RecipeNode::Recipe(Recipe {
                    id: "textBody".into(),
                    method: HttpMethod::Post,
                    url: Procedure::parse(
                        &process,
                        r#"`${profile("host")}/anything/login`"#,
                    ),
                    body: Some(RecipeBody::Raw {
                        data: Procedure::parse(
                            &process,
                            r#"`{"username": "${profile("username")}", "password": "${password()}"}`"#,
                        ),
                    }),
                    query: indexmap! {
                        "sudo".into() => "yes_please".into(),
                        "fast".into() => "no_thanks".into(),
                    },
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
                            persist: false,
                            method: HttpMethod::Get,
                            url: url.clone(),
                            query: indexmap! {
                                "value".into() => [
                                    Procedure::parse(
                                        &process,
                                        r#"profile("field1")"#,
                                    ),
                                    Procedure::parse(
                                        &process,
                                        r#"profile("field2")"#,
                                    ),
                                ].into(),
                            },
                            ..Recipe::factory(())
                        }),
                        RecipeNode::Recipe(Recipe {
                            id: "jsonBody".into(),
                            name: Some("Modify User".into()),
                            method: HttpMethod::Put,
                            url: url.clone(),
                            body: Some(RecipeBody::Json {
                                data: json!({"username": "new username"})
                                    .into(),
                            }),
                            authentication: Some(Authentication::Bearer {
                                token: Procedure::parse(
                                    &process,
                                    "authToken()",
                                ),
                            }),
                            headers: indexmap! {
                                "accept".into() => "application/json".into(),
                            },
                            ..Recipe::factory(())
                        }),
                        RecipeNode::Recipe(Recipe {
                            id: "jsonBodyButNot".into(),
                            name: Some("Modify User".into()),
                            method: HttpMethod::Put,
                            url: url.clone(),
                            body: Some(RecipeBody::Json {
                                data: json!(r#"{"warning": "NOT an object"}"#)
                                    .into(),
                            }),
                            authentication: Some(Authentication::Basic {
                                username: Procedure::parse(
                                    &process,
                                    r#"profile("username")"#,
                                ),
                                password: password.clone(),
                            }),
                            headers: indexmap! {
                                "accept".into() => "application/json".into(),
                            },
                            ..Recipe::factory(())
                        }),
                        RecipeNode::Recipe(Recipe {
                            id: "formUrlencodedBody".into(),
                            name: Some("Modify User".into()),
                            method: HttpMethod::Put,
                            url: url.clone(),
                            body: Some(RecipeBody::FormUrlencoded {
                                data: indexmap! {
                                    "username".into() => "new username".into(),
                                    "password".into() => password.clone(),
                                },
                            }),
                            headers: indexmap! {
                                "accept".into() => "application/json".into(),
                            },
                            ..Recipe::factory(())
                        }),
                        RecipeNode::Recipe(Recipe {
                            id: "formMultipartBody".into(),
                            name: Some("Modify User".into()),
                            method: HttpMethod::Put,
                            url: url.clone(),
                            body: Some(RecipeBody::FormMultipart {
                                data: indexmap! {
                                    "username".into() => "new username".into(),
                                    "password".into() => password,
                                },
                            }),
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
        assert_eq!(actual, expected);
    }
}
