use crate::{
    context::TuiContext,
    message::Message,
    view::{
        common::{
            actions::{IntoMenuActions, MenuAction},
            header_table::HeaderTable,
            text_window::{TextWindow, TextWindowProps},
        },
        context::UpdateContext,
        draw::{Draw, DrawMetadata, Generate},
        event::{Child, Emitter, Event, EventHandler, OptionEvent, ToEmitter},
        state::Identified,
        util::{highlight, view_text},
        Component, ViewContext,
    },
};
use derive_more::Display;
use ratatui::{layout::Layout, prelude::Constraint, text::Text, Frame};
use slumber_core::{
    http::{content_type::ContentType, RequestRecord},
    util::{format_byte_size, MaybeStr},
};
use std::sync::Arc;
use strum::EnumIter;

/// Display rendered HTTP request state. The request could still be in flight,
/// it just needs to have been built successfully.
#[derive(Debug)]
pub struct RequestView {
    actions_emitter: Emitter<RequestMenuAction>,
    /// Store pointer to the request, so we can access it in the update step
    request: Arc<RequestRecord>,
    /// Persist the visible body, because it may vary from the actual body.
    /// `None` iff the request has no body
    body: Option<Identified<Text<'static>>>,
    body_text_window: Component<TextWindow>,
}

impl RequestView {
    pub fn new(request: Arc<RequestRecord>) -> Self {
        let body = init_body(&request);
        Self {
            actions_emitter: Default::default(),
            request,
            body,
            body_text_window: Default::default(),
        }
    }
}

impl EventHandler for RequestView {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event.opt().emitted(self.actions_emitter, |menu_action| {
            match menu_action {
                RequestMenuAction::CopyUrl => ViewContext::send_message(
                    Message::CopyText(self.request.url.to_string()),
                ),
                RequestMenuAction::CopyBody => {
                    // Copy exactly what the user sees. Currently requests
                    // don't support formatting/querying but that could change
                    if let Some(body) = &self.body {
                        ViewContext::send_message(Message::CopyText(
                            body.to_string(),
                        ));
                    }
                }
                RequestMenuAction::ViewBody => {
                    if let Some(body) = &self.body {
                        view_text(body, self.request.mime());
                    }
                }
            }
        })
    }

    fn menu_actions(&self) -> Vec<MenuAction> {
        RequestMenuAction::into_actions(self)
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        vec![self.body_text_window.to_child_mut()]
    }
}

impl Draw for RequestView {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        let request = &self.request;
        let [version_area, url_area, headers_area, body_area] =
            Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(2),
                Constraint::Length(request.headers.len() as u16 + 2),
                Constraint::Min(0),
            ])
            .areas(metadata.area());

        // This can get cut off which is jank but there isn't a good fix. User
        // can copy the URL to see the full thing
        frame.render_widget(
            format!("{} {}", request.method, request.http_version),
            version_area,
        );
        frame.render_widget(request.url.to_string(), url_area);
        frame.render_widget(
            HeaderTable {
                headers: &request.headers,
            }
            .generate(),
            headers_area,
        );
        if let Some(body) = &self.body {
            self.body_text_window.draw(
                frame,
                TextWindowProps {
                    text: body,
                    margins: Default::default(),
                    footer: None,
                },
                body_area,
                true,
            );
        }
    }
}

impl ToEmitter<RequestMenuAction> for RequestView {
    fn to_emitter(&self) -> Emitter<RequestMenuAction> {
        self.actions_emitter
    }
}

/// Items in the actions popup menu
#[derive(Copy, Clone, Debug, Display, EnumIter)]
pub enum RequestMenuAction {
    #[display("Copy URL")]
    CopyUrl,
    #[display("Copy Body")]
    CopyBody,
    #[display("View Body")]
    ViewBody,
}

impl IntoMenuActions<RequestView> for RequestMenuAction {
    fn enabled(&self, data: &RequestView) -> bool {
        match self {
            Self::CopyUrl => true,
            Self::CopyBody | Self::ViewBody => data.body.is_some(),
        }
    }
}

/// Calculate body text, including syntax highlighting. We have to clone the
/// body to prevent a self-reference
fn init_body(request: &RequestRecord) -> Option<Identified<Text<'static>>> {
    let content_type = ContentType::from_headers(&request.headers).ok();
    request
        .body()
        .map(|body| {
            highlight::highlight_if(
                content_type,
                format!("{:#}", MaybeStr(body)).into(),
            )
            .into()
        })
        .or_else(|| {
            // No body available: check if it's because the recipe has no body,
            // or if we threw it away. This will have some false
            // positives/negatives if the recipe had a body added/removed, but
            // it's good enough
            let collection = ViewContext::collection();
            let recipe =
                collection.recipes.get(&request.recipe_id)?.recipe()?;
            if recipe.body.is_some() {
                let config = &TuiContext::get().config;

                Some(
                    Text::raw(format!(
                        "Body not available. Streamed bodies, or bodies over \
                        {}, are not persisted",
                        format_byte_size(config.http.large_body_size)
                    ))
                    .into(),
                )
            } else {
                None
            }
        })
}
