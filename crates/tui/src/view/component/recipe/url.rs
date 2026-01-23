use crate::view::{
    component::{
        Canvas, Child, Component, ComponentId, Draw, DrawMetadata, ToChild,
        editable_template::EditableTemplate,
    },
    persistent::SessionKey,
};
use ratatui::text::Text;
use slumber_core::collection::RecipeId;
use slumber_template::Template;

/// URL display with override capabilities
#[derive(Debug)]
pub struct UrlDisplay {
    id: ComponentId,
    /// Rendered URL
    url: EditableTemplate<UrlKey>,
}

impl UrlDisplay {
    pub fn new(recipe_id: RecipeId, url: Template) -> Self {
        let url =
            EditableTemplate::new("URL", UrlKey(recipe_id), url, false, false);
        Self {
            id: ComponentId::default(),
            url,
        }
    }

    /// Get the preview text widget. This is used where the URL is drawn
    /// non-interactively
    pub fn preview(&self) -> &Text {
        self.url.text()
    }

    /// If the template has been overridden, get the new template
    pub fn override_value(&self) -> Option<Template> {
        self.url
            .is_overridden()
            .then(|| self.url.template().clone())
    }
}

impl Component for UrlDisplay {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.url.to_child_mut()]
    }
}

impl Draw for UrlDisplay {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        canvas.draw(&self.url, (), metadata.area(), true);
    }
}

/// Persistent key for URL override template
#[derive(Clone, Debug, PartialEq)]
struct UrlKey(RecipeId);

impl SessionKey for UrlKey {
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

    /// Test edit/reset via keybind
    #[rstest]
    fn test_edit(harness: TestHarness, terminal: TestTerminal) {
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            UrlDisplay::new(
                RecipeId::factory(()),
                "/users/{{ username }}".into(),
            ),
        );

        // Check initial state
        assert_eq!(component.override_value(), None);

        // Edit URL
        component
            .int()
            .send_key(KeyCode::Char('e'))
            .send_text("!!!")
            .send_key(KeyCode::Enter)
            .assert()
            .empty();
        assert_eq!(
            component.override_value(),
            Some("/users/{{ username }}!!!".into())
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
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            UrlDisplay::new(
                RecipeId::factory(()),
                "/users/{{ username }}".into(),
            ),
        );

        // Check initial state
        assert_eq!(component.override_value(), None);

        // Edit URL
        component
            .int()
            .action(&["Edit URL"])
            .send_keys([KeyCode::Char('!'), KeyCode::Enter])
            .assert()
            .empty();
        assert_eq!(
            component.override_value(),
            Some("/users/{{ username }}!".into())
        );

        // Edit URL
        component.int().action(&["Reset URL"]).assert().empty();
        assert_eq!(component.override_value(), None);
    }

    /// Basic auth fields should load persisted overrides
    #[rstest]
    fn test_persisted_load(harness: TestHarness, terminal: TestTerminal) {
        let recipe_id = RecipeId::factory(());
        harness
            .persistent_store()
            .set_session(UrlKey(recipe_id.clone()), "persisted/url".into());
        let component = TestComponent::new(
            &harness,
            &terminal,
            UrlDisplay::new(recipe_id, "default/url".into()),
        );

        assert_eq!(component.override_value(), Some("persisted/url".into()));
    }
}
