use crate::{
    context::TuiContext,
    view::{
        Generate,
        common::{actions::MenuItem, modal::ModalQueue, table::Table},
        component::{
            Canvas, Component, ComponentId, Draw, DrawMetadata, ToChild,
            internal::Child,
            misc::TextBoxModal,
            recipe_pane::persistence::{RecipeOverrideKey, RecipeTemplate},
        },
        context::UpdateContext,
        event::{Emitter, Event, EventMatch},
        state::{fixed_select::FixedSelect, select::SelectTableProps},
    },
};
use derive_more::derive::Display;
use ratatui::{
    layout::Layout, prelude::Constraint, text::Span, widgets::TableState,
};
use slumber_config::Action;
use slumber_core::collection::{Authentication, RecipeId};
use slumber_template::Template;
use std::iter;
use strum::{EnumCount, EnumIter};

/// Display authentication settings for a recipe
#[derive(Debug)]
pub struct AuthenticationDisplay {
    id: ComponentId,
    /// Emitter for the callback from editing the authentication field(s)
    override_emitter: Emitter<SaveAuthenticationOverride>,
    /// Emitter for menu actions
    actions_emitter: Emitter<AuthenticationMenuAction>,
    state: State,
    /// Modal to edit template overrides. One modal is used for all templates
    edit_modal: ModalQueue<TextBoxModal>,
}

impl AuthenticationDisplay {
    pub fn new(recipe_id: RecipeId, authentication: Authentication) -> Self {
        let state = match authentication {
            Authentication::Basic { username, password } => {
                let username = RecipeTemplate::new(
                    RecipeOverrideKey::auth_basic_username(recipe_id.clone()),
                    username,
                    None,
                    false,
                );
                let password = RecipeTemplate::new(
                    RecipeOverrideKey::auth_basic_password(recipe_id.clone()),
                    // See note on this field def for why we unwrap
                    password.unwrap_or_default(),
                    None,
                    false,
                );
                State::Basic {
                    username,
                    password,
                    selected_field: Default::default(),
                }
            }
            Authentication::Bearer { token } => State::Bearer {
                token: RecipeTemplate::new(
                    RecipeOverrideKey::auth_bearer_token(recipe_id.clone()),
                    token,
                    None,
                    false,
                ),
            },
        };
        Self {
            id: ComponentId::default(),
            override_emitter: Emitter::default(),
            actions_emitter: Emitter::default(),
            state,
            edit_modal: ModalQueue::default(),
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
                State::Bearer { token, .. } => Authentication::Bearer {
                    token: token.template().clone(),
                },
            })
        } else {
            None
        }
    }

    /// Open a modal to let the user edit temporary override values
    fn open_edit_modal(
        &mut self,
        emitter: Emitter<SaveAuthenticationOverride>,
    ) {
        let (label, template) = match &self.state {
            State::Basic {
                username,
                password,
                selected_field,
                ..
            } => match selected_field.selected() {
                BasicFields::Username => ("username", username),
                BasicFields::Password => ("password", password),
            },
            State::Bearer { token, .. } => ("bearer token", token),
        };
        self.edit_modal.open(template.edit_modal(
            format!("Edit {label}"),
            // Defer the state update into an event so it can get &mut
            move |template| emitter.emit(SaveAuthenticationOverride(template)),
        ));
    }
}

impl Component for AuthenticationDisplay {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            .action(|action, propagate| match action {
                Action::Edit => {
                    self.open_edit_modal(self.override_emitter);
                }
                Action::Reset => self.state.reset_override(),
                _ => propagate.set(),
            })
            .emitted(
                self.override_emitter,
                |SaveAuthenticationOverride(template)| {
                    self.state.set_override(template);
                },
            )
            .emitted(self.actions_emitter, |menu_action| match menu_action {
                AuthenticationMenuAction::Edit => {
                    self.open_edit_modal(self.override_emitter);
                }
                AuthenticationMenuAction::Reset => self.state.reset_override(),
            })
    }

    fn menu(&self) -> Vec<MenuItem> {
        let emitter = self.actions_emitter;
        vec![
            emitter
                .menu(AuthenticationMenuAction::Edit, "Edit Authentication")
                .shortcut(Some(Action::Edit))
                .into(),
            emitter
                .menu(AuthenticationMenuAction::Reset, "Reset Authentication")
                .enable(self.state.is_overridden())
                .shortcut(Some(Action::Reset))
                .into(),
        ]
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        let field = match &mut self.state {
            State::Basic { selected_field, .. } => {
                Some(selected_field.to_child_mut())
            }
            State::Bearer { .. } => None,
        };
        iter::once(self.edit_modal.to_child_mut())
            .chain(field)
            .collect()
    }
}

impl Draw for AuthenticationDisplay {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let styles = &TuiContext::get().styles;
        let [label_area, content_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)])
                .areas(metadata.area());

        let label = match &self.state {
            State::Basic { .. } => "Basic",
            State::Bearer { .. } => "Bearer",
        };
        canvas.render_widget(
            Span::styled(
                format!("Authentication Type: {label}"),
                styles.text.title,
            ),
            label_area,
        );

        match &self.state {
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
                canvas.draw(
                    selected_field,
                    SelectTableProps { table },
                    content_area,
                    true,
                );
            }
            State::Bearer { token } => {
                canvas.render_widget(token.preview().generate(), content_area);
            }
        }

        canvas.draw_portal(&self.edit_modal, (), true);
    }
}

/// Local event to save a user's override value(s). Triggered from the edit
/// modal.
#[derive(Debug)]
struct SaveAuthenticationOverride(Template);

#[derive(Copy, Clone, Debug)]
enum AuthenticationMenuAction {
    Edit,
    Reset,
}

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
        selected_field: FixedSelect<BasicFields, TableState>,
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

    /// Override the value template for whichever field is selected, and
    /// recompute the template preview
    fn set_override(&mut self, template: Template) {
        match self {
            Self::Basic {
                username,
                password,
                selected_field,
            } => match selected_field.selected() {
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
            } => match selected_field.selected() {
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
        test_util::{TestHarness, TestTerminal, harness, terminal},
        view::{
            component::{
                RecipeOverrideStore,
                recipe_pane::persistence::RecipeOverrideValue,
            },
            test_util::TestComponent,
        },
    };
    use persisted::PersistedStore;
    use rstest::rstest;
    use slumber_util::Factory;
    use terminput::KeyCode;

    /// Test edit basic username+password token via keybinds
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
        );

        // Check initial state
        assert_eq!(component.override_value(), None);

        // Edit username
        component
            .int()
            .send_key(KeyCode::Char('e'))
            .send_text("!!!")
            .send_key(KeyCode::Enter)
            .assert_empty();
        assert_eq!(
            component.override_value(),
            Some(Authentication::Basic {
                username: "user1!!!".into(),
                password: Some("hunter2".into())
            })
        );

        // Reset username
        component.int().send_key(KeyCode::Char('z')).assert_empty();
        assert_eq!(component.override_value(), None);

        // Edit password
        component
            .int()
            .send_keys([KeyCode::Down, KeyCode::Char('e')])
            .send_text("???")
            .send_key(KeyCode::Enter)
            .assert_empty();
        assert_eq!(
            component.override_value(),
            Some(Authentication::Basic {
                username: "user1".into(),
                password: Some("hunter2???".into())
            })
        );

        // Reset password
        component.int().send_key(KeyCode::Char('z')).assert_empty();
        assert_eq!(component.override_value(), None);
    }

    /// Test edit basic username via keybinds
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
        );

        // Edit password
        component
            .int()
            .send_keys([KeyCode::Down, KeyCode::Char('e'), KeyCode::Enter])
            .assert_empty();
        assert_eq!(
            component.override_value(),
            Some(Authentication::Basic {
                username: "user1".into(),
                // None gets replaced by empty string. They're functionally
                // equivalent because the encoding maps to {username}:{password}
                password: Some("".into())
            })
        );
    }

    /// Test edit bearer token via keybinds
    #[rstest]
    fn test_edit_bearer(harness: TestHarness, terminal: TestTerminal) {
        let authentication = Authentication::Bearer {
            token: "i am a token".into(),
        };
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            AuthenticationDisplay::new(RecipeId::factory(()), authentication),
        );

        // Check initial state
        assert_eq!(component.override_value(), None);

        // Edit token
        component
            .int()
            .send_key(KeyCode::Char('e'))
            .send_text("!!!")
            .send_key(KeyCode::Enter)
            .assert_empty();
        assert_eq!(
            component.override_value(),
            Some(Authentication::Bearer {
                token: "i am a token!!!".into()
            })
        );

        // Reset token
        component.int().send_key(KeyCode::Char('z')).assert_empty();
        assert_eq!(component.override_value(), None);
    }

    /// Test edit/reset via menu action
    #[rstest]
    fn test_edit_action(harness: TestHarness, terminal: TestTerminal) {
        let authentication = Authentication::Bearer {
            token: "i am a token".into(),
        };
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            AuthenticationDisplay::new(RecipeId::factory(()), authentication),
        );

        component
            .int()
            .action(&["Edit Authentication"])
            .send_keys([KeyCode::Char('!'), KeyCode::Enter])
            .assert_empty();
        assert_eq!(
            component.override_value(),
            Some(Authentication::Bearer {
                token: "i am a token!".into()
            })
        );

        component
            .int()
            .action(&["Reset Authentication"])
            .assert_empty();
        assert_eq!(component.override_value(), None);
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
        );

        assert_eq!(
            component.override_value(),
            Some(Authentication::Basic {
                username: "user".into(),
                password: Some("hunter2".into()),
            })
        );
    }

    /// Bearer auth fields should load persisted overrides
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
        let authentication = Authentication::Bearer { token: "".into() };
        let component = TestComponent::new(
            &harness,
            &terminal,
            AuthenticationDisplay::new(recipe_id, authentication),
        );

        assert_eq!(
            component.override_value(),
            Some(Authentication::Bearer {
                token: "token".into()
            })
        );
    }
}
