use crate::{
    context::TuiContext,
    http::{RequestStateSummary, RequestStore},
    util::ResultReported,
    view::{
        Generate, UpdateContext, ViewContext,
        common::{
            Pane,
            button::ButtonGroup,
            select::{Select, SelectEventType, SelectListProps},
        },
        component::{
            Canvas, Component, ComponentId, Draw, DrawMetadata,
            internal::{Child, ToChild},
            misc::ConfirmButton,
        },
        event::{Event, EventMatch},
        persistent::PersistentKey,
    },
};
use ratatui::text::{Line, Span, Text};
use serde::Serialize;
use slumber_config::Action;
use slumber_core::{
    collection::{ProfileId, RecipeId},
    http::RequestId,
};

/// Browse request/response history for a recipe
#[derive(Debug)]
pub struct History {
    id: ComponentId,
    select: Select<RequestStateSummary>,
    /// Are we in the process of deleting the selected request? If so, we'll
    /// show a delete confirmation instead of the normal list.
    deleting: bool,
    /// Confirmation buttons for a deletion. This needs to be reset between
    /// deletes.
    delete_confirm_buttons: ButtonGroup<ConfirmButton>,
}

impl History {
    /// TODO
    pub fn new(
        recipe_id: Option<&RecipeId>,
        profile_id: Option<&ProfileId>,
        request_store: &RequestStore,
    ) -> Self {
        // Make sure all requests for this profile+recipe are loaded

        let requests = recipe_id
            // If there is no recipe selected, just use an empty list
            .and_then(|recipe_id| {
                request_store
                    .load_summaries(profile_id, recipe_id)
                    .reported(&ViewContext::messages_tx())
            })
            .map(Vec::from_iter)
            .unwrap_or_default();
        let select = Select::builder(requests)
            .subscribe([SelectEventType::Select])
            .persisted(&SelectedRequestKey)
            .build();

        Self {
            id: ComponentId::default(),
            select,
            deleting: false,
            delete_confirm_buttons: Default::default(),
        }
    }

    /// TODO
    pub fn selected_request_id(&self) -> Option<RequestId> {
        self.select.selected().map(RequestStateSummary::id)
    }

    /// TODO
    pub fn select_request(&mut self, request_id: RequestId) {
        self.select.select(&request_id);
    }

    /// Delete the selected request from the request store and our own list
    fn delete_selected(&mut self, request_store: &mut RequestStore) {
        // It doesn't make sense to get to this point in the workflow without
        // a selected request ID, but we don't want to panic if we do
        if let Some(request) = self.select.selected() {
            request_store
                .delete_request(request.id())
                .reported(&ViewContext::messages_tx());
        }
        self.select.delete_selected();
        // TODO make sure the visible item is wiped out
    }
}

impl Component for History {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(
        &mut self,
        context: &mut UpdateContext,
        event: Event,
    ) -> EventMatch {
        event.m().action(|action, propagate| match action {
            Action::Delete => {
                if self.select.selected().is_some() {
                    // Morph into a confirmation modal
                    self.deleting = true;
                }
            }
            // Only consume submission if we're in delete confirmation
            Action::Submit if self.deleting => {
                if self.delete_confirm_buttons.selected().to_bool() {
                    self.delete_selected(context.request_store);
                }
                // Reset state for next time
                self.deleting = false;
                self.delete_confirm_buttons = Default::default();
            }
            _ => propagate.set(),
        })
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        if self.deleting {
            vec![self.delete_confirm_buttons.to_child_mut()]
        } else {
            vec![self.select.to_child_mut()]
        }
    }
}

impl Draw for History {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let title = if self.deleting {
            "Delete Request?"
        } else {
            "Request History"
        };

        let block = Pane {
            title,
            has_focus: metadata.has_focus(),
        }
        .generate();
        let area = block.inner(metadata.area());
        canvas.render_widget(block, metadata.area());

        if self.deleting {
            canvas.draw(&self.delete_confirm_buttons, (), area, true);
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

/// Persistence key for the selected request
///
/// Exported for testing in the root component
#[derive(Debug, Serialize)]
pub struct SelectedRequestKey;

impl PersistentKey for SelectedRequestKey {
    type Value = RequestId;
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
    use slumber_util::Factory;
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
                Some(recipe_id),
                Some(profile_id),
                &harness.request_store.borrow_mut(),
            ),
        );

        // Initial state
        component.int().drain_draw().assert_empty();
        assert_eq!(component.selected_request_id(), Some(exchanges[0].id));

        // Select the next one
        component.int().drain_draw().assert_empty();
        assert_eq!(component.selected_request_id(), Some(exchanges[1].id));
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

        let mut component = TestComponent::new(
            &harness,
            &terminal,
            History::new(
                Some(recipe_id),
                Some(profile_id),
                &harness.request_store.borrow_mut(),
            ),
        );

        // Initial state
        component.int().drain_draw().assert_empty();
        assert_eq!(component.selected_request_id(), Some(exchanges[0].id));

        // Delete the first. Second is now selected
        component
            .int()
            .send_keys([KeyCode::Delete, KeyCode::Enter])
            .assert_empty();
        assert_eq!(component.selected_request_id(), Some(exchanges[1].id));

        // Delete the second. Nothing selected now
        component
            .int()
            .send_keys([KeyCode::Delete, KeyCode::Enter])
            .assert_empty();
        assert_eq!(component.selected_request_id(), None);

        // Make sure both the request store and the DB were updated
        let requests = harness
            .request_store
            .borrow_mut()
            .load_summaries(Some(profile_id), recipe_id)
            .unwrap()
            .collect_vec();
        assert_eq!(&requests, &[] as &[RequestStateSummary]);
        assert_eq!(&harness.database.get_all_requests().unwrap(), &[]);
    }
}
