use crate::{
    config::{Chain, RequestRecipeId},
    http::{Request, Response},
    template::TemplateString,
};
use factori::factori;
use indexmap::IndexMap;
use reqwest::{header::HeaderMap, Method, StatusCode};

factori!(Request, {
    default {
        recipe_id = String::new().into(),
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
        body = String::new(),
    }
});

factori!(Chain, {
    default {
        id = String::new(),
        source = RequestRecipeId::default(),
        name = None,
        path = None
    }
});

// Some helpful conversion implementations
impl From<&str> for RequestRecipeId {
    fn from(value: &str) -> Self {
        value.to_owned().into()
    }
}

impl From<&str> for TemplateString {
    fn from(value: &str) -> Self {
        value.to_owned().into()
    }
}
