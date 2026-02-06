mod authentication;
mod body;
mod recipe;
mod table;
mod url;

use crate::{
    message::{Message, RecipeCopyTarget},
    view::{
        Component, Generate, ViewContext,
        common::{Pane, actions::MenuItem},
        component::{
            Canvas, ComponentId, Draw, DrawMetadata,
            internal::{Child, ToChild},
            recipe_detail::recipe::RecipeDisplay,
        },
        context::UpdateContext,
        event::{Emitter, Event, EventMatch},
    },
};
use itertools::{Itertools, Position};
use ratatui::{
    layout::Alignment,
    prelude::{Buffer, Rect},
    text::{Line, Text},
    widgets::Widget,
};
use slumber_config::Action;
use slumber_core::{
    collection::{Folder, RecipeId, RecipeNode},
    http::BuildOptions,
};
use slumber_util::doc_link;

/// Display detail for the current recipe node, which could be a recipe, a
/// folder, or empty. This also handles the prompt form. When there are prompts
/// open, the recipe node detail is replaced with the prompts.
#[derive(Debug)]
pub struct RecipeDetail {
    id: ComponentId,
    /// Emitter for menu actions
    actions_emitter: Emitter<RecipeMenuAction>,
    /// UI state derived from the selected node+profile. When either changes,
    /// the component has to be rebuilt
    state: RecipeNodeState,
}

impl RecipeDetail {
    /// Build the recipe detail pane. This should be called whenever the
    /// selected recipe or profile changes. A change to either triggers a
    /// full refresh of the pane.
    pub fn new(selected_recipe_node: Option<&RecipeNode>) -> Self {
        let state = match selected_recipe_node {
            None => RecipeNodeState::None,
            Some(RecipeNode::Folder(folder)) => RecipeNodeState::Folder {
                id: folder.id.clone(),
            },
            Some(RecipeNode::Recipe(recipe)) => RecipeNodeState::Recipe {
                id: recipe.id.clone(),
                display: (RecipeDisplay::new(recipe)),
            },
        };
        Self {
            id: ComponentId::new(),
            actions_emitter: Emitter::default(),
            state,
        }
    }

    /// Generate a [BuildOptions] instance based on current UI state. Return
    /// `None` only when there is no recipe selected.
    pub fn build_options(&self) -> Option<BuildOptions> {
        match &self.state {
            RecipeNodeState::None | RecipeNodeState::Folder { .. } => None,
            RecipeNodeState::Recipe { display, .. } => {
                Some(display.build_options())
            }
        }
    }
}

impl Component for RecipeDetail {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            .emitted(self.actions_emitter, RecipeMenuAction::handle)
    }

    fn menu(&self) -> Vec<MenuItem> {
        let has_recipe = match &self.state {
            RecipeNodeState::None | RecipeNodeState::Folder { .. } => false,
            RecipeNodeState::Recipe { .. } => true,
        };
        RecipeMenuAction::menu(self.actions_emitter, has_recipe)
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        match &mut self.state {
            RecipeNodeState::None | RecipeNodeState::Folder { .. } => {
                vec![]
            }
            RecipeNodeState::Recipe { display, .. } => {
                vec![display.to_child_mut()]
            }
        }
    }
}

impl Draw for RecipeDetail {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        // Render outermost block
        let title = ViewContext::add_binding_hint(
            match &self.state {
                RecipeNodeState::Folder { .. } => "Folder",
                RecipeNodeState::Recipe { .. } | RecipeNodeState::None => {
                    "Recipe"
                }
            },
            Action::SelectTopPane,
        );
        let mut block = Pane {
            title: &title,
            has_focus: metadata.has_focus(),
        }
        .generate();
        let inner_area = block.inner(metadata.area());

        // Include the folder/recipe ID in the header
        match &self.state {
            RecipeNodeState::None => {}
            RecipeNodeState::Folder { id, .. }
            | RecipeNodeState::Recipe { id, .. } => {
                block = block.title(
                    Line::from(id.to_string())
                        .alignment(Alignment::Right)
                        .style(ViewContext::styles().text.title),
                );
            }
        }
        canvas.render_widget(block, metadata.area());

        match &self.state {
            RecipeNodeState::None => canvas.render_widget(
                Text::from(vec![
                    "No recipes defined; add one to your collection".into(),
                    doc_link("api/request_collection/request_recipe").into(),
                ]),
                inner_area,
            ),
            RecipeNodeState::Folder { id } => {
                // Folder *should* always be defined
                if let Some(folder) =
                    ViewContext::collection().recipes.get_folder(id)
                {
                    // Recompute the text on every render. This is a bit simpler
                    // than storing it, and shouldn't be too expensive
                    canvas.render_widget(FolderTree { folder }, inner_area);
                }
            }
            RecipeNodeState::Recipe { display, .. } => {
                canvas.draw(display, (), inner_area, true);
            }
        }
    }
}

/// Display state for a recipe node
#[derive(Debug, Default)]
enum RecipeNodeState {
    /// Recipe list is empty
    #[default]
    None,
    /// Folder is selected
    Folder { id: RecipeId },
    /// Recipe is selected
    Recipe {
        id: RecipeId,
        /// Interactive recipe previews
        display: RecipeDisplay,
    },
}

/// Items in the actions popup menu. This is used by both the list and detail
/// components. Handling is stateless so it's shared between them.
#[derive(Debug)]
#[expect(clippy::enum_variant_names)]
pub enum RecipeMenuAction {
    CopyUrl,
    CopyAsCli,
    CopyAsCurl,
    CopyAsPython,
}

impl RecipeMenuAction {
    /// Build a list of these actions
    pub fn menu(emitter: Emitter<Self>, has_recipe: bool) -> Vec<MenuItem> {
        vec![MenuItem::Group {
            name: "Copy".into(),
            children: vec![
                emitter.menu(Self::CopyUrl, "URL").enable(has_recipe).into(),
                emitter
                    .menu(Self::CopyAsCli, "as CLI")
                    .enable(has_recipe)
                    .into(),
                emitter
                    .menu(Self::CopyAsCurl, "as cURL")
                    .enable(has_recipe)
                    .into(),
                emitter
                    .menu(Self::CopyAsPython, "as Python")
                    .enable(has_recipe)
                    .into(),
            ],
        }]
    }

    /// Send a global message/event to handle this event
    pub fn handle(self) {
        fn copy(target: RecipeCopyTarget) {
            ViewContext::push_message(Message::CopyRecipe(target));
        }

        match self {
            Self::CopyUrl => copy(RecipeCopyTarget::Url),
            Self::CopyAsCli => copy(RecipeCopyTarget::Cli),
            Self::CopyAsCurl => copy(RecipeCopyTarget::Curl),
            Self::CopyAsPython => copy(RecipeCopyTarget::Python),
        }
    }
}

/// Widget to display a folder and its children as a tree
struct FolderTree<'a> {
    folder: &'a Folder,
}

impl Widget for FolderTree<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
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

        // We could probably be more efficient and write as we go instead of
        // accumulating into Text, but whatever
        let mut lines = vec![self.folder.name().into()];
        add_lines(&mut lines, self.folder, &mut Vec::new());
        let text: Text = lines.into();
        text.render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{TestTerminal, terminal};
    use rstest::rstest;
    use slumber_core::{collection::Recipe, test_util::by_id};
    use slumber_util::{Factory, yaml::SourceLocation};

    #[rstest]
    fn test_folder_tree(#[with(14, 10)] terminal: TestTerminal) {
        let folder = Folder {
            id: "1f".into(),
            location: SourceLocation::default(),
            name: None,
            children: by_id([
                RecipeNode::Recipe(Recipe::factory("1.1r")),
                RecipeNode::Recipe(Recipe::factory("1.2r")),
                // Nested folder
                RecipeNode::Folder(Folder {
                    id: "1.3f".into(),
                    location: SourceLocation::default(),
                    name: None,
                    children: by_id([RecipeNode::Recipe(Recipe::factory(
                        "1.3.1r",
                    ))]),
                }),
                // Empty folder
                RecipeNode::Folder(Folder {
                    id: "1.4f".into(),
                    location: SourceLocation::default(),
                    name: None,
                    children: Default::default(),
                }),
                // End with a nested folder to make sure the leftmost
                // decorations don't appear
                RecipeNode::Folder(Folder {
                    id: "1.5f".into(),
                    location: SourceLocation::default(),
                    name: None,
                    children: by_id([
                        RecipeNode::Recipe(Recipe::factory("1.5.1r")),
                        RecipeNode::Folder(Folder {
                            id: "1.5.2f".into(),
                            location: SourceLocation::default(),
                            name: None,
                            children: by_id([RecipeNode::Recipe(
                                Recipe::factory("1.5.2.1r"),
                            )]),
                        }),
                    ]),
                }),
            ]),
        };

        terminal.draw(|f| {
            f.render_widget(FolderTree { folder: &folder }, f.area());
        });

        terminal.assert_buffer_lines([
            "1f",
            "├─1.1r",
            "├─1.2r",
            "├─1.3f",
            "│ └─1.3.1r",
            "├─1.4f",
            "└─1.5f",
            "  ├─1.5.1r",
            "  └─1.5.2f",
            "    └─1.5.2.1r",
        ]);
    }
}
