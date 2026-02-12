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
    /// Selected sidebar. If the sidebar is closed, we still track this so we
    /// know which sidebar to show if it's toggled open
    sidebar: Sidebar,
}

impl ViewState {
    /// Get the current sidebar/pane layout
    pub fn layout(&self) -> PrimaryLayout {
        self.layout
    }

    /// Get the selected sidebar
    ///
    /// There is always a sidebar selected, **even if it's not visible**. If
    /// the sidebar is closed, this will be whichever sidebar was most recently
    /// visible.
    pub fn sidebar(&self) -> Sidebar {
        self.sidebar
    }

    /// Open the sidebar with specific content
    pub fn open_sidebar(&mut self, sidebar: Sidebar) {
        self.layout = PrimaryLayout::Sidebar(SidebarPane::Sidebar);
        self.sidebar = sidebar;
    }

    /// Close the sidebar and return to the wide view
    pub fn close_sidebar(&mut self) {
        if let PrimaryLayout::Sidebar(selected_pane) = self.layout {
            // Retain selected pane where possible
            self.layout = PrimaryLayout::Wide(selected_pane.into());
        }
    }

    /// Open/close the sidebar
    pub fn toggle_sidebar(&mut self) {
        match self.layout {
            PrimaryLayout::Wide(pane) => {
                // Toggle operations should be their own inverse, so we do NOT
                // want to select the sidebar pane
                self.layout = PrimaryLayout::Sidebar(pane.into());
            }
            PrimaryLayout::Fullscreen(pane) => {
                self.layout = PrimaryLayout::Sidebar(pane);
            }
            PrimaryLayout::Sidebar(_) => self.close_sidebar(),
        }
    }

    /// Select the previous pane in the cycle
    pub fn previous_pane(&mut self) {
        fn previous<T: PartialEq + IntoEnumIterator>(value: T) -> T {
            after(T::iter().rev(), value)
        }

        match &mut self.layout {
            PrimaryLayout::Wide(pane) => *pane = previous(*pane),
            PrimaryLayout::Sidebar(pane) => *pane = previous(*pane),
            // Exit fullscreen before swapping panes
            PrimaryLayout::Fullscreen(pane) => {
                self.layout = PrimaryLayout::Sidebar(previous(*pane));
            }
        }
    }

    /// Select the next pane in the cycle
    pub fn next_pane(&mut self) {
        fn next<T: PartialEq + IntoEnumIterator>(value: T) -> T {
            after(T::iter(), value)
        }

        match &mut self.layout {
            PrimaryLayout::Wide(pane) => *pane = next(*pane),
            PrimaryLayout::Sidebar(pane) => *pane = next(*pane),
            // Exit fullscreen before swapping panes
            PrimaryLayout::Fullscreen(pane) => {
                self.layout = PrimaryLayout::Sidebar(next(*pane));
            }
        }
    }

    /// Move focus to the upper pane in the layout
    pub fn select_top_pane(&mut self) {
        match &mut self.layout {
            PrimaryLayout::Wide(pane) => {
                *pane = WidePane::Top;
            }
            PrimaryLayout::Sidebar(pane) | PrimaryLayout::Fullscreen(pane) => {
                *pane = SidebarPane::Top;
            }
        }
    }

    /// Move focus to the lower pane in the layout
    pub fn select_bottom_pane(&mut self) {
        match &mut self.layout {
            PrimaryLayout::Wide(pane) => {
                *pane = WidePane::Bottom;
            }
            PrimaryLayout::Sidebar(pane) | PrimaryLayout::Fullscreen(pane) => {
                *pane = SidebarPane::Bottom;
            }
        }
    }

    /// Move focus to the Recipe pane
    pub fn select_recipe_pane(&mut self) {
        // Recipe pane is visible on top in all views
        self.select_top_pane();
    }

    /// Move focus to the Profile pane. If it's not in this view, do nothing
    pub fn select_profile_pane(&mut self) {
        match &mut self.layout {
            PrimaryLayout::Wide(_) => {}
            PrimaryLayout::Sidebar(pane) | PrimaryLayout::Fullscreen(pane) => {
                match self.sidebar {
                    Sidebar::Recipe | Sidebar::History => {}
                    Sidebar::Profile => *pane = SidebarPane::Bottom,
                }
            }
        }
    }

    /// Move focus to the Exchange pane. If it's not in this view, do nothing
    pub fn select_exchange_pane(&mut self) {
        match &mut self.layout {
            PrimaryLayout::Wide(pane) => {
                *pane = WidePane::Bottom;
            }
            PrimaryLayout::Sidebar(pane) | PrimaryLayout::Fullscreen(pane) => {
                match self.sidebar {
                    Sidebar::Recipe | Sidebar::History => {
                        *pane = SidebarPane::Bottom;
                    }
                    Sidebar::Profile => {} // Exchange pane isn't visible
                }
            }
        }
    }

    /// Is the selected pane fullscreened?
    pub fn is_fullscreen(&self) -> bool {
        matches!(self.layout, PrimaryLayout::Fullscreen(_))
    }

    /// Enter/exit fullscreen mode for the currently selected pane
    pub fn toggle_fullscreen(&mut self) {
        match self.layout {
            PrimaryLayout::Wide(pane) => {
                self.layout = PrimaryLayout::Fullscreen(pane.into());
            }
            PrimaryLayout::Fullscreen(pane) => {
                // We don't store what the layout was *before* fullscreen, so
                // go back to having the sidebar open. It's a bit clunky but
                // it's better than going to the sidebar closed, because the
                // sidebar may be the fullscreened pane.
                self.layout = PrimaryLayout::Sidebar(pane);
            }
            PrimaryLayout::Sidebar(pane) => {
                self.layout = PrimaryLayout::Fullscreen(pane);
            }
        }
    }

    /// Exit fullscreen mode for the currently selected pane
    pub fn exit_fullscreen(&mut self) {
        if let PrimaryLayout::Fullscreen(pane) = self.layout {
            self.layout = PrimaryLayout::Sidebar(pane);
        }
    }
}

impl Default for ViewState {
    fn default() -> Self {
        ViewState {
            layout: PrimaryLayout::Sidebar(SidebarPane::Top),
            sidebar: Sidebar::Recipe,
        }
    }
}

/// Which panes are visible, and which one is selected?
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PrimaryLayout {
    /// Sidebar is closed, so the main panes are *wider*
    Wide(WidePane),
    /// Sidebar is open
    Sidebar(SidebarPane),
    /// A single pane is visible (could be the sidebar pane)
    Fullscreen(SidebarPane),
}

/// Selectable pane in [PrimaryLayout::Wide]
#[derive(Copy, Clone, Debug, PartialEq, EnumIter, Serialize, Deserialize)]
pub enum WidePane {
    Top,
    Bottom,
}

impl From<SidebarPane> for WidePane {
    fn from(pane: SidebarPane) -> Self {
        match pane {
            SidebarPane::Sidebar | SidebarPane::Top => Self::Top,
            SidebarPane::Bottom => Self::Bottom,
        }
    }
}

/// Selectable pane in [PrimaryLayout::Sidebar]
#[derive(Copy, Clone, Debug, PartialEq, EnumIter, Serialize, Deserialize)]
pub enum SidebarPane {
    Sidebar,
    Top,
    Bottom,
}

impl From<WidePane> for SidebarPane {
    fn from(pane: WidePane) -> Self {
        match pane {
            WidePane::Top => Self::Top,
            WidePane::Bottom => Self::Bottom,
        }
    }
}

/// List content that can be displayed in the sidebar
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Sidebar {
    Profile,
    Recipe,
    History,
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

    impl From<(PrimaryLayout, Sidebar)> for ViewState {
        fn from((layout, sidebar): (PrimaryLayout, Sidebar)) -> Self {
            Self { layout, sidebar }
        }
    }

    impl From<PrimaryLayout> for ViewState {
        fn from(layout: PrimaryLayout) -> Self {
            Self {
                layout,
                sidebar: Sidebar::Recipe,
            }
        }
    }

    /// Test various transitions between different states and layouts
    #[rstest]
    // Sidebar
    #[case::toggle_sidebar_open(
        (PrimaryLayout::Wide(WidePane::Bottom), Sidebar::Profile).into(),
        ViewState::toggle_sidebar,
        // Selected pane is retained
        (PrimaryLayout::Sidebar(SidebarPane::Bottom), Sidebar::Profile).into(),
    )]
    #[case::toggle_sidebar_close(
        (PrimaryLayout::Sidebar(SidebarPane::Bottom), Sidebar::Profile).into(),
        ViewState::toggle_sidebar,
        // Selected pane is retained
        (PrimaryLayout::Wide(WidePane::Bottom), Sidebar::Profile).into(),
    )]
    #[case::toggle_sidebar_close_sidebar_selected(
        (PrimaryLayout::Sidebar(SidebarPane::Sidebar), Sidebar::Profile).into(),
        ViewState::toggle_sidebar,
        // Can't keep the sidebar selected, so default to the top pane
        (PrimaryLayout::Wide(WidePane::Top), Sidebar::Profile).into(),
    )]
    // Fullscreen
    #[case::toggle_fullscreen_open(
        PrimaryLayout::Sidebar(SidebarPane::Sidebar).into(),
        ViewState::toggle_fullscreen,
        PrimaryLayout::Fullscreen(SidebarPane::Sidebar).into(),
    )]
    #[case::toggle_fullscreen_close(
        PrimaryLayout::Fullscreen(SidebarPane::Sidebar).into(),
        ViewState::toggle_fullscreen,
        PrimaryLayout::Sidebar(SidebarPane::Sidebar).into(),
    )]
    #[case::toggle_fullscreen_open_wide(
        PrimaryLayout::Wide(WidePane::Bottom).into(),
        ViewState::toggle_fullscreen,
        // Pane is mapped correctly
        PrimaryLayout::Fullscreen(SidebarPane::Bottom).into(),
    )]
    // Exiting fullscreen always puts us back in sidebar layout, even if we
    // started in wide
    #[case::toggle_fullscreen_from_wide(
        PrimaryLayout::Wide(WidePane::Bottom).into(),
        |state: &mut ViewState| {
            state.toggle_fullscreen();
            state.toggle_fullscreen();
        },
        PrimaryLayout::Sidebar(SidebarPane::Bottom).into(),
    )]
    fn test_transitions(
        #[case] initial: ViewState,
        #[case] transition: impl FnOnce(&mut ViewState),
        #[case] expected: ViewState,
    ) {
        let mut state = initial;
        transition(&mut state);
        assert_eq!(state, expected);
    }
}
