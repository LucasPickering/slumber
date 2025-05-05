use crate::{GlobalArgs, Subcommand};
use anyhow::Context;
use clap::Parser;
use std::{fs::OpenOptions, io::Write, path::PathBuf, process::ExitCode};

/// We use a static source file, to get control of whitespace/comments.
/// Generating a collection and serializing it would be like driving from the
/// back seat with a broom stick.
const SOURCE: &str = include_str!("new.js");
const DEFAULT_PATH: &str = "slumber.js";

/// Generate a new Slumber collection file
#[derive(Clone, Debug, Parser)]
pub struct NewCommand {
    /// Path to write the new file to. If omitted, fall back to the global
    /// `--file` argument, or default to `slumber.js`
    file: Option<PathBuf>,
    /// If a file already exists at the path, overwrite it instead of failing
    #[clap(long)]
    overwrite: bool,
}

impl Subcommand for NewCommand {
    async fn execute(self, global: GlobalArgs) -> anyhow::Result<ExitCode> {
        let path = self
            .file
            .or(global.file)
            .unwrap_or_else(|| DEFAULT_PATH.into());

        let mut file = OpenOptions::new()
            .create_new(!self.overwrite)
            .create(self.overwrite)
            .write(true)
            .truncate(self.overwrite)
            .open(&path)
            .with_context(|| format!("Error opening file {path:?}"))?;
        file.write_all(SOURCE.as_bytes())
            .with_context(|| format!("Error writing to file {path:?}"))?;

        eprintln!("New collection created at `{}`", path.display());

        Ok(ExitCode::SUCCESS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::indexmap;
    use petitscript::ast::{Expression, TemplateChunk};
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use slumber_core::{
        collection::{
            Collection, Folder, LoadedCollection, Profile, Recipe, RecipeBody,
            RecipeNode,
        },
        http::HttpMethod,
        ps::{self, PetitEngine},
        render::Procedure,
        test_util::by_id,
    };
    use slumber_util::{Factory, TempDir, temp_dir};
    use std::{env, fs};

    /// Test creating a new collection file, specifying the path in various ways
    #[rstest]
    #[case::default_path(None, None, "slumber.js")]
    #[case::global_path_arg(None, Some("global.js"), "global.js")]
    #[case::local_path_arg(Some("local.js"), Some("global.js"), "local.js")]
    #[tokio::test]
    async fn test_new(
        temp_dir: TempDir,
        #[case] file_arg: Option<&str>,
        #[case] global_file_arg: Option<&str>,
        #[case] expected_created_path: PathBuf,
    ) {
        // Lock an empty env as a proxy for the cwd. We can't just construct
        // full paths because the default value will always be relative to cwd
        let _guard = env_lock::lock_env([] as [(&str, Option<&str>); 0]);
        env::set_current_dir(&*temp_dir).unwrap();

        let command = NewCommand {
            file: file_arg.map(PathBuf::from),
            overwrite: false,
        };
        let global_args = GlobalArgs {
            file: global_file_arg.map(PathBuf::from),
        };

        command.execute(global_args).await.unwrap();
        let contents = fs::read_to_string(&expected_created_path)
            .with_context(|| {
                format!(
                    "Error reading results from expected output path \
                    {expected_created_path:?}"
                )
            })
            .unwrap();
        assert_eq!(contents, SOURCE);
    }

    /// Test that the initial collection is a valid collection with some
    /// specific contents
    #[test]
    fn test_deserialize() {
        let LoadedCollection {
            collection: actual, ..
        } = PetitEngine::new().load_collection(SOURCE).unwrap();
        let url = Procedure::template([
            // `${profile("host")}/anything`
            TemplateChunk::expression(ps::profile_field("host").into()),
            "/anything".into(),
        ]);
        let expected = Collection {
            profiles: by_id([Profile {
                id: "example".into(),
                name: Some("Example Profile".into()),
                default: false,
                data: indexmap! {
                    "host".into() => "https://httpbin.org".into()
                },
            }]),
            recipes: by_id([
                RecipeNode::Recipe(Recipe {
                    id: "example1".into(),
                    name: Some("Example Request 1".into()),
                    method: HttpMethod::Get,
                    url: url.clone(),
                    ..Recipe::factory(())
                }),
                RecipeNode::Folder(Folder {
                    id: "example_folder".into(),
                    name: Some("Example Folder".into()),
                    children: by_id([RecipeNode::Recipe(Recipe {
                        id: "example2".into(),
                        name: Some("Example Request 2".into()),
                        method: HttpMethod::Post,
                        url,
                        body: Some(RecipeBody::Json {
                            data: Procedure::test(
                                // JSON.parse(response("example1")).data
                                Expression::reference("JSON")
                                    .call(
                                        "parse",
                                        [ps::call_fn(
                                            "response",
                                            ["example1".into()],
                                            [],
                                        )
                                        .into()],
                                    )
                                    .property("data"),
                            ),
                        }),
                        ..Recipe::factory(())
                    })]),
                }),
            ])
            .into(),
        };
        assert_eq!(actual, expected);
    }
}
