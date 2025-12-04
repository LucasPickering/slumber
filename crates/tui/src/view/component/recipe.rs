//! Recipe list and detail panes

mod authentication;
mod body;
mod override_template;
mod prompt_form;
mod recipe;
mod table;
mod url;

use crate::{
    context::TuiContext,
    http::RequestConfig,
    message::{Message, RecipeCopyTarget},
    util::{PersistentKey, PersistentStore},
    view::{
        Component, Generate, ViewContext,
        common::{Pane, actions::MenuItem},
        component::{
            Canvas, ComponentId, Draw, DrawMetadata,
            internal::{Child, ToChild},
            recipe::{prompt_form::PromptForm, recipe::RecipeDisplay},
            sidebar_list::{
                SidebarList, SidebarListEvent, SidebarListItem,
                SidebarListProps, SidebarListState,
            },
        },
        context::UpdateContext,
        event::{Emitter, Event, EventMatch, ToEmitter},
        state::StateCell,
    },
};
use itertools::{Itertools, Position};
use ratatui::{
    layout::Alignment,
    text::{Line, Text},
};
use serde::Serialize;
use slumber_config::Action;
use slumber_core::{
    collection::{
        Folder, HasId, ProfileId, RecipeId, RecipeLookupKey, RecipeNode,
        RecipeNodeType,
    },
    render::Prompt,
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
        RecipeMenuAction::menu(
            self.actions_emitter,
            self.list.selected().is_some_and(RecipeListItem::is_recipe),
        )
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

    fn is_recipe(&self) -> bool {
        matches!(self.kind, RecipeNodeType::Recipe)
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
#[derive(Debug, Default)]
pub struct RecipeDetail {
    id: ComponentId,
    /// Emitter for menu actions
    actions_emitter: Emitter<RecipeMenuAction>,
    /// All UI state derived from the recipe is stored together, and reset when
    /// the recipe or profile changes
    recipe_state: StateCell<RecipeStateKey, Option<RecipeDisplay>>,
    /// A form for answering prompts from the request render engine. This
    /// receives prompts from the render tasks via messages, and whenever there
    /// is at least one prompt, we show this in place of the recipe node detail
    prompt_form: PromptForm,
}

#[derive(Debug)]
pub struct RecipePaneProps<'a> {
    /// ID of the recipe *or* folder selected
    pub selected_recipe_node: Option<&'a RecipeNode>,
    pub selected_profile_id: Option<&'a ProfileId>,
}

impl RecipeDetail {
    /// Get a definition of the request that should be sent from the current
    /// recipe settings
    pub fn request_config(&self) -> Option<RequestConfig> {
        let state_key = self.recipe_state.borrow_key();
        let recipe_id = state_key.recipe_id.clone()?;
        let profile_id = state_key.selected_profile_id.clone();
        let recipe_state = self.recipe_state.borrow();
        let options = recipe_state.as_ref()?.build_options();
        Some(RequestConfig {
            profile_id,
            recipe_id,
            options,
        })
    }

    /// Prompt the user for input
    pub fn prompt(&mut self, prompt: Prompt) {
        self.prompt_form.prompt(prompt);
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
        RecipeMenuAction::menu(
            self.actions_emitter,
            self.recipe_state.borrow().is_some(),
        )
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![
            self.prompt_form.to_child_mut(),
            self.recipe_state.get_mut().to_child_mut(),
        ]
    }
}

impl<'a> Draw<RecipePaneProps<'a>> for RecipeDetail {
    fn draw(
        &self,
        canvas: &mut Canvas,
        props: RecipePaneProps<'a>,
        metadata: DrawMetadata,
    ) {
        let context = TuiContext::get();

        // Render outermost block
        let title = context.input_engine.add_hint(
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
        let mut block = block.generate();
        let inner_area = block.inner(metadata.area());

        // If there are prompts available, render that instead
        if self.prompt_form.has_prompts() {
            canvas.render_widget(block, metadata.area());
            canvas.draw(&self.prompt_form, (), inner_area, true);
            return;
        }

        // Include the folder/recipe ID in the header
        if let Some(node) = props.selected_recipe_node {
            block = block.title(
                Line::from(node.id().to_string())
                    .alignment(Alignment::Right)
                    .style(context.styles.text.title),
            );
        }
        canvas.render_widget(block, metadata.area());

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
            || match props.selected_recipe_node {
                Some(RecipeNode::Recipe(recipe)) => {
                    Some(RecipeDisplay::new(recipe))
                }
                Some(RecipeNode::Folder(_)) | None => None,
            },
        );

        match props.selected_recipe_node {
            None => canvas.render_widget(
                Text::from(vec![
                    "No recipes defined; add one to your collection".into(),
                    doc_link("api/request_collection/request_recipe").into(),
                ]),
                inner_area,
            ),
            Some(RecipeNode::Folder(folder)) => {
                canvas.render_widget(folder.generate(), inner_area);
            }
            Some(RecipeNode::Recipe(_)) => {
                if let Some(recipe_state) = &*recipe_state {
                    canvas.draw(recipe_state, (), inner_area, true);
                }
            }
        }
    }
}

/// Template preview state will be recalculated when any of these fields change
#[derive(Clone, Debug, Default, PartialEq)]
struct RecipeStateKey {
    selected_profile_id: Option<ProfileId>,
    recipe_id: Option<RecipeId>,
}

/// Items in the actions popup menu. This is used by both the list and detail
/// components. Handling is stateless so it's shared between them.
#[derive(Copy, Clone, Debug)]
enum RecipeMenuAction {
    CopyUrl,
    CopyAsCli,
    CopyAsCurl,
    CopyAsPython,
    DeleteRecipe,
}

impl RecipeMenuAction {
    /// Build a list of these actions
    fn menu(emitter: Emitter<Self>, has_recipe: bool) -> Vec<MenuItem> {
        vec![
            MenuItem::Group {
                name: "Copy".into(),
                children: vec![
                    emitter
                        .menu(Self::CopyUrl, "URL")
                        .enable(has_recipe)
                        .into(),
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
            },
            emitter
                .menu(Self::DeleteRecipe, "Delete Requests")
                .enable(has_recipe)
                .into(),
        ]
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
            Self::DeleteRecipe => {
                ViewContext::push_event(Event::DeleteRecipeRequests);
            }
        }
    }
}

/// Render folder as a tree
impl Generate for &Folder {
    type Output<'this>
        = Text<'this>
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

        let mut lines = vec![self.name().into()];
        add_lines(&mut lines, self, &mut Vec::new());
        lines.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use slumber_core::{collection::Recipe, test_util::by_id};
    use slumber_util::{Factory, yaml::SourceLocation};

    #[test]
    fn test_folder_tree() {
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
