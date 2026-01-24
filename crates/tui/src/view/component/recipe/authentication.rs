use crate::view::{
    common::{
        component_select::{
            ComponentSelect, ComponentSelectProps, SelectStyles,
        },
        select::{Select, SelectEventKind},
    },
    component::{
        Canvas, Component, ComponentId, Draw, DrawMetadata, ToChild,
        editable_template::EditableTemplate, internal::Child,
    },
    context::{UpdateContext, ViewContext},
    event::{Event, EventMatch, ToEmitter},
    persistent::SessionKey,
};
use ratatui::{layout::Layout, prelude::Constraint, text::Span};
use slumber_core::collection::{Authentication, RecipeId};
use slumber_template::Template;

/// Display authentication settings for a recipe
#[derive(Debug)]
pub struct AuthenticationDisplay {
    id: ComponentId,
    state: State,
}

impl AuthenticationDisplay {
    pub fn new(recipe_id: RecipeId, authentication: Authentication) -> Self {
        let state = match authentication {
            Authentication::Basic { username, password } => {
                State::Basic(BasicAuthentication::new(
                    recipe_id,
                    username,
                    password.unwrap_or_default(),
                ))
            }
            Authentication::Bearer { token } => State::Bearer {
                token: EditableTemplate::new(
                    "Token",
                    AuthenticationKey::Token(recipe_id.clone()),
                    token,
                    false,
                    false,
                ),
            },
        };
        Self {
            id: ComponentId::default(),
            state,
        }
    }

    /// If the user has applied a temporary edit to the auth settings, get the
    /// override value. Return `None` to use the recipe's stock auth.
    pub fn override_value(&self) -> Option<Authentication> {
        if self.state.is_overridden() {
            Some(match &self.state {
                State::Basic(basic) => Authentication::Basic {
                    username: basic.username().clone(),
                    // We don't use an option internally because an empty
                    // password is equivalent to no password
                    password: Some(basic.password().clone()),
                },
                State::Bearer { token, .. } => Authentication::Bearer {
                    token: token.template().clone(),
                },
            })
        } else {
            None
        }
    }
}

impl Component for AuthenticationDisplay {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        match &mut self.state {
            State::Basic(basic) => vec![basic.to_child_mut()],
            State::Bearer { token } => vec![token.to_child_mut()],
        }
    }
}

impl Draw for AuthenticationDisplay {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let styles = ViewContext::styles();
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
            State::Basic(basic) => {
                canvas.draw(basic, (), content_area, true);
            }
            State::Bearer { token } => {
                canvas.draw(token, (), content_area, true);
            }
        }
    }
}

/// Private to hide enum variants
#[derive(Debug)]
enum State {
    Basic(BasicAuthentication),
    Bearer {
        token: EditableTemplate<AuthenticationKey>,
    },
}

impl State {
    /// Have *any* fields been overridden?
    fn is_overridden(&self) -> bool {
        match self {
            Self::Basic(basic) => {
                basic.select.items().any(|item| item.value.is_overridden())
            }
            Self::Bearer { token } => token.is_overridden(),
        }
    }
}

/// Wrapper for basic authentication state. This needs to be a separate
/// component because it has its own event handling for the contained Select
#[derive(Debug)]
struct BasicAuthentication {
    id: ComponentId,
    /// A list of exactly two fields: [username, password]. This can't use
    /// `FixedSelect` because there's associated data attached to each
    /// field
    select: ComponentSelect<BasicField>,
}

impl BasicAuthentication {
    fn new(
        recipe_id: RecipeId,
        username: Template,
        password: Template,
    ) -> Self {
        let username = EditableTemplate::new(
            "Username",
            AuthenticationKey::Username(recipe_id.clone()),
            username,
            false,
            false,
        );
        let password = EditableTemplate::new(
            "Password",
            AuthenticationKey::Password(recipe_id.clone()),
            password,
            false,
            false,
        );
        let select = Select::builder(vec![
            BasicField::new("Username", username),
            BasicField::new("Password", password),
        ])
        .subscribe([SelectEventKind::Select])
        .build();
        Self {
            id: ComponentId::default(),
            select: ComponentSelect::new(select),
        }
    }

    fn username(&self) -> &Template {
        self.select[0].value.template()
    }

    fn password(&self) -> &Template {
        self.select[1].value.template()
    }
}

impl Component for BasicAuthentication {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            .emitted(self.select.to_emitter(), |event| match event.kind {
                SelectEventKind::Select => {
                    // When changing selection, stop editing the previous item
                    for row in self.select.items_mut() {
                        row.value.submit_edit();
                    }
                }
                SelectEventKind::Submit | SelectEventKind::Toggle => {}
            })
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.select.to_child_mut()]
    }
}

impl Draw for BasicAuthentication {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        canvas.draw(
            &self.select,
            ComponentSelectProps {
                styles: SelectStyles::table(),
                ..Default::default()
            },
            metadata.area(),
            true,
        );
    }
}

/// One row in a basic auth form. Each form has exactly two rows: Username and
/// Password
#[derive(Debug)]
struct BasicField {
    id: ComponentId,
    label: &'static str,
    value: EditableTemplate<AuthenticationKey>,
}

impl BasicField {
    fn new(
        label: &'static str,
        template: EditableTemplate<AuthenticationKey>,
    ) -> Self {
        Self {
            id: ComponentId::default(),
            label,
            value: template,
        }
    }
}

impl Component for BasicField {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.value.to_child_mut()]
    }
}

impl Draw for BasicField {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let [label_area, value_area] =
            Layout::horizontal([Constraint::Length(10), Constraint::Min(1)])
                .areas(metadata.area());

        canvas.render_widget(self.label, label_area);
        canvas.draw(&self.value, (), value_area, true);
    }
}

/// Session persistent key for override templates
#[derive(Clone, Debug, PartialEq)]
enum AuthenticationKey {
    Token(RecipeId),
    Username(RecipeId),
    Password(RecipeId),
}

impl SessionKey for AuthenticationKey {
    type Value = Template;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestTerminal, terminal},
        view::test_util::{TestComponent, TestHarness, harness},
    };
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
            .assert()
            .empty();
        assert_eq!(
            component.override_value(),
            Some(Authentication::Basic {
                username: "user1!!!".into(),
                password: Some("hunter2".into())
            })
        );

        // Reset username
        component
            .int()
            .send_key(KeyCode::Char('z'))
            .assert()
            .empty();
        assert_eq!(component.override_value(), None);

        // Edit password
        component
            .int()
            .send_keys([KeyCode::Down, KeyCode::Char('e')])
            .send_text("???")
            .send_key(KeyCode::Enter)
            .assert()
            .empty();
        assert_eq!(
            component.override_value(),
            Some(Authentication::Basic {
                username: "user1".into(),
                password: Some("hunter2???".into())
            })
        );

        // Reset password
        component
            .int()
            .send_key(KeyCode::Char('z'))
            .assert()
            .empty();
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
            .assert()
            .empty();
        // There's no override because the password wasn't actually modified
        assert_eq!(component.override_value(), None);
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
            .assert()
            .empty();
        assert_eq!(
            component.override_value(),
            Some(Authentication::Bearer {
                token: "i am a token!!!".into()
            })
        );

        // Reset token
        component
            .int()
            .send_key(KeyCode::Char('z'))
            .assert()
            .empty();
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
            .action(&["Edit Token"])
            .send_keys([KeyCode::Char('!'), KeyCode::Enter])
            .assert()
            .empty();
        assert_eq!(
            component.override_value(),
            Some(Authentication::Bearer {
                token: "i am a token!".into()
            })
        );

        component.int().action(&["Reset Token"]).assert().empty();
        assert_eq!(component.override_value(), None);
    }

    /// Basic auth fields should load persisted overrides
    #[rstest]
    fn test_persisted_load_basic(harness: TestHarness, terminal: TestTerminal) {
        let recipe_id = RecipeId::factory(());
        harness.persistent_store().set_session(
            AuthenticationKey::Username(recipe_id.clone()),
            "user".into(),
        );
        harness.persistent_store().set_session(
            AuthenticationKey::Password(recipe_id.clone()),
            "hunter2".into(),
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
        harness.persistent_store().set_session(
            AuthenticationKey::Token(recipe_id.clone()),
            "token".into(),
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
