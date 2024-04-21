//! Recipe list

use crate::{
    collection::{Recipe, RecipeId, RecipeNode, RecipeTree},
    tui::{
        context::TuiContext,
        input::Action,
        view::{
            common::{list::List, Pane},
            draw::{Draw, Generate},
            event::{Event, EventHandler, EventQueue},
            state::{
                persistence::{Persistable, Persistent, PersistentKey},
                select::{Dynamic, SelectState},
            },
            Component,
        },
    },
};
use itertools::Itertools;
use ratatui::{prelude::Rect, text::Span, Frame};

#[derive(Debug)]
pub struct RecipeListPane {
    recipes: Component<Persistent<SelectState<Dynamic, RecipeListItem>>>,
}

pub struct RecipeListPaneProps {
    pub is_selected: bool,
}

/// Each folder/recipe in the list, plus metadata
#[derive(Debug)]
struct RecipeListItem {
    node: RecipeNode,
    depth: usize,
}

impl RecipeListPane {
    pub fn new(recipes: &RecipeTree) -> Self {
        // When highlighting a new recipe, load it from the repo
        fn on_select(_: &mut RecipeListItem) {
            // If a recipe isn't selected, this will do nothing
            EventQueue::push(Event::HttpLoadRequest);
        }

        // Flatten the tree into a list
        let recipes = recipes
            .iter()
            .map(|(lookup_key, node)| RecipeListItem {
                node: node.clone(),
                depth: lookup_key.len() - 1,
            })
            .collect_vec();

        Self {
            recipes: Persistent::new(
                PersistentKey::RecipeId,
                SelectState::new(recipes).on_select(on_select),
            )
            .into(),
        }
    }

    /// Which recipe in the recipe list is selected? `None` iff the list is
    /// empty OR a folder is selected.
    pub fn selected_recipe(&self) -> Option<&Recipe> {
        self.recipes
            .selected()
            .and_then(|list_item| list_item.node.recipe())
    }
}

impl EventHandler for RecipeListPane {
    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        vec![self.recipes.as_child()]
    }
}

impl Draw<RecipeListPaneProps> for RecipeListPane {
    fn draw(&self, frame: &mut Frame, props: RecipeListPaneProps, area: Rect) {
        self.recipes.set_area(area); // Needed for tracking cursor events
        let title = TuiContext::get()
            .input_engine
            .add_hint("Recipes", Action::SelectRecipeList);
        let list = List {
            block: Some(Pane {
                title: &title,
                is_focused: props.is_selected,
            }),
            list: &self.recipes,
        };
        frame.render_stateful_widget(
            list.generate(),
            area,
            &mut self.recipes.state_mut(),
        )
    }
}

/// Persist recipe by ID
impl Persistable for RecipeListItem {
    type Persisted = RecipeId;

    fn get_persistent(&self) -> &Self::Persisted {
        self.node.id()
    }
}

/// Needed for persistence loading
impl PartialEq<RecipeListItem> for RecipeId {
    fn eq(&self, other: &RecipeListItem) -> bool {
        self == other.node.id()
    }
}

impl Generate for &RecipeListItem {
    type Output<'this> = Span<'this> where Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        // Apply indentation based on folder depth
        let content = format!(
            "{indent:width$}{name}",
            indent = "",
            name = self.node.name(),
            width = self.depth
        );

        let theme = &TuiContext::get().theme;
        let style = match self.node {
            RecipeNode::Folder(_) => theme.recipe_list.folder,
            RecipeNode::Recipe(_) => theme.recipe_list.recipe,
        };

        Span {
            content: content.into(),
            style,
        }
    }
}
