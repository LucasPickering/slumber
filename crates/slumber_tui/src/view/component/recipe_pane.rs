mod authentication;
mod body;
mod recipe;

use crate::{
    context::TuiContext,
    message::RequestConfig,
    view::{
        common::{actions::ActionsModal, Pane},
        component::{primary::PrimaryPane, recipe_pane::recipe::RecipeDisplay},
        draw::{Draw, DrawMetadata, Generate, ToStringGenerate},
        event::{Event, EventHandler, Update},
        state::StateCell,
        Component, ModalPriority, ViewContext,
    },
};
use derive_more::Display;
use ratatui::{
    text::{Line, Text},
    Frame,
};
use slumber_config::Action;
use slumber_core::{
    collection::{Folder, HasId, ProfileId, RecipeId, RecipeNode},
    util::doc_link,
};
use strum::{EnumCount, EnumIter};

/// Display for the current recipe node, which could be a recipe, a folder, or
/// empty
#[derive(Debug, Default)]
pub struct RecipePane {
    /// All UI state derived from the recipe is stored together, and reset when
    /// the recipe or profile changes
    recipe_state: StateCell<RecipeStateKey, Option<Component<RecipeDisplay>>>,
}

#[derive(Clone)]
pub struct RecipePaneProps<'a> {
    /// ID of the recipe *or* folder selected
    pub selected_recipe_node: Option<&'a RecipeNode>,
    pub selected_profile_id: Option<&'a ProfileId>,
}

impl RecipePane {
    /// Get a definition of the request that should be sent from the current
    /// recipe settings
    pub fn request_config(&self) -> Option<RequestConfig> {
        let state_key = self.recipe_state.get_key()?;
        let recipe_id = state_key.recipe_id.clone()?;
        let profile_id = state_key.selected_profile_id.clone();
        let recipe_state = self.recipe_state.get()?;
        let options = recipe_state.as_ref()?.data().build_options();
        Some(RequestConfig {
            recipe_id,
            profile_id,
            options,
        })
    }
}

impl EventHandler for RecipePane {
    fn update(&mut self, event: Event) -> Update {
        if let Some(action) = event.action() {
            match action {
                Action::LeftClick => {
                    ViewContext::push_event(Event::new_local(
                        PrimaryPane::Recipe,
                    ));
                }
                Action::OpenActions => {
                    let state = self.recipe_state.get_mut();
                    ViewContext::open_modal(
                        ActionsModal::new(RecipeMenuAction::disabled_actions(
                            state.is_some(),
                            state
                                .and_then(Option::as_mut)
                                .is_some_and(|state| state.data().has_body()),
                        )),
                        ModalPriority::Low,
                    )
                }
                _ => return Update::Propagate(event),
            }
        } else {
            return Update::Propagate(event);
        }
        Update::Consumed
    }

    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        self.recipe_state
            .get_mut()
            .and_then(|state| Some(state.as_mut()?.as_child()))
            .into_iter()
            .collect()
    }
}

impl<'a> Draw<RecipePaneProps<'a>> for RecipePane {
    fn draw(
        &self,
        frame: &mut Frame,
        props: RecipePaneProps<'a>,
        metadata: DrawMetadata,
    ) {
        // Render outermost block
        let title = TuiContext::get().input_engine.add_hint(
            match props.selected_recipe_node {
                Some(RecipeNode::Folder(_)) => "Folder",
                Some(RecipeNode::Recipe(_)) | None => "Recipe",
            },
            Action::SelectRecipe,
        );
        let block = Pane {
            title: &title,
            has_focus: metadata.has_focus(),
        };
        let block = block.generate();
        let inner_area = block.inner(metadata.area());
        frame.render_widget(block, metadata.area());

        // Whenever the recipe or profile changes, generate a preview for
        // each templated value. Almost anything that could change the
        // preview will either involve changing one of those two things, or
        // would require reloading the whole collection which will reset
        // UI state.
        let recipe_state = self.recipe_state.get_or_update(
            RecipeStateKey {
                selected_profile_id: props.selected_profile_id.cloned(),
                recipe_id: props
                    .selected_recipe_node
                    .map(RecipeNode::id)
                    .cloned(),
            },
            || match props.selected_recipe_node {
                Some(RecipeNode::Recipe(recipe)) => Some(
                    RecipeDisplay::new(recipe, props.selected_profile_id)
                        .into(),
                ),
                Some(RecipeNode::Folder(_)) | None => None,
            },
        );

        match props.selected_recipe_node {
            None => frame.render_widget(
                Text::from(vec![
                    "No recipes defined; add one to your collection".into(),
                    doc_link("api/request_collection/request_recipe").into(),
                ]),
                inner_area,
            ),
            Some(RecipeNode::Folder(folder)) => {
                frame.render_widget(folder.generate(), inner_area);
            }
            Some(RecipeNode::Recipe(_)) => {
                // Unwrap is safe because we just initialized state above
                recipe_state
                    .as_ref()
                    .unwrap()
                    .draw(frame, (), inner_area, true)
            }
        };
    }
}

/// Template preview state will be recalculated when any of these fields change
#[derive(Debug, PartialEq)]
struct RecipeStateKey {
    selected_profile_id: Option<ProfileId>,
    recipe_id: Option<RecipeId>,
}

/// Items in the actions popup menu. This is also used by the recipe list
/// component, so the action is handled in the parent.
#[derive(
    Copy, Clone, Debug, Default, Display, EnumCount, EnumIter, PartialEq,
)]
pub enum RecipeMenuAction {
    #[default]
    #[display("Edit Collection")]
    EditCollection,
    #[display("Copy URL")]
    CopyUrl,
    #[display("Copy Body")]
    CopyBody,
    #[display("Copy as cURL")]
    CopyCurl,
}

impl RecipeMenuAction {
    pub fn disabled_actions(
        has_recipe: bool,
        has_body: bool,
    ) -> &'static [Self] {
        if has_recipe {
            if has_body {
                &[]
            } else {
                &[Self::CopyBody]
            }
        } else {
            &[Self::CopyUrl, Self::CopyBody, Self::CopyCurl]
        }
    }
}

impl ToStringGenerate for RecipeMenuAction {}

/// Render folder as a tree
impl<'a> Generate for &'a Folder {
    type Output<'this> = Text<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        // Generate something like:
        // Users
        // ├─Get Users
        // ├─Get User
        // ├─inner
        // │ ├─Get User
        // │ ├─inner2
        // │ │ └─Get User
        // │ └─Get User
        // └─Modify User

        let mut lines = vec![self.name().into()];
        fn add_lines<'a>(
            lines: &mut Vec<Line<'a>>,
            folder: &'a Folder,
            depth: usize,
        ) {
            let len = folder.children.len();
            for (i, node) in folder.children.values().enumerate() {
                let mut line = Line::default();

                // Add decoration
                for _ in 0..depth {
                    line.push_span("│ ");
                }
                line.push_span(if i < len - 1 { "├─" } else { "└─" });

                line.push_span(node.name());
                lines.push(line);
                if let RecipeNode::Folder(folder) = node {
                    add_lines(lines, folder, depth + 1);
                }
            }
        }
        add_lines(&mut lines, self, 0);
        lines.into()
    }
}
