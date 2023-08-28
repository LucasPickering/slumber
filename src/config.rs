use anyhow::{bail, Context};
use log::warn;
use serde::Deserialize;
use std::{collections::HashMap, fs::File, path::Path};

/// The support file names to be automatically loaded as a config. We only
/// support loading from one file at a time, so if more than one of these is
/// defined, we'll take the earliest and print a warning.
pub const CONFIG_FILES: &[&str] = &[
    "slumber.yml",
    "slumber.yaml",
    ".slumber.yml",
    ".slumber.yaml",
];

#[derive(Clone, Debug, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub environments: Vec<Environment>,
    #[serde(default)]
    pub requests: Vec<RequestNode>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Environment {
    pub name: String,
    pub data: HashMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
pub enum RequestNode {
    Folder {
        name: TemplateString,
        requests: Vec<RequestNode>,
    },
    Request(Request),
}

#[derive(Clone, Debug, Deserialize)]
pub struct Request {
    pub name: TemplateString,
    pub method: TemplateString,
    pub url: TemplateString,
    pub body: Option<serde_yaml::Value>,
    #[serde(default)]
    pub headers: HashMap<String, TemplateString>,
}

/// A string that can contain templated content
#[derive(Clone, Debug, Deserialize)]
pub struct TemplateString(String);

impl Config {
    pub fn load(collection_file: Option<&Path>) -> anyhow::Result<Self> {
        // Figure out which file we want to load from
        let path = match collection_file {
            Some(path) => path,
            None => {
                let paths: Vec<&Path> = CONFIG_FILES
                    .iter()
                    .map(Path::new)
                    .filter(|p| p.exists())
                    .collect();
                match paths.as_slice() {
                    [] => bail!("TODO"),
                    [path] => path,
                    [first, rest @ ..] => {
                        warn!(
                            "Multiple config files detected. {first:?} will \
                        be used and the following will be ignored: {rest:?}"
                        );
                        *first
                    }
                }
            }
        };

        // First, parse the file to raw YAML values, so we can apply
        // anchor/alias merging. Then parse that to our config type
        // Poor man's try block, so we don't have to repeat context
        let parse = || -> anyhow::Result<Config> {
            let mut file = File::open(path)?;
            let mut yaml_value =
                serde_yaml::from_reader::<_, serde_yaml::Value>(&mut file)?;
            yaml_value.apply_merge()?;
            Ok(serde_yaml::from_value(yaml_value)?)
        };
        parse()
            .with_context(|| format!("Error parsing config from file {path:?}"))
    }
}
