use crate::{
    context::TuiContext,
    util::ResultReported,
    view::{
        common::{table::Table, text_box::TextBox},
        component::{
            misc::TextBoxModal,
            recipe_pane::persistence::{RecipeOverrideKey, RecipeTemplate},
            Component,
        },
        context::UpdateContext,
        draw::{Draw, DrawMetadata, Generate},
        event::{
            Child, Emitter, EmitterHandle, EmitterId, Event, EventHandler,
            Update,
        },
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
use slumber_core::{
    collection::{Authentication, RecipeId},
    template::Template,
};
use strum::{EnumCount, EnumIter};

/// Display authentication settings for a recipe
#[derive(Debug)]
pub struct AuthenticationDisplay {
    emitter_id: EmitterId,
    state: State,
}

impl AuthenticationDisplay {
    pub fn new(recipe_id: RecipeId, authentication: Authentication) -> Self {
        let state = match authentication {
            Authentication::Basic { username, password } => {
                let username = RecipeTemplate::new(
                    RecipeOverrideKey::auth_basic_username(recipe_id.clone()),
                    username,
                    None,
                );
                let password = RecipeTemplate::new(
                    RecipeOverrideKey::auth_basic_password(recipe_id.clone()),
                    // See note on this field def for why we unwrap
                    password.unwrap_or_default(),
                    None,
                );
                State::Basic {
                    username,
                    password,
                    selected_field: Default::default(),
                }
            }
            Authentication::Bearer(token) => State::Bearer {
                token: RecipeTemplate::new(
                    RecipeOverrideKey::auth_bearer_token(recipe_id.clone()),
                    token,
                    None,
                ),
            },
        };
        Self {
            emitter_id: EmitterId::new(),
            state,
        }
    }

    /// If the user has applied a temporary edit to the auth settings, get the
    /// override value. Return `None` to use the recipe's stock auth.
    pub fn override_value(&self) -> Option<Authentication> {
        if self.state.is_overridden() {
            Some(match &self.state {
                State::Basic {
                    username, password, ..
                } => Authentication::Basic {
                    username: username.template().clone(),
                    // See note on field def for why we always use Some
                    password: Some(password.template().clone()),
                },
                State::Bearer { token, .. } => {
                    Authentication::Bearer(token.template().clone())
                }
            })
        } else {
            None
        }
    }
}

impl EventHandler for AuthenticationDisplay {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Update {
        let action = event.action();
        if let Some(Action::Edit) = action {
            self.state.open_edit_modal(self.handle());
        } else if let Some(Action::Reset) = action {
            self.state.reset_override();
        } else if let Some(SaveAuthenticationOverride(value)) =
            self.emitted(&event)
        {
            self.state.set_override(value);
        } else {
            return Update::Propagate(event);
        }
        Update::Consumed
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        match &mut self.state {
            State::Basic { selected_field, .. } => {
                vec![selected_field.to_child_mut()]
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
                username,
                password,
                selected_field,
            } => {
                let table = Table {
                    rows: vec![
                        ["Username:".into(), username.preview().generate()],
                        ["Password:".into(), password.preview().generate()],
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
            State::Bearer { token } => {
                frame.render_widget(token.preview().generate(), content_area);
                "Bearer"
            }
        };

        let mut title: Line = Span::styled(
            format!("Authentication Type: {label}"),
            styles.text.title,
        )
        .into();
        if self.state.is_overridden() {
            title.push_span(Span::styled(" (edited)", styles.text.hint));
        }
        frame.render_widget(title, label_area);
    }
}

/// Emit events to ourselves for override editing
impl Emitter for AuthenticationDisplay {
    type Emitted = SaveAuthenticationOverride;

    fn id(&self) -> EmitterId {
        self.emitter_id
    }
}

/// Local event to save a user's override value(s). Triggered from the edit
/// modal. These will be raw string values, consumer has to parse them to
/// templates.
#[derive(Debug)]
pub struct SaveAuthenticationOverride(String);

/// Private to hide enum variants
#[derive(Debug)]
enum State {
    Basic {
        username: RecipeTemplate,
        /// This field is optional in the actual recipe, but it's a lot easier
        /// if we just replace `None` with an empty template. This allows the
        /// user to edit it and makes rendering easier. It's functionally
        /// equivalent when building the request.
        password: RecipeTemplate,
        /// Track which field is selected, for editability
        selected_field: Component<FixedSelectState<BasicFields, TableState>>,
    },
    Bearer {
        token: RecipeTemplate,
    },
}

impl State {
    /// Have *any* fields been overridden?
    fn is_overridden(&self) -> bool {
        match self {
            Self::Basic {
                username, password, ..
            } => username.is_overridden() || password.is_overridden(),
            Self::Bearer { token } => token.is_overridden(),
        }
    }

    /// Open a modal to let the user edit temporary override values
    fn open_edit_modal(
        &self,
        emitter: EmitterHandle<SaveAuthenticationOverride>,
    ) {
        let (label, value) = match &self {
            Self::Basic {
                username,
                password,
                selected_field,
                ..
            } => match selected_field.data().selected() {
                BasicFields::Username => {
                    ("username", username.template().display())
                }
                BasicFields::Password => {
                    ("password", password.template().display())
                }
            },
            Self::Bearer { token, .. } => {
                ("bearer token", token.template().display())
            }
        };
        ViewContext::open_modal(TextBoxModal::new(
            format!("Edit {label}"),
            TextBox::default()
                .default_value(value.into_owned())
                .validator(|value| value.parse::<Template>().is_ok()),
            move |value| {
                // Defer the state update into an event, so it can get &mut
                emitter.emit(SaveAuthenticationOverride(value))
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
        match self {
            Self::Basic {
                username,
                password,
                selected_field,
            } => match selected_field.data().selected() {
                BasicFields::Username => {
                    username.set_override(template);
                }
                BasicFields::Password => {
                    password.set_override(template);
                }
            },
            Self::Bearer { token } => {
                token.set_override(template);
            }
        }
    }

    /// Reset the value template override to the default from the recipe, and
    /// recompute the template preview
    fn reset_override(&mut self) {
        match self {
            Self::Basic {
                username,
                password,
                selected_field,
            } => match selected_field.data().selected() {
                BasicFields::Username => {
                    username.reset_override();
                }
                BasicFields::Password => {
                    password.reset_override();
                }
            },
            Self::Bearer { token } => {
                token.reset_override();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{harness, terminal, TestHarness, TestTerminal},
        view::{
            component::{
                recipe_pane::persistence::RecipeOverrideValue,
                RecipeOverrideStore,
            },
            test_util::TestComponent,
        },
    };
    use crossterm::event::KeyCode;
    use persisted::PersistedStore;
    use rstest::rstest;
    use slumber_core::test_util::Factory;

    #[rstest]
    fn test_edit_basic(harness: TestHarness, terminal: TestTerminal) {
        let authentication = Authentication::Basic {
            username: "user1".into(),
            password: Some("hunter2".into()),
        };
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            AuthenticationDisplay::new(RecipeId::factory(()), authentication),
            (),
        );

        // Check initial state
        assert_eq!(component.data().override_value(), None);

        // Edit username
        component.send_key(KeyCode::Char('e')).assert_empty();
        component.send_text("!!!").assert_empty();
        component.send_key(KeyCode::Enter).assert_empty();
        assert_eq!(
            component.data().override_value(),
            Some(Authentication::Basic {
                username: "user1!!!".into(),
                password: Some("hunter2".into())
            })
        );

        // Reset username
        component.send_key(KeyCode::Char('z')).assert_empty();
        assert_eq!(component.data().override_value(), None);

        // Edit password
        component.send_key(KeyCode::Down).assert_empty();
        component.send_key(KeyCode::Char('e')).assert_empty();
        component.send_text("???").assert_empty();
        component.send_key(KeyCode::Enter).assert_empty();
        assert_eq!(
            component.data().override_value(),
            Some(Authentication::Basic {
                username: "user1".into(),
                password: Some("hunter2???".into())
            })
        );

        // Reset password
        component.send_key(KeyCode::Char('z')).assert_empty();
        assert_eq!(component.data().override_value(), None);
    }

    #[rstest]
    fn test_edit_basic_empty_password(
        harness: TestHarness,
        terminal: TestTerminal,
    ) {
        let authentication = Authentication::Basic {
            username: "user1".into(),
            password: None,
        };
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            AuthenticationDisplay::new(RecipeId::factory(()), authentication),
            (),
        );

        // Edit password
        component.send_key(KeyCode::Down).assert_empty();
        component.send_key(KeyCode::Char('e')).assert_empty();
        component.send_key(KeyCode::Enter).assert_empty();
        assert_eq!(
            component.data().override_value(),
            Some(Authentication::Basic {
                username: "user1".into(),
                // None gets replaced by empty string. They're functionally
                // equivalent because the encoding maps to {username}:{password}
                password: Some("".into())
            })
        );
    }

    #[rstest]
    fn test_edit_bearer(harness: TestHarness, terminal: TestTerminal) {
        let authentication = Authentication::Bearer("i am a token".into());
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            AuthenticationDisplay::new(RecipeId::factory(()), authentication),
            (),
        );

        // Check initial state
        assert_eq!(component.data().override_value(), None);

        // Edit token
        component.send_key(KeyCode::Char('e')).assert_empty();
        component.send_text("!!!").assert_empty();
        component.send_key(KeyCode::Enter).assert_empty();
        assert_eq!(
            component.data().override_value(),
            Some(Authentication::Bearer("i am a token!!!".into()))
        );

        // Reset token
        component.send_key(KeyCode::Char('z')).assert_empty();
        assert_eq!(component.data().override_value(), None);
    }

    /// Basic auth fields should load persisted overrides
    #[rstest]
    fn test_persisted_load_basic(harness: TestHarness, terminal: TestTerminal) {
        let recipe_id = RecipeId::factory(());
        RecipeOverrideStore::store_persisted(
            &RecipeOverrideKey::auth_basic_username(recipe_id.clone()),
            &RecipeOverrideValue::Override("user".into()),
        );
        RecipeOverrideStore::store_persisted(
            &RecipeOverrideKey::auth_basic_password(recipe_id.clone()),
            &RecipeOverrideValue::Override("hunter2".into()),
        );
        let authentication = Authentication::Basic {
            username: "".into(),
            password: None,
        };
        let component = TestComponent::new(
            &harness,
            &terminal,
            AuthenticationDisplay::new(recipe_id, authentication),
            (),
        );

        assert_eq!(
            component.data().override_value(),
            Some(Authentication::Basic {
                username: "user".into(),
                password: Some("hunter2".into()),
            })
        );
    }

    /// Basic auth fields should load persisted overrides
    #[rstest]
    fn test_persisted_load_bearer(
        harness: TestHarness,
        terminal: TestTerminal,
    ) {
        let recipe_id = RecipeId::factory(());
        RecipeOverrideStore::store_persisted(
            &RecipeOverrideKey::auth_bearer_token(recipe_id.clone()),
            &RecipeOverrideValue::Override("token".into()),
        );
        let authentication = Authentication::Bearer("".into());
        let component = TestComponent::new(
            &harness,
            &terminal,
            AuthenticationDisplay::new(recipe_id, authentication),
            (),
        );

        assert_eq!(
            component.data().override_value(),
            Some(Authentication::Bearer("token".into()))
        );
    }
}
