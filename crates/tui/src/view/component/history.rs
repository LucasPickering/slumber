use crate::{
    context::TuiContext,
    http::{RequestStateSummary, RequestStore},
    util::ResultReported,
    view::{
        UpdateContext, ViewContext,
        common::{button::ButtonGroup, list::List, modal::Modal},
        component::{Component, misc::ConfirmButton},
        draw::{Draw, DrawMetadata, Generate},
        event::{Child, Event, EventHandler, OptionEvent, ToEmitter},
        state::select::{SelectState, SelectStateEvent, SelectStateEventType},
    },
};
use ratatui::{
    Frame,
    layout::Constraint,
    text::{Line, Span},
};
use slumber_config::Action;
use slumber_core::{collection::RecipeId, http::RequestId};

/// Browse request/response history for a recipe
#[derive(Debug)]
pub struct History {
    recipe_name: String,
    select: Component<SelectState<RequestStateSummary>>,
    /// Are we in the process of deleting the selected request? If so, we'll
    /// show a delete confirmation instead of the normal list.
    deleting: bool,
    /// Confirmation buttons for a deletion. This can't be part of the above
    /// option because it makes the emitter handling logic in `update()`
    /// annoying. This needs to be reset between deletes.
    delete_confirm_buttons: Component<ButtonGroup<ConfirmButton>>,
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
            deleting: false,
            delete_confirm_buttons: Default::default(),
        }
    }

    /// Delete the selected request from the request store and our own list
    fn delete_selected(&mut self, request_store: &mut RequestStore) {
        // It doesn't make sense to get to this point in the workflow without
        // a selected request ID, but we don't want to panic if we do
        if let Some(request) = self.select.data().selected() {
            request_store
                .delete_request(request.id())
                .reported(&ViewContext::messages_tx());
        }
        self.select.data_mut().delete_selected();
        if self.select.data().is_empty() {
            // Let the root know there's nothing left. This is necessary because
            // the select doesn't emit an event when the final item is deleted
            ViewContext::push_event(Event::HttpSelectRequest(None));
        }
    }
}

impl Modal for History {
    fn title(&self) -> Line<'_> {
        if self.deleting {
            "Delete Request?".into()
        } else {
            vec![
                "History for ".into(),
                Span::styled(
                    self.recipe_name.as_str(),
                    TuiContext::get().styles.text.primary,
                ),
            ]
            .into()
        }
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        let height = if self.deleting {
            1
        } else {
            self.select.data().len().min(20) as u16
        };
        (Constraint::Length(40), Constraint::Length(height))
    }
}

impl EventHandler for History {
    fn update(
        &mut self,
        context: &mut UpdateContext,
        event: Event,
    ) -> Option<Event> {
        event
            .opt()
            .action(|action, propagate| match action {
                Action::Delete => {
                    if self.select.data().selected().is_some() {
                        // Morph into a confirmation modal
                        self.deleting = true;
                    }
                }
                _ => propagate.set(),
            })
            .emitted(self.delete_confirm_buttons.to_emitter(), |event| {
                if event == ConfirmButton::Yes {
                    self.delete_selected(context.request_store);
                }
                // Reset state for next time
                self.deleting = false;
                self.delete_confirm_buttons = Default::default();
            })
            .emitted(self.select.to_emitter(), |event| {
                if let SelectStateEvent::Select(index) = event {
                    ViewContext::push_event(Event::HttpSelectRequest(Some(
                        self.select.data()[index].id(),
                    )));
                }
            })
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        if self.deleting {
            vec![self.delete_confirm_buttons.to_child_mut()]
        } else {
            vec![self.select.to_child_mut()]
        }
    }
}

impl Draw for History {
    fn draw(&self, frame: &mut Frame, (): (), metadata: DrawMetadata) {
        if self.deleting {
            self.delete_confirm_buttons
                .draw(frame, (), metadata.area(), true);
        } else {
            self.select.draw(
                frame,
                List::from(self.select.data()),
                metadata.area(),
                true,
            );
        }
    }
}

impl Generate for &RequestStateSummary {
    type Output<'this>
        = Line<'this>
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestHarness, TestTerminal, harness, terminal},
        view::test_util::TestComponent,
    };
    use crossterm::event::KeyCode;
    use itertools::Itertools;
    use rstest::rstest;
    use slumber_core::http::Exchange;
    use slumber_util::{Factory, assert_matches};

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

        let requests = harness
            .request_store
            .borrow_mut()
            .load_summaries(Some(profile_id), recipe_id)
            .unwrap()
            .collect();
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            History::new(recipe_id, requests, None),
        );

        // Initial state
        let selected = assert_matches!(
            component.int().drain_draw().events(),
            &[Event::HttpSelectRequest(Some(selected))] => selected,
        );
        assert_eq!(selected, exchanges[0].id);

        // Select the next one
        let selected = assert_matches!(
            component.int().send_key(KeyCode::Down).events(),
            &[Event::HttpSelectRequest(Some(selected))] => selected,
        );
        assert_eq!(selected, exchanges[1].id);
    }

    /// Test that we can delete requests from the store
    #[rstest]
    fn test_delete(harness: TestHarness, terminal: TestTerminal) {
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

        let requests = harness
            .request_store
            .borrow_mut()
            .load_summaries(Some(profile_id), recipe_id)
            .unwrap()
            .collect();
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            History::new(recipe_id, requests, None),
        );

        // Initial state
        let selected = assert_matches!(
            component.int().drain_draw().events(),
            &[Event::HttpSelectRequest(Some(selected))] => selected,
        );
        assert_eq!(selected, exchanges[0].id);

        // Delete the first. Second is now selected
        let selected = assert_matches!(
            component
                .int()
                .send_keys([KeyCode::Delete, KeyCode::Enter])
                .events(),
            &[Event::HttpSelectRequest(Some(selected))] => selected,
        );
        assert_eq!(selected, exchanges[1].id);

        // Delete the second. Nothing selected now
        assert_matches!(
            component
                .int()
                .send_keys([KeyCode::Delete, KeyCode::Enter])
                .events(),
            &[Event::HttpSelectRequest(None)],
        );

        // Make sure both the request store and the DB were updated
        let requests = harness
            .request_store
            .borrow_mut()
            .load_summaries(Some(profile_id), recipe_id)
            .unwrap()
            .collect_vec();
        assert_eq!(&requests, &[]);
        assert_eq!(&harness.database.get_all_requests().unwrap(), &[]);
    }
}
