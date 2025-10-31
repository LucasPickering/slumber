//! Internal state for the [Component] struct. This defines common logic for
//! components., and exposes a small API for accessing both local and global
//! component state.

use crate::view::{
    common::actions::MenuItem,
    context::UpdateContext,
    event::{Event, EventMatch},
    util::format_type_name,
};
use derive_more::Display;
use persisted::{PersistedContainer, PersistedLazyRefMut, PersistedStore};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    widgets::{StatefulWidget, Widget},
};
use std::{
    any,
    cell::RefCell,
    collections::HashMap,
    mem,
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
    /// Generally event matching should be done with [Event::m] and the
    /// matching methods defined by [EventMatch].
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event.m()
    }

    /// Provide a list of actions that are accessible from the actions menu.
    /// This list may be static (e.g. determined from an enum) or dynamic. When
    /// the user opens the actions menu, all available actions for all
    /// **focused** components will be collected and show in the menu. If an
    /// action is selected, an event will be emitted with that action value.
    fn menu(&self) -> Vec<MenuItem> {
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
    K::Value: PartialEq,
    C: Component + PersistedContainer<Value = K::Value>,
{
    fn id(&self) -> ComponentId {
        self.deref().id()
    }

    fn update(
        &mut self,
        context: &mut UpdateContext,
        event: Event,
    ) -> EventMatch {
        self.deref_mut().update(context, event)
    }

    fn menu(&self) -> Vec<MenuItem> {
        self.deref().menu()
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
    fn collect_actions(&mut self) -> Vec<MenuItem>
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
}

impl<T: Component + ?Sized> ComponentExt for T {
    fn is_visible(&self) -> bool {
        VISIBLE_COMPONENTS.with_borrow(|map| map.contains_key(&self.id()))
    }

    fn collect_actions(&mut self) -> Vec<MenuItem>
    where
        Self: Sized,
    {
        fn inner(items: &mut Vec<MenuItem>, component: &mut dyn Component) {
            // Only include actions from visible+focused components
            if component.is_visible() && has_focus(component) {
                items.extend(component.menu());
                for mut child in component.children() {
                    inner(items, child.component());
                }
            }
        }

        let mut items = Vec::new();
        inner(&mut items, self);
        items
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
pub trait Draw<Props = ()> {
    /// Draw the component into the frame.
    ///
    /// This is what each component will implement itself, but this **should not
    /// be called directly.** Instead, call [Canvas::draw] to ensure the
    /// wrapping draw logic is called correctly.
    fn draw(&self, canvas: &mut Canvas, props: Props, metadata: DrawMetadata);
}

/// A wrapper around a [Frame] that manages draw state for a single frame of
/// drawing.
#[derive(derive_more::Debug)]
pub struct Canvas<'buf, 'fr> {
    frame: &'fr mut Frame<'buf>,
    /// Each [Portal] element is rendered to its own buffer. The buffers are
    /// then merged together into the main frame buffer at the end of the draw.
    /// If there are multiple portals, the *later* rendered portals take
    /// priority.
    portals: Vec<Buffer>,
}

impl<'buf, 'fr> Canvas<'buf, 'fr> {
    /// Wrap a frame for a single walk down the draw tree
    pub fn new(frame: &'fr mut Frame<'buf>) -> Self {
        Self {
            frame,
            portals: vec![],
        }
    }

    /// Draw an entire component tree to the canvas
    pub fn draw_all<T, Props>(
        frame: &'fr mut Frame<'buf>,
        root: &T,
        props: Props,
    ) where
        T: Component + Draw<Props>,
    {
        // Clear the set of visible components so we can start fresh
        VISIBLE_COMPONENTS.with_borrow_mut(HashMap::clear);

        let mut canvas = Self::new(frame);
        canvas.draw(root, props, canvas.area(), true);

        // Merge portaled buffers into the main buffer
        let main_buffer = canvas.frame.buffer_mut();
        for portal_buffer in &canvas.portals {
            main_buffer.merge(portal_buffer);
        }
    }

    /// Draw a component to the screen
    ///
    /// ## Params
    ///
    /// - `component`: Component to draw
    /// - `props`: Arbitrary data to pass to the component's `draw()` method
    /// - `area`: Area of the screen to draw the component to
    /// - `has_focus`: Should this component receive future keyboard events?
    pub fn draw<T, Props>(
        &mut self,
        component: &T,
        props: Props,
        area: Rect,
        has_focus: bool,
    ) where
        T: Component + Draw<Props> + ?Sized,
    {
        let metadata = DrawMetadata { area, has_focus };

        // Mark this component as visible so it can receive events
        VISIBLE_COMPONENTS.with_borrow_mut(|visible_components| {
            visible_components.insert(component.id(), metadata);
        });

        component.draw(self, props, metadata);
    }

    /// Draw a component to the screen *outside of its normal draw order.*
    ///
    /// See [Portal] for more info. Unlike [Self::draw], this does *not* take
    /// an `area` param because the component determines its own draw area via
    /// [Portal::area].
    pub fn draw_portal<T, Props>(
        &mut self,
        component: &T,
        props: Props,
        has_focus: bool,
    ) where
        T: Component + Draw<Props> + Portal,
    {
        // Ask the component what area it wants to portal to. Clamp it to fit
        // inside the frame.
        let area = component.area(self.area()).clamp(self.area());

        // We want to draw the portal to its own buffer, so we merge it into the
        // main buffer *at the end*. We need a Frame to draw to, but ratatui
        // doesn't expose a way to create one. We can reuse the existing frame,
        // and just swap out its buffer with a new one.
        //
        // The portal buffer will only contain the area that the portal wants to
        // draw to. Ratatui is smart enough to shift the buffers to align by
        // area before merging.
        let main_buffer =
            mem::replace(self.frame.buffer_mut(), Buffer::empty(area));
        self.draw(component, props, area, has_focus);
        // Swap the main buffer back into the frame
        let portal = mem::replace(self.frame.buffer_mut(), main_buffer);
        // Store what we rendered, to be merged in later
        self.portals.push(portal);
    }

    /// [Frame::area]
    pub fn area(&self) -> Rect {
        self.frame.area()
    }

    /// [Frame::buffer_mut]
    pub fn buffer_mut(&mut self) -> &mut Buffer {
        self.frame.buffer_mut()
    }

    /// [Frame::render_widget]
    pub fn render_widget<W: Widget>(&mut self, widget: W, area: Rect) {
        self.frame.render_widget(widget, area);
    }

    /// [Frame::render_stateful_widget]
    pub fn render_stateful_widget<W>(
        &mut self,
        widget: W,
        area: Rect,
        state: &mut W::State,
    ) where
        W: StatefulWidget,
    {
        self.frame.render_stateful_widget(widget, area, state);
    }
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
    K::Value: PartialEq,
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
        let propagated: Option<Event> = component.update(context, event).into();

        // Little bit of logging innit
        let status = if propagated.is_some() {
            "propagated"
        } else {
            "consumed"
        };
        trace!(status);

        propagated
    })
}

/// Should this component handle the given event?
fn should_handle(component: &dyn Component, event: &Event) -> bool {
    match event {
        // These events are triggered internally and generally only consumed by
        // a single specific component. Therefore they should be handled by
        // anyone regardless of state. The intended consume will eat it,
        // everyone else will ignore it
        Event::HttpSelectRequest(_) | Event::Emitted { .. } => true,

        // Keyboard events are sent only to visible+focused components
        Event::Input {
            event: Key(_) | Paste(_),
            ..
        } => has_focus(component),

        // Mouse events are sent to any visible component under the event
        Event::Input {
            event: Mouse(mouse_event),
            ..
        } => component.is_visible() && intersects(component, *mouse_event),

        // We expect everything else to have already been killed
        Event::Input { .. } => {
            warn!(?event, "Unexpected event kind");
            false
        }
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

/// An on-screen element that can be drawn outside of its normal draw order.
///
/// This allows certain types of components to subvert their normal draw order
/// and be drawn *on top* of all other components. It detachs the component's
/// draw order from its logical location in the component tree. Useful for
/// modals and other elements that must be drawn on top. This concept allows
/// components to logically live where they belong in a component tree,
/// simplifying state management and reducing the need for indirect event-based
/// logic.
pub trait Portal {
    /// Get the area of the screen that this component should be drawn to
    fn area(&self, canvas_area: Rect) -> Rect;
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

    /// The root component. This exists just to push [Branch] down the tree
    /// one layer to enable tests that hide/unfocus the branch.
    #[derive(Debug, Default)]
    struct Root {
        id: ComponentId,
        branch: Branch,
    }

    struct RootProps {
        branch_mode: Mode,
        branch_props: BranchProps,
    }

    impl Component for Root {
        fn id(&self) -> ComponentId {
            self.id
        }

        fn children(&mut self) -> Vec<Child<'_>> {
            vec![self.branch.to_child_mut()]
        }
    }

    impl Draw<RootProps> for Root {
        fn draw(
            &self,
            canvas: &mut Canvas,
            props: RootProps,
            metadata: DrawMetadata,
        ) {
            if props.branch_mode != Mode::Hidden {
                canvas.draw(
                    &self.branch,
                    props.branch_props,
                    metadata.area(),
                    props.branch_mode == Mode::Focused,
                );
            }
        }
    }

    #[derive(Debug, Default)]
    struct Branch {
        id: ComponentId,
        /// How many events have we consumed *ourselves*?
        count: u32,
        a: Leaf,
        b: Leaf,
        c: Leaf,
    }

    struct BranchProps {
        a: Mode,
        b: Mode,
        c: Mode,
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

        fn update(&mut self, _: &mut UpdateContext, _: Event) -> EventMatch {
            self.count += 1;
            None.into()
        }

        fn children(&mut self) -> Vec<Child<'_>> {
            vec![
                self.a.to_child_mut(),
                self.b.to_child_mut(),
                self.c.to_child_mut(),
            ]
        }
    }

    impl Draw<BranchProps> for Branch {
        fn draw(
            &self,
            canvas: &mut Canvas,
            props: BranchProps,
            metadata: DrawMetadata,
        ) {
            let [a_area, b_area, c_area] =
                Layout::vertical([1, 1, 1]).areas(metadata.area());

            for (component, area, mode) in [
                (&self.a, a_area, props.a),
                (&self.b, b_area, props.b),
                (&self.c, c_area, props.c),
            ] {
                if mode != Mode::Hidden {
                    canvas.draw(component, (), area, mode == Mode::Focused);
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

        fn update(&mut self, _: &mut UpdateContext, _: Event) -> EventMatch {
            self.count += 1;
            None.into()
        }
    }

    impl Draw for Leaf {
        fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
            canvas.render_widget("hello!", metadata.area());
        }
    }

    #[derive(PartialEq)]
    enum Mode {
        Focused,
        Visible,
        Hidden,
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
    fn component() -> Root {
        Root::default()
    }

    /// Render a simple component tree and test that events are propagated as
    /// expected, and that state updates as the visible and focused components
    /// change.
    #[rstest]
    fn test_render_component_tree(
        harness: TestHarness,
        terminal: TestTerminal,
        mut component: Root,
    ) {
        let a_coords = (0, 0);
        let b_coords = (0, 1);
        let c_coords = (0, 2);

        let assert_events =
            |component: &mut Root, expected_counts: [u32; 4]| {
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
                    component.branch.count, expected_root,
                    "count mismatch on root component"
                );
                assert_eq!(
                    component.branch.a.count, expected_a,
                    "count mismatch on component a"
                );
                assert_eq!(
                    component.branch.b.count, expected_b,
                    "count mismatch on component b"
                );
                assert_eq!(
                    component.branch.c.count, expected_c,
                    "count mismatch on component c"
                );

                // Reset state for the next assertion
                component.branch.reset();
            };

        // Initial event handling - nothing is visible so nothing should consume
        assert_events(&mut component, [0, 0, 0, 0]);

        // Visible components get events
        terminal.draw(|frame| {
            Canvas::draw_all(
                frame,
                &component,
                RootProps {
                    branch_mode: Mode::Focused,
                    branch_props: BranchProps {
                        a: Mode::Focused,
                        b: Mode::Visible,
                        c: Mode::Hidden,
                    },
                },
            );
        });
        // Root - inherited mouse event from c, which is hidden
        // a - keyboard + mouse
        // b - mouse
        // c - hidden
        assert_events(&mut component, [1, 2, 1, 0]);

        // Switch things up, make sure new state is reflected
        terminal.draw(|frame| {
            Canvas::draw_all(
                frame,
                &component,
                RootProps {
                    branch_mode: Mode::Focused,
                    branch_props: BranchProps {
                        a: Mode::Visible,
                        b: Mode::Hidden,
                        c: Mode::Focused,
                    },
                },
            );
        });
        // Root - inherited mouse event from b, which is hidden
        // a - mouse
        // b - hidden
        // c - keyboard + mouse
        assert_events(&mut component, [1, 1, 0, 2]);

        // Hide all children, root should eat everything
        terminal.draw(|frame| {
            Canvas::draw_all(
                frame,
                &component,
                RootProps {
                    branch_mode: Mode::Focused,
                    branch_props: BranchProps {
                        a: Mode::Hidden,
                        b: Mode::Hidden,
                        c: Mode::Hidden,
                    },
                },
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
        mut component: Root,
    ) {
        terminal.draw(|frame| {
            Canvas::draw_all(
                frame,
                &component,
                // The inner a/b/c are focused but their parent is hidden
                RootProps {
                    branch_mode: Mode::Hidden,
                    branch_props: BranchProps {
                        a: Mode::Focused,
                        b: Mode::Focused,
                        c: Mode::Focused,
                    },
                },
            );
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
        mut component: Root,
    ) {
        // We are visible but *not* in focus
        terminal.draw(|frame| {
            Canvas::draw_all(
                frame,
                &component,
                // The inner a/b/c are focused but their parent isn't
                RootProps {
                    branch_mode: Mode::Visible,
                    branch_props: BranchProps {
                        a: Mode::Focused,
                        b: Mode::Focused,
                        c: Mode::Focused,
                    },
                },
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
