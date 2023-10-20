//! The building blocks of the view

mod misc;
mod primary;

use crate::{
    config::{RequestCollection, RequestRecipeId},
    http::RequestRecord,
    template::Prompt,
    tui::{
        input::Action,
        message::Message,
        view::{
            component::{
                misc::{ErrorModal, HelpText, NotificationText, PromptModal},
                primary::{
                    ListPaneProps, ProfileListPane, RecipeListPane,
                    RequestPane, RequestPaneProps, ResponsePane,
                    ResponsePaneProps,
                },
            },
            state::{Notification, PrimaryPane, RequestState, StatefulSelect},
            util::layout,
            Frame, RenderContext,
        },
    },
};
use chrono::Utc;
use crossterm::event::Event;
use ratatui::prelude::{Constraint, Direction, Rect};
use std::{
    collections::{hash_map, HashMap},
    fmt::Debug,
};
use tracing::{error, trace};

/// The main building block that makes up the view. This is modeled after React,
/// with some key differences:
///
/// - State can be exposed from child to parent
///   - This is arguably an anti-pattern, but it's a simple solution. Rust makes
///     it possible to expose only immutable references, so I think it's fine.
/// - State changes are managed via message passing rather that callbacks. See
///   [Component::update_all] and [Component::update]. This happens during the
///   message phase of the TUI.
/// - Rendering is provided by a separate trait: [Draw]
pub trait Component: Debug {
    /// Update the state of *just* this component according to the message.
    /// Returned outcome indicates what to do afterwards.
    fn update(&mut self, message: ViewMessage) -> UpdateOutcome {
        // By default just forward to our parent
        UpdateOutcome::Propagate(message)
    }

    /// Update the state of this component *and* its children, starting at the
    /// lowest descendant. Recursively walk up the tree until a component
    /// consumes the message.
    fn update_all(&mut self, mut message: ViewMessage) -> UpdateOutcome {
        // If we have a child, send them the message. If not, eat it ourselves
        for child in self.focused_children() {
            let outcome = child.update_all(message);
            if let UpdateOutcome::Propagate(returned) = outcome {
                // Keep going to the next child. It's possible the child
                // returned something other than the original message, which
                // we'll just pass along anyway.
                message = returned;
            } else {
                trace!(?child, "View message consumed");
                return outcome;
            }
        }
        // None of our children handled it, we'll take it ourselves
        self.update(message)
    }

    /// Which, if any, of this component's children currently has focus? The
    /// focused component will receive first dibs on any update messages, in
    /// the order of the returned list. If none of the children consume the
    /// message, it will be passed to this component.
    fn focused_children(&mut self) -> Vec<&mut dyn Component> {
        Vec::new()
    }
}

/// Something that can be drawn onto screen as one or more TUI widgets.
///
/// Conceptually this is bascially part of `Component`, but having it separate
/// allows the `Props` associated type. Otherwise, there's no way to make a
/// trait object from `Component` across components with different props.
pub trait Draw {
    /// Props are additional temporary values that a struct may need in order
    /// to render. Useful for passing down state values that are managed by
    /// the parent, to avoid duplicating that state in the child.
    type Props<'a> = () where Self: 'a;

    fn draw<'a>(
        &'a self,
        context: &RenderContext,
        props: Self::Props<'a>,
        frame: &mut Frame,
        chunk: Rect,
    );
}

/// A trigger for state change in the view. Messages are handled by
/// [Component::update_all], and each component is responsible for modifying
/// its own state accordingly. Messages can also trigger other view messages
/// to propagate state changes, as well as side-effect messages to trigger
/// app-wide changes (e.g. launch a request).
///
/// This is conceptually different from [Message] in that view messages never
/// queued, they are handled immediately. Maybe "message" is a misnomer here and
/// we should rename this?
#[derive(Debug)]
pub enum ViewMessage {
    /// Input from the user, which may or may not correspond to a bound action.
    /// Most components just care about the action, but some require raw input
    InputAction {
        event: Event,
        action: Option<Action>,
    },

    // HTTP
    /// User wants to send a new request
    HttpSendRequest,
    /// New HTTP request was spawned
    HttpRequest {
        recipe_id: RequestRecipeId,
    },
    /// An HTTP request succeeded
    HttpResponse {
        record: RequestRecord,
    },
    /// An HTTP request failed
    HttpError {
        recipe_id: RequestRecipeId,
        error: anyhow::Error,
    },
    /// Historical request was loaded from the repository
    HttpLoad {
        record: RequestRecord,
    },

    /// Prompt the user for input
    Prompt(Prompt),

    // Errors
    Error(anyhow::Error),

    // Notifications
    Notify(Notification),
}

/// The result of a component state update operation. This corresponds to a
/// single input [ViewMessage].
#[derive(Debug)]
pub enum UpdateOutcome {
    /// The consuming component updated its state accordingly, and no further
    /// changes are necessary
    Consumed,
    /// The returned message should be passed to the parent component. This can
    /// mean one of two things:
    ///
    /// - The updated component did not handle the message, and it should
    ///   bubble up the tree
    /// - The updated component *did* make changes according to the message,
    ///   and is sending a related message up the tree for ripple-effect
    ///   changes
    ///
    /// This dual meaning is maybe a little janky. There's an argument that
    /// rippled changes should be a separate variant that would cause the
    /// caller to reset back to the bottom of the component tree. There's
    /// no immediate need for that though so I'm keeping it simpler for
    /// now.
    Propagate(ViewMessage),
    /// The component consumed the message, and wants to trigger an app-wide
    /// action in response to it. The action should be queued on the controller
    /// so it can be handled asyncronously.
    SideEffect(Message),
}

/// The root view component
#[derive(Debug)]
pub struct Root {
    // ===== Own State =====
    /// Cached request state. A recipe will appear in this map if two
    /// conditions are met:
    /// - It has at least one *successful* request in history
    /// - It has beed focused by the user during this process
    /// This will be populated on-demand when a user selects a recipe in the
    /// list.
    active_requests: HashMap<RequestRecipeId, RequestState>,
    primary_panes: StatefulSelect<PrimaryPane>,

    // ==== Children =====
    profile_list_pane: ProfileListPane,
    recipe_list_pane: RecipeListPane,
    request_pane: RequestPane,
    response_pane: ResponsePane,
    error_modal: ErrorModal,
    prompt_modal: PromptModal,
    notification_text: Option<NotificationText>,
}

impl Root {
    pub fn new(collection: &RequestCollection) -> Self {
        Self {
            // State
            // TODO populate the initially selected request on startup
            active_requests: HashMap::new(),
            primary_panes: StatefulSelect::new(),

            // Children
            profile_list_pane: ProfileListPane::new(
                collection.profiles.to_owned(),
            ),
            recipe_list_pane: RecipeListPane::new(
                collection.recipes.to_owned(),
            ),
            request_pane: RequestPane::new(),
            response_pane: ResponsePane::new(),
            error_modal: ErrorModal::new(),
            prompt_modal: PromptModal::new(),
            notification_text: None,
        }
    }

    /// Get the request state to be displayed
    fn active_request(&self) -> Option<&RequestState> {
        let recipe = self.recipe_list_pane.selected_recipe()?;
        self.active_requests.get(&recipe.id)
    }

    /// Mark the current HTTP request as failed
    fn fail_request(
        &mut self,
        recipe_id: RequestRecipeId,
        error: anyhow::Error,
    ) {
        // TODO this is a disaster, we need to link requests with errors using
        // IDs.
        match self.active_requests.entry(recipe_id) {
            hash_map::Entry::Occupied(mut entry)
                if entry.get().is_loading() =>
            {
                entry.insert(RequestState::Error {
                    error,
                    start_time: entry.get().start_time(),
                    end_time: Utc::now(),
                });
            }
            other => {
                // We don't expect anything but a loading state to be there,
                // but don't overwrite anything else either
                error!(
                    request = ?other,
                    "Cannot store error for request with non-loading state",
                )
            }
        }
    }

    /// Store an HTTP record loaded from the repository, if newer than the
    /// current request
    fn load_request(&mut self, record: RequestRecord) {
        // Make sure we don't overwrite any request that was made
        // more recently than this
        match self.active_requests.entry(record.request.recipe_id.clone()) {
            hash_map::Entry::Occupied(mut entry) => {
                if record.start_time > entry.get().start_time() {
                    // Do *not* create the response state eagerly, because it
                    // requires prettification
                    entry.insert(RequestState::response(record));
                }
            }
            hash_map::Entry::Vacant(entry) => {
                entry.insert(RequestState::response(record));
            }
        };
    }
}

impl Component for Root {
    fn update(&mut self, message: ViewMessage) -> UpdateOutcome {
        match message {
            // HTTP state messages
            ViewMessage::HttpSendRequest => {
                if let Some(recipe) = self.recipe_list_pane.selected_recipe() {
                    return UpdateOutcome::SideEffect(
                        Message::HttpSendRequest {
                            // Reach into the children to grab state (ugly!)
                            recipe_id: recipe.id.clone(),
                            profile_id: self
                                .profile_list_pane
                                .selected_profile()
                                .map(|profile| profile.id.clone()),
                        },
                    );
                }
            }
            ViewMessage::HttpRequest { recipe_id } => {
                self.active_requests
                    .insert(recipe_id, RequestState::loading());
            }
            ViewMessage::HttpResponse { record } => {
                self.active_requests.insert(
                    record.request.recipe_id.clone(),
                    RequestState::response(record),
                );
            }
            ViewMessage::HttpError { recipe_id, error } => {
                self.fail_request(recipe_id, error)
            }
            ViewMessage::HttpLoad { record } => self.load_request(record),

            // Other state messages
            ViewMessage::Notify(notification) => {
                self.notification_text =
                    Some(NotificationText::new(notification));
            }

            // Input messages
            ViewMessage::InputAction {
                action: Some(Action::Quit),
                ..
            } => return UpdateOutcome::SideEffect(Message::Quit),
            ViewMessage::InputAction {
                action: Some(Action::ReloadCollection),
                ..
            } => {
                return UpdateOutcome::SideEffect(
                    Message::CollectionStartReload,
                )
            }
            ViewMessage::InputAction {
                action: Some(Action::FocusPrevious),
                ..
            } => self.primary_panes.previous(),
            ViewMessage::InputAction {
                action: Some(Action::FocusNext),
                ..
            } => self.primary_panes.next(),

            // Everything else gets ate
            _ => {}
        }
        UpdateOutcome::Consumed
    }

    fn focused_children(&mut self) -> Vec<&mut dyn Component> {
        vec![
            &mut self.error_modal,
            &mut self.prompt_modal,
            match self.primary_panes.selected() {
                PrimaryPane::ProfileList => {
                    &mut self.profile_list_pane as &mut dyn Component
                }
                PrimaryPane::RecipeList => &mut self.recipe_list_pane,
                PrimaryPane::Request => &mut self.request_pane,
                PrimaryPane::Response => &mut self.response_pane,
            },
        ]
    }
}

impl Draw for Root {
    fn draw(
        &self,
        context: &RenderContext,
        _: (),
        frame: &mut Frame,
        chunk: Rect,
    ) {
        // Create layout
        let [main_chunk, footer_chunk] = layout(
            chunk,
            Direction::Vertical,
            [Constraint::Min(0), Constraint::Length(1)],
        );
        // Split the main pane horizontally
        let [left_chunk, right_chunk] = layout(
            main_chunk,
            Direction::Horizontal,
            [Constraint::Max(40), Constraint::Percentage(50)],
        );

        // Split left column vertically
        let [profiles_chunk, recipes_chunk] = layout(
            left_chunk,
            Direction::Vertical,
            [Constraint::Max(16), Constraint::Min(0)],
        );

        // Split right column vertically
        let [request_chunk, response_chunk] = layout(
            right_chunk,
            Direction::Vertical,
            [Constraint::Percentage(50), Constraint::Percentage(50)],
        );

        // Primary panes
        let panes = &self.primary_panes;
        self.profile_list_pane.draw(
            context,
            ListPaneProps {
                is_selected: panes.is_selected(&PrimaryPane::ProfileList),
            },
            frame,
            profiles_chunk,
        );
        self.recipe_list_pane.draw(
            context,
            ListPaneProps {
                is_selected: panes.is_selected(&PrimaryPane::RecipeList),
            },
            frame,
            recipes_chunk,
        );
        self.request_pane.draw(
            context,
            RequestPaneProps {
                is_selected: panes.is_selected(&PrimaryPane::Request),
                selected_recipe: self.recipe_list_pane.selected_recipe(),
            },
            frame,
            request_chunk,
        );
        self.response_pane.draw(
            context,
            ResponsePaneProps {
                is_selected: panes.is_selected(&PrimaryPane::Response),
                active_request: self.active_request(),
            },
            frame,
            response_chunk,
        );

        // Footer
        match &self.notification_text {
            Some(notification_text) => {
                notification_text.draw(context, (), frame, footer_chunk)
            }
            None => HelpText.draw(context, (), frame, footer_chunk),
        }

        // Render modals last so they go on top
        self.prompt_modal.draw(context, (), frame, frame.size());
        self.error_modal.draw(context, (), frame, frame.size());
    }
}
