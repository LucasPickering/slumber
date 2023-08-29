use crate::config::{
    Environment, RequestCollection, RequestNode, RequestRecipe,
};
use anyhow::Context;
use reqwest::{Client, Request, Response};
use std::collections::HashMap;

mod config;
mod template;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let collection = RequestCollection::load(None).await?;
    println!("{collection:#?}");
    let engine = Engine::new(collection);
    let request = engine.build_request()?;
    engine.execute_request(request).await?;
    Ok(())
}

#[derive(Debug)]
struct Engine {
    recipes: RequestRecipes,
    environments: Vec<Environment>,
    http_client: Client,
    user_state: UserState,
}

#[derive(Debug)]
struct UserState {
    // TODO use string here instead
    selected_environment: Option<usize>,
    selected_recipe: Option<String>,
}

impl Engine {
    fn new(collection: RequestCollection) -> Self {
        let http_client = Client::new();
        let user_state = UserState {
            selected_environment: Some(0),
            selected_recipe: Some("get-users".into()),
        };
        Self {
            environments: collection.environments,
            recipes: RequestRecipes::flatten(collection.requests).unwrap(),
            http_client,
            user_state,
        }
    }

    fn environment(&self) -> Option<&HashMap<String, String>> {
        self.user_state
            .selected_environment
            .map(|index| &self.environments[index].data)
    }

    fn recipe(&self) -> Option<&RequestRecipe> {
        self.user_state
            .selected_recipe
            .as_ref()
            .map(|id| &self.recipes.recipes[id])
    }

    fn build_request(&self) -> anyhow::Result<Request> {
        // TODO add error contexts
        let environment = self.environment().unwrap();
        let recipe = self.recipe().unwrap();
        let method = recipe.method.render(environment)?.parse()?;
        let url = recipe.url.render(environment)?;
        self.http_client
            .request(method, url)
            .build()
            .context("TODO")
    }

    async fn execute_request(
        &self,
        request: Request,
    ) -> reqwest::Result<Response> {
        self.http_client.execute(request).await
    }
}

#[derive(Debug)]
struct RequestRecipes {
    recipes: HashMap<String, RequestRecipe>,
}

impl RequestRecipes {
    fn flatten(recipes: Vec<RequestNode>) -> anyhow::Result<Self> {
        fn helper(
            ass: &mut RequestRecipes,
            recipes: Vec<RequestNode>,
        ) -> anyhow::Result<()> {
            for recipe in recipes {
                match recipe {
                    RequestNode::Folder { requests, .. } => {
                        helper(ass, requests)?
                    }
                    RequestNode::Request(recipe) => {
                        if ass
                            .recipes
                            .insert(recipe.id.clone(), recipe)
                            .is_some()
                        {
                            todo!()
                        }
                    }
                }
            }
            Ok(())
        }

        let mut ass = Self {
            recipes: HashMap::new(),
        };
        helper(&mut ass, recipes)?;
        Ok(ass)
    }
}
