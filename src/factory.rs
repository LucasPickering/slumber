use crate::{
    collection::{Chain, ChainSource, Profile, ProfileId, RecipeId},
    db::CollectionDatabase,
    http::{Body, Request, RequestId, RequestRecord, Response},
    template::{Prompt, Prompter, Template, TemplateContext},
};
use chrono::Utc;
use factori::{create, factori};
use indexmap::IndexMap;
use reqwest::{header::HeaderMap, Method, StatusCode};

factori!(Profile, {
    default {
        id = "profile1".into(),
        name = None,
        data = Default::default(),
    }
});

factori!(Request, {
    default {
        id = RequestId::new(),
        profile_id = None,
        recipe_id = "recipe1".into(),
        method = Method::GET,
        url = "/url".into(),
        headers = HeaderMap::new(),
        query = IndexMap::new(),
        body = None,
    }
});

factori!(Response, {
    default {
        status = StatusCode::OK,
        headers = HeaderMap::new(),
        body = Body::default(),
    }
});

// Apparently you can't use a macro in the factori init expression so we have
// to hide them behind functions
fn request() -> Request {
    create!(Request)
}
fn response() -> Response {
    create!(Response)
}

factori!(RequestRecord, {
    default {
        id = RequestId::new(),
        request = request(),
        response = response(),
        start_time = Utc::now(),
        end_time = Utc::now(),
    }
});

factori!(Chain, {
    default {
        id = "chain1".into(),
        source = ChainSource::Request(RecipeId::default()),
        sensitive = false,
        selector = None,
    }
});

factori!(TemplateContext, {
    default {
        profile = None
        chains = Default::default()
        prompter = Box::<TestPrompter>::default(),
        database = CollectionDatabase::testing()
        overrides = Default::default()
    }
});

/// Return a static value when prompted, or no value if none is given
#[derive(Debug, Default)]
pub struct TestPrompter {
    value: Option<String>,
}

impl TestPrompter {
    pub fn new<T: Into<String>>(value: Option<T>) -> Self {
        Self {
            value: value.map(Into::into),
        }
    }
}

impl Prompter for TestPrompter {
    fn prompt(&self, prompt: Prompt) {
        // If no value was given, don't respond at all
        if let Some(value) = self.value.as_ref() {
            prompt.respond(value.clone())
        }
    }
}

// Some helpful conversion implementations
impl From<&str> for ProfileId {
    fn from(value: &str) -> Self {
        value.to_owned().into()
    }
}

impl From<&str> for RecipeId {
    fn from(value: &str) -> Self {
        value.to_owned().into()
    }
}

impl From<&str> for Template {
    fn from(value: &str) -> Self {
        value.to_owned().try_into().unwrap()
    }
}
