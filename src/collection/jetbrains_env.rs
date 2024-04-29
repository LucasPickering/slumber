use anyhow::{anyhow, Context};
use std::{hash::Hash, path::Path, fs::File, io::Read};
use indexmap::IndexMap;
use serde::{Serialize, Deserialize};
use serde_json::Value;

use super::{ProfileId, Profile};
use crate::template::Template;

const JETBRAINS_CLIENT_ENV: &str = "http-client.env.json";
const JETBRAINS_PRIVATE_CLIENT_ENV: &str = "http-client.private.env.json";


#[derive(Debug, Clone)]
pub struct JetbrainsEnv {
    env: Option<ClientEnvJson>,
}

impl JetbrainsEnv {
    pub fn from_directory(dir: &Path) -> anyhow::Result<Self> {
        if dir.is_file() {
            return Err(anyhow!("Can only search directory!"))
        }

        let public = ClientEnvJson::from_public(dir)?;
        println!("{:?}", public);
        let merged = match ClientEnvJson::from_private(dir) {
            Ok(private) => public.merge(private),
            _ => public,
        };

        Ok(Self {
            env: Some(merged)
        })
    }
}

fn merge_maps<K: Eq + Hash + Clone, V: Clone>(map1: IndexMap<K, V>, map2: IndexMap<K, V>) -> IndexMap<K, V> {
    let mut merged = map1.clone();
    merged.extend(map2);
    merged
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EnvForProfile {
    #[serde(flatten)]
    items: IndexMap<String, Value>
}

impl EnvForProfile {
    fn merge(self, other: Self) -> Self {
        Self {
            items: merge_maps(self.items, other.items)
        }
    } 

    fn to_templates(self, globals: IndexMap<String, Template>) -> anyhow::Result<IndexMap<String, Template>> {
        let mut data: IndexMap<String, Template> = IndexMap::new(); 
        for (key, value) in self.items.into_iter() {
            let key = key.to_string();
            let template = match value {
                Value::String(s) => s.to_string().try_into()?,
                Value::Number(n) => n.to_string().try_into()?,
                _ => return Err(anyhow!("Only strings and numbers are suppored in Jetbrains HTTP Client Envs!")),
            };

            data.insert(key, template);
        }
        Ok(merge_maps(data, globals))
    }
}

/// A jetbrains client env file
/// `https://www.jetbrains.com/help/idea/exploring-http-syntax.html#http-client-env-json`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientEnvJson {
    #[serde(flatten)] 
    items: IndexMap<String, EnvForProfile> 
} 

impl ClientEnvJson {
    fn from_file(
        env_file: impl AsRef<Path>
    ) -> anyhow::Result<Self> {
        let env_file = env_file.as_ref();
    
        let mut file = File::open(env_file)
            .context(format!("Failed to open env file {env_file:?}"))?;
        let mut text = String::new();
        file.read_to_string(&mut text)
            .context(format!("Error reading env file {env_file:?}"))?;

        Ok(serde_json::from_str(&text)
            .context(format!("Invalid env file!"))?)
    } 

    fn from_public(dir: &Path) -> anyhow::Result<Self> {
        Self::from_file(dir.join(JETBRAINS_CLIENT_ENV))
    }

    fn from_private(dir: &Path) -> anyhow::Result<Self> {
        Self::from_file(dir.join(JETBRAINS_PRIVATE_CLIENT_ENV))
    }

    fn merge(self, other: Self) -> Self {
        let mut items = self.items.clone();
        for (profile_name, profile_a) in other.items.into_iter() {
            let profile_b = items.get(&profile_name).unwrap().to_owned(); 
            let both = profile_a.merge(profile_b);
            items.insert(profile_name, both);
        }
        Self { items } 
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        println!("{:?}", client);
    }

    #[test]
    fn read_from_dir() {
        let env = JetbrainsEnv::from_directory(Path::new("./test_data")).unwrap();
        println!("{env:?}");
    }

}
