use crate::{
    config::{Chain, RequestRecipeId},
    http::{Request, Response},
};
use factori::factori;
use reqwest::StatusCode;
use std::collections::HashMap;

factori!(Request, {
    default {
        recipe_id = String::new().into(),
        method = "GET".into(),
        url = "/url".into(),
        headers = HashMap::new(),
        query = HashMap::new(),
        body = None,
    }
});

factori!(Response, {
    default {
        status = StatusCode::OK,
        headers = HashMap::new(),
        content = String::new(),
    }
});

factori!(Chain, {
    default {
        id = String::new(),
        source = RequestRecipeId::default(),
        name = None,
    }
});
