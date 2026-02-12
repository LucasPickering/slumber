use crate::view::{
    Component, Generate, ViewContext,
    common::{
        Pane,
        actions::MenuItem,
        select::{FilterItem, Select, SelectEventKind, SelectListProps},
        text_box::{TextBox, TextBoxEvent, TextBoxProps},
    },
    component::{
        Canvas, ComponentId, Draw, DrawMetadata,
        internal::{Child, ToChild},
        misc::{SidebarEvent, SidebarFormat, SidebarProps},
        recipe_detail::RecipeMenuAction,
    },
    context::UpdateContext,
    event::{BroadcastEvent, Emitter, Event, EventMatch, ToEmitter},
    persistent::{PersistentKey, PersistentStore},
};
use ratatui::{
    layout::{Constraint, Layout},
    text::{Span, Text},
};
use serde::Serialize;
use slumber_config::Action;
use slumber_core::collection::{
    HasId, RecipeId, RecipeLookupKey, RecipeNode, RecipeNodeType,
};
use std::{borrow::Cow, collections::HashSet};

/// Collapsible tree-list of folders and recipes
#[derive(Debug)]
pub struct RecipeList {
    id: ComponentId,
    /// Emitter for open/close events
    emitter: Emitter<SidebarEvent>,
    /// Emitter for menu actions
    actions_emitter: Emitter<RecipeMenuAction>,
    /// Recipe/folder list
    select: Select<RecipeListItem>,
    /// Set of all folders that are collapsed
    collapsed: Collapsed,
    /// Text box for filtering down items in the list
    filter: TextBox,
    /// Is the user typing in the filter box? User has to explicitly grab focus
    /// on the box to start typing
    filter_focused: bool,
}

impl RecipeList {
    pub fn new() -> Self {
        let collapsed =
            Collapsed(PersistentStore::get(&CollapsedKey).unwrap_or_default());
        let filter = TextBox::default()
            .placeholder(format!(
                "{binding} to filter",
                binding = ViewContext::binding_display(Action::Search)
            ))
            .subscribe([
                TextBoxEvent::Cancel,
                TextBoxEvent::Change,
                TextBoxEvent::Submit,
            ]);
        let select = Self::build_select(&collapsed, filter.text());

        Self {
            id: ComponentId::default(),
            emitter: Emitter::default(),
            actions_emitter: Emitter::default(),
            collapsed,
            select,
            filter,
            filter_focused: false,
        }
    }

    /// ID and type of the selected recipe node, or `None` if the list is empty
    pub fn selected(&self) -> Option<(&RecipeId, RecipeNodeType)> {
        self.select.selected().map(|item| (&item.id, item.kind()))
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
        if let Some(selected) = self.select.selected()
            && selected.is_folder()
        {
            // We have to clone the folder ID to drop the ref to self.list
            let folder_id = selected.id.clone();
            if self.collapsed.collapse(folder_id, collapse) {
                self.rebuild_select();
            }
        }
    }

    /// Rebuild the select state after a collapse event or filter change
    fn rebuild_select(&mut self) {
        self.select = Self::build_select(&self.collapsed, self.filter.text());
    }

    /// Build/rebuild a select based on the item list
    fn build_select(
        collapsed: &Collapsed,
        filter: &str,
    ) -> Select<RecipeListItem> {
        let recipes = &ViewContext::collection().recipes;

        let items = recipes
            .iter()
            // Remove collapsed recipes
            .filter(|(lookup_key, _)| collapsed.is_visible(lookup_key))
            .map(|(lookup_key, node)| {
                RecipeListItem::new(
                    node,
                    collapsed.is_collapsed(node.id()),
                    lookup_key.depth(),
                )
            })
            .collect();

        Select::builder(items)
            .subscribe([SelectEventKind::Select])
            .filter(filter)
            .persisted(&SelectedRecipeKey)
            .build()
    }
}

impl Component for RecipeList {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            .click(|_, _| self.emitter.emit(SidebarEvent::Open))
            .action(|action, propagate| match action {
                Action::Left => self.collapse_selected(Collapse::Collapse),
                Action::Right => self.collapse_selected(Collapse::Expand),
                Action::Toggle => self.collapse_selected(Collapse::Toggle),
                Action::Search => self.filter_focused = true,
                _ => propagate.set(),
            })
            // Emitted events from select
            .emitted(self.select.to_emitter(), |event| match event.kind {
                SelectEventKind::Select => {
                    // Let everyone know the selected recipe changed
                    ViewContext::push_message(BroadcastEvent::SelectedRecipe(
                        self.selected_recipe_id().cloned(),
                    ));
                }
                // Ignore submission - it should send a request
                SelectEventKind::Submit | SelectEventKind::Toggle => {}
            })
            // Emitted events from filter
            .emitted(self.filter.to_emitter(), |event| match event {
                TextBoxEvent::Change => self.rebuild_select(),
                TextBoxEvent::Cancel | TextBoxEvent::Submit => {
                    self.filter_focused = false;
                }
            })
            .emitted(self.actions_emitter, RecipeMenuAction::handle)
    }

    fn menu(&self) -> Vec<MenuItem> {
        let has_recipe =
            self.select.selected().is_some_and(|item| match item.kind {
                RecipeNodeType::Folder => false,
                RecipeNodeType::Recipe => true,
            });
        RecipeMenuAction::menu(self.actions_emitter, has_recipe)
    }

    fn persist(&self, store: &mut PersistentStore) {
        store.set_opt(
            &SelectedRecipeKey,
            self.select.selected().map(|item| &item.id),
        );
        store.set(&CollapsedKey, &self.collapsed.0);
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.select.to_child(), self.filter.to_child()]
    }
}

impl Draw<SidebarProps> for RecipeList {
    fn draw(
        &self,
        canvas: &mut Canvas,
        props: SidebarProps,
        metadata: DrawMetadata,
    ) {
        // Both formats use a pane outline
        let title = ViewContext::add_binding_hint("Recipe", Action::RecipeList);
        let block = Pane {
            title: &title,
            has_focus: metadata.has_focus(),
        }
        .generate();
        let area = block.inner(metadata.area());
        canvas.render_widget(block, metadata.area());

        match props.format {
            SidebarFormat::Header => {
                let value: Text = self
                    .select
                    .selected()
                    .map(|item| item.name.as_str().into())
                    .unwrap_or_else(|| "None".into());
                canvas.render_widget(value, area);
            }
            SidebarFormat::List => {
                // Expanded sidebar
                let [filter_area, list_area] = Layout::vertical([
                    Constraint::Length(1),
                    Constraint::Min(0),
                ])
                .areas(area);
                canvas.draw(
                    &self.filter,
                    TextBoxProps::default(),
                    filter_area,
                    self.filter_focused,
                );
                canvas.draw(
                    &self.select,
                    SelectListProps::pane(),
                    list_area,
                    true,
                );
            }
        }
    }
}

impl ToEmitter<SidebarEvent> for RecipeList {
    fn to_emitter(&self) -> Emitter<SidebarEvent> {
        self.emitter
    }
}

/// Set of all folders that are collapsed
/// Invariant: No recipes, only folders
///
/// We persist the entire set. This will accrue removed folders over time
/// (if they were collapsed at the time of deletion). That isn't really an
/// issue though, it just means it'll be pre-collapsed if the user ever
/// adds the folder back. Not worth working around.
#[derive(Debug)]
struct Collapsed(HashSet<RecipeId>);

impl Collapsed {
    /// Is the given folder collapsed?
    fn is_collapsed(&self, folder_id: &RecipeId) -> bool {
        self.0.contains(folder_id)
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
        match collapse {
            Collapse::Expand => self.0.remove(&folder_id),
            Collapse::Collapse => self.0.insert(folder_id),
            Collapse::Toggle => {
                if self.0.contains(&folder_id) {
                    self.0.remove(&folder_id);
                } else {
                    self.0.insert(folder_id);
                }
                true
            }
        }
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

    fn kind(&self) -> RecipeNodeType {
        self.kind
    }

    fn is_folder(&self) -> bool {
        matches!(self.kind, RecipeNodeType::Folder)
    }
}

// For row selection
impl PartialEq<RecipeId> for RecipeListItem {
    fn eq(&self, id: &RecipeId) -> bool {
        &self.id == id
    }
}

impl FilterItem for RecipeListItem {
    fn search_terms(&self) -> impl IntoIterator<Item = Cow<'_, str>> {
        self.search_terms.iter().map(Cow::from)
    }
}

impl Generate for &RecipeListItem {
    type Output<'this>
        = Span<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
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
