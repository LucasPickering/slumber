use crate::{GlobalArgs, Subcommand};
use anyhow::Context;
use clap::Parser;
use slumber_util::git_link;
use std::{fs::OpenOptions, io::Write, path::PathBuf, process::ExitCode};

const DEFAULT_PATH: &str = "slumber.yml";

/// Generate a new Slumber collection file
#[derive(Clone, Debug, Parser)]
pub struct NewCommand {
    /// Path to write the new file to. If omitted, fall back to the global
    /// `--file` argument, or default to `slumber.yml`
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
            .with_context(|| {
                format!("Error opening file `{}`", path.display())
            })?;
        file.write_all(source().as_bytes()).with_context(|| {
            format!("Error writing to file `{}`", path.display())
        })?;

        eprintln!("New collection created at `{}`", path.display());

        Ok(ExitCode::SUCCESS)
    }
}

fn source() -> String {
    /// We use a static source file, to get control of whitespace/comments.
    /// Generating a collection and serializing it would be like driving from
    /// the back seat with a broom stick.
    const SOURCE: &str = include_str!("new.yml");
    /// This string will be replaced with the link to the schema file
    const SCHEMA_REPLACEMENT: &str = "{{#schema}}";

    SOURCE.replace(SCHEMA_REPLACEMENT, &git_link("schemas/collection.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::indexmap;
    use pretty_assertions::assert_eq;
    use rstest::rstest;
    use serde_json::json;
    use slumber_core::{
        collection::{
            Collection, Folder, Profile, Recipe, RecipeBody, RecipeNode,
        },
        http::HttpMethod,
        test_util::by_id,
    };
    use slumber_util::{Factory, TempDir, temp_dir, yaml::SourceLocation};
    use std::fs;

    /// Test creating a new collection file, specifying the path in various ways
    #[rstest]
    #[case::default_path(None, None, "slumber.yml")]
    #[case::global_path_arg(None, Some("global.yml"), "global.yml")]
    #[case::local_path_arg(Some("local.yml"), Some("global.yml"), "local.yml")]
    #[tokio::test]
    async fn test_new(
        temp_dir: TempDir,
        #[case] file_arg: Option<&str>,
        #[case] global_file_arg: Option<&str>,
        #[case] expected_created_path: PathBuf,
    ) {
        // Lock the cwd. We can't just construct full paths because the default
        // value will always be relative to cwd
        let _guard = env_lock::lock_current_dir(&*temp_dir).unwrap();

        let command = NewCommand {
            file: file_arg.map(PathBuf::from),
            overwrite: false,
        };
        let global_args = GlobalArgs {
            file: global_file_arg.map(PathBuf::from),
            ..Default::default()
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
        assert_eq!(contents, source());
    }

    /// Test that the initial collection is a valid collection with some
    /// specific contents
    #[test]
    fn test_deserialize() {
        let collection: Collection = Collection::parse(&source()).unwrap();
        let expected = Collection {
            name: Some("My Collection".into()),
            profiles: by_id([Profile {
                id: "example".into(),
                location: SourceLocation::default(),
                name: Some("Example Profile".into()),
                default: false,
                data: indexmap! {
                    "host".into() => "https://my-host".into()
                },
            }]),
            recipes: by_id([
                RecipeNode::Recipe(Recipe {
                    id: "example_get".into(),
                    name: Some("Example GET".into()),
                    method: HttpMethod::Get,
                    url: "{{ host }}/get".into(),
                    ..Recipe::factory(())
                }),
                RecipeNode::Folder(Folder {
                    id: "example_folder".into(),
                    location: SourceLocation::default(),
                    name: Some("Example Folder".into()),
                    children: by_id([RecipeNode::Recipe(Recipe {
                        id: "example_post".into(),
                        name: Some("Example POST".into()),
                        method: HttpMethod::Post,
                        url: "{{ host }}/post".into(),
                        body: Some(
                            RecipeBody::json(
                                json!({"data": "{{ response('example_get') \
                                | jsonpath('$.data') }}"}),
                            )
                            .unwrap(),
                        ),
                        ..Recipe::factory(())
                    })]),
                }),
            ])
            .into(),
        };
        assert_eq!(collection, expected);
    }

    /// Make sure version replacement works in the schema link
    #[test]
    fn test_schema_link() {
        let source = source();
        let first_line = source.lines().next().unwrap();
        let expected = format!(
            "# yaml-language-server: $schema={}",
            git_link("schemas/collection.json")
        );
        assert_eq!(first_line, expected);
    }
}
