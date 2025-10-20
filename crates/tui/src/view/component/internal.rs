//! Internal state for the [Component] struct. This defines common logic for
//! components., and exposes a small API for accessing both local and global
//! component state.

use crate::view::{
    common::actions::MenuAction, context::UpdateContext, event::Event,
};
use derive_more::Display;
use persisted::{PersistedContainer, PersistedLazyRefMut, PersistedStore};
use ratatui::{Frame, layout::Rect};
use std::{
    any,
    cell::RefCell,
    collections::HashMap,
    fmt::Debug,
    ops::{Deref, DerefMut},
};
use terminput::{
    Event::{Key, Mouse, Paste},
    MouseEvent,
};
use tracing::{instrument, trace, trace_span, warn};
use uuid::Uuid;

thread_local! {
    /// All components that were drawn during the last draw phase. The purpose
    /// of this is to allow each component to return an exhaustive list of its
    /// children during event handling, then we can automatically filter that
    /// list down to just the ones that are visible. This prevents the need to
    /// duplicate visibility logic in both the draw and the children getters.
    ///
    /// This is cleared at the start of each draw, then retained through the
    /// next draw.
    ///
    /// For each drawn component, this stores metadata related to its last draw.
    /// We store this data out-of-band because it simplifies what each
    /// individual component has to store, and centralizes the interior
    /// mutability.
    static VISIBLE_COMPONENTS: RefCell<HashMap<ComponentId, DrawMetadata>> =
        Default::default();

    /// Track whichever components are *currently* being drawn. Whenever we
    /// draw a child, push it onto the stack. Pop off when done drawing it. This
    /// makes it easy to track when we're done with a draw phase.
    static STACK: RefCell<Vec<ComponentId>> = Default::default();
}

/// A UI element that can handle user/async input.
///
/// This trait facilitates an on-demand tree structure, where each node in the
/// tree can furnish its list of children. Events will be propagated bottom-up
/// (i.e. leaf-to-root), and each element has the opportunity to consume the
/// event so it stops bubbling. Each instance of each component gets a unique ID
/// that identifies it in the component tree during both event handling and
/// drawing. See [Component::id].
///
/// While components *typically* can be drawn to the screen, draw functionality
/// is not provided by this trait. Instead, it's a separate trait called [Draw].
/// See that trait for an explanation why.
pub trait Component: ToChild {
    /// Get a unique ID for this component
    ///
    /// **The returned ID must be consistent between draws.** The implementing
    /// component is responsible for generating an ID for itself and returning
    /// the same ID on each call. See [ComponentId] for more.
    fn id(&self) -> ComponentId;

    /// Update the state of *just* this component according to the event.
    /// Returned outcome indicates whether the event was consumed (`None`), or
    /// it should be propagated to our parent (`Some`). Use
    /// [EventQueue](crate::view::event::EventQueue) to queue subsequent
    /// events, and the given message sender to queue async messages.
    ///
    /// Generally event matching should be done with [Event::opt] and the
    /// matching methods defined by
    /// [OptionEvent](crate::view::event::OptionEvent).
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        Some(event)
    }

    /// Provide a list of actions that are accessible from the actions menu.
    /// This list may be static (e.g. determined from an enum) or dynamic. When
    /// the user opens the actions menu, all available actions for all
    /// **focused** components will be collected and show in the menu. If an
    /// action is selected, an event will be emitted with that action value.
    fn menu_actions(&self) -> Vec<MenuAction> {
        Vec::new()
    }

    /// Get **all** children of this component. This includes children that are
    /// not currently visible, and ones that are out of focus, meaning they
    /// shouldn't receive keyboard events. The event handling infrastructure is
    /// responsible for filtering out children that shouldn't receive events.
    ///
    /// The event handling sequence goes something like:
    /// - Get list of children
    /// - Filter out children that aren't visible
    /// - For keyboard events, filter out children that aren't in focus (mouse
    ///   events can still be handled by unfocused components)
    /// - Pass the event to the first child in the list
    ///     - If it consumes the event, stop
    ///     - If it propagates, move on to the next child, and so on
    /// - If none of the children consume the event, go up the tree to the
    ///   parent and try again.
    fn children(&mut self) -> Vec<Child<'_>> {
        Vec::new()
    }
}

// This can't be a blanket impl on DerefMut because that causes a collision in
// the blanket impls of ToChild
impl<S, K, C> Component for PersistedLazyRefMut<'_, S, K, C>
where
    S: PersistedStore<K>,
    K: persisted::PersistedKey,
    K::Value: Debug + PartialEq,
    C: Component + PersistedContainer<Value = K::Value>,
{
    fn id(&self) -> ComponentId {
        self.deref().id()
    }

    fn update(
        &mut self,
        context: &mut UpdateContext,
        event: Event,
    ) -> Option<Event> {
        self.deref_mut().update(context, event)
    }

    fn menu_actions(&self) -> Vec<MenuAction> {
        self.deref().menu_actions()
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        self.deref_mut().children()
    }
}

/// An extension trait for [Component]
///
/// This provides all the functionality of [Component] that does *not* need to
/// be implemented by each individual component type. Any method on a component
/// that does not need to be (or cannot be) overridden by implementors of
/// [Component] should be defined here instead.
pub trait ComponentExt: Component {
    /// Was this component drawn to the screen during the previous draw phase?
    fn is_visible(&self) -> bool;

    /// Collect all available menu actions from all **focused** descendents of
    /// this component (including this component). This takes a mutable
    /// reference so we don't have to duplicate the code that provides children;
    /// it will *not* mutate anything.
    fn collect_actions(&mut self) -> Vec<MenuAction>
    where
        Self: Sized;

    /// Handle an event for this component *or* its children, starting at the
    /// lowest descendant. Recursively walk up the tree until a component
    /// consumes the event.
    fn update_all(
        &mut self,
        context: &mut UpdateContext,
        event: Event,
    ) -> Option<Event>
    where
        Self: Sized;

    /// Draw the component into the frame
    ///
    /// This is what you should call when you want to draw a child component.
    /// **This should not be reimplemented by implementors of this trait.** Just
    /// implement [Draw::draw_impl] instead.
    fn draw<Props>(
        &self,
        frame: &mut Frame,
        props: Props,
        area: Rect,
        has_focus: bool,
    ) where
        Self: Draw<Props>;
}

impl<T: Component + ?Sized> ComponentExt for T {
    fn is_visible(&self) -> bool {
        VISIBLE_COMPONENTS.with_borrow(|map| map.contains_key(&self.id()))
    }

    fn collect_actions(&mut self) -> Vec<MenuAction>
    where
        Self: Sized,
    {
        fn inner(actions: &mut Vec<MenuAction>, component: &mut dyn Component) {
            // Only include actions from visible+focused components
            if component.is_visible() && has_focus(component) {
                actions.extend(component.menu_actions());
                for mut child in component.children() {
                    inner(actions, child.component());
                }
            }
        }

        let mut actions = Vec::new();
        inner(&mut actions, self);
        actions
    }

    fn update_all(
        &mut self,
        context: &mut UpdateContext,
        event: Event,
    ) -> Option<Event>
    where
        Self: Sized,
    {
        update_all(any::type_name::<Self>(), self, context, event)
    }

    fn draw<Props>(
        &self,
        frame: &mut Frame,
        props: Props,
        area: Rect,
        has_focus: bool,
    ) where
        Self: Draw<Props>,
    {
        // Update internal state for event handling
        let metadata = DrawMetadata { area, has_focus };
        let guard = DrawGuard::new(self.id(), metadata);

        self.draw_impl(frame, props, metadata);
        drop(guard); // Make sure guard stays alive until here
    }
}

/// Something that can be drawn onto screen as one or more TUI widgets.
///
/// Conceptually this is basically part of `Component`, but having it separate
/// allows the `Props` type parameter. Otherwise, there's no way to make a
/// trait object from `Component` across components with different props.
///
/// Props are additional temporary values that a struct may need in order
/// to render. Useful for passing down state values that are managed by
/// the parent to avoid duplicating that state in the child. In most
/// cases, `Props` would make more sense as an associated type, but there are
/// some component types (e.g. `SelectState`) that have multiple `Draw` impls.
/// Using an associated type also makes prop types with lifetimes much less
/// ergonomic.
pub trait Draw<Props = ()>: Component {
    /// Draw the component into the frame.
    ///
    /// This is what each component will implement itself, but this **should not
    /// be called directly.** Instead, call [ComponentExt::draw] on child
    /// components to ensure the wrapping draw logic is called correctly.
    /// This is called `draw_impl` instead of `draw` to make it distinct
    /// from [ComponentExt::draw].
    fn draw_impl(
        &self,
        frame: &mut Frame,
        props: Props,
        metadata: DrawMetadata,
    );
}

/// Metadata associated with each draw action, which may instruct how the draw
/// should occur.
#[derive(Copy, Clone, Debug, Default)]
pub struct DrawMetadata {
    /// Which area on the screen should we draw to?
    area: Rect,
    /// Does the drawn component have focus? Focus indicates the component
    /// receives keyboard events. Most of the time, the focused element should
    /// get some visual indicator that it's in focus.
    has_focus: bool,
}

impl DrawMetadata {
    /// Which area on the screen should we draw to?
    pub fn area(self) -> Rect {
        self.area
    }

    /// Does the component have focus, i.e. is it the component that should
    /// receive keyboard events?
    pub fn has_focus(self) -> bool {
        self.has_focus
    }
}

/// A wrapper for a dynamically dispatched [Component]. This is used to
/// return a collection of event handlers from [Component::children]. Almost
/// all cases will use the [Borrowed](Self::Borrowed) variant, but
/// [Owned](Self::Owned) is useful for types that need to wrap the mutable
/// reference in some type of guard. See [ToChild].
pub enum Child<'a> {
    Borrowed {
        name: &'static str,
        component: &'a mut dyn Component,
    },
    Owned {
        name: &'static str,
        component: Box<dyn 'a + Component>,
    },
}

impl<'a> Child<'a> {
    /// Get a descriptive name for this component type
    pub fn name(&self) -> &'static str {
        match self {
            Self::Borrowed { name, .. } => name,
            Self::Owned { name, .. } => name,
        }
    }

    /// Get the contained component trait object
    pub fn component<'b>(&'b mut self) -> &'b mut dyn Component
    where
        // 'b is the given &self, 'a is the contained &dyn Component
        'a: 'b,
    {
        match self {
            Self::Borrowed { component, .. } => *component,
            Self::Owned { component, .. } => &mut **component,
        }
    }
}

/// Abstraction to convert a component type into [Child], which is a wrapper for
/// a trait object. For 99% of components the blanket implementation will cover
/// this. This only needs to be implemented manually for types that need an
/// extra step to extract mutable data.
pub trait ToChild {
    fn to_child_mut(&mut self) -> Child<'_>;
}

impl<T: Component + Sized> ToChild for T {
    fn to_child_mut(&mut self) -> Child<'_> {
        Child::Borrowed {
            name: any::type_name::<Self>(),
            component: self,
        }
    }
}

/// A mutable reference to the contents of [persisted::PersistedLazy] must be
/// wrapped in [PersistedLazyRefMut], which requires us to return an owned child
/// rather than a borrowed one.
impl<S, K, C> ToChild for persisted::PersistedLazy<S, K, C>
where
    S: PersistedStore<K>,
    K: persisted::PersistedKey,
    K::Value: Debug + PartialEq,
    C: Component + PersistedContainer<Value = K::Value>,
{
    fn to_child_mut(&mut self) -> Child<'_> {
        Child::Owned {
            name: any::type_name::<Self>(),
            component: Box::new(self.get_mut()),
        }
    }
}

/// Handle an event for an entire component tree. This is the internal
/// implementation for [ComponentExt::update_all].
#[instrument(
    level = "trace",
    skip_all,
    fields(component = format_type_name(name)),
)]
fn update_all(
    name: &str,
    component: &mut dyn Component,
    context: &mut UpdateContext,
    mut event: Event,
) -> Option<Event> {
    // If we can't handle the event, our children can't either
    if !should_handle(component, &event) {
        return Some(event);
    }

    // If we have a child, send them the event. If not, eat it ourselves
    for mut child in component.children() {
        // RECURSION
        let propagated =
            update_all(child.name(), child.component(), context, event);
        match propagated {
            Some(returned) => {
                // Keep going to the next child. The propagated event
                // *should* just be whatever we passed in, but we have
                // no way of verifying that
                event = returned;
            }
            None => {
                return None;
            }
        }
    }

    // None of our children handled it, we'll take it ourselves. Event is
    // already traced in the root span, so don't dupe it.
    trace_span!("component.update").in_scope(|| {
        let update = component.update(context, event);
        trace!(propagated = ?update);
        update
    })
}

/// Get a minified name for a type. Common prefixes are stripped from the type
/// to reduce clutter
fn format_type_name(type_name: &str) -> String {
    type_name
        .replace("slumber_tui::view::common::", "")
        .replace("slumber_tui::view::component::", "")
        .replace("slumber_tui::view::test_util::", "")
        .replace("slumber_tui::view::util::", "")
}

/// Should this component handle the given event? This is based on a few
/// criteria:
/// - Am I currently visible? I.e. was I drawn on the last draw phase?
/// - If it's a non-mouse event, do I have focus?
/// - If it's a mouse event, was it over me? Mouse events should always go to
///   the clicked element, even when unfocused, because that's intuitive.
fn should_handle(component: &dyn Component, event: &Event) -> bool {
    // If this component isn't currently in the visible tree, it shouldn't
    // handle any events
    if !component.is_visible() {
        trace!("Skipping component, not visible");
        return false;
    }

    if let Event::Input { event, .. } = event {
        match event {
            Key(_) | Paste(_) => has_focus(component),

            Mouse(mouse_event) => {
                // Check if the mouse is over the component
                intersects(component, *mouse_event)
            }

            // We expect everything else to have already been killed
            _ => {
                warn!(?event, "Unexpected event kind");
                false
            }
        }
    } else {
        true
    }
}

/// Did the given mouse event occur over/on this component?
fn intersects(component: &dyn Component, mouse_event: MouseEvent) -> bool {
    // If the component isn't in the map, that means it's not visible
    VISIBLE_COMPONENTS.with_borrow(|map| {
        let metadata = map.get(&component.id());
        metadata.is_some_and(|metadata| {
            metadata.area().intersects(Rect {
                x: mouse_event.column,
                y: mouse_event.row,
                width: 1,
                height: 1,
            })
        })
    })
}

/// Was this component in focus during the previous draw phase?
fn has_focus(component: &dyn Component) -> bool {
    // If the component isn't in the map, that means it's not visible
    VISIBLE_COMPONENTS.with_borrow(|map| {
        let metadata = map.get(&component.id());
        metadata.is_some_and(|metadata| metadata.has_focus())
    })
}

/// Unique ID to refer to a single component
///
/// A component should generate a unique ID for itself upon construction (via
/// [ComponentId::new] or [ComponentId::default]) and use the same ID for its
/// entire lifespan. This ID should be returned from [Component::id].
#[derive(Copy, Clone, Debug, Display, Eq, Hash, PartialEq)]
pub struct ComponentId(Uuid);

impl ComponentId {
    fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

/// Generate a new random ID
impl Default for ComponentId {
    fn default() -> Self {
        Self::new()
    }
}

/// A RAII guard to ensure the mutable thread-global state is updated correctly
/// throughout the span of a draw. This should be created at the start of each
/// component's draw, and dropped at the finish of that draw.
struct DrawGuard {
    id: ComponentId,
    is_root: bool,
}

impl DrawGuard {
    fn new(id: ComponentId, metadata: DrawMetadata) -> Self {
        // Push onto the render stack, so children know about their parent
        let is_root = STACK.with_borrow_mut(|stack| {
            stack.push(id);
            stack.len() == 1
        });

        VISIBLE_COMPONENTS.with_borrow_mut(|visible_components| {
            // If we're the root component, then anything in the visibility list
            // is from the previous draw, so we want to clear it out
            if is_root {
                visible_components.clear();
            }
            visible_components.insert(id, metadata);
        });
        Self { id, is_root }
    }
}

impl Drop for DrawGuard {
    fn drop(&mut self) {
        let popped = STACK.with_borrow_mut(std::vec::Vec::pop);

        // Do some sanity checks here
        match popped {
            Some(popped) if popped == self.id => {}
            Some(popped) => panic!(
                "Popped incorrect component off render stack; \
                expected `{expected}`, got `{popped}`",
                expected = self.id
            ),
            None => panic!(
                "Failed to pop component `{expected}` off render stack; \
                stack is empty",
                expected = self.id
            ),
        }
        if self.is_root {
            assert!(
                STACK.with_borrow(std::vec::Vec::is_empty),
                "Render stack is not empty after popping root component"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{TestHarness, TestTerminal, harness, terminal};
    use ratatui::layout::Layout;
    use rstest::{fixture, rstest};
    use slumber_config::Action;
    use slumber_util::assert_matches;
    use terminput::{
        KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers,
        MouseButton, MouseEventKind,
    };

    #[derive(Debug, Default)]
    struct Branch {
        id: ComponentId,
        /// How many events have we consumed *ourselves*?
        count: u32,
        a: Leaf,
        b: Leaf,
        c: Leaf,
    }

    struct Props {
        a: Mode,
        b: Mode,
        c: Mode,
    }

    enum Mode {
        Focused,
        Visible,
        Hidden,
    }

    impl Branch {
        fn reset(&mut self) {
            self.count = 0;
            self.a.reset();
            self.b.reset();
            self.c.reset();
        }
    }

    impl Component for Branch {
        fn id(&self) -> ComponentId {
            self.id
        }

        fn update(&mut self, _: &mut UpdateContext, _: Event) -> Option<Event> {
            self.count += 1;
            None
        }

        fn children(&mut self) -> Vec<Child<'_>> {
            vec![
                self.a.to_child_mut(),
                self.b.to_child_mut(),
                self.c.to_child_mut(),
            ]
        }
    }

    impl Draw<Props> for Branch {
        fn draw_impl(
            &self,
            frame: &mut Frame,
            props: Props,
            metadata: DrawMetadata,
        ) {
            let [a_area, b_area, c_area] =
                Layout::vertical([1, 1, 1]).areas(metadata.area());

            for (component, area, mode) in [
                (&self.a, a_area, props.a),
                (&self.b, b_area, props.b),
                (&self.c, c_area, props.c),
            ] {
                if !matches!(mode, Mode::Hidden) {
                    component.draw(
                        frame,
                        (),
                        area,
                        matches!(mode, Mode::Focused),
                    );
                }
            }
        }
    }

    #[derive(Debug, Default)]
    struct Leaf {
        id: ComponentId,
        /// How many events have we consumed?
        count: u32,
    }

    impl Leaf {
        fn reset(&mut self) {
            self.count = 0;
        }
    }

    impl Component for Leaf {
        fn id(&self) -> ComponentId {
            self.id
        }

        fn update(&mut self, _: &mut UpdateContext, _: Event) -> Option<Event> {
            self.count += 1;
            None
        }
    }

    impl Draw for Leaf {
        fn draw_impl(&self, frame: &mut Frame, (): (), metadata: DrawMetadata) {
            frame.render_widget("hello!", metadata.area());
        }
    }

    fn keyboard_event() -> Event {
        Event::Input {
            event: terminput::Event::Key(KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            }),
            action: Some(Action::Submit),
        }
    }

    fn mouse_event((x, y): (u16, u16)) -> Event {
        Event::Input {
            event: terminput::Event::Mouse(MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: x,
                row: y,
                modifiers: KeyModifiers::NONE,
            }),
            action: Some(Action::LeftClick),
        }
    }

    /// Get a testing component. This *doesn't* use `TestComponent` because
    /// we want to test logic internal to the component, so we need to do some
    /// wonky things unique to these tests that require calling the component
    /// methods directly.
    #[fixture]
    fn component() -> Branch {
        Branch::default()
    }

    /// Render a simple component tree and test that events are propagated as
    /// expected, and that state updates as the visible and focused components
    /// change.
    #[rstest]
    fn test_render_component_tree(
        harness: TestHarness,
        terminal: TestTerminal,
        mut component: Branch,
    ) {
        // One level of nesting
        let area = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 3,
        };
        let a_coords = (0, 0);
        let b_coords = (0, 1);
        let c_coords = (0, 2);

        let assert_events =
            |component: &mut Branch, expected_counts: [u32; 4]| {
                let events = [
                    keyboard_event(),
                    mouse_event(a_coords),
                    mouse_event(b_coords),
                    mouse_event(c_coords),
                ];
                // Now visible components get events
                let mut update_context = UpdateContext {
                    request_store: &mut harness.request_store.borrow_mut(),
                };
                for event in events {
                    component.update_all(&mut update_context, event);
                }
                let [expected_root, expected_a, expected_b, expected_c] =
                    expected_counts;
                assert_eq!(
                    component.count, expected_root,
                    "count mismatch on root component"
                );
                assert_eq!(
                    component.a.count, expected_a,
                    "count mismatch on component a"
                );
                assert_eq!(
                    component.b.count, expected_b,
                    "count mismatch on component b"
                );
                assert_eq!(
                    component.c.count, expected_c,
                    "count mismatch on component c"
                );

                // Reset state for the next assertion
                component.reset();
            };

        // Initial event handling - nothing is visible so nothing should consume
        assert_events(&mut component, [0, 0, 0, 0]);

        // Visible components get events
        terminal.draw(|frame| {
            component.draw(
                frame,
                Props {
                    a: Mode::Focused,
                    b: Mode::Visible,
                    c: Mode::Hidden,
                },
                area,
                true,
            );
        });
        // Root - inherited mouse event from c, which is hidden
        // a - keyboard + mouse
        // b - mouse
        // c - hidden
        assert_events(&mut component, [1, 2, 1, 0]);

        // Switch things up, make sure new state is reflected
        terminal.draw(|frame| {
            component.draw(
                frame,
                Props {
                    a: Mode::Visible,
                    b: Mode::Hidden,
                    c: Mode::Focused,
                },
                area,
                true,
            );
        });
        // Root - inherited mouse event from b, which is hidden
        // a - mouse
        // b - hidden
        // c - keyboard + mouse
        assert_events(&mut component, [1, 1, 0, 2]);

        // Hide all children, root should eat everything
        terminal.draw(|frame| {
            component.draw(
                frame,
                Props {
                    a: Mode::Hidden,
                    b: Mode::Hidden,
                    c: Mode::Hidden,
                },
                area,
                true,
            );
        });
        assert_events(&mut component, [4, 0, 0, 0]);
    }

    /// If the parent component is hidden, nobody gets to see events, even if
    /// the children have been drawn. This is a very odd scenario and shouldn't
    /// happen in the wild, but it's good to have it be well-defined.
    #[rstest]
    fn test_parent_hidden(
        harness: TestHarness,
        terminal: TestTerminal,
        mut component: Branch,
    ) {
        terminal.draw(|frame| {
            let area = frame.area();
            component.a.draw(frame, (), area, true);
            component.b.draw(frame, (), area, true);
            component.c.draw(frame, (), area, true);
        });
        // Event should *not* be handled because the parent is hidden
        assert_matches!(
            component.update_all(
                &mut UpdateContext {
                    request_store: &mut harness.request_store.borrow_mut(),
                },
                keyboard_event()
            ),
            Some(_)
        );
    }

    /// If the parent is unfocused but the child is focused, the child should
    /// *not* receive focus-only events.
    #[rstest]
    fn test_parent_unfocused(
        harness: TestHarness,
        terminal: TestTerminal,
        mut component: Branch,
    ) {
        // We are visible but *not* in focus
        terminal.draw(|frame| {
            let area = frame.area();
            component.draw(
                frame,
                Props {
                    a: Mode::Focused,
                    b: Mode::Visible,
                    c: Mode::Visible,
                },
                area,
                false,
            );
        });

        // Event should *not* be handled because the parent is unfocused
        assert_matches!(
            component.update_all(
                &mut UpdateContext {
                    request_store: &mut harness.request_store.borrow_mut(),
                },
                keyboard_event()
            ),
            Some(_)
        );
    }
}
