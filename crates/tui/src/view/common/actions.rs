use crate::view::{
    Generate,
    common::select::{Select, SelectEventKind, SelectListProps},
    component::{
        Canvas, Child, Component, ComponentExt, ComponentId, Draw,
        DrawMetadata, ToChild,
    },
    context::{UpdateContext, ViewContext},
    event::{Emitter, Event, EventMatch, LocalEvent, ToEmitter},
};
use itertools::Itertools;
use ratatui::{
    layout::{Constraint, Offset, Position},
    prelude::Rect,
    symbols::merge::MergeStrategy,
    text::Span,
    widgets::{Block, Borders, Clear},
};
use slumber_config::Action;
use std::fmt::{self, Display};
use unicode_width::UnicodeWidthStr;

/// Popup menu to list and trigger arbitrary actions.
///
/// The user opens the action menu with a keybinding, at which point the list of
/// available actions is built dynamically via [Component::menu]. When an action
/// is selected, the modal is closed and that action will be emitted as a
/// dynamic event, to be handled by the component that originally supplied it.
/// Each component that provides actions should store an [Emitter] specifically
/// for its actions, which will be provided to each supplied action and can be
/// used to check and consume the action events.
#[derive(Debug, Default)]
pub struct ActionMenu {
    id: ComponentId,
    /// Menu content, which is `Some` when the menu is open
    content: Option<ActionMenuContent>,
}

impl ActionMenu {
    /// Open the actions menu with the given actions/groups
    pub fn open(&mut self, items: Vec<MenuItem>) {
        self.content = Some(ActionMenuContent::new(items));
    }

    fn close(&mut self) {
        self.content = None;
    }
}

impl Component for ActionMenu {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        // Don't eat events until we're open
        let Some(content) = &self.content else {
            return event.m();
        };
        let emitter = content.emitter;

        event
            .m()
            .action(|action, propagate| match action {
                Action::Cancel | Action::Quit => self.close(),
                _ => propagate.set(),
            })
            // If clicked outside the content, close. The content eats click
            // events so those don't make it here
            .click(|_, _| self.close())
            .emitted(
                emitter,
                // Unwraps are safe because we can only get an event if the
                // content exists
                |_: ActionSubmit| self.content.take().unwrap().submit(),
            )
            .any(|event| match event {
                // Eat any input events, since we're the sole focus holder
                Event::Input { .. } => None,
                _ => Some(event),
            })
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.content.to_child_mut()]
    }
}

impl Draw for ActionMenu {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        if let Some(content) = &self.content {
            // Our given area is the full screen so we can capture cursor
            // events. Draw the content just to the middle block
            let area = content.area(metadata.area());
            canvas.draw(content, (), area, true);
        }
    }
}

/// Data for a particular action menu. This is created when the menu is opened
/// based on what actions are available
#[derive(Debug)]
struct ActionMenuContent {
    id: ComponentId,
    /// Original action tree that this modal is derived from. This is stored in
    /// its original state
    items: Vec<MenuItem>,
    /// Stack of menu levels. Push when a group becomes visible, pop when it's
    /// hidden. The visual data is cloned from `self.items` so we can organize
    /// this in a stack. The original item tree doesn't allow that, and we
    /// can't use references to the tree because it would be self-referential.
    /// INVARIANT: len >= 1
    stack: Vec<Select<MenuItemDisplay>>,
    /// The index of the layer in the stack that the user is controlling. This
    /// index is always valid because the stack is never empty
    /// INVARIANT: selected_layer < self.stack.len()
    active_layer: usize,
    /// Emitter to tell the parent when we're executing an action. Submission
    /// requires an owned `self`, so it has to be done as the parent closes us
    emitter: Emitter<ActionSubmit>,
}

impl ActionMenuContent {
    fn new(items: Vec<MenuItem>) -> Self {
        let root_select = build_select(map_items(&items));
        Self {
            id: ComponentId::default(),
            items,
            stack: vec![root_select],
            active_layer: 0,
            emitter: Emitter::default(),
        }
    }

    /// Clear all layers right of the given layer
    fn clear_children(&mut self, layer: usize) {
        self.stack.drain((layer + 1)..);
        // Defensive programming!!
        assert!(
            !self.stack.is_empty(),
            "Action menu stack must have at least one element"
        );
    }

    /// Open the children of a group in the active layer. This assumes the
    /// active layer is the top layer, so call [Self::clear_children] first.
    fn open_group(&mut self, items: Vec<MenuItemDisplay>) {
        self.stack.push(build_select(items));
    }

    /// Check if the given input action is bound to a menu action **in any
    /// layer**. This will start with the left-most layer and check each layer
    /// for an item bound to that action. If it's found, select that layer+item
    /// and return `true`. If not, return `false`.
    fn select_by_action(&mut self, action: Action) -> bool {
        for (layer, select) in self.stack.iter_mut().enumerate() {
            // Check if this input is bound to any item in this select
            let bound_index = select.items().position(|item| match item {
                // Disallow shortcuts for disabled items
                MenuItemDisplay::Action {
                    shortcut, enabled, ..
                } if *enabled => shortcut == &Some(action),
                _ => false,
            });

            // This action is bound to something!
            if let Some(bound_index) = bound_index {
                self.active_layer = layer;
                select.select_index(bound_index);
                return true;
            }
        }
        false
    }

    /// Select the previous layer to the left in the stack
    fn previous_layer(&mut self) {
        self.active_layer = self.active_layer.saturating_sub(1);
    }

    /// Select the next layer to the right in the stack
    fn next_layer(&mut self) {
        self.active_layer = (self.active_layer + 1).min(self.stack.len() - 1);
    }

    /// Perform the selected action
    fn submit(self) {
        // To find the submitted action, we need to walk down the original
        // action tree in parallel with the stack. In each stack layer we have
        // a selected index, which we'll use to grab the next tree layer.
        let mut items = self.items;
        for i in 0..=self.active_layer {
            // Indexing is safe because active_layer < stack.len()
            let select = &self.stack[i];

            let Some(selected_index) = select.selected_index() else {
                return; // Possible if the final action menu is empty
            };
            let item = items.swap_remove(selected_index);
            if i < self.active_layer {
                // We have more layers to go - we're looking for a group
                let MenuItem::Group { children, .. } = item else {
                    panic!("Expected group at layer {i}, found {item:?}");
                };
                items = children;
            } else {
                // This is the last layer - we're looking for an action
                let MenuItem::Action(action) = item else {
                    panic!("Expected action at layer {i}, found {item:?}");
                };

                // Emit an event on behalf of the component that supplied this
                // action. The component will use its own supplied emitter ID to
                // consume the event
                action.emitter.emit(action.value);
            }
        }
    }

    /// Get the smallest possible area that can contain all menu layers
    fn area(&self, canvas_area: Rect) -> Rect {
        // First, calculate the area for each item
        let areas = self.areas(Position::default());

        // Then shift them all to be centered on the *first* layer. We don't
        // want them to shift when additional layers are opened
        let first = areas.first().expect("Menu stack cannot be empty");
        let centered = canvas_area.centered(
            Constraint::Length(first.width),
            Constraint::Length(first.height),
        );
        // We know everything is relative to (0,0) to begin with
        let offset = Offset {
            x: centered.x.into(),
            y: centered.y.into(),
        };

        // The overall area is just the union of each layer's area. This
        // will get the smallest possible area that contains all layers
        areas
            .into_iter()
            .map(|area| area.offset(offset))
            .reduce(Rect::union)
            .unwrap_or_default()
    }

    /// Get the area for each open layer, starting from the given position
    ///
    /// Each area will include margin for a border. This is used in multiple
    /// places to get consistent results.
    fn areas(&self, position: Position) -> Vec<Rect> {
        fn layer_width(layer: &Select<MenuItemDisplay>) -> u16 {
            // Get the longest item
            layer
                .items()
                .map(|item| item.to_string().width() as u16)
                .max()
                .unwrap_or(0)
        }

        // Accumulate x/y offset across all layers
        let mut offset_x = 0;
        let mut offset_y = 0;

        self.stack
            .iter()
            .map(|layer| {
                let area = Rect {
                    width: layer_width(layer) + 2,
                    height: layer.len() as u16 + 2,
                    x: position.x + offset_x,
                    y: position.y + offset_y,
                };

                // Add to the offsets for a potential child
                offset_x += area.width - 1; // -1 to overlap borders
                offset_y += layer.selected_index().unwrap_or(0) as u16;

                area
            })
            .collect()
    }
}

impl Component for ActionMenuContent {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(
        &mut self,
        context: &mut UpdateContext,
        event: Event,
    ) -> EventMatch {
        let mut prop = event
            .m()
            .click(|position, propagate| {
                // Select clicked layer
                for (i, layer) in self.stack.iter().enumerate() {
                    if layer.contains(context, position) {
                        self.active_layer = i;
                        // Clicks are propagated by default, but we want to eat!
                        propagate.unset();
                    }
                }
            })
            .action(|action, propagate| match action {
                // Navigate between layers with left/right
                Action::Left => self.previous_layer(),
                Action::Right => self.next_layer(),
                // Check if the input is bound to any action in *any* list in
                // the stack
                _ if self.select_by_action(action) => {
                    // Submission is deferred because it requires an
                    // owned value
                    self.emitter.emit(ActionSubmit);
                }
                _ => propagate.set(),
            });

        // Check for events from all layers of the stack. It's possible for
        // inactive layers to emit events because of mouse input (e.g.
        // scrolling).
        //
        // Iterate with indexes + while loop because we may modify the stack
        // while iterating. We need to recheck the bound after each iteration
        // to prevent iterating past the end when children are cleared
        let mut layer = 0;
        while layer < self.stack.len() {
            let emitter = self.stack[layer].to_emitter();
            prop = prop.emitted(emitter, |event| match event.kind {
                SelectEventKind::Select => {
                    // When changing selection, any existing child menus are no
                    // longer relevant so close them
                    self.clear_children(layer);
                    // If the selected item is a group, open a new child menu
                    let selected = &self.stack[layer][event];
                    if let MenuItemDisplay::Group { children, .. } = selected {
                        self.open_group(children.clone());
                    }
                }
                SelectEventKind::Submit => {
                    // Submitting on an action closes the menu and emits the
                    // action. Submitting on a group moves to the children
                    //
                    // We have to handle the submission event here instead of
                    // letting the modal queue handle it because **not all
                    // submissions close the modal**; submission on a group
                    // just enters the next layer
                    let selected = &self.stack[layer][event];
                    match selected {
                        MenuItemDisplay::Action { .. } => {
                            // Submission is deferred because it requires an
                            // owned value
                            self.emitter.emit(ActionSubmit);
                        }
                        // The group should already be open because it had to be
                        // selected before it was submitted
                        MenuItemDisplay::Group { .. } => self.next_layer(),
                    }
                }
                SelectEventKind::Toggle => {}
            });
            layer += 1;
        }

        prop
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        // Reverse the list so lowest children get priority. It shouldn't
        // actually matter though because only the active menu is focused so
        // only that one gets key events, and there's no visual overlap so they
        // shouldn't be competing for mouse events

        self.stack
            .iter_mut()
            .rev()
            .map(ToChild::to_child_mut)
            .collect()
    }
}

impl Draw for ActionMenuContent {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let styles = ViewContext::styles().menu;
        let areas = self.areas(metadata.area().as_position());

        // Clear content/styling underneath all layers. This has to happen
        // before rendering any blocks so we don't clear the border from a
        // parent layer; that breaks the border merging. And we can't just clear
        // the entire parent area because there may be content within the
        // rectangle that isn't behind any single layer
        for area in &areas {
            canvas.render_widget(Clear, *area);
        }

        for (i, (layer, area)) in self.stack.iter().zip(areas).enumerate() {
            // Add border
            let block = Block::new()
                .borders(Borders::ALL)
                .border_style(styles.border)
                .border_type(styles.border_type)
                .style(styles.normal)
                .merge_borders(MergeStrategy::Fuzzy);
            let inner_area = block.inner(area);
            canvas.render_widget(block, area);

            let is_active = i == self.active_layer;
            let props = SelectListProps {
                scrollbar_margin: 0,
            };
            canvas.draw(layer, props, inner_area, is_active);
        }
    }
}

/// Emitted event to tell the parent when the user has submitted an action
#[derive(Debug)]
struct ActionSubmit;

/// An entry in an action menu
#[derive(Debug, derive_more::Display)]
pub enum MenuItem {
    /// A executable action
    #[display("{}", _0.name)]
    Action(MenuAction),
    /// A grouping of related actions, which can be opened in a nested menu
    #[display("{name}")]
    Group { name: String, children: Vec<Self> },
}

impl MenuItem {
    /// Is this menu item enabled?
    #[cfg(test)]
    pub fn enabled(&self) -> bool {
        match self {
            MenuItem::Action(action) => action.enabled,
            MenuItem::Group { .. } => true,
        }
    }
}

impl From<MenuAction> for MenuItem {
    fn from(value: MenuAction) -> Self {
        Self::Action(value)
    }
}

/// One item in an action menu modal. The action menu is built dynamically, and
/// each action is tied back to the component that supplied it via an [Emitter].
#[derive(Debug)]
pub struct MenuAction {
    name: String,
    value: Box<dyn LocalEvent>,
    /// Because actions are sourced from multiple components, we use a
    /// type-erased emitter here. When the action is selected, we'll emit it on
    /// behalf of the supplier, who will then downcast and consume it in its
    /// update() handler.
    emitter: Emitter<dyn LocalEvent>,
    enabled: bool,
    /// Input action bound to this menu action
    shortcut: Option<Action>,
}

impl MenuAction {
    /// Create a new menu action. This uses the builder-lite pattern to
    /// customize the created event
    pub fn new<T: LocalEvent>(
        emitter: Emitter<T>,
        action: T,
        name: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            emitter: emitter.upcast(),
            enabled: true,
            shortcut: None,
            value: Box::new(action),
        }
    }

    /// Enable/disable this action
    pub fn enable(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Set/clear the shortcut for this action
    pub fn shortcut(mut self, shortcut: Option<Action>) -> Self {
        self.shortcut = shortcut;
        self
    }
}

/// Minimal version of [MenuItem] that can be cloned repeatedly to build
/// [Select]s
#[derive(Clone, Debug)]
enum MenuItemDisplay {
    Action {
        name: String,
        enabled: bool,
        shortcut: Option<Action>,
    },
    Group {
        name: String,
        children: Vec<Self>,
    },
}

impl Display for MenuItemDisplay {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self {
            // If a shortcut is given, include the binding in the text
            Self::Action {
                name,
                shortcut: Some(shortcut),
                ..
            } => {
                let s = ViewContext::add_binding_hint(name, *shortcut);
                write!(fmt, "{s}")
            }
            Self::Action { name, .. } => {
                write!(fmt, "{name}")
            }
            Self::Group { name, .. } => {
                write!(fmt, "{name} ▶")
            }
        }
    }
}

impl Generate for &MenuItemDisplay {
    type Output<'this>
        = Span<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        match &self {
            MenuItemDisplay::Action { name, shortcut, .. } => {
                // If a shortcut is given, include the binding in the text
                shortcut
                    .map(|shortcut| {
                        ViewContext::add_binding_hint(name, shortcut).into()
                    })
                    .unwrap_or_else(|| name.as_str().into())
            }
            MenuItemDisplay::Group { name, .. } => format!("{name} ▶").into(),
        }
    }
}

/// Map data tree to a tree that can be freely cloned and displayed
fn map_items(items: &[MenuItem]) -> Vec<MenuItemDisplay> {
    items
        .iter()
        .map(|item| match item {
            MenuItem::Action(action) => MenuItemDisplay::Action {
                name: action.name.clone(),
                enabled: action.enabled,
                shortcut: action.shortcut,
            },
            MenuItem::Group { name, children } => MenuItemDisplay::Group {
                name: name.clone(),
                // Recursion!
                children: map_items(children),
            },
        })
        .collect()
}

/// Build a select state from a list of menu items
fn build_select(items: Vec<MenuItemDisplay>) -> Select<MenuItemDisplay> {
    let disabled_indexes = items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            matches!(item, MenuItemDisplay::Action { enabled, .. } if !enabled)
        })
        .map(|(i, _)| i)
        .collect_vec();

    Select::builder(items)
        .disabled_indexes(disabled_indexes)
        .subscribe([SelectEventKind::Select, SelectEventKind::Submit])
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestTerminal, terminal},
        view::{
            event::ToEmitter,
            test_util::{TestComponent, TestHarness, harness},
        },
    };
    use rstest::rstest;
    use terminput::KeyCode;

    /// A component that provides some actions
    #[derive(derive_more::Debug)]
    struct Actionable {
        id: ComponentId,
        emitter: Emitter<TestAction>,
        /// List of returned actions is customizable for different test cases
        #[debug(skip)]
        get_actions: Box<dyn Fn(Emitter<TestAction>) -> Vec<MenuItem>>,
    }

    impl Actionable {
        fn new(
            get_actions: impl 'static + Fn(Emitter<TestAction>) -> Vec<MenuItem>,
        ) -> Self {
            Self {
                id: ComponentId::default(),
                emitter: Default::default(),
                get_actions: Box::new(get_actions),
            }
        }
    }

    impl Default for Actionable {
        fn default() -> Self {
            // By default, return all actions
            let get_actions = |emitter: Emitter<TestAction>| {
                vec![
                    // Disablify is first to test that disabled actions are
                    // skipped
                    emitter
                        .menu(TestAction::Disabled, "Disabled")
                        .enable(false)
                        .shortcut(Some(Action::Reset))
                        .into(),
                    emitter.menu(TestAction::Action1, "Action 1").into(),
                    emitter.menu(TestAction::Action2, "Action 2").into(),
                    emitter
                        .menu(TestAction::Shortcutted, "Shortcutted")
                        .shortcut(Some(Action::Edit))
                        .into(),
                    MenuItem::Group {
                        name: "Nested".into(),
                        children: vec![
                            emitter
                                .menu(TestAction::Nested1, "Nested 1")
                                .into(),
                            emitter
                                .menu(TestAction::Nested2, "Nested 2")
                                .into(),
                            MenuItem::Group {
                                name: "Nested Group".into(),
                                children: vec![
                                    emitter
                                        .menu(
                                            TestAction::NestedNested1,
                                            "Nested Nested 1",
                                        )
                                        .into(),
                                ],
                            },
                        ],
                    },
                ]
            };
            Self::new(get_actions)
        }
    }

    impl Component for Actionable {
        fn id(&self) -> ComponentId {
            self.id
        }

        fn menu(&self) -> Vec<MenuItem> {
            (self.get_actions)(self.to_emitter())
        }
    }

    impl Draw for Actionable {
        fn draw(&self, _: &mut Canvas, (): (), _: DrawMetadata) {}
    }

    impl ToEmitter<TestAction> for Actionable {
        fn to_emitter(&self) -> Emitter<TestAction> {
            self.emitter
        }
    }

    #[derive(Debug, PartialEq)]
    enum TestAction {
        Disabled,
        Action1,
        Action2,
        Shortcutted,
        // Second level!!
        Nested1,
        Nested2,
        // Third level!!
        NestedNested1,
    }

    /// Test basic action menu interactions
    #[rstest]
    fn test_actions(harness: TestHarness, terminal: TestTerminal) {
        let mut component =
            TestComponent::new(&harness, &terminal, Actionable::default());

        // Select a basic action
        component
            .int()
            .action(&["Action 2"])
            .assert()
            .emitted([TestAction::Action2]);

        // Actions can be performed by shortcut
        component
            .int()
            .send_keys([KeyCode::Char('x'), KeyCode::Char('e')])
            .assert()
            .emitted([TestAction::Shortcutted]);

        // Disabled action *cannot* be performed by shortcut
        component
            .int()
            .send_keys([KeyCode::Char('x'), KeyCode::Char('z')])
            .assert()
            .emitted([]);
    }

    /// Various input sequences on multiple levels of nested actions
    #[rstest]
    // Navigate to the nested menu by arrow key
    #[case::right_arrow(
        &[KeyCode::Up, KeyCode::Right, KeyCode::Down, KeyCode::Enter],
        TestAction::Nested2,
    )]
    // Navigate back up to a parent layer with the left arrow
    #[case::left_arrow(
        &[KeyCode::Up, KeyCode::Right, KeyCode::Left, KeyCode::Up, KeyCode::Enter],
        TestAction::Shortcutted,
    )]
    // Navigate to the nested menu by Enter
    #[case::enter(
        &[KeyCode::Up, KeyCode::Enter, KeyCode::Enter],
        TestAction::Nested1,
    )]
    // Navigate to the innermost menu
    #[case::nested_nested(
        &[KeyCode::Up, KeyCode::Right, KeyCode::Up, KeyCode::Right, KeyCode::Enter],
        TestAction::NestedNested1,
    )]
    // Shortcuts should work regardless of which layer is active
    #[case::shortcut_from_other(
        &[KeyCode::Up, KeyCode::Right, KeyCode::Char('e')],
        TestAction::Shortcutted,
    )]
    fn test_actions_nested(
        harness: TestHarness,
        #[with(80, 20)] terminal: TestTerminal,
        #[case] inputs: &[KeyCode],
        #[case] expected_action: TestAction,
    ) {
        let mut component =
            TestComponent::new(&harness, &terminal, Actionable::default());
        component
            .int()
            .send_key(KeyCode::Char('x'))
            .send_keys(inputs.iter().copied())
            .assert()
            .emitted([expected_action]);
    }

    /// There once was a bug where the select event wasn't handled correctly
    /// for the pre-selected item in the list. If the first item is a group, it
    /// should open correctly on first draw.
    #[rstest]
    fn test_first_group_selected(harness: TestHarness, terminal: TestTerminal) {
        let get_actions = |emitter: Emitter<TestAction>| {
            vec![MenuItem::Group {
                name: "Nested".into(),
                children: vec![
                    emitter.menu(TestAction::Nested1, "Nested 1").into(),
                    emitter.menu(TestAction::Nested2, "Nested 2").into(),
                ],
            }]
        };

        let mut component = TestComponent::new(
            &harness,
            &terminal,
            Actionable::new(get_actions),
        );

        // Group should be expanded when the modal is first opened
        component
            .int()
            .send_keys([KeyCode::Char('x'), KeyCode::Right, KeyCode::Enter])
            .assert()
            .emitted([TestAction::Nested1]);
    }
}
