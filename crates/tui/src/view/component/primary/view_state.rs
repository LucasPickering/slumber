use serde::{Deserialize, Serialize};
use strum::{EnumIter, IntoEnumIterator};

/// Which panes are visible in the primary view?
///
/// This serves as a state machine to manage transitions between various
/// possible states of the primary view. It defines which panes are visible.
/// Invalid view states are unrepresentable with this type.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewState {
    /// Which panes should be shown?
    layout: PrimaryLayout,
    /// If `true`, the selected pane should take up the entire screen, and
    /// other panes are not visible.
    fullscreen: bool,
}

impl ViewState {
    /// Get the current layout
    pub fn layout(&self) -> PrimaryLayout {
        self.layout
    }

    /// Open the profile list in the sidebar
    pub fn open_profile_list(&mut self) {
        self.modify_layout(|layout| {
            *layout = PrimaryLayout::Profile(ProfileSelectPane::List);
        });
    }

    /// Open the recipe list in the sidebar
    pub fn open_recipe_list(&mut self) {
        self.modify_layout(|layout| {
            *layout = PrimaryLayout::Recipe(RecipeSelectPane::List);
        });
    }

    /// Are we in a layout with the sidebar open?
    pub fn is_sidebar_open(&self) -> bool {
        match self.layout {
            PrimaryLayout::Default(_) => false,
            PrimaryLayout::Profile(_) | PrimaryLayout::Recipe(_) => true,
        }
    }

    /// Close the sidebar and return to the default view
    pub fn close_sidebar(&mut self) {
        self.layout = PrimaryLayout::Default(DefaultPane::Recipe);
    }

    /// Select the previous pane in the cycle
    pub fn previous_pane(&mut self) {
        fn previous<T: PartialEq + IntoEnumIterator>(value: T) -> T {
            after(T::iter().rev(), value)
        }
        self.modify_layout(|layout| match layout {
            PrimaryLayout::Default(pane) => *pane = previous(*pane),
            PrimaryLayout::Profile(pane) => *pane = previous(*pane),
            PrimaryLayout::Recipe(pane) => *pane = previous(*pane),
        });
    }

    /// Select the next pane in the cycle
    pub fn next_pane(&mut self) {
        fn next<T: PartialEq + IntoEnumIterator>(value: T) -> T {
            after(T::iter(), value)
        }
        self.modify_layout(|layout| match layout {
            PrimaryLayout::Default(pane) => *pane = next(*pane),
            PrimaryLayout::Profile(pane) => *pane = next(*pane),
            PrimaryLayout::Recipe(pane) => *pane = next(*pane),
        });
    }

    /// Move focus to the upper pane in the layout
    pub fn select_top_pane(&mut self) {
        self.modify_layout(|layout| match layout {
            PrimaryLayout::Default(pane) => *pane = DefaultPane::Recipe,
            PrimaryLayout::Profile(pane) => *pane = ProfileSelectPane::Recipe,
            PrimaryLayout::Recipe(pane) => *pane = RecipeSelectPane::Recipe,
        });
    }

    /// Move focus to the lower pane in the layout
    pub fn select_bottom_pane(&mut self) {
        self.modify_layout(|layout| match layout {
            PrimaryLayout::Default(pane) => *pane = DefaultPane::Exchange,
            PrimaryLayout::Profile(pane) => *pane = ProfileSelectPane::Profile,
            PrimaryLayout::Recipe(pane) => *pane = RecipeSelectPane::Exchange,
        });
    }

    /// Move focus to the Recipe pane
    pub fn select_recipe_pane(&mut self) {
        self.modify_layout(|layout| match layout {
            PrimaryLayout::Default(pane) => *pane = DefaultPane::Recipe,
            PrimaryLayout::Profile(pane) => *pane = ProfileSelectPane::Recipe,
            PrimaryLayout::Recipe(pane) => *pane = RecipeSelectPane::Recipe,
        });
    }

    /// Move focus to the Profile pane. If it's not in this view, do nothing
    pub fn select_profile_pane(&mut self) {
        self.modify_layout(|layout| match layout {
            PrimaryLayout::Profile(pane) => *pane = ProfileSelectPane::Profile,
            // Profile pane isn't visible in these layouts
            PrimaryLayout::Default(_) | PrimaryLayout::Recipe(_) => {}
        });
    }

    /// Move focus to the Exchange pane. If it's not in this view, do nothing
    pub fn select_exchange_pane(&mut self) {
        self.modify_layout(|layout| match layout {
            PrimaryLayout::Default(pane) => *pane = DefaultPane::Exchange,
            PrimaryLayout::Profile(_) => {} // Pane not visible
            PrimaryLayout::Recipe(pane) => *pane = RecipeSelectPane::Exchange,
        });
    }

    /// Is the selected pane fullscreened?
    pub fn is_fullscreen(&self) -> bool {
        self.fullscreen
    }

    /// Enter/exit fullscreen mode for the currently selected pane
    pub fn toggle_fullscreen(&mut self) {
        self.fullscreen ^= true;
    }

    /// Exit fullscreen mode for the currently selected pane
    pub fn exit_fullscreen(&mut self) {
        self.fullscreen = false;
    }

    /// Modify the current layout with a closure. This encapsulates layout
    /// mutations so we can check for changes. If the layout ever changes, we
    /// exit fullscreen.
    fn modify_layout(&mut self, f: impl FnOnce(&mut PrimaryLayout)) {
        let old = self.layout;
        f(&mut self.layout);
        if self.layout != old {
            self.fullscreen = false;
        }
    }
}

impl Default for ViewState {
    fn default() -> Self {
        ViewState {
            layout: PrimaryLayout::Default(DefaultPane::Recipe),
            fullscreen: false,
        }
    }
}

/// Which panes are visible, and which one is selected?
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PrimaryLayout {
    /// Default layout: all sidebars are collapsed
    Default(DefaultPane),
    /// Profile list is open in the sidebar
    Profile(ProfileSelectPane),
    /// Recipe list is open in the sidebar
    Recipe(RecipeSelectPane),
}

/// Selectable pane in [PrimaryLayout::Default]
#[derive(Copy, Clone, Debug, PartialEq, EnumIter, Serialize, Deserialize)]
pub enum DefaultPane {
    Recipe,
    Exchange,
}

/// Selectable pane in [PrimaryLayout::Profile]
#[derive(Copy, Clone, Debug, PartialEq, EnumIter, Serialize, Deserialize)]
pub enum ProfileSelectPane {
    List,
    Recipe,
    Profile,
}

/// Selectable pane in [PrimaryLayout::Recipe]
#[derive(Copy, Clone, Debug, PartialEq, EnumIter, Serialize, Deserialize)]
pub enum RecipeSelectPane {
    List,
    Recipe,
    Exchange,
}

/// Get the item after `value` in the iterator
fn after<T: PartialEq + IntoEnumIterator>(
    iter: impl Clone + Iterator<Item = T>,
    value: T,
) -> T {
    iter.cycle()
        .skip_while(|v| *v != value)
        .nth(1) // Get one *after* the found value
        .expect("Iterator is cycled so it always returns")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    /// Transition into and out of fullscreen
    #[test]
    fn test_fullscreen() {
        let mut state = ViewState {
            layout: PrimaryLayout::Default(DefaultPane::Recipe),
            fullscreen: true,
        };

        // Toggle out
        state.toggle_fullscreen();
        assert!(!state.fullscreen);

        // Toggle back in
        state.toggle_fullscreen();
        assert!(state.fullscreen);

        // Exit
        state.exit_fullscreen();
        assert!(!state.fullscreen);

        // Exit again does nothing
        state.exit_fullscreen();
        assert!(!state.fullscreen);
    }

    /// Changing pane focus should exit fullscreen
    #[rstest]
    #[case::open_profile_list(
        PrimaryLayout::Default(DefaultPane::Recipe),
        ViewState::open_profile_list
    )]
    #[case::open_recipe_list(
        PrimaryLayout::Default(DefaultPane::Recipe),
        ViewState::open_recipe_list
    )]
    #[case::select_recipe_pane(
        PrimaryLayout::Default(DefaultPane::Exchange),
        ViewState::select_recipe_pane
    )]
    #[case::select_profile_pane(
        PrimaryLayout::Profile(ProfileSelectPane::List),
        ViewState::select_profile_pane
    )]
    #[case::select_exchange_pane(
        PrimaryLayout::Default(DefaultPane::Recipe),
        ViewState::select_exchange_pane
    )]
    #[case::previous_pane(
        PrimaryLayout::Default(DefaultPane::Recipe),
        ViewState::previous_pane
    )]
    #[case::next_pane(
        PrimaryLayout::Default(DefaultPane::Recipe),
        ViewState::next_pane
    )]
    fn test_fullscreen_switch_panes(
        #[case] layout: PrimaryLayout,
        #[case] mutator: fn(&mut ViewState),
    ) {
        let mut state = ViewState {
            layout,
            fullscreen: true,
        };

        // Mutator should exit state
        mutator(&mut state);
        assert!(!state.fullscreen);
    }
}
