use anyhow::{anyhow, Context};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs::File, io::Read, path::Path};

use super::{Profile, ProfileId};
use crate::template::Template;

const JETBRAINS_CLIENT_ENV_NAME: &str = "http-client.env.json";
const JETBRAINS_PRIVATE_CLIENT_ENV_NAME: &str = "http-client.private.env.json";

/// The files that should be included in the import
#[derive(Debug, PartialEq)]
pub enum JetbrainsEnvImport {
    /// Search for `http-client.env.json` and include it 
    Public,
    /// Search for `http-client.env.json` and `http-clienv.private.env.json` and include them
    PublicAndPrivate,
}

/// Represents the `http-client` and `http-client.private` files
/// used for the HTTP enviroment
#[derive(Debug, Clone)]
pub struct JetbrainsEnv {
    env: ClientEnvJson,
}

impl JetbrainsEnv {
    /// Search a directory for `http-client.env.json` and
    /// `http-client.private.env.json` This must be in the same directory as
    /// your HTTP file
    pub fn from_directory(dir: &Path, import_type: JetbrainsEnvImport) -> anyhow::Result<Self> {
        if dir.is_file() {
            return Err(anyhow!("Can only search directory!"));
        }

        // There is a public env file and a private one
        // The private file is optional and merged with the public one
        // Private just exists so you can git ignore it
        let mut all_envs = ClientEnvJson::from_public(dir)?;
        if import_type == JetbrainsEnvImport::PublicAndPrivate {
            if let Ok(private) = ClientEnvJson::from_private(dir) {
                all_envs = all_envs.merge(private)?;
            } 
        } 


        Ok(Self { env: all_envs })
    }

    /// Convert the jetbrains env into slumber profiles
    /// The globals must be included from the REST file
    pub fn to_profiles(
        &self,
        globals: IndexMap<String, Template>,
    ) -> anyhow::Result<IndexMap<ProfileId, Profile>> {
        let mut profiles: IndexMap<ProfileId, Profile> = IndexMap::new();
        for (profile_name, env_item) in self.env.items.iter() {
            let templates = env_item.to_templates(&globals)?;
            let id: ProfileId = profile_name.to_string().into();
            let lookup_id = id.clone();

            let profile = Profile {
                id,
                name: Some(profile_name.into()),
                data: templates,
            };
            profiles.insert(lookup_id, profile);
        }
        Ok(profiles)
    }
}

/// Each individual profile
/// This contains a map of JSON values
#[derive(Debug, Clone, Serialize, Deserialize)]
struct EnvForProfile {
    #[serde(flatten)]
    items: IndexMap<String, Value>,
}

impl EnvForProfile {
    /// Turn the jetbrains env into a map of templates
    /// Inject any global variables (written in the http file) into each
    /// enviroment
    fn to_templates(
        &self,
        globals: &IndexMap<String, Template>,
    ) -> anyhow::Result<IndexMap<String, Template>> {
        let mut data: IndexMap<String, Template> = IndexMap::new();
        for (key, value) in self.items.iter() {
            let template = match value {
                Value::String(s) => s.to_string().try_into()?,
                Value::Number(n) => n.to_string().try_into()?,
                Value::Bool(b) => b.to_string().try_into()?,
                _ => return Err(anyhow!("Only strings, numbers and bools are supported in Jetbrains HTTP Client Envs!")),
            };

            data.insert(key.into(), template);
        }

        for (key, value) in globals.iter() {
            data.insert(key.into(), value.clone());
        }

        Ok(data)
    }
}

/// A jetbrains client env file
/// `https://www.jetbrains.com/help/idea/exploring-http-syntax.html#http-client-env-json`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientEnvJson {
    #[serde(flatten)]
    items: IndexMap<String, EnvForProfile>,
}

impl ClientEnvJson {
    fn from_file(env_file: impl AsRef<Path>) -> anyhow::Result<Self> {
        let env_file = env_file.as_ref();

        let mut file = File::open(env_file)
            .context(format!("Failed to open env file {env_file:?}"))?;
        let mut text = String::new();
        file.read_to_string(&mut text)
            .context(format!("Error reading env file {env_file:?}"))?;

        let env = serde_json::from_str(&text)
            .context(format!("Invalid env file!"))?;
        Ok(env)
    }

    /// Attempt to load `http-client.env.json`
    fn from_public(dir: &Path) -> anyhow::Result<Self> {
        Self::from_file(dir.join(JETBRAINS_CLIENT_ENV_NAME))
    }

    /// Attempt to load `http-client.private.env.json`
    fn from_private(dir: &Path) -> anyhow::Result<Self> {
        Self::from_file(dir.join(JETBRAINS_PRIVATE_CLIENT_ENV_NAME))
    }

    /// Used for merging the public and private files
    fn merge(self, other: Self) -> anyhow::Result<Self> {
        let mut items = self.items.clone();
        for (profile_name, profile_a) in other.items.into_iter() {
            let mut current_profile = items.get(&profile_name)
                .ok_or(anyhow!("The profiles in your public and private file do not match!"))?
                .to_owned();

            current_profile.items.extend(profile_a.items);
            items.insert(profile_name, current_profile);
        }
        Ok(Self { items })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::indexmap;
    use serde_json::json;

    #[test]
    fn parse_client_env() {
        let example = r#"{
            "development": {
                "host": "localhost",
                "id-value": 12345,
                "username": "",
                "password": "",
                "my-var": "my-dev-value"
            },

            "production": {
                "host": "example.com",
                "id-value": 6789,
                "username": "",
                "password": "",
                "my-var": "my-prod-value"
            }
        }"#;

        let client: ClientEnvJson = serde_json::from_str(example).unwrap();
        let dev = client.items.get("development").unwrap();
        let prod = client.items.get("production").unwrap();

        let host = dev.items.get("host").unwrap();
        assert_eq!(host, &Value::String("localhost".into()));

        let id = prod.items.get("id-value").unwrap();
        assert_eq!(id, &json!(6789));
    }

    #[test]
    fn read_from_dir() {
        let env_file =
            JetbrainsEnv::from_directory(Path::new("./test_data"), JetbrainsEnvImport::PublicAndPrivate).unwrap();

        let dev = env_file.env.items.get("development").unwrap();
        let host = dev.items.get("host").unwrap();
        assert_eq!(host, &Value::String("localhost".into()));

        let secret = dev.items.get("super-secret-number").unwrap();
        assert_eq!(secret, &json!(12345));
    }

    #[test]
    fn convert_to_profiles() {
        let example = r#"{
    "development": {
        "host": "localhost",
        "id-value": 12345,
        "username": "",
        "password": "",
        "my-var": "my-dev-value"
    },

    "production": {
        "host": "example.com",
        "id-value": 6789,
        "username": "",
        "password": "",
        "my-var": "my-prod-value"
    }
}"#;

        let loaded_json: ClientEnvJson = serde_json::from_str(example).unwrap();
        let env = JetbrainsEnv { env: loaded_json };

        let variables: IndexMap<String, Template> = indexmap! {
            "fruit".into() => "apple".try_into().unwrap(),
            "meat".into() => "{{cow}}".try_into().unwrap(),
        };

        let profiles = env.to_profiles(IndexMap::new()).unwrap();

        let id: ProfileId = "development".into();
        let dev = profiles.get(&id).unwrap();
        let val = dev.data.get("id-value").unwrap();
        assert_eq!(val, &Template::from("12345"));

        let profiles_globals = env.to_profiles(variables).unwrap();

        let id: ProfileId = "development".into();
        let dev = profiles_globals.get(&id).unwrap();
        let val = dev.data.get("id-value").unwrap();
        assert_eq!(val, &Template::from("12345"));

        let fruit = dev.data.get("fruit").unwrap();
        assert_eq!(fruit, &Template::from("apple"));
    }
}
