use serde::{Deserialize, Serialize};

/// Which panes are visible in the primary view?
///
/// This serves as a state machine to manage transitions between various
/// possible states of the primary view. It defines which panes are visible.
///
/// The layout is exposed through [Self::layout].
#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewState {
    // The internal state representation looks very different from the external
    // layout. The goal of the layout is to directly mirror what is currently
    // visible, so it has some duplicative information. This internal state is
    // as minimal as possible to eliminate impossible states.
    /// Which pane has focus?
    ///
    /// Not all selected panes are valid in all states. The sidebar can't be
    /// focused when it's closed. Each state transition needs to ensure this
    /// remains valid.
    selected_pane: SelectedPane,
    /// Selected sidebar. If the sidebar is closed, we still track this so we
    /// know which sidebar to show if it's toggled open
    sidebar: Sidebar,
    /// Is the sidebar open? This will be `true` if in the sidebar layout, *or*
    /// if a pane is fullscreened but the sidebar was visible in the last
    /// multi-pane layout.
    sidebar_open: bool,
    /// Is the selected pane fullscreened?
    fullscreen: bool,
}

impl ViewState {
    /// Get the current sidebar/pane layout
    pub fn layout(&self) -> PrimaryLayout {
        let top_pane = PrimaryPane::Recipe;
        // Bottom pane depends on the sidebar
        let bottom_pane = match self.sidebar {
            Sidebar::Profile => PrimaryPane::Profile,
            Sidebar::Recipe | Sidebar::History => PrimaryPane::Exchange,
        };

        if self.fullscreen {
            let pane = match self.selected_pane {
                SelectedPane::Sidebar => PrimaryPane::Sidebar(self.sidebar),
                SelectedPane::Top => top_pane,
                SelectedPane::Bottom => bottom_pane,
            };
            PrimaryLayout::Fullscreen {
                pane: SelectedState {
                    value: pane,
                    selected: true,
                },
            }
        } else {
            // Multiple panes are visible
            let top = SelectedState {
                value: top_pane,
                selected: self.selected_pane == SelectedPane::Top,
            };
            let bottom = SelectedState {
                value: bottom_pane,
                selected: self.selected_pane == SelectedPane::Bottom,
            };
            if self.sidebar_open {
                // Sidebar is open
                let sidebar = SelectedState {
                    value: self.sidebar,
                    selected: self.selected_pane == SelectedPane::Sidebar,
                };
                PrimaryLayout::Sidebar {
                    sidebar,
                    top,
                    bottom,
                }
            } else {
                // Sidebar closed
                PrimaryLayout::Wide { top, bottom }
            }
        }
    }

    /// Open the sidebar with specific content
    pub fn open_sidebar(&mut self, sidebar: Sidebar) {
        self.sidebar = sidebar;
        self.sidebar_open = true;
        self.selected_pane = SelectedPane::Sidebar;
    }

    /// Reset to the recipe sidebar
    ///
    /// Call this when submitting/cancelling a secondary sidebar. This will
    /// *not* close the sidebar, just revert to the default content.
    pub fn reset_sidebar(&mut self) {
        self.sidebar = Sidebar::Recipe;
    }

    /// Open/close the sidebar
    pub fn toggle_sidebar(&mut self) {
        // A toggle is a "soft" close, so we don't wipe out the selected
        // sidebar. When re-opening, we want to re-show the same sidebar.
        self.sidebar_open ^= true;
        if !self.sidebar_open && self.selected_pane == SelectedPane::Sidebar {
            self.selected_pane = SelectedPane::Top;
        }
    }

    /// Select the previous pane in the cycle
    pub fn previous_pane(&mut self) {
        self.exit_fullscreen();
        self.selected_pane =
            offset(self.selectable_panes(), self.selected_pane, -1);
    }

    /// Select the next pane in the cycle
    pub fn next_pane(&mut self) {
        self.exit_fullscreen();
        self.selected_pane =
            offset(self.selectable_panes(), self.selected_pane, 1);
    }

    /// Get the set of selectable panes for the current state
    fn selectable_panes(&self) -> &'static [SelectedPane] {
        if self.sidebar_open {
            &[
                SelectedPane::Sidebar,
                SelectedPane::Top,
                SelectedPane::Bottom,
            ]
        } else {
            &[SelectedPane::Top, SelectedPane::Bottom]
        }
    }

    /// Move focus to the upper pane in the layout
    pub fn select_top_pane(&mut self) {
        self.selected_pane = SelectedPane::Top;
    }

    /// Move focus to the lower pane in the layout
    pub fn select_bottom_pane(&mut self) {
        self.selected_pane = SelectedPane::Bottom;
    }

    /// Move focus to the Recipe pane
    pub fn select_recipe_pane(&mut self) {
        // Recipe pane is visible on top in all views
        self.select_top_pane();
    }

    /// Move focus to the Profile pane. If it's not in this view, do nothing
    pub fn select_profile_pane(&mut self) {
        // If the profile pane isn't visible, do nothing
        if self.sidebar_open && self.sidebar == Sidebar::Profile {
            self.select_bottom_pane();
        }
    }

    /// Move focus to the Exchange pane. If it's not in this view, do nothing
    pub fn select_exchange_pane(&mut self) {
        // If the Exchange pane isn't visible, do nothing
        if !self.sidebar_open || self.sidebar != Sidebar::Profile {
            self.select_bottom_pane();
        }
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
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            sidebar: Sidebar::Recipe,
            sidebar_open: true,
            fullscreen: false,
            selected_pane: SelectedPane::Top,
        }
    }
}

/// User-facing pane state. This maps 1:1 with what will be rendered.
///
/// Any state this represents should be theoretically drawable, but won't
/// necessarily be a valid state that the user can get into. The goal of this
/// is to minimize the work that `PrimaryView` has to do during the draw. So
/// this represents exactly what should be drawn with minimal interpretation.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum PrimaryLayout {
    /// Layout with no sidebar visible. The primary panes are *wider*.
    Wide {
        top: SelectedState<PrimaryPane>,
        bottom: SelectedState<PrimaryPane>,
    },
    /// Layout with the sidebar open and two panes visible
    Sidebar {
        sidebar: SelectedState<Sidebar>,
        top: SelectedState<PrimaryPane>,
        bottom: SelectedState<PrimaryPane>,
    },
    /// A single pane is visible (could be the sidebar pane)
    Fullscreen { pane: SelectedState<PrimaryPane> },
}

/// A pane plus its focus state
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SelectedState<T> {
    pub value: T,
    pub selected: bool,
}

/// A selectable pane in the primary view
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum PrimaryPane {
    Recipe,
    Exchange,
    Profile,
    /// Tall skinny guy
    Sidebar(Sidebar),
}

/// Internal state for which pane is selected
///
/// This uses positions instead of semantic meaning (recipe/exchange/etc.)
/// because it makes it layout-agnostic and easy to cycle.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
enum SelectedPane {
    Sidebar,
    Top,
    Bottom,
}

/// List content that can be displayed in the sidebar
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Sidebar {
    Profile,
    Recipe,
    History,
}

/// Get a next/previous pane in the list based on the offset
fn offset(
    all: &[SelectedPane],
    current: SelectedPane,
    offset: isize,
) -> SelectedPane {
    // This panic is possible if the current pane isn't valid (e.g. sidebar
    // selected but not open). There's a prop test to cover this.
    let current = all
        .iter()
        .position(|v| v == &current)
        .expect("Pane not in list");
    let index =
        ((current as isize + offset).rem_euclid(all.len() as isize)) as usize;
    all[index]
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::{collection, sample, test_runner::TestRunner};
    use rstest::rstest;

    /// Test various transitions between different states and layouts
    #[rstest]
    // Sidebar
    #[case::toggle_sidebar_open(
        ViewState {
            selected_pane: SelectedPane::Bottom,
            sidebar: Sidebar::Profile,
            sidebar_open: false,
            fullscreen: false,
        },
        ViewState::toggle_sidebar,
        // Selected pane is retained
        ViewState {
            selected_pane: SelectedPane::Bottom,
            sidebar: Sidebar::Profile,
            sidebar_open: true,
            fullscreen: false,
        },
    )]
    #[case::toggle_sidebar_close(
        ViewState {
            selected_pane: SelectedPane::Bottom,
            sidebar: Sidebar::Profile,
            sidebar_open: true,
            fullscreen: false,
        },
        ViewState::toggle_sidebar,
        // Selected pane is retained
        ViewState {
            selected_pane: SelectedPane::Bottom,
            sidebar: Sidebar::Profile,
            sidebar_open: false,
            fullscreen: false,
        },
    )]
    #[case::toggle_sidebar_close_sidebar_selected(
        ViewState {
            selected_pane: SelectedPane::Sidebar,
            sidebar: Sidebar::Profile,
            sidebar_open: true,
            fullscreen: false,
        },
        ViewState::toggle_sidebar,
        // Can't keep the sidebar selected, so default to the top pane
        ViewState {
            selected_pane: SelectedPane::Top,
            sidebar: Sidebar::Profile,
            sidebar_open: false,
            fullscreen: false,
        },
    )]
    #[case::reset_sidebar(
        ViewState {
            sidebar: Sidebar::History,
            sidebar_open: true,
            ..Default::default()
        },
        ViewState::reset_sidebar,
        // Sidebar remains open
        ViewState {
            sidebar: Sidebar::Recipe,
            sidebar_open: true,
            ..Default::default()
        },
    )]
    // Fullscreen
    #[case::toggle_fullscreen_open(
        ViewState {
            selected_pane: SelectedPane::Sidebar,
            sidebar: Sidebar::Recipe,
            sidebar_open: true,
            fullscreen: false,
        },
        ViewState::toggle_fullscreen,
        ViewState {
            selected_pane: SelectedPane::Sidebar,
            sidebar: Sidebar::Recipe,
            sidebar_open: true,
            fullscreen: true,
        },
    )]
    #[case::toggle_fullscreen_close(
        ViewState {
            selected_pane: SelectedPane::Sidebar,
            sidebar: Sidebar::Recipe,
            sidebar_open: true,
            fullscreen: true,
        },
        ViewState::toggle_fullscreen,
        ViewState {
            selected_pane: SelectedPane::Sidebar,
            sidebar: Sidebar::Recipe,
            sidebar_open: true,
            fullscreen: false,
        },
    )]
    #[case::toggle_fullscreen_open_wide(
        ViewState {
            sidebar_open: false,
            fullscreen: false,
            ..Default::default()
        },
        ViewState::toggle_fullscreen,
        ViewState {
            sidebar_open: false,
            fullscreen: true,
            ..Default::default()
        },
    )]
    // Pane cycling
    #[case::cycle_exits_fullscreen(
        ViewState {
            selected_pane: SelectedPane::Top,
            fullscreen: true,
            ..Default::default()
        },
        ViewState::next_pane,
        ViewState {
            selected_pane: SelectedPane::Bottom,
            fullscreen: false,
            ..Default::default()
        },
    )]
    #[case::sidebar_open_prev(
        ViewState {
            selected_pane: SelectedPane::Sidebar,
            sidebar_open: true,
            ..Default::default()
        },
        ViewState::previous_pane,
        ViewState {
            selected_pane: SelectedPane::Bottom,
            sidebar_open: true,
            ..Default::default()
        },
    )]
    #[case::sidebar_open_next(
        ViewState {
            selected_pane: SelectedPane::Bottom,
            sidebar_open: true,
            ..Default::default()
        },
        ViewState::next_pane,
        ViewState {
            selected_pane: SelectedPane::Sidebar,
            sidebar_open: true,
            ..Default::default()
        },
    )]
    #[case::sidebar_closed_prev(
        ViewState {
            selected_pane: SelectedPane::Top,
            sidebar_open: false,
            ..Default::default()
        },
        ViewState::previous_pane,
        ViewState {
            selected_pane: SelectedPane::Bottom,
            sidebar_open: false,
            ..Default::default()
        },
    )]
    #[case::sidebar_closed_next(
        ViewState {
            selected_pane: SelectedPane::Bottom,
            sidebar_open: false,
            ..Default::default()
        },
        ViewState::next_pane,
        ViewState {
            selected_pane: SelectedPane::Top,
            sidebar_open: false,
            ..Default::default()
        },
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

    /// Prop test: no combination of transitions can get us into an invalid
    /// state
    ///
    /// Invalid states are:
    /// - Sidebar selected while closed
    ///
    /// (I thought there would be more but I guess that's it)
    #[test]
    fn test_invalid_states_prop() {
        type Transition = fn(&mut ViewState);

        // These need static aliases to be used in the prop test
        fn open_recipe_sidebar(state: &mut ViewState) {
            state.open_sidebar(Sidebar::Recipe);
        }
        fn open_profile_sidebar(state: &mut ViewState) {
            state.open_sidebar(Sidebar::Profile);
        }
        fn open_history_sidebar(state: &mut ViewState) {
            state.open_sidebar(Sidebar::History);
        }

        const TRANSITIONS: &[Transition] = &[
            open_recipe_sidebar,
            open_profile_sidebar,
            open_history_sidebar,
            ViewState::reset_sidebar,
            ViewState::toggle_sidebar,
            ViewState::previous_pane,
            ViewState::next_pane,
            ViewState::select_top_pane,
            ViewState::select_bottom_pane,
            ViewState::select_recipe_pane,
            ViewState::select_profile_pane,
            ViewState::select_exchange_pane,
        ];

        // I hate the proptest! macro, prefer manual
        let test = |transitions: Vec<Transition>| {
            let mut state = ViewState::default();
            for transition in transitions {
                transition(&mut state);

                // Ensure state is valid
                if !state.sidebar_open {
                    assert_ne!(
                        state.selected_pane,
                        SelectedPane::Sidebar,
                        "Sidebar pane cannot be selected while sidebar is closed"
                    );
                }
            }
            Ok(())
        };
        let mut runner = TestRunner::default();
        runner
            .run(
                &collection::vec(sample::select(TRANSITIONS), 1..8usize),
                test,
            )
            .unwrap();
    }
}
