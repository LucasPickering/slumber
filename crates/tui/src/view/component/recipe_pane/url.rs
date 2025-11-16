use crate::view::{
    UpdateContext,
    common::{actions::MenuItem, modal::ModalQueue},
    component::{
        Canvas, Child, Component, ComponentId, Draw, DrawMetadata, ToChild,
        misc::TextBoxModal,
        recipe_pane::persistence::{RecipeOverrideKey, RecipeTemplate},
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
    /// Emitter for the callback from editing the URL
    override_emitter: Emitter<SaveUrlOverride>,
    /// Emitter for menu actions
    actions_emitter: Emitter<UrlMenuAction>,
    /// Rendered URL
    url: RecipeTemplate,
    /// Modal to edit template override
    edit_modal: ModalQueue<TextBoxModal>,
}

impl UrlDisplay {
    pub fn new(recipe_id: RecipeId, url: Template) -> Self {
        let url = RecipeTemplate::new(
            RecipeOverrideKey::url(recipe_id),
            url,
            None,
            false,
        );
        Self {
            id: ComponentId::default(),
            override_emitter: Emitter::default(),
            actions_emitter: Emitter::default(),
            url,
            edit_modal: ModalQueue::default(),
        }
    }

    /// If the template has been overridden, get the new template
    pub fn override_value(&self) -> Option<Template> {
        self.url
            .is_overridden()
            .then(|| self.url.template().clone())
    }

    /// Open a modal to let the user edit the temporary override URL
    fn open_edit_modal(&mut self) {
        let emitter = self.override_emitter;
        self.edit_modal.open(self.url.edit_modal(
            "Edit URL".to_owned(),
            // Defer the state update into an event so it can use &mut
            move |template| emitter.emit(SaveUrlOverride(template)),
        ));
    }
}

impl Component for UrlDisplay {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            .action(|action, propagate| match action {
                Action::Edit => self.open_edit_modal(),
                Action::Reset => self.url.reset_override(),
                _ => propagate.set(),
            })
            .emitted(self.override_emitter, |SaveUrlOverride(template)| {
                self.url.set_override(template);
            })
            .emitted(self.actions_emitter, |menu_action| match menu_action {
                UrlMenuAction::Edit => self.open_edit_modal(),
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
        vec![self.edit_modal.to_child_mut()]
    }
}

impl Draw for UrlDisplay {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        canvas.render_widget(self.url.preview(), metadata.area());
        canvas.draw_portal(&self.edit_modal, (), true);
    }
}

/// Local event to save a user's override value(s). Triggered from the edit
/// modal.
#[derive(Debug)]
struct SaveUrlOverride(Template);

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
        view::{component::RecipeOverrideStore, test_util::TestComponent},
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
        RecipeOverrideStore::set(
            &RecipeOverrideKey::url(recipe_id.clone()),
            &"persisted/url".into(),
        );
        let component = TestComponent::new(
            &harness,
            &terminal,
            UrlDisplay::new(recipe_id, "default/url".into()),
        );

        assert_eq!(component.override_value(), Some("persisted/url".into()));
    }
}
