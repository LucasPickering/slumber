use serde::{Deserialize, Serialize};
use strum::{EnumIter, IntoEnumIterator};

/// Which panes are visible in the primary view?
///
/// This serves as a state machine to manage transitions between various
/// possible states of the primary view. It defines which panes are visible.
/// Invalid view states are unrepresentable with this type.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewState {
    /// What panes are currently visible?
    layout: PrimaryLayout,
    /// If `true`, the selected pane should take up the entire screen, and
    /// other panes are not visible.
    fullscreen: bool,
}

impl ViewState {
    /// Get the current sidebar/pane layout
    pub fn layout(&self) -> PrimaryLayout {
        self.layout
    }

    /// Get the open sidebar, or `None` if the sidebar is closed
    pub fn sidebar(&self) -> Option<Sidebar> {
        match self.layout {
            PrimaryLayout::Default(_) => None,
            PrimaryLayout::Sidebar { sidebar, .. } => Some(sidebar),
        }
    }

    /// Open the profile list in the sidebar
    pub fn open_profile_list(&mut self) {
        self.modify_layout(|layout| {
            *layout = PrimaryLayout::sidebar(Sidebar::Profile);
        });
    }

    /// Open the recipe list in the sidebar
    pub fn open_recipe_list(&mut self) {
        self.modify_layout(|layout| {
            *layout = PrimaryLayout::sidebar(Sidebar::Recipe);
        });
    }

    /// Close the sidebar and return to the default view
    pub fn close_sidebar(&mut self) {
        self.layout = PrimaryLayout::Default(DefaultPane::Top);
    }

    /// Select the previous pane in the cycle
    pub fn previous_pane(&mut self) {
        fn previous<T: PartialEq + IntoEnumIterator>(value: T) -> T {
            after(T::iter().rev(), value)
        }
        self.modify_layout(|layout| match layout {
            PrimaryLayout::Default(pane) => *pane = previous(*pane),
            PrimaryLayout::Sidebar {
                selected_pane: pane,
                ..
            } => *pane = previous(*pane),
        });
    }

    /// Select the next pane in the cycle
    pub fn next_pane(&mut self) {
        fn next<T: PartialEq + IntoEnumIterator>(value: T) -> T {
            after(T::iter(), value)
        }
        self.modify_layout(|layout| match layout {
            PrimaryLayout::Default(pane) => *pane = next(*pane),
            PrimaryLayout::Sidebar {
                selected_pane: pane,
                ..
            } => *pane = next(*pane),
        });
    }

    /// Move focus to the upper pane in the layout
    pub fn select_top_pane(&mut self) {
        self.modify_layout(|layout| match layout {
            PrimaryLayout::Default(pane) => *pane = DefaultPane::Top,
            PrimaryLayout::Sidebar {
                selected_pane: pane,
                ..
            } => *pane = SidebarPane::Top,
        });
    }

    /// Move focus to the lower pane in the layout
    pub fn select_bottom_pane(&mut self) {
        self.modify_layout(|layout| match layout {
            PrimaryLayout::Default(pane) => *pane = DefaultPane::Bottom,
            PrimaryLayout::Sidebar {
                selected_pane: pane,
                ..
            } => *pane = SidebarPane::Bottom,
        });
    }

    /// Move focus to the Recipe pane
    pub fn select_recipe_pane(&mut self) {
        self.select_top_pane();
    }

    /// Move focus to the Profile pane. If it's not in this view, do nothing
    pub fn select_profile_pane(&mut self) {
        self.modify_layout(|layout| match layout {
            PrimaryLayout::Sidebar {
                sidebar: Sidebar::Profile,
                selected_pane: pane,
            } => *pane = SidebarPane::Bottom,
            PrimaryLayout::Default(_)
            | PrimaryLayout::Sidebar {
                sidebar: Sidebar::Recipe,
                ..
            } => {}
        });
    }

    /// Move focus to the Exchange pane. If it's not in this view, do nothing
    pub fn select_exchange_pane(&mut self) {
        self.modify_layout(|layout| match layout {
            PrimaryLayout::Default(pane) => *pane = DefaultPane::Bottom,
            PrimaryLayout::Sidebar {
                sidebar: Sidebar::Recipe,
                selected_pane: pane,
            } => *pane = SidebarPane::Bottom,
            PrimaryLayout::Sidebar {
                sidebar: Sidebar::Profile,
                ..
            } => {}
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
            layout: PrimaryLayout::Default(DefaultPane::Top),
            fullscreen: false,
        }
    }
}

/// Which panes are visible, and which one is selected?
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PrimaryLayout {
    /// Default layout: all sidebars are collapsed
    Default(DefaultPane),
    /// Sidebar is open
    Sidebar {
        sidebar: Sidebar,
        selected_pane: SidebarPane,
    },
}

impl PrimaryLayout {
    /// Open a sidebar layout
    fn sidebar(sidebar: Sidebar) -> Self {
        Self::Sidebar {
            sidebar,
            selected_pane: SidebarPane::Sidebar,
        }
    }
}

/// Selectable pane in [PrimaryLayout::Default]
#[derive(Copy, Clone, Debug, PartialEq, EnumIter, Serialize, Deserialize)]
pub enum DefaultPane {
    Top,
    Bottom,
}

/// Selectable pane in [PrimaryLayout::Sidebar]
#[derive(Copy, Clone, Debug, PartialEq, EnumIter, Serialize, Deserialize)]
pub enum SidebarPane {
    Sidebar,
    Top,
    Bottom,
}

/// List content that can be displayed in the sidebar
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Sidebar {
    Profile,
    Recipe,
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
            layout: PrimaryLayout::Default(DefaultPane::Top),
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
        PrimaryLayout::Default(DefaultPane::Top),
        ViewState::open_profile_list
    )]
    #[case::open_recipe_list(
        PrimaryLayout::Default(DefaultPane::Top),
        ViewState::open_recipe_list
    )]
    #[case::select_recipe_pane(
        PrimaryLayout::Default(DefaultPane::Bottom),
        ViewState::select_recipe_pane
    )]
    #[case::select_profile_pane(
        PrimaryLayout::sidebar(Sidebar::Profile),
        ViewState::select_profile_pane
    )]
    #[case::select_exchange_pane(
        PrimaryLayout::Default(DefaultPane::Top),
        ViewState::select_exchange_pane
    )]
    #[case::previous_pane(
        PrimaryLayout::Default(DefaultPane::Top),
        ViewState::previous_pane
    )]
    #[case::next_pane(
        PrimaryLayout::Default(DefaultPane::Top),
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
