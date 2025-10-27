use crate::{
    context::TuiContext,
    message::Message,
    view::{
        Component, Generate, ViewContext,
        common::{
            header_table::HeaderTable,
            text_window::{TextWindow, TextWindowProps},
        },
        component::{
            Canvas, ComponentId, Draw, DrawMetadata,
            internal::{Child, ToChild},
        },
        context::UpdateContext,
        event::{Event, OptionEvent},
        state::Identified,
        util::{format_byte_size, highlight, view_text},
    },
};
use ratatui::{layout::Layout, prelude::Constraint, text::Text};
use slumber_config::Action;
use slumber_core::{
    http::{RequestId, RequestRecord, content_type::ContentType},
    util::MaybeStr,
};
use std::sync::Arc;

/// Display rendered HTTP request state. The request could still be in flight,
/// it just needs to have been built successfully.
#[derive(Debug)]
pub struct RequestView {
    id: ComponentId,
    /// Store pointer to the request, so we can access it in the update step
    request: Arc<RequestRecord>,
    /// Persist the visible body, because it may vary from the actual body.
    /// `None` iff the request has no body
    body: Option<Identified<Text<'static>>>,
    body_text_window: TextWindow,
}

impl RequestView {
    pub fn new(request: Arc<RequestRecord>) -> Self {
        let body = init_body(&request);
        Self {
            id: ComponentId::default(),
            request,
            body,
            body_text_window: Default::default(),
        }
    }

    pub fn id(&self) -> RequestId {
        self.request.id
    }

    pub fn has_body(&self) -> bool {
        self.body.is_some()
    }

    pub fn copy_url(&self) {
        ViewContext::send_message(Message::CopyText(
            self.request.url.to_string(),
        ));
    }

    pub fn view_body(&self) {
        if let Some(body) = &self.body {
            view_text(body, self.request.mime());
        }
    }

    pub fn copy_body(&self) {
        // Copy exactly what the user sees. Currently requests don't support
        // formatting/querying but that could change
        if let Some(body) = &self.body {
            ViewContext::send_message(Message::CopyText(body.to_string()));
        }
    }
}

impl Component for RequestView {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event.opt().action(|action, propagate| match action {
            Action::View => self.view_body(),
            _ => propagate.set(),
        })
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.body_text_window.to_child_mut()]
    }
}

impl Draw for RequestView {
    fn draw_impl(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
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
        canvas.render_widget(
            format!("{} {}", request.method, request.http_version),
            version_area,
        );
        canvas.render_widget(request.url.to_string(), url_area);
        canvas.render_widget(
            HeaderTable {
                headers: &request.headers,
            }
            .generate(),
            headers_area,
        );
        if let Some(body) = &self.body {
            canvas.draw(
                &self.body_text_window,
                TextWindowProps {
                    text: body,
                    margins: Default::default(),
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
