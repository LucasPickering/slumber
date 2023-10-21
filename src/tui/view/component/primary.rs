//! Primary pane components

use crate::{
    config::{Profile, RequestRecipe},
    tui::{
        input::Action,
        message::Message,
        view::{
            component::{Component, Draw, UpdateOutcome, ViewMessage},
            state::{
                PrimaryPane, RequestState, RequestTab, ResponseTab,
                StatefulList, StatefulSelect,
            },
            util::{layout, BlockBrick, ListBrick, TabBrick, ToTui},
            Frame, RenderContext,
        },
    },
};
use ratatui::{
    prelude::{Alignment, Constraint, Direction, Rect},
    text::{Line, Text},
    widgets::{Paragraph, Wrap},
};

#[derive(Debug)]
pub struct ProfileListPane {
    profiles: StatefulList<Profile>,
}

impl ProfileListPane {
    pub fn new(profiles: Vec<Profile>) -> Self {
        Self {
            profiles: StatefulList::with_items(profiles),
        }
    }

    /// Which profile in the list is selected? `None` iff the list is empty.
    /// Exposing inner state is hacky but it's an easy shortcut
    pub fn selected_profile(&self) -> Option<&Profile> {
        self.profiles.selected()
    }
}

impl Component for ProfileListPane {
    fn update(&mut self, message: ViewMessage) -> UpdateOutcome {
        match message {
            ViewMessage::InputAction {
                action: Some(Action::Up),
                ..
            } => {
                self.profiles.previous();
                UpdateOutcome::Consumed
            }
            ViewMessage::InputAction {
                action: Some(Action::Down),
                ..
            } => {
                self.profiles.next();
                UpdateOutcome::Consumed
            }
            _ => UpdateOutcome::Propagate(message),
        }
    }
}

impl Draw for ProfileListPane {
    type Props<'a> = ListPaneProps where Self: 'a;

    fn draw(
        &self,
        context: &RenderContext,
        props: Self::Props<'_>,
        frame: &mut Frame,
        chunk: Rect,
    ) {
        let list = ListBrick {
            block: BlockBrick {
                title: PrimaryPane::ProfileList.to_string(),
                is_focused: props.is_selected,
            },
            list: &self.profiles,
        };
        frame.render_stateful_widget(
            list.to_tui(context),
            chunk,
            &mut self.profiles.state_mut(),
        )
    }
}

#[derive(Debug)]
pub struct RecipeListPane {
    recipes: StatefulList<RequestRecipe>,
}

impl RecipeListPane {
    pub fn new(recipes: Vec<RequestRecipe>) -> Self {
        Self {
            recipes: StatefulList::with_items(recipes),
        }
    }

    /// Which recipe in the list is selected? `None` iff the list is empty.
    /// Exposing inner state is hacky but it's an easy shortcut
    pub fn selected_recipe(&self) -> Option<&RequestRecipe> {
        self.recipes.selected()
    }
}

impl Component for RecipeListPane {
    fn update(&mut self, message: ViewMessage) -> UpdateOutcome {
        /// Helper to load a request from the repo whenever we select a new
        /// recipe
        fn load_from_repo(pane: &RecipeListPane) -> UpdateOutcome {
            match pane.recipes.selected() {
                Some(recipe) => {
                    UpdateOutcome::SideEffect(Message::RepositoryStartLoad {
                        recipe_id: recipe.id.clone(),
                    })
                }
                None => UpdateOutcome::Consumed,
            }
        }

        match message {
            ViewMessage::InputAction {
                action: Some(Action::Interact),
                ..
            } => {
                // Parent has to be responsible for sending the request because
                // it also needs access to the profile list state
                UpdateOutcome::Propagate(ViewMessage::HttpSendRequest)
            }
            ViewMessage::InputAction {
                action: Some(Action::Up),
                ..
            } => {
                self.recipes.previous();
                load_from_repo(self)
            }
            ViewMessage::InputAction {
                action: Some(Action::Down),
                ..
            } => {
                self.recipes.next();
                load_from_repo(self)
            }
            _ => UpdateOutcome::Propagate(message),
        }
    }
}

impl Draw for RecipeListPane {
    type Props<'a> = ListPaneProps where Self: 'a;

    fn draw(
        &self,
        context: &RenderContext,
        props: Self::Props<'_>,
        frame: &mut Frame,
        chunk: Rect,
    ) {
        let pane_kind = PrimaryPane::RecipeList;
        let list = ListBrick {
            block: BlockBrick {
                title: pane_kind.to_string(),
                is_focused: props.is_selected,
            },
            list: &self.recipes,
        };
        frame.render_stateful_widget(
            list.to_tui(context),
            chunk,
            &mut self.recipes.state_mut(),
        )
    }
}

#[derive(Debug)]
pub struct RequestPane {
    tabs: StatefulSelect<RequestTab>,
}

impl RequestPane {
    pub fn new() -> Self {
        Self {
            tabs: StatefulSelect::default(),
        }
    }
}

impl Component for RequestPane {
    fn update(&mut self, message: ViewMessage) -> UpdateOutcome {
        match message {
            ViewMessage::InputAction {
                action: Some(Action::Left),
                ..
            } => {
                self.tabs.previous();
                UpdateOutcome::Consumed
            }
            ViewMessage::InputAction {
                action: Some(Action::Right),
                ..
            } => {
                self.tabs.next();
                UpdateOutcome::Consumed
            }
            _ => UpdateOutcome::Propagate(message),
        }
    }
}

impl Draw for RequestPane {
    type Props<'a> = RequestPaneProps<'a>;

    fn draw(
        &self,
        context: &RenderContext,
        props: Self::Props<'_>,
        frame: &mut Frame,
        chunk: Rect,
    ) {
        // Render outermost block
        let pane_kind = PrimaryPane::Request;
        let block = BlockBrick {
            title: pane_kind.to_string(),
            is_focused: props.is_selected,
        };
        let block = block.to_tui(context);
        let inner_chunk = block.inner(chunk);
        frame.render_widget(block, chunk);

        // Render request contents
        if let Some(recipe) = props.selected_recipe {
            let [url_chunk, tabs_chunk, content_chunk] = layout(
                inner_chunk,
                Direction::Vertical,
                [
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Min(0),
                ],
            );

            // URL
            frame.render_widget(
                Paragraph::new(format!("{} {}", recipe.method, recipe.url)),
                url_chunk,
            );

            // Navigation tabs
            let tabs = TabBrick { tabs: &self.tabs };
            frame.render_widget(tabs.to_tui(context), tabs_chunk);

            // Request content
            let text: Text = match self.tabs.selected() {
                RequestTab::Body => recipe
                    .body
                    .as_ref()
                    .map(|b| b.to_string())
                    .unwrap_or_default()
                    .into(),
                RequestTab::Query => recipe.query.to_tui(context),
                RequestTab::Headers => recipe.headers.to_tui(context),
            };
            frame.render_widget(Paragraph::new(text), content_chunk);
        }
    }
}

#[derive(Debug)]
pub struct ResponsePane {
    tabs: StatefulSelect<ResponseTab>,
}

impl ResponsePane {
    pub fn new() -> Self {
        Self {
            tabs: StatefulSelect::default(),
        }
    }
}

impl Component for ResponsePane {
    fn update(&mut self, message: ViewMessage) -> UpdateOutcome {
        match message {
            ViewMessage::InputAction {
                action: Some(Action::Left),
                ..
            } => {
                self.tabs.previous();
                UpdateOutcome::Consumed
            }
            ViewMessage::InputAction {
                action: Some(Action::Right),
                ..
            } => {
                self.tabs.next();
                UpdateOutcome::Consumed
            }
            _ => UpdateOutcome::Propagate(message),
        }
    }
}

impl Draw for ResponsePane {
    type Props<'a> = ResponsePaneProps<'a>;

    fn draw(
        &self,
        context: &RenderContext,
        props: Self::Props<'_>,
        frame: &mut Frame,
        chunk: Rect,
    ) {
        // Render outermost block
        let pane_kind = PrimaryPane::Response;
        let block = BlockBrick {
            title: pane_kind.to_string(),
            is_focused: props.is_selected,
        };
        let block = block.to_tui(context);
        let inner_chunk = block.inner(chunk);
        frame.render_widget(block, chunk);

        // Don't render anything else unless we have a request state
        if let Some(request_state) = props.active_request {
            let [header_chunk, content_chunk] = layout(
                inner_chunk,
                Direction::Vertical,
                [Constraint::Length(1), Constraint::Min(0)],
            );
            let [header_left_chunk, header_right_chunk] = layout(
                header_chunk,
                Direction::Horizontal,
                [Constraint::Length(20), Constraint::Min(0)],
            );

            // Time-related data. start_time and duration should always be
            // defined together
            if let (Some(start_time), Some(duration)) =
                (request_state.start_time(), request_state.duration())
            {
                frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        start_time.to_tui(context),
                        " / ".into(),
                        duration.to_tui(context),
                    ]))
                    .alignment(Alignment::Right),
                    header_right_chunk,
                );
            }

            match &request_state {
                RequestState::Building { .. } => {
                    frame.render_widget(
                        Paragraph::new("Initializing request..."),
                        header_left_chunk,
                    );
                }

                // :(
                RequestState::BuildError { error } => {
                    frame.render_widget(
                        Paragraph::new(error.to_tui(context))
                            .wrap(Wrap::default()),
                        content_chunk,
                    );
                }

                RequestState::Loading { .. } => {
                    frame.render_widget(
                        Paragraph::new("Loading..."),
                        header_left_chunk,
                    );
                }

                RequestState::Response {
                    record,
                    pretty_body,
                } => {
                    let response = &record.response;
                    // Status code
                    frame.render_widget(
                        Paragraph::new(response.status.to_string()),
                        header_left_chunk,
                    );

                    // Split the main chunk again to allow tabs
                    let [tabs_chunk, content_chunk] = layout(
                        content_chunk,
                        Direction::Vertical,
                        [Constraint::Length(1), Constraint::Min(0)],
                    );

                    // Navigation tabs
                    let tabs = TabBrick { tabs: &self.tabs };
                    frame.render_widget(tabs.to_tui(context), tabs_chunk);

                    // Main content for the response
                    let tab_text = match self.tabs.selected() {
                        // Render the pretty body if it's available, otherwise
                        // fall back to the regular one
                        ResponseTab::Body => pretty_body
                            .as_deref()
                            .unwrap_or(response.body.text())
                            .into(),
                        ResponseTab::Headers => {
                            response.headers.to_tui(context)
                        }
                    };
                    frame
                        .render_widget(Paragraph::new(tab_text), content_chunk);
                }

                // Sadge
                RequestState::RequestError { error, .. } => {
                    frame.render_widget(
                        Paragraph::new(error.to_tui(context))
                            .wrap(Wrap::default()),
                        content_chunk,
                    );
                }
            }
        }
    }
}

pub struct ListPaneProps {
    pub is_selected: bool,
}

pub struct RequestPaneProps<'a> {
    pub is_selected: bool,
    pub selected_recipe: Option<&'a RequestRecipe>,
}

pub struct ResponsePaneProps<'a> {
    pub is_selected: bool,
    pub active_request: Option<&'a RequestState>,
}
