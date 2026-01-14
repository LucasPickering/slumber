use crate::{
    context::TuiContext,
    http::{RequestStateSummary, RequestStore},
    util::ResultReported,
    view::{
        Generate, UpdateContext, ViewContext,
        common::{
            Pane,
            actions::MenuItem,
            select::{Select, SelectEvent, SelectEventType, SelectListProps},
        },
        component::{
            Canvas, Component, ComponentId, Draw, DrawMetadata,
            internal::{Child, ToChild},
        },
        event::{DeleteTarget, Emitter, Event, EventMatch, ToEmitter},
    },
};
use ratatui::text::{Line, Span, Text};
use slumber_config::Action;
use slumber_core::{
    collection::{ProfileId, RecipeId},
    http::RequestId,
};

/// Browse request/response history for a recipe
#[derive(Debug)]
pub struct History {
    id: ComponentId,
    actions_emitter: Emitter<HistoryAction>,
    select: Select<RequestStateSummary>,
}

impl History {
    /// Construct a new history modal with the given list of requests. Parent
    /// is responsible for loading the list from the request store.
    pub fn new(
        recipe_id: &RecipeId,
        profile_id: Option<&ProfileId>,
        request_store: &RequestStore,
        selected_request_id: Option<RequestId>,
    ) -> Self {
        // Make sure all requests for this profile+recipe are loaded
        let requests = request_store
            .load_summaries(profile_id, recipe_id)
            .reported(&ViewContext::messages_tx())
            .map(Vec::from_iter)
            .unwrap_or_default();
        let select = Select::builder(requests)
            .subscribe([SelectEventType::Select])
            .preselect_opt(selected_request_id.as_ref())
            .build();

        Self {
            id: ComponentId::default(),
            actions_emitter: Emitter::default(),
            select,
        }
    }
}

impl Component for History {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            .action(|action, propagate| match action {
                Action::Delete => ViewContext::push_event(
                    Event::DeleteRequests(DeleteTarget::Request),
                ),
                _ => propagate.set(),
            })
            .emitted(self.actions_emitter, |menu_action| match menu_action {
                HistoryAction::Delete => ViewContext::push_event(
                    Event::DeleteRequests(DeleteTarget::Request),
                ),
                HistoryAction::DeleteAll => ViewContext::push_event(
                    Event::DeleteRequests(DeleteTarget::Recipe),
                ),
            })
            .emitted(self.select.to_emitter(), |event| {
                if let SelectEvent::Select(index) = event {
                    ViewContext::push_event(Event::HttpSelectRequest(Some(
                        self.select[index].id(),
                    )));
                }
            })
    }

    fn menu(&self) -> Vec<MenuItem> {
        let emitter = self.actions_emitter;
        let has_requests = !self.select.is_empty();
        vec![
            emitter
                .menu(HistoryAction::Delete, "Delete Request")
                .shortcut(Some(Action::Delete))
                .enable(has_requests)
                .into(),
            emitter
                .menu(HistoryAction::DeleteAll, "Delete All")
                .enable(has_requests)
                .into(),
        ]
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.select.to_child_mut()]
    }
}

impl Draw for History {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let block = Pane {
            title: "Request History",
            has_focus: metadata.has_focus(),
        }
        .generate();
        let area = block.inner(metadata.area());
        canvas.render_widget(block, metadata.area());

        if self.select.is_empty() {
            canvas.render_widget("No requests", area);
        } else {
            canvas.draw(&self.select, SelectListProps::modal(), area, true);
        }
    }
}

impl Generate for &RequestStateSummary {
    type Output<'this>
        = Text<'this>
    where
        Self: 'this;

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
            Line::from_iter([
                self.start_time().generate(),
                " / ".into(),
                self.duration().generate(),
            ]),
            description.into(),
        ]
        .into()
    }
}

/// Allow selection by ID
impl PartialEq<RequestId> for RequestStateSummary {
    fn eq(&self, id: &RequestId) -> bool {
        &self.id() == id
    }
}

#[derive(Copy, Clone, Debug)]
enum HistoryAction {
    /// Delete the selected request
    Delete,
    /// Delete all requests for this recipe (optionally filter by profile)
    DeleteAll,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestHarness, TestTerminal, harness, terminal},
        view::test_util::TestComponent,
    };
    use itertools::Itertools;
    use rstest::rstest;
    use slumber_core::http::Exchange;
    use slumber_util::{Factory, assert_matches};
    use terminput::KeyCode;

    /// Test that we can browse requests, and selecting one updates root state
    #[rstest]
    fn test_navigation(harness: TestHarness, terminal: TestTerminal) {
        let profile_id = harness.collection.first_profile_id();
        let recipe_id = harness.collection.first_recipe_id();
        // Populate the DB
        let exchanges = (0..2)
            .map(|_| {
                Exchange::factory((Some(profile_id.clone()), recipe_id.clone()))
            })
            // Sort to match the modal
            .sorted_by_key(|exchange| exchange.start_time)
            .rev()
            .collect_vec();
        for exchange in &exchanges {
            harness.database.insert_exchange(exchange).unwrap();
        }

        let mut component = TestComponent::new(
            &harness,
            &terminal,
            History::new(
                recipe_id,
                Some(profile_id),
                &harness.request_store(),
                None,
            ),
        );

        // Initial state
        let selected = assert_matches!(
            component.int().drain_draw().into_propagated().as_slice(),
            &[Event::HttpSelectRequest(Some(selected))] => selected,
        );
        assert_eq!(selected, exchanges[0].id);

        // Select the next one
        let selected = assert_matches!(
            component.int().send_key(KeyCode::Down).into_propagated().as_slice(),
            &[Event::HttpSelectRequest(Some(selected))] => selected,
        );
        assert_eq!(selected, exchanges[1].id);
    }
}
