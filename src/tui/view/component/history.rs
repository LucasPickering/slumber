use crate::{
    collection::Recipe,
    http::RequestId,
    tui::{
        context::TuiContext,
        view::{
            common::{list::List, modal::Modal},
            component::Component,
            draw::{Draw, DrawMetadata, Generate},
            event::{Event, EventHandler},
            state::{select::SelectState, RequestStateSummary},
            ViewContext,
        },
    },
};
use ratatui::{
    layout::Constraint,
    text::{Line, Span},
    Frame,
};

/// Browse request/response history for a recipe
#[derive(Debug)]
pub struct History {
    recipe_name: String,
    select: Component<SelectState<RequestStateSummary>>,
}

impl History {
    /// Construct a new history modal with the given list of requests. Parent
    /// is responsible for loading the list from the request store.
    pub fn new(
        recipe: &Recipe,
        requests: Vec<RequestStateSummary>,
        selected_request_id: Option<RequestId>,
    ) -> Self {
        let select = SelectState::builder(requests)
            .preselect_opt(selected_request_id.as_ref())
            // When an item is selected, load it up
            .on_select(|exchange| {
                ViewContext::push_event(Event::HttpSelectRequest(Some(
                    exchange.id(),
                )))
            })
            .build();

        Self {
            recipe_name: recipe.name().to_owned(),
            select: select.into(),
        }
    }
}

impl Modal for History {
    fn title(&self) -> Line<'_> {
        vec![
            "History for ".into(),
            Span::styled(
                self.recipe_name.as_str(),
                TuiContext::get().styles.text.primary,
            ),
        ]
        .into()
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        (
            Constraint::Length(40),
            Constraint::Length(self.select.data().items().len().min(20) as u16),
        )
    }
}

impl EventHandler for History {
    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        vec![self.select.as_child()]
    }
}

impl Draw for History {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        self.select.draw(
            frame,
            List::new(self.select.data().items()),
            metadata.area(),
            true,
        );
    }
}

impl Generate for &RequestStateSummary {
    type Output<'this> = Line<'this> where Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        let styles = &TuiContext::get().styles;
        let description: Span = match self {
            RequestStateSummary::Building { .. } => "Initializing...".into(),
            RequestStateSummary::BuildError { .. } => {
                Span::styled("Build error", styles.text.error)
            }
            RequestStateSummary::Loading { .. } => "Loading...".into(),
            RequestStateSummary::Response(exchange) => {
                exchange.status.generate()
            }
            RequestStateSummary::RequestError { .. } => {
                Span::styled("Request error", styles.text.error)
            }
        };
        vec![self.time().generate(), " ".into(), description].into()
    }
}

/// Allow selection by ID
impl PartialEq<RequestStateSummary> for RequestId {
    fn eq(&self, other: &RequestStateSummary) -> bool {
        self == &other.id()
    }
}
