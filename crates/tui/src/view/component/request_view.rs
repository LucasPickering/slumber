use crate::{
    context::TuiContext,
    message::Message,
    view::{
        common::{
            actions::ActionsModal,
            header_table::HeaderTable,
            text_window::{TextWindow, TextWindowProps},
        },
        context::UpdateContext,
        draw::{Draw, DrawMetadata, Generate, ToStringGenerate},
        event::{Child, Event, EventHandler, Update},
        state::{Identified, StateCell},
        util::highlight,
        Component, ViewContext,
    },
};
use derive_more::Display;
use ratatui::{layout::Layout, prelude::Constraint, text::Text, Frame};
use slumber_config::Action;
use slumber_core::{
    http::{content_type::ContentType, RequestId, RequestRecord},
    util::{format_byte_size, MaybeStr},
};
use std::sync::Arc;
use strum::{EnumCount, EnumIter};

/// Display rendered HTTP request state. The request could still be in flight,
/// it just needs to have been built successfully.
#[derive(Debug, Default)]
pub struct RequestView {
    state: StateCell<RequestId, State>,
    body_text_window: Component<TextWindow>,
}

pub struct RequestViewProps {
    pub request: Arc<RequestRecord>,
}

/// Inner state, which should be reset when request changes
#[derive(Debug)]
struct State {
    /// Store pointer to the request, so we can access it in the update step
    request: Arc<RequestRecord>,
    /// Persist the visible body, because it may vary from the actual body.
    /// `None` iff the request has no body
    body: Option<Identified<Text<'static>>>,
}

/// Items in the actions popup menu
#[derive(
    Copy, Clone, Debug, Default, Display, EnumCount, EnumIter, PartialEq,
)]
enum MenuAction {
    #[default]
    #[display("Edit Collection")]
    EditCollection,
    #[display("Copy URL")]
    CopyUrl,
    #[display("Copy Body")]
    CopyBody,
}

impl ToStringGenerate for MenuAction {}

impl EventHandler for RequestView {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Update {
        if let Some(Action::OpenActions) = event.action() {
            let disabled = if self
                .state
                .get_mut()
                .and_then(|state| state.body.as_ref())
                .is_some()
            {
                [].as_slice()
            } else {
                &[MenuAction::CopyBody]
            };
            ViewContext::open_modal(ActionsModal::new(disabled));
        } else if let Some(action) = event.local::<MenuAction>() {
            match action {
                MenuAction::EditCollection => {
                    ViewContext::send_message(Message::CollectionEdit)
                }
                MenuAction::CopyUrl => {
                    if let Some(state) = self.state.get() {
                        ViewContext::send_message(Message::CopyText(
                            state.request.url.to_string(),
                        ))
                    }
                }
                MenuAction::CopyBody => {
                    // Copy exactly what the user sees. Currently requests
                    // don't support formatting/querying but that could change
                    if let Some(body) = self.state.get().and_then(|state| {
                        let body = state.body.as_ref()?;
                        Some(body.to_string())
                    }) {
                        ViewContext::send_message(Message::CopyText(body));
                    }
                }
            }
        } else {
            return Update::Propagate(event);
        }
        Update::Consumed
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        vec![self.body_text_window.to_child_mut()]
    }
}

impl Draw<RequestViewProps> for RequestView {
    fn draw(
        &self,
        frame: &mut Frame,
        props: RequestViewProps,
        metadata: DrawMetadata,
    ) {
        let state = self.state.get_or_update(&props.request.id, || State {
            request: Arc::clone(&props.request),
            body: init_body(&props.request),
        });

        let [url_area, headers_area, body_area] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(props.request.headers.len() as u16 + 2),
            Constraint::Min(0),
        ])
        .areas(metadata.area());

        // This can get cut off which is jank but there isn't a good fix. User
        // can copy the URL to see the full thing
        frame.render_widget(props.request.url.to_string(), url_area);
        frame.render_widget(
            HeaderTable {
                headers: &props.request.headers,
            }
            .generate(),
            headers_area,
        );
        if let Some(body) = &state.body {
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
