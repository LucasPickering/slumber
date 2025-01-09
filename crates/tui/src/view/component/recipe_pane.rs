mod authentication;
mod body;
mod persistence;
mod recipe;
mod table;

pub use persistence::RecipeOverrideStore;

use crate::{
    context::TuiContext,
    message::RequestConfig,
    view::{
        common::{
            actions::{IntoMenuActions, MenuAction},
            Pane,
        },
        component::recipe_pane::recipe::RecipeDisplay,
        context::UpdateContext,
        draw::{Draw, DrawMetadata, Generate},
        event::{Child, Emitter, Event, EventHandler, OptionEvent, ToEmitter},
        state::StateCell,
        Component, ViewContext,
    },
};
use derive_more::Display;
use itertools::{Itertools, Position};
use mime::Mime;
use ratatui::{
    text::{Line, Text},
    Frame,
};
use reqwest::header;
use slumber_config::Action;
use slumber_core::{
    collection::{Folder, HasId, ProfileId, RecipeId, RecipeNode},
    util::doc_link,
};
use std::cell::Ref;
use strum::EnumIter;

/// Display for the current recipe node, which could be a recipe, a folder, or
/// empty
#[derive(Debug, Default)]
pub struct RecipePane {
    /// Emitter for the on-click event, to focus the pane
    click_emitter: Emitter<RecipePaneEvent>,
    /// Emitter for menu actions, to be handled by our parent
    actions_emitter: Emitter<RecipeMenuAction>,
    /// All UI state derived from the recipe is stored together, and reset when
    /// the recipe or profile changes
    recipe_state: StateCell<RecipeStateKey, Component<Option<RecipeDisplay>>>,
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
        let options = recipe_state.data().as_ref()?.build_options();
        Some(RequestConfig {
            recipe_id,
            profile_id,
            options,
        })
    }

    /// Get the value that the `Content-Type` header will have for a generated
    /// request. This will use the preview of the header if present, otherwise
    /// it will fall back to the content type of the body, if known (e.g. JSON).
    /// Otherwise, return `None`.
    pub fn mime(&self) -> Option<Mime> {
        let state = self.recipe_state.get()?;
        let display = state.data().as_ref()?;
        display
            .header(header::CONTENT_TYPE)
            .and_then(|value| value.parse::<Mime>().ok())
            .or_else(|| {
                // Use the type of the body to determine MIME
                let recipe_id =
                    Ref::filter_map(self.recipe_state.get_key()?, |key| {
                        key.recipe_id.as_ref()
                    })
                    .ok()?;
                let collection = ViewContext::collection();
                let recipe = collection.recipes.get(&recipe_id)?.recipe()?;
                recipe.body.as_ref()?.mime()
            })
    }

    /// Execute a function with the recipe's body text, if available. Body text
    /// is only available for recipes with non-form bodies.
    pub fn with_body_text(&self, f: impl FnOnce(&Text)) {
        let Some(state) = self.recipe_state.get() else {
            return;
        };
        let Some(display) = state.data().as_ref() else {
            return;
        };
        let Some(body_text) = display.body_text() else {
            return;
        };
        f(&body_text)
    }
}

impl EventHandler for RecipePane {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event.opt().action(|action, propagate| match action {
            Action::LeftClick => {
                self.click_emitter.emit(RecipePaneEvent::Click)
            }
            _ => propagate.set(),
        })
    }

    fn menu_actions(&self) -> Vec<MenuAction> {
        RecipeMenuAction::into_actions(self)
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        if let Some(state) = self.recipe_state.get_mut() {
            vec![state.to_child_mut()]
        } else {
            vec![]
        }
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
            &RecipeStateKey {
                selected_profile_id: props.selected_profile_id.cloned(),
                recipe_id: props
                    .selected_recipe_node
                    .map(RecipeNode::id)
                    .cloned(),
            },
            || {
                match props.selected_recipe_node {
                    Some(RecipeNode::Recipe(recipe)) => {
                        Some(RecipeDisplay::new(recipe))
                    }
                    Some(RecipeNode::Folder(_)) | None => None,
                }
                .into()
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
                recipe_state.draw_opt(frame, (), inner_area, true)
            }
        };
    }
}

/// Notify parent when this pane is clicked
impl ToEmitter<RecipePaneEvent> for RecipePane {
    fn to_emitter(&self) -> Emitter<RecipePaneEvent> {
        self.click_emitter
    }
}

/// Notify parent when one of this pane's actions is selected
impl ToEmitter<RecipeMenuAction> for RecipePane {
    fn to_emitter(&self) -> Emitter<RecipeMenuAction> {
        self.actions_emitter
    }
}

/// Emitted event for the recipe pane component
#[derive(Debug)]
pub enum RecipePaneEvent {
    Click,
}

/// Template preview state will be recalculated when any of these fields change
#[derive(Clone, Debug, PartialEq)]
struct RecipeStateKey {
    selected_profile_id: Option<ProfileId>,
    recipe_id: Option<RecipeId>,
}

/// Items in the actions popup menu. This is also used by the recipe list
/// component, so the action is handled in the parent.
#[derive(Copy, Clone, Debug, Display, EnumIter)]
pub enum RecipeMenuAction {
    #[display("Copy URL")]
    CopyUrl,
    #[display("Copy as cURL")]
    CopyCurl,
    #[display("View Body")]
    ViewBody,
    #[display("Copy Body")]
    CopyBody,
}

impl IntoMenuActions<RecipePane> for RecipeMenuAction {
    fn enabled(&self, data: &RecipePane) -> bool {
        let recipe = data.recipe_state.get().and_then(|state| {
            Ref::filter_map(state, |state| state.data().as_ref()).ok()
        });
        match self {
            // Enabled if we have any recipe
            Self::CopyUrl | Self::CopyCurl => recipe.is_some(),
            // Enabled if we have a body
            Self::ViewBody | Self::CopyBody => {
                recipe.is_some_and(|recipe| recipe.has_body())
            }
        }
    }
}

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
            parent_positions: &mut Vec<Position>,
        ) {
            for (position, node) in folder.children.values().with_position() {
                let mut line = Line::default();

                // Add decoration
                for parent_position in parent_positions.iter() {
                    let padding = match parent_position {
                        // Extend the parent's line down if it has more children
                        Position::First | Position::Middle => "│ ",
                        Position::Last | Position::Only => "  ",
                    };
                    line.push_span(padding);
                }
                line.push_span(match position {
                    Position::First | Position::Middle => "├─",
                    Position::Last | Position::Only => "└─",
                });

                line.push_span(node.name());
                lines.push(line);
                if let RecipeNode::Folder(folder) = node {
                    parent_positions.push(position);
                    add_lines(lines, folder, parent_positions);
                    parent_positions.pop();
                }
            }
        }
        add_lines(&mut lines, self, &mut Vec::new());
        lines.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use slumber_core::{
        collection::Recipe,
        test_util::{by_id, Factory},
    };

    #[test]
    fn test_folder_tree() {
        let folder = Folder {
            id: "1f".into(),
            name: None,
            children: by_id([
                RecipeNode::Recipe(Recipe::factory("1.1r")),
                RecipeNode::Recipe(Recipe::factory("1.2r")),
                // Nested folder
                RecipeNode::Folder(Folder {
                    id: "1.3f".into(),
                    name: None,
                    children: by_id([RecipeNode::Recipe(Recipe::factory(
                        "1.3.1r",
                    ))]),
                }),
                // Empty folder
                RecipeNode::Folder(Folder {
                    id: "1.4f".into(),
                    name: None,
                    children: Default::default(),
                }),
                // End with a nested folder to make sure the leftmost
                // decorations don't appear
                RecipeNode::Folder(Folder {
                    id: "1.5f".into(),
                    name: None,
                    children: by_id([
                        RecipeNode::Recipe(Recipe::factory("1.5.1r")),
                        RecipeNode::Folder(Folder {
                            id: "1.5.2f".into(),
                            name: None,
                            children: by_id([RecipeNode::Recipe(
                                Recipe::factory("1.5.2.1r"),
                            )]),
                        }),
                    ]),
                }),
            ]),
        };

        let expected = "\
1f
├─1.1r
├─1.2r
├─1.3f
│ └─1.3.1r
├─1.4f
└─1.5f
  ├─1.5.1r
  └─1.5.2f
    └─1.5.2.1r";
        assert_eq!(folder.generate().to_string(), expected);
    }
}
