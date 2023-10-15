//! The building blocks of the view

mod primary;

use crate::{
    config::{RequestCollection, RequestRecipeId},
    http::RequestRecord,
    tui::{
        input::Action,
        message::Message,
        view::{
            component::primary::{
                ListPaneProps, ProfileListPane, RecipeListPane, RequestPane,
                RequestPaneProps, ResponsePane, ResponsePaneProps,
            },
            state::{Notification, PrimaryPane, RequestState, StatefulSelect},
            util::{centered_rect, layout, ButtonBrick, ToTui},
            Frame, RenderContext,
        },
    },
};
use chrono::Utc;
use itertools::Itertools;
use ratatui::{
    prelude::{Alignment, Constraint, Direction, Rect},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use std::{
    collections::{hash_map, HashMap},
    fmt::Debug,
};
use tracing::error;

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
    fn update_all(&mut self, message: ViewMessage) -> UpdateOutcome {
        // If we have a child, send them the message. If not, eat it ourselves
        match self.focused_child() {
            Some(child) => {
                let outcome = child.update_all(message);
                if let UpdateOutcome::Propagate(message) = outcome {
                    self.update(message)
                } else {
                    outcome
                }
            }
            None => self.update(message),
        }
    }

    /// Which, if any, of this component's children currently has focus? The
    /// focused component will receive first dibs on any update messages.
    fn focused_child(&mut self) -> Option<&mut dyn Component> {
        None
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
    /// Input from the user
    Input(Action),

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

    // Errors
    Error(anyhow::Error),
    ClearError,

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
    error_popup: Option<ErrorPopup>,
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
                collection.profiles.clone(),
            ),
            recipe_list_pane: RecipeListPane::new(collection.requests.clone()),
            request_pane: RequestPane::new(),
            response_pane: ResponsePane::new(),
            error_popup: None,
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
            ViewMessage::Error(error) => {
                self.error_popup = Some(ErrorPopup::new(error));
            }
            ViewMessage::ClearError => {
                self.error_popup = None;
            }
            ViewMessage::Notify(notification) => {
                self.notification_text =
                    Some(NotificationText::new(notification));
            }

            // Input messages
            ViewMessage::Input(Action::Quit) => {
                return UpdateOutcome::SideEffect(Message::Quit);
            }
            ViewMessage::Input(Action::ReloadCollection) => {
                return UpdateOutcome::SideEffect(
                    Message::CollectionStartReload,
                );
            }
            ViewMessage::Input(Action::FocusPrevious) => {
                self.primary_panes.previous();
            }
            ViewMessage::Input(Action::FocusNext) => {
                self.primary_panes.next();
            }

            _ => return UpdateOutcome::Propagate(message),
        }
        UpdateOutcome::Consumed
    }

    fn focused_child(&mut self) -> Option<&mut dyn Component> {
        // Popups take priority, then fall back to the selected pane
        let child = self
            .error_popup
            .as_mut()
            .map(|v| v as &mut dyn Component)
            .unwrap_or(match self.primary_panes.selected() {
                PrimaryPane::ProfileList => &mut self.profile_list_pane,
                PrimaryPane::RecipeList => &mut self.recipe_list_pane,
                PrimaryPane::Request => &mut self.request_pane,
                PrimaryPane::Response => &mut self.response_pane,
            });
        Some(child)
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

        // Render popups last so they go on top
        if let Some(error_popup) = &self.error_popup {
            error_popup.draw(context, (), frame, frame.size());
        }
    }
}

#[derive(Debug)]
pub struct ErrorPopup {
    error: anyhow::Error,
}

impl ErrorPopup {
    pub fn new(error: anyhow::Error) -> Self {
        Self { error }
    }
}

impl Component for ErrorPopup {
    fn update(&mut self, message: ViewMessage) -> UpdateOutcome {
        match message {
            ViewMessage::Input(Action::Interact | Action::Close) => {
                UpdateOutcome::Propagate(ViewMessage::ClearError)
            }
            _ => UpdateOutcome::Propagate(message),
        }
    }
}

impl Draw for ErrorPopup {
    fn draw(
        &self,
        context: &RenderContext,
        _: (),
        frame: &mut Frame,
        chunk: Rect,
    ) {
        // Grab a spot in the middle of the screen
        let chunk = centered_rect(60, 20, chunk);
        let block = Block::default().title("Error").borders(Borders::ALL);
        let [content_chunk, footer_chunk] = layout(
            block.inner(chunk),
            Direction::Vertical,
            [Constraint::Min(0), Constraint::Length(1)],
        );

        frame.render_widget(Clear, chunk);
        frame.render_widget(block, chunk);
        frame.render_widget(
            Paragraph::new(self.error.to_tui(context)).wrap(Wrap::default()),
            content_chunk,
        );

        // Prompt the user to get out of here
        frame.render_widget(
            Paragraph::new(
                ButtonBrick {
                    text: "OK",
                    is_highlighted: true,
                }
                .to_tui(context),
            )
            .alignment(Alignment::Center),
            footer_chunk,
        );
    }
}

#[derive(Debug)]
pub struct HelpText;

impl Draw for HelpText {
    fn draw(
        &self,
        context: &RenderContext,
        _: (),
        frame: &mut Frame,
        chunk: Rect,
    ) {
        let actions = [
            Action::Quit,
            Action::ReloadCollection,
            Action::FocusNext,
            Action::FocusPrevious,
            Action::Close,
        ];
        let text = actions
            .into_iter()
            .map(|action| {
                context
                    .input_engine
                    .binding(action)
                    .as_ref()
                    .map(ToString::to_string)
                    // This *shouldn't* happen, all actions get a binding
                    .unwrap_or_else(|| "???".into())
            })
            .join(" / ");
        frame.render_widget(Paragraph::new(text), chunk);
    }
}

#[derive(Debug)]
pub struct NotificationText {
    notification: Notification,
}

impl NotificationText {
    pub fn new(notification: Notification) -> Self {
        Self { notification }
    }
}

impl Draw for NotificationText {
    fn draw(
        &self,
        context: &RenderContext,
        _: (),
        frame: &mut Frame,
        chunk: Rect,
    ) {
        frame.render_widget(
            Paragraph::new(self.notification.to_tui(context)),
            chunk,
        );
    }
}
