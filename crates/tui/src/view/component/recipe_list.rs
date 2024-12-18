use crate::{
    context::TuiContext,
    view::{
        common::{actions::ActionsModal, list::List, modal::ModalHandle, Pane},
        component::recipe_pane::RecipeMenuAction,
        context::UpdateContext,
        draw::{Draw, DrawMetadata, Generate},
        event::{Child, Emitter, EmitterId, Event, EventHandler, Update},
        state::select::{SelectState, SelectStateEvent, SelectStateEventType},
        util::persistence::{Persisted, PersistedLazy},
        Component, ViewContext,
    },
};
use derive_more::{Deref, DerefMut};
use persisted::{PersistedKey, SingletonKey};
use ratatui::{text::Text, Frame};
use serde::{Deserialize, Serialize};
use slumber_config::Action;
use slumber_core::collection::{
    HasId, RecipeId, RecipeLookupKey, RecipeNodeType, RecipeTree,
};
use std::collections::HashSet;

/// List/tree of recipes and folders. This is mostly just a list, but with some
/// extra logic to allow expanding/collapsing nodes. This could be made into a
/// more generic component, but that adds abstraction that's not necessary
/// because this is the only tree in the app. For similar reasons, we don't use
/// the library tui-tree-widget, because it requires more abstraction that it
/// saves us in code.
///
/// This implementation leans heavily on the fact that all nodes in the tree
/// have a unique ID, which is another reason why it deserves its own
/// implementation.
#[derive(Debug)]
pub struct RecipeListPane {
    emitter_id: EmitterId,
    /// The visible list of items is tracked using normal list state, so we can
    /// easily re-use existing logic. We'll rebuild this any time a folder is
    /// expanded/collapsed (i.e whenever the list of items changes)
    select: Component<
        PersistedLazy<SelectedRecipeKey, SelectState<RecipeListItem>>,
    >,
    /// Set of all folders that are collapsed
    /// Invariant: No recipes, only folders
    ///
    /// We persist the entire set. This will accrue removed folders over time
    /// (if they were collapsed at the time of deletion). That isn't really an
    /// issue though, it just means it'll be pre-collapsed if the user ever
    /// adds the folder back. Not worth working around.
    collapsed: Persisted<SingletonKey<Collapsed>>,
    actions_handle: ModalHandle<ActionsModal<RecipeMenuAction>>,
}

/// Persisted key for the ID of the selected recipe
#[derive(Debug, Serialize, PersistedKey)]
#[persisted(Option<RecipeId>)]
struct SelectedRecipeKey;

impl RecipeListPane {
    pub fn new(recipes: &RecipeTree) -> Self {
        // This clone is unfortunate, but we can't hold onto a reference to the
        // recipes
        let collapsed: Persisted<SingletonKey<Collapsed>> =
            Persisted::default();
        let persistent = PersistedLazy::new(
            SelectedRecipeKey,
            collapsed.build_select_state(recipes),
        );
        Self {
            emitter_id: EmitterId::new(),
            select: persistent.into(),
            collapsed,
            actions_handle: ModalHandle::default(),
        }
    }

    /// ID and kind of whatever recipe/folder in the list is selected. `None`
    /// iff the list is empty
    pub fn selected_node(&self) -> Option<(&RecipeId, RecipeNodeType)> {
        self.select
            .data()
            .selected()
            .map(|node| (&node.id, node.kind))
    }

    /// Set the currently selected folder as expanded/collapsed (or toggle it).
    /// If a folder is not selected, do nothing. Returns whether a change was
    /// made.
    fn set_selected_collapsed(&mut self, state: CollapseState) -> bool {
        let select = self.select.data_mut();
        let folder = select.selected().filter(|node| node.is_folder());
        let changed = if let Some(folder) = folder {
            let collapsed = &mut self.collapsed;
            match state {
                CollapseState::Expand => collapsed.get_mut().remove(&folder.id),
                CollapseState::Collapse => {
                    collapsed.get_mut().insert(folder.id.clone())
                }
                CollapseState::Toggle => {
                    if collapsed.contains(&folder.id) {
                        collapsed.get_mut().remove(&folder.id);
                    } else {
                        collapsed.get_mut().insert(folder.id.clone());
                    }
                    true
                }
            }
        } else {
            false
        };

        // If we changed the set of what is visible, rebuild the list state
        if changed {
            let mut new_select_state = self
                .collapsed
                .build_select_state(&ViewContext::collection().recipes);

            // Carry over the selection
            if let Some(selected) = select.selected() {
                new_select_state.select(selected.id());
            }
            *select.get_mut() = new_select_state;
        }

        changed
    }
}

impl EventHandler for RecipeListPane {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Update {
        if let Some(action) = event.action() {
            match action {
                Action::LeftClick => self.emit(RecipeListPaneEvent::Click),
                Action::Left => {
                    self.set_selected_collapsed(CollapseState::Collapse);
                }
                Action::Right => {
                    self.set_selected_collapsed(CollapseState::Expand);
                }
                Action::OpenActions => {
                    let recipe = self
                        .select
                        .data()
                        .selected()
                        .filter(|node| node.is_recipe());
                    let has_body = recipe
                        .map(|recipe| {
                            ViewContext::collection()
                                .recipes
                                .get_recipe(&recipe.id)
                                .and_then(|recipe| recipe.body.as_ref())
                                .is_some()
                        })
                        .unwrap_or(false);
                    self.actions_handle.open(ActionsModal::new(
                        RecipeMenuAction::disabled_actions(
                            recipe.is_some(),
                            has_body,
                        ),
                    ));
                }
                _ => return Update::Propagate(event),
            }
        } else if let Some(event) = self.select.emitted(&event) {
            match event {
                SelectStateEvent::Select(_) => {
                    // When highlighting a new recipe, load its most recent
                    // request from the DB. If a recipe isn't selected, this
                    // will do nothing
                    ViewContext::push_event(Event::HttpSelectRequest(None));
                }
                SelectStateEvent::Submit(_) => {}
                SelectStateEvent::Toggle(_) => {
                    self.set_selected_collapsed(CollapseState::Toggle);
                }
            }
        } else if let Some(menu_action) = self.actions_handle.emitted(&event) {
            // Menu actions are handled by the parent, so forward them
            self.emit(RecipeListPaneEvent::MenuAction(*menu_action));
        } else {
            return Update::Propagate(event);
        }

        Update::Consumed
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        vec![self.select.to_child_mut()]
    }
}

impl Draw for RecipeListPane {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        let context = TuiContext::get();

        let title = context
            .input_engine
            .add_hint("Recipes", Action::SelectRecipeList);
        let block = Pane {
            title: &title,
            has_focus: metadata.has_focus(),
        }
        .generate();
        let area = block.inner(metadata.area());
        frame.render_widget(block, metadata.area());

        self.select
            .draw(frame, List::from(&**self.select.data()), area, true);
    }
}

/// Notify parent when this pane is clicked
impl Emitter for RecipeListPane {
    type Emitted = RecipeListPaneEvent;

    fn id(&self) -> EmitterId {
        self.emitter_id
    }
}

/// Emitted event type for the recipe list pane
#[derive(Debug)]
pub enum RecipeListPaneEvent {
    Click,
    MenuAction(RecipeMenuAction),
}

/// Simplified version of [RecipeNode], to be used in the display tree. This
/// only stores whatever data is necessary to render the list
#[derive(Debug)]
struct RecipeListItem {
    id: RecipeId,
    name: String,
    kind: RecipeNodeType,
    depth: usize,
    collapsed: bool,
}

impl RecipeListItem {
    fn is_folder(&self) -> bool {
        matches!(self.kind, RecipeNodeType::Folder)
    }

    fn is_recipe(&self) -> bool {
        matches!(self.kind, RecipeNodeType::Recipe)
    }
}

impl HasId for RecipeListItem {
    type Id = RecipeId;

    fn id(&self) -> &Self::Id {
        &self.id
    }

    fn set_id(&mut self, id: Self::Id) {
        self.id = id;
    }
}

impl PartialEq<RecipeListItem> for RecipeId {
    fn eq(&self, item: &RecipeListItem) -> bool {
        self == item.id()
    }
}

impl<'a> Generate for &'a RecipeListItem {
    type Output<'this> = Text<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
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

/// Set of collapsed folders. Newtype allows us to encapsulate some extra
/// functionality
#[derive(Debug, Default, Deref, DerefMut, Serialize, Deserialize)]
#[serde(transparent)]
struct Collapsed(HashSet<RecipeId>);

/// Ternary state for modifying node collapse state
enum CollapseState {
    Expand,
    Collapse,
    Toggle,
}

impl Collapsed {
    /// Is this specific folder collapsed?
    fn is_collapsed(&self, folder_id: &RecipeId) -> bool {
        self.0.contains(folder_id)
    }

    /// Is the given node visible? This takes lookup key so it can check all
    /// ancestors for visibility too.
    fn is_visible(&self, lookup_key: &RecipeLookupKey) -> bool {
        // If any ancestors are collapsed, this is *not* visible
        let [ancestors @ .., _] = lookup_key.as_slice() else {
            panic!("Recipe lookup key cannot be empty")
        };
        !ancestors.iter().any(|id| self.is_collapsed(id))
    }

    /// Construct select list based on which nodes are currently visible
    fn build_select_state(
        &self,
        recipes: &RecipeTree,
    ) -> SelectState<RecipeListItem> {
        let items = recipes
            .iter()
            // Filter out hidden nodes
            .filter(|(lookup_key, _)| self.is_visible(lookup_key))
            .map(|(lookup_key, node)| RecipeListItem {
                id: node.id().clone(),
                name: node.name().to_owned(),
                kind: node.into(),
                collapsed: self.is_collapsed(node.id()),
                depth: lookup_key.as_slice().len() - 1,
            })
            .collect();

        SelectState::builder(items)
            .subscribe([
                SelectStateEventType::Select,
                SelectStateEventType::Toggle,
            ])
            .build()
    }
}
