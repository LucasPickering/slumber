use crate::{
    config::{Environment, RequestCollection, RequestNode, RequestRecipe},
    ui::App,
};
use anyhow::Context;
use reqwest::{Client, Request, Response};
use std::collections::HashMap;

mod config;
mod template;
mod ui;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let collection = RequestCollection::load(None).await?;
    println!("{collection:#?}");
    let engine = Engine::new(collection);
    App::start(&engine.environments, &engine.recipes)?;
    let request = engine.build_request()?;
    // engine.execute_request(request).await?;
    Ok(())
}

#[derive(Debug)]
struct Engine {
    environments: Vec<Environment>,
    recipes: Vec<RequestRecipe>,
    http_client: Client,
    user_state: UserState,
}

#[derive(Debug)]
struct UserState {
    // TODO use strings here instead?
    selected_environment: Option<usize>,
    selected_recipe: Option<usize>,
}

impl Engine {
    fn new(collection: RequestCollection) -> Self {
        let http_client = Client::new();
        let user_state = UserState {
            selected_environment: Some(0),
            selected_recipe: Some(0),
        };
        Self {
            environments: collection.environments,
            recipes: flatten(collection.requests),
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
            .map(|index| &self.recipes[index])
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

fn flatten(recipes: Vec<RequestNode>) -> Vec<RequestRecipe> {
    fn helper(all_recipes: &mut Vec<RequestRecipe>, recipes: Vec<RequestNode>) {
        for recipe in recipes {
            match recipe {
                RequestNode::Folder { requests, .. } => {
                    helper(all_recipes, requests)
                }
                RequestNode::Request(recipe) => {
                    all_recipes.push(recipe);
                }
            }
        }
    }

    let mut all_recipes = Vec::new();
    helper(&mut all_recipes, recipes);
    all_recipes
}
