use crate::view::{
    UpdateContext,
    common::{actions::MenuItem, template_preview::TemplatePreview},
    component::{
        Canvas, Child, Component, ComponentId, Draw, DrawMetadata, ToChild,
        recipe::override_template::{EditableTemplate, RecipeOverrideKey},
    },
    event::{Emitter, Event, EventMatch},
};
use slumber_config::Action;
use slumber_core::collection::RecipeId;
use slumber_template::Template;

/// URL display with override capabilities
#[derive(Debug)]
pub struct UrlDisplay {
    id: ComponentId,

    /// Emitter for menu actions
    actions_emitter: Emitter<UrlMenuAction>,
    /// Rendered URL
    url: EditableTemplate,
}

impl UrlDisplay {
    pub fn new(recipe_id: RecipeId, url: Template) -> Self {
        let url = EditableTemplate::new(
            RecipeOverrideKey::url(recipe_id),
            url,
            None,
            false,
        );
        Self {
            id: ComponentId::default(),
            actions_emitter: Emitter::default(),
            url,
        }
    }

    /// Get the preview widget. This is used where the URL is drawn
    /// non-interactively
    pub fn preview(&self) -> &TemplatePreview {
        self.url.preview()
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

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            .emitted(self.actions_emitter, |menu_action| match menu_action {
                UrlMenuAction::Edit => self.url.edit(),
                UrlMenuAction::Reset => self.url.reset_override(),
            })
    }

    fn menu(&self) -> Vec<MenuItem> {
        let emitter = self.actions_emitter;
        vec![
            emitter
                .menu(UrlMenuAction::Edit, "Edit URL")
                .shortcut(Some(Action::Edit))
                .into(),
            emitter
                .menu(UrlMenuAction::Reset, "Reset URL")
                .enable(self.url.is_overridden())
                .shortcut(Some(Action::Reset))
                .into(),
        ]
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

#[derive(Copy, Clone, Debug)]
enum UrlMenuAction {
    Edit,
    Reset,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestHarness, TestTerminal, harness, terminal},
        view::test_util::TestComponent,
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
            .assert_empty();
        assert_eq!(
            component.override_value(),
            Some("/users/{{ username }}!!!".into())
        );

        // Reset token
        component.int().send_key(KeyCode::Char('z')).assert_empty();
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
            .assert_empty();
        assert_eq!(
            component.override_value(),
            Some("/users/{{ username }}!".into())
        );

        // Edit URL
        component.int().action(&["Reset URL"]).assert_empty();
        assert_eq!(component.override_value(), None);
    }

    /// Basic auth fields should load persisted overrides
    #[rstest]
    fn test_persisted_load(harness: TestHarness, terminal: TestTerminal) {
        let recipe_id = RecipeId::factory(());
        harness.set_persisted_session(
            &RecipeOverrideKey::url(recipe_id.clone()),
            "persisted/url".into(),
        );
        let component = TestComponent::new(
            &harness,
            &terminal,
            UrlDisplay::new(recipe_id, "default/url".into()),
        );

        assert_eq!(component.override_value(), Some("persisted/url".into()));
    }
}
