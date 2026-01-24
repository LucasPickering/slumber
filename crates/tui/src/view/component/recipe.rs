//! Recipe list and detail panes

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
            recipe::recipe::RecipeDisplay,
            sidebar_list::{
                SidebarList, SidebarListEvent, SidebarListItem,
                SidebarListProps, SidebarListState,
            },
        },
        context::UpdateContext,
        event::{Emitter, Event, EventMatch, ToEmitter},
        persistent::{PersistentKey, PersistentStore},
    },
};
use itertools::{Itertools, Position};
use ratatui::{
    layout::Alignment,
    prelude::{Buffer, Rect},
    text::{Line, Text},
    widgets::Widget,
};
use serde::Serialize;
use slumber_config::Action;
use slumber_core::{
    collection::{
        Folder, HasId, RecipeId, RecipeLookupKey, RecipeNode, RecipeNodeType,
    },
    http::BuildOptions,
};
use slumber_util::doc_link;
use std::{borrow::Cow, collections::HashSet};

/// Wrapper for [SidebarList] that provides recipe-specific behavior. The recipe
/// list is actually a tree with collapsible nodes.
#[derive(Debug)]
pub struct RecipeList {
    id: ComponentId,
    /// Inner list state management
    list: SidebarList<RecipeListState>,
    /// Emitter for menu actions
    actions_emitter: Emitter<RecipeMenuAction>,
}

impl RecipeList {
    /// ID and type of the selected recipe node, or `None` if the list is empty
    pub fn selected(&self) -> Option<(&RecipeId, RecipeNodeType)> {
        self.list.selected().map(|item| (item.id(), item.kind()))
    }

    /// Which recipe in the recipe list is selected? `None` iff the list is
    /// empty OR a folder is selected.
    pub fn selected_recipe_id(&self) -> Option<&RecipeId> {
        self.selected().and_then(|(id, kind)| {
            if matches!(kind, RecipeNodeType::Recipe) {
                Some(id)
            } else {
                None
            }
        })
    }

    /// Modify expand/collapse state on the selected node
    fn collapse_selected(&mut self, collapse: Collapse) {
        if let Some(selected) = self.list.selected()
            && selected.is_folder()
        {
            // We have to clone the folder ID to drop the ref to self.list
            let folder_id = selected.id.clone();
            if self.list.state_mut().collapse(folder_id, collapse) {
                self.list.rebuild_select();
            }
        }
    }
}

impl Default for RecipeList {
    fn default() -> Self {
        let collapsed = PersistentStore::get(&CollapsedKey).unwrap_or_default();
        let state = RecipeListState { collapsed };
        Self {
            id: ComponentId::default(),
            list: SidebarList::new(state),
            actions_emitter: Emitter::default(),
        }
    }
}

impl Component for RecipeList {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            .action(|action, propagate| match action {
                // For lists with collapsible groups, handle collapse/expand.
                // As of now there's only one list that uses this (recipe), but
                // we handle it here because it simplifies the control flow
                Action::Left => self.collapse_selected(Collapse::Collapse),
                Action::Right => self.collapse_selected(Collapse::Expand),
                Action::Toggle => self.collapse_selected(Collapse::Toggle),

                _ => propagate.set(),
            })
            .emitted(self.actions_emitter, RecipeMenuAction::handle)
    }

    fn menu(&self) -> Vec<MenuItem> {
        let has_recipe =
            self.list.selected().is_some_and(|item| match item.kind {
                RecipeNodeType::Folder => false,
                RecipeNodeType::Recipe => true,
            });
        RecipeMenuAction::menu(self.actions_emitter, has_recipe)
    }

    fn persist(&self, store: &mut PersistentStore) {
        store.set(&CollapsedKey, &self.list.state().collapsed);
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.list.to_child_mut()]
    }
}

impl Draw<SidebarListProps> for RecipeList {
    fn draw(
        &self,
        canvas: &mut Canvas,
        props: SidebarListProps,
        metadata: DrawMetadata,
    ) {
        canvas.draw(&self.list, props, metadata.area(), metadata.has_focus());
    }
}

impl ToEmitter<SidebarListEvent> for RecipeList {
    fn to_emitter(&self) -> Emitter<SidebarListEvent> {
        self.list.to_emitter()
    }
}

/// State for a list/tree of recipes and folders. This is state for
/// [SidebarList].
///
/// Collapse state has to be stored in here instead of [RecipeList] because it
/// needs to be accessible when the select list is built, which happens in
/// [RecipeListState::items].
#[derive(Debug)]
struct RecipeListState {
    /// Set of all folders that are collapsed
    /// Invariant: No recipes, only folders
    ///
    /// We persist the entire set. This will accrue removed folders over time
    /// (if they were collapsed at the time of deletion). That isn't really an
    /// issue though, it just means it'll be pre-collapsed if the user ever
    /// adds the folder back. Not worth working around.
    collapsed: HashSet<RecipeId>,
}

impl RecipeListState {
    /// Is the given folder collapsed?
    fn is_collapsed(&self, folder_id: &RecipeId) -> bool {
        self.collapsed.contains(folder_id)
    }

    /// Is the given node visible? This takes lookup key so it can check all
    /// ancestors for visibility too.
    fn is_visible(&self, lookup_key: &RecipeLookupKey) -> bool {
        // If any ancestors are collapsed, this is *not* visible
        !lookup_key
            .ancestors()
            .iter()
            .any(|id| self.is_collapsed(id))
    }

    /// Set the currently selected folder as expanded/collapsed (or toggle it).
    /// Returns whether a change was made.
    fn collapse(&mut self, folder_id: RecipeId, collapse: Collapse) -> bool {
        let collapsed = &mut self.collapsed;
        match collapse {
            Collapse::Expand => collapsed.remove(&folder_id),
            Collapse::Collapse => collapsed.insert(folder_id),
            Collapse::Toggle => {
                if collapsed.contains(&folder_id) {
                    collapsed.remove(&folder_id);
                } else {
                    collapsed.insert(folder_id);
                }
                true
            }
        }
    }
}

impl SidebarListState for RecipeListState {
    const TITLE: &str = "Recipe";
    const ACTION: Action = Action::SelectRecipeList;

    type Item = RecipeListItem;
    type PersistentKey = SelectedRecipeKey;

    fn persistent_key(&self) -> Self::PersistentKey {
        SelectedRecipeKey
    }

    fn items(&self) -> Vec<Self::Item> {
        let recipes = &ViewContext::collection().recipes;

        // No filter - calculate visible nodes based on collapsed state
        recipes
            .iter()
            .filter(|(lookup_key, _)| self.is_visible(lookup_key))
            .map(|(lookup_key, node)| {
                RecipeListItem::new(
                    node,
                    self.is_collapsed(node.id()),
                    lookup_key.depth(),
                )
            })
            .collect()
    }
}

/// Simplified version of [RecipeNode], to be used in the display tree. This
/// only stores whatever data is necessary to render the list
#[derive(Debug)]
struct RecipeListItem {
    id: RecipeId,
    name: String,
    /// The name of this item and *all* of its children, grandchildren, etc.For
    /// This is used during filtering, so that a folder always shows when any
    /// of its children match. This duplicates a lot of strings in the recipe
    /// tree, but the overall size should be very low so it has no meaningful
    /// impact.
    search_terms: Vec<String>,
    kind: RecipeNodeType,
    depth: usize,
    collapsed: bool,
}

impl RecipeListItem {
    fn new(node: &RecipeNode, collapsed: bool, depth: usize) -> Self {
        fn add_search_terms(terms: &mut Vec<String>, node: &RecipeNode) {
            terms.push(node.name().to_owned());
            if let RecipeNode::Folder(folder) = node {
                for child in folder.children.values() {
                    // Recursion!
                    add_search_terms(terms, child);
                }
            }
        }

        let mut search_terms = vec![];
        add_search_terms(&mut search_terms, node);

        Self {
            id: node.id().clone(),
            name: node.name().to_owned(),
            search_terms,
            kind: node.into(),
            collapsed,
            depth,
        }
    }

    pub fn kind(&self) -> RecipeNodeType {
        self.kind
    }

    fn is_folder(&self) -> bool {
        matches!(self.kind, RecipeNodeType::Folder)
    }
}

impl SidebarListItem for RecipeListItem {
    type Id = RecipeId;

    fn id(&self) -> &Self::Id {
        &self.id
    }

    fn display_header(&self) -> Cow<'_, str> {
        self.name.as_str().into()
    }

    fn display_list(&self) -> Cow<'_, str> {
        // Add indentation and icons for list display
        let icon = match self.kind {
            RecipeNodeType::Folder if self.collapsed => "▶",
            RecipeNodeType::Folder => "▼",
            RecipeNodeType::Recipe => "",
        };

        // Apply indentation
        format!(
            "{indent:width$}{icon}{name}",
            indent = "",
            name = self.name,
            width = self.depth
        )
        .into()
    }

    fn filter_terms(&self) -> Vec<Cow<'_, str>> {
        self.search_terms
            .iter()
            .map(|term| term.as_str().into())
            .collect()
    }
}

/// Ternary action for modifying node collapse state
enum Collapse {
    /// If the selected node is collapsed, expand it
    Expand,
    /// If the selected node is expanded, collapse it
    #[expect(clippy::enum_variant_names)]
    Collapse,
    /// If the selected node is collapsed, expand it. If it's expanded, collapse
    /// it.
    Toggle,
}

/// Persisted key for the ID of the selected recipe
#[derive(Debug, Serialize)]
struct SelectedRecipeKey;

impl PersistentKey for SelectedRecipeKey {
    // Intentionally don't persist None. That's only possible if the recipe map
    // is empty. If it is, we're forced into None. If not, we want to default to
    // the first recipe.
    type Value = RecipeId;
}

/// Persistence key for collapsed state
#[derive(Debug, Default, Serialize)]
struct CollapsedKey;

impl PersistentKey for CollapsedKey {
    type Value = HashSet<RecipeId>;
}

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
enum RecipeMenuAction {
    CopyUrl,
    CopyAsCli,
    CopyAsCurl,
    CopyAsPython,
}

impl RecipeMenuAction {
    /// Build a list of these actions
    fn menu(emitter: Emitter<Self>, has_recipe: bool) -> Vec<MenuItem> {
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
    fn handle(self) {
        fn copy(target: RecipeCopyTarget) {
            ViewContext::send_message(Message::CopyRecipe(target));
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
