//! Recipe list

use crate::{
    collection::{Recipe, RecipeId},
    tui::{
        context::TuiContext,
        input::Action,
        view::{
            common::{list::List, Pane},
            draw::{Draw, Generate},
            event::{Event, EventHandler, UpdateContext},
            state::{
                persistence::{Persistable, Persistent, PersistentKey},
                select::{Dynamic, SelectState},
            },
            Component,
        },
    },
};
use ratatui::{prelude::Rect, Frame};

#[derive(Debug)]
pub struct RecipeListPane {
    recipes: Component<Persistent<SelectState<Dynamic, Recipe>>>,
}

pub struct RecipeListPaneProps {
    pub is_selected: bool,
}

impl RecipeListPane {
    pub fn new(recipes: Vec<Recipe>) -> Self {
        // When highlighting a new recipe, load it from the repo
        fn on_select(context: &mut UpdateContext, _: &mut Recipe) {
            context.queue_event(Event::HttpLoadRequest);
        }

        // Trigger a request on submit
        fn on_submit(context: &mut UpdateContext, _: &mut Recipe) {
            // Parent has to be responsible for actually sending the request
            // because it also needs access to the profile list state
            context.queue_event(Event::HttpSendRequest);
        }

        Self {
            recipes: Persistent::new(
                PersistentKey::RecipeId,
                SelectState::new(recipes)
                    .on_select(on_select)
                    .on_submit(on_submit),
            )
            .into(),
        }
    }

    pub fn recipes(&self) -> &SelectState<Dynamic, Recipe> {
        &self.recipes
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
impl Persistable for Recipe {
    type Persisted = RecipeId;

    fn get_persistent(&self) -> &Self::Persisted {
        &self.id
    }
}
