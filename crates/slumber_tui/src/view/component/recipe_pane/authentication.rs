use crate::{
    context::TuiContext,
    util::ResultReported,
    view::{
        common::{
            table::Table, template_preview::TemplatePreview, text_box::TextBox,
        },
        component::{misc::TextBoxModal, Component},
        draw::{Draw, DrawMetadata, Generate},
        event::{Event, EventHandler, Update},
        state::fixed_select::FixedSelectState,
        ViewContext,
    },
};
use derive_more::derive::Display;
use ratatui::{
    layout::Layout,
    prelude::Constraint,
    text::{Line, Span},
    widgets::TableState,
    Frame,
};
use slumber_config::Action;
use slumber_core::{collection::Authentication, template::Template};
use strum::{EnumCount, EnumIter};

/// Display authentication settings for a recipe
#[derive(Debug)]
pub struct AuthenticationDisplay {
    state: State,
    overridden: bool,
}

impl AuthenticationDisplay {
    pub fn new(authentication: Authentication) -> Self {
        Self {
            state: State::new(authentication),
            overridden: false,
        }
    }

    /// If the user has applied a temporary edit to the auth settings, get the
    /// override value. Return `None` to use the recipe's stock auth.
    pub fn override_value(&self) -> Option<Authentication> {
        if self.overridden {
            Some(self.state.authentication())
        } else {
            None
        }
    }

    /// Open a modal to let the user edit temporary override values
    fn open_edit_modal(&self) {
        let (label, value) = match &self.state {
            State::Basic {
                username,
                password,
                selected_field,
                ..
            } => match selected_field.data().selected() {
                BasicFields::Username => ("username", username.display()),
                BasicFields::Password => (
                    "password",
                    password
                        .as_ref()
                        .map(Template::display)
                        .unwrap_or_default(),
                ),
            },
            State::Bearer { token, .. } => ("bearer token", token.display()),
        };
        ViewContext::open_modal(TextBoxModal::new(
            format!("Edit {label}"),
            TextBox::default()
                .default_value(value.into_owned())
                .validator(|value| value.parse::<Template>().is_ok()),
            |value| {
                // Defer the state update into an event, so it can get &mut
                ViewContext::push_event(Event::new_local(SaveOverride(value)))
            },
        ))
    }

    /// Override the value template for whichever field is selected, and
    /// recompute the template preview
    fn set_override(&mut self, value: &str) {
        let Some(template) = value
            .parse::<Template>()
            // The template *should* always parse because the text box has a
            // validator, but this is just a safety check
            .reported(&ViewContext::messages_tx())
        else {
            return;
        };
        let preview = TemplatePreview::new(template.clone(), None);
        self.overridden = true;
        match &mut self.state {
            State::Basic {
                username,
                username_preview,
                password,
                password_preview,
                selected_field,
            } => match selected_field.data().selected() {
                BasicFields::Username => {
                    *username = template;
                    *username_preview = preview;
                }
                BasicFields::Password => {
                    // Note: if the password was unset before, we're going to
                    // change it to empty string. The behavior between the two
                    // is the exact same so it's fine
                    *password = Some(template);
                    *password_preview = Some(preview);
                }
            },
            State::Bearer {
                token,
                token_preview,
            } => {
                *token = template;
                *token_preview = preview;
            }
        }
    }
}

impl EventHandler for AuthenticationDisplay {
    fn update(&mut self, event: Event) -> Update {
        if let Some(Action::Edit) = event.action() {
            self.open_edit_modal();
        } else if let Some(SaveOverride(value)) = event.local() {
            self.set_override(value);
        } else {
            return Update::Propagate(event);
        }
        Update::Consumed
    }

    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        match &mut self.state {
            State::Basic { selected_field, .. } => {
                vec![selected_field.as_child()]
            }
            State::Bearer { .. } => vec![],
        }
    }
}

impl Draw for AuthenticationDisplay {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        let styles = &TuiContext::get().styles;

        let [label_area, content_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)])
                .areas(metadata.area());
        let label = match &self.state {
            State::Basic {
                username_preview,
                password_preview,
                selected_field,
                ..
            } => {
                let table = Table {
                    rows: vec![
                        ["Username:".into(), username_preview.generate()],
                        [
                            "Password:".into(),
                            password_preview
                                .as_ref()
                                .map(Generate::generate)
                                // Missing password behaves the same as an empty
                                // string, so we can just show empty here
                                .unwrap_or_default(),
                        ],
                    ],
                    column_widths: &[Constraint::Length(9), Constraint::Min(0)],
                    ..Default::default()
                };
                selected_field.draw(
                    frame,
                    table.generate(),
                    content_area,
                    true,
                );
                "Basic"
            }
            State::Bearer { token_preview, .. } => {
                frame.render_widget(token_preview.generate(), content_area);
                "Bearer"
            }
        };

        let mut title: Line = Span::styled(
            format!("Authentication Type: {label}"),
            styles.text.title,
        )
        .into();
        if self.overridden {
            title.push_span(Span::styled(" (edited)", styles.text.hint));
        }
        frame.render_widget(title, label_area);
    }
}

/// Internal component state, specific to the authentication type
#[derive(Debug)]
enum State {
    Basic {
        username: Template,
        username_preview: TemplatePreview,
        password: Option<Template>,
        password_preview: Option<TemplatePreview>,
        /// Track which field is selected, for editability
        selected_field: Component<FixedSelectState<BasicFields, TableState>>,
    },
    Bearer {
        token: Template,
        token_preview: TemplatePreview,
    },
}

impl State {
    fn new(authentication: Authentication) -> Self {
        let create_preview =
            |template: &Template| TemplatePreview::new(template.clone(), None);
        match authentication {
            Authentication::Basic { username, password } => {
                let username_preview = create_preview(&username);
                let password_preview = password.as_ref().map(create_preview);
                Self::Basic {
                    username,
                    username_preview,
                    password,
                    password_preview,
                    selected_field: Default::default(),
                }
            }
            Authentication::Bearer(token) => {
                let token_preview = create_preview(&token);
                Self::Bearer {
                    token,
                    token_preview,
                }
            }
        }
    }

    /// Get the current authentication value. This will clone the templates
    fn authentication(&self) -> Authentication {
        match self {
            State::Basic {
                username, password, ..
            } => Authentication::Basic {
                username: username.clone(),
                password: password.clone(),
            },
            State::Bearer { token, .. } => {
                Authentication::Bearer(token.clone())
            }
        }
    }
}

/// Fields in a basic auth form
#[derive(
    Copy, Clone, Debug, Default, Display, EnumCount, EnumIter, PartialEq,
)]
enum BasicFields {
    #[default]
    Username,
    Password,
}

/// Local event to save a user's override value(s). Triggered from the edit
/// modal. These will be raw string values, consumer has to parse them to
/// templates.
#[derive(Debug)]
struct SaveOverride(String);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{harness, terminal, TestHarness, TestTerminal},
        view::test_util::{TestComponent, WithModalQueue},
    };
    use crossterm::event::KeyCode;
    use rstest::rstest;

    #[rstest]
    fn test_edit_basic(_harness: TestHarness, terminal: TestTerminal) {
        let authentication = Authentication::Basic {
            username: "user1".into(),
            password: Some("hunter2".into()),
        };
        let mut component = TestComponent::new(
            &terminal,
            WithModalQueue::new(AuthenticationDisplay::new(authentication)),
            (),
        );

        // Check initial state
        assert_eq!(component.data().inner().override_value(), None);

        // Edit username
        component.send_key(KeyCode::Char('e')).assert_empty();
        component.send_text("!!!").assert_empty();
        component.send_key(KeyCode::Enter).assert_empty();
        assert_eq!(
            component.data().inner().override_value(),
            Some(Authentication::Basic {
                username: "user1!!!".into(),
                password: Some("hunter2".into())
            })
        );

        // Edit password
        component.send_key(KeyCode::Down).assert_empty();
        component.send_key(KeyCode::Char('e')).assert_empty();
        component.send_text("???").assert_empty();
        component.send_key(KeyCode::Enter).assert_empty();
        assert_eq!(
            component.data().inner().override_value(),
            Some(Authentication::Basic {
                username: "user1!!!".into(),
                password: Some("hunter2???".into())
            })
        );
    }

    #[rstest]
    fn test_edit_basic_empty_password(
        _harness: TestHarness,
        terminal: TestTerminal,
    ) {
        let authentication = Authentication::Basic {
            username: "user1".into(),
            password: None,
        };
        let mut component = TestComponent::new(
            &terminal,
            WithModalQueue::new(AuthenticationDisplay::new(authentication)),
            (),
        );

        // Edit password
        component.send_key(KeyCode::Down).assert_empty();
        component.send_key(KeyCode::Char('e')).assert_empty();
        component.send_key(KeyCode::Enter).assert_empty();
        assert_eq!(
            component.data().inner().override_value(),
            Some(Authentication::Basic {
                username: "user1".into(),
                // None gets replaced by empty string. They're functionally
                // equivalent because the encoding maps to {username}:{password}
                password: Some("".into())
            })
        );
    }

    #[rstest]
    fn test_edit_bearer(_harness: TestHarness, terminal: TestTerminal) {
        let authentication = Authentication::Bearer("i am a token".into());
        let mut component = TestComponent::new(
            &terminal,
            WithModalQueue::new(AuthenticationDisplay::new(authentication)),
            (),
        );

        // Check initial state
        assert_eq!(component.data().inner().override_value(), None);

        // Edit token
        component.send_key(KeyCode::Char('e')).assert_empty();
        component.send_text("!!!").assert_empty();
        component.send_key(KeyCode::Enter).assert_empty();
        assert_eq!(
            component.data().inner().override_value(),
            Some(Authentication::Bearer("i am a token!!!".into()))
        );
    }
}
