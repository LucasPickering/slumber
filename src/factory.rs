use crate::{
    config::{Chain, RequestRecipeId},
    http::{Repository, Request, RequestId, RequestRecord, Response},
    template::{TemplateContext, TemplateString},
};
use chrono::Utc;
use factori::{create, factori};
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
        id = String::new(),
        source = RequestRecipeId::default(),
        name = None,
        path = None
    }
});

factori!(TemplateContext, {
    default {
        profile = Default::default()
        chains = Default::default()
        repository = Repository::testing()
        overrides = Default::default()
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
