use crate::{
    context::TuiContext,
    http::RequestStateSummary,
    util::ResultReported,
    view::{
        common::{list::List, modal::Modal},
        component::Component,
        draw::{Draw, DrawMetadata, Generate},
        event::{Child, Event, EventHandler, OptionEvent, ToEmitter},
        state::select::{SelectState, SelectStateEvent, SelectStateEventType},
        UpdateContext, ViewContext,
    },
};
use ratatui::{
    layout::Constraint,
    text::{Line, Span},
    Frame,
};
use slumber_core::{collection::RecipeId, http::RequestId};

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
        recipe_id: &RecipeId,
        requests: Vec<RequestStateSummary>,
        selected_request_id: Option<RequestId>,
    ) -> Self {
        let recipe_name = ViewContext::collection()
            .recipes
            .try_get_recipe(recipe_id)
            .reported(&ViewContext::messages_tx())
            .map(|recipe| recipe.name().to_owned())
            .unwrap_or_else(|| recipe_id.to_string());
        let select = SelectState::builder(requests)
            .subscribe([SelectStateEventType::Select])
            .preselect_opt(selected_request_id.as_ref())
            .build();

        Self {
            recipe_name,
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
            Constraint::Length(self.select.data().len().min(20) as u16),
        )
    }
}

impl EventHandler for History {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event.opt().emitted(self.select.to_emitter(), |event| {
            if let SelectStateEvent::Select(index) = event {
                ViewContext::push_event(Event::HttpSelectRequest(Some(
                    self.select.data()[index].id(),
                )))
            }
        })
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        vec![self.select.to_child_mut()]
    }
}

impl Draw for History {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        self.select.draw(
            frame,
            List::from(self.select.data()),
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
            RequestStateSummary::Cancelled { .. } => "Cancelled".into(),
            RequestStateSummary::Response(exchange) => {
                exchange.status.generate()
            }
            RequestStateSummary::RequestError { .. } => {
                Span::styled("Request error", styles.text.error)
            }
        };
        vec![
            self.start_time().generate(),
            " / ".into(),
            self.duration().generate(),
            " ".into(),
            description,
        ]
        .into()
    }
}

/// Allow selection by ID
impl PartialEq<RequestStateSummary> for RequestId {
    fn eq(&self, other: &RequestStateSummary) -> bool {
        self == &other.id()
    }
}
