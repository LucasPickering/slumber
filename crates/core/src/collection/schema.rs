//! Helpers for JSON Schema generation

use crate::{
    collection::{
        Authentication, Collection, Folder, Profile, Recipe, RecipeBody,
        RecipeTree,
    },
    http::HttpMethod,
    test_util::by_id,
};
use indexmap::indexmap;
use serde_json::json;
use slumber_util::{Factory, yaml::SourceLocation};

impl Collection {
    /// JSON Schema example value
    pub fn example() -> Self {
        Self {
            name: Some("Example".into()),
            profiles: by_id([
                Profile::example(),
                Profile {
                    id: "remote".into(),
                    name: Some("Remote".into()),
                    default: false,
                    data: indexmap! {
                        "host".into() => "https://myfishes.fish".into()
                    },
                },
            ]),
            recipes: RecipeTree::new(by_id([
                Recipe::example().into(),
                Folder {
                    id: "my_folder".into(),
                    location: SourceLocation::default(),
                    name: Some("My Folder".to_owned()),
                    children: by_id([
                        Recipe::factory("recipe1").into(),
                        Recipe::factory("recipe2").into(),
                    ]),
                }
                .into(),
            ]))
            .unwrap(),
        }
    }
}

impl Profile {
    /// JSON Schema example value
    pub fn example() -> Self {
        Profile {
            id: "local".into(),
            name: Some("Local".into()),
            default: true,
            data: indexmap! {
                "host".into() => "http://localhost:8000".into()
            },
        }
    }
}

impl Folder {
    /// JSON Schema example value
    pub fn example() -> Self {
        Folder {
            id: "my_folder".into(),
            location: SourceLocation::default(),
            name: Some("My Folder".into()),
            children: by_id([Recipe::example().into()]),
        }
    }
}

impl Recipe {
    /// JSON Schema example value
    pub fn example() -> Self {
        Recipe {
            id: "my_recipe".into(),
            location: SourceLocation::default(),
            name: Some("My Recipe".into()),
            method: HttpMethod::Post,
            persist: true,
            url: "http://localhost:8000/fish".into(),
            body: Some(RecipeBody::Json(
                json!({
                    "name": "Barry",
                    "species": "Barracuda",
                })
                .try_into()
                .unwrap(),
            )),
            authentication: Some(Authentication::Basic {
                username: "mememe".into(),
                password: Some(
                    "{{ prompt(message='Password', sensitive=true) }}".into(),
                ),
            }),
            query: indexmap! {
                "submit".into() => "true".into(),
                "param_with_multiple_values".into() => ["value1", "value2"].into(),
            },
            headers: indexmap! {
                "Accept".into() => "application/json".into(),
            },
        }
    }
}
