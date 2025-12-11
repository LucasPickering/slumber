//! Internal state for the [Component] struct. This defines common logic for
//! components., and exposes a small API for accessing both local and global
//! component state.

use crate::{
    input::InputEvent,
    util::PersistentStore,
    view::{
        common::actions::MenuItem,
        context::UpdateContext,
        event::{Event, EventMatch},
        util::format_type_name,
    },
};
use derive_more::Display;
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Position, Rect},
    widgets::{StatefulWidget, Widget},
};
use std::{
    any,
    collections::HashMap,
    mem,
    sync::atomic::{AtomicU64, Ordering},
};
use tracing::{instrument, trace, trace_span, warn};

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
    fn update(
        &mut self,
        _context: &mut UpdateContext,
        event: Event,
    ) -> EventMatch {
        event.m()
    }

    /// Does this component contain the given cursor position?
    ///
    /// This is used to determine if the component should receive mouse events
    /// for this position. This should typically not be overridden. The default
    /// implementation checks if the component's last draw area contains the
    /// point.
    fn contains(&self, context: &UpdateContext, position: Position) -> bool {
        // By default, we want to receive any mouse event in our draw area
        context
            .component_map
            .area(self)
            .is_some_and(|area| area.contains(position))
    }

    /// Provide a list of actions that are accessible from the actions menu.
    /// This list may be static (e.g. determined from an enum) or dynamic. When
    /// the user opens the actions menu, all available actions for all
    /// **focused** components will be collected and show in the menu. If an
    /// action is selected, an event will be emitted with that action value.
    fn menu(&self) -> Vec<MenuItem> {
        Vec::new()
    }

    /// Persist state to the persistence store. This is called at the end of
    /// each update phase. The view will automatically call it for each
    /// component in the tree, so implementors do **not** need to call it
    /// recursively for their children.
    ///
    /// Components are responsible for restoring persisted values from the
    /// store themselves, using [PersistentStore::get]. This should happen in
    /// each component's constructor.
    fn persist(&self, _store: &mut PersistentStore) {}

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

/// An extension trait for [Component]
///
/// This provides all the functionality of [Component] that does *not* need to
/// be implemented by each individual component type. Any method on a component
/// that does not need to be (or cannot be) overridden by implementors of
/// [Component] should be defined here instead.
pub trait ComponentExt: Component {
    /// Collect all available menu actions from all **focused** descendents of
    /// this component (including this component). This takes a mutable
    /// reference so we don't have to duplicate the code that provides children;
    /// it will *not* mutate anything.
    fn collect_actions(&mut self, context: &UpdateContext) -> Vec<MenuItem>
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

    /// Call [Component::persist] for all components in the tree.
    fn persist_all(&mut self, store: &mut PersistentStore)
    where
        Self: Sized;
}

impl<T: Component + ?Sized> ComponentExt for T {
    fn collect_actions(&mut self, context: &UpdateContext) -> Vec<MenuItem>
    where
        Self: Sized,
    {
        fn inner(
            context: &UpdateContext,
            items: &mut Vec<MenuItem>,
            component: &mut dyn Component,
        ) {
            // Only include actions from focused components
            if context.component_map.has_focus(component) {
                items.extend(component.menu());
                for mut child in component.children() {
                    if let Some(component) = child.component() {
                        inner(context, items, component);
                    }
                }
            }
        }

        let mut items = Vec::new();
        inner(context, &mut items, self);
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

    fn persist_all(&mut self, store: &mut PersistentStore)
    where
        T: Sized,
    {
        persist_all(store, self);
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
/// some component types (e.g. `Select`) that have multiple `Draw` impls.
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
pub struct Canvas<'buf> {
    /// Main frame buffer
    buffer: &'buf mut Buffer,
    /// Each [Portal] element is rendered to its own buffer. The buffers are
    /// then merged together into the main frame buffer at the end of the draw.
    /// If there are multiple portals, the *later* rendered portals take
    /// priority.
    portals: Vec<Buffer>,
    /// Throughout a draw, we track which components are drawn and where. At
    /// the end of the draw, this is returned to the caller so it can be used
    /// during the subsequent update phase.
    components: ComponentMap,
}

impl<'buf> Canvas<'buf> {
    /// Wrap a frame for a single walk down the draw tree
    pub fn new(buffer: &'buf mut Buffer) -> Self {
        Self {
            buffer,
            portals: vec![],
            components: ComponentMap::default(),
        }
    }

    /// Create a new canvas and draw an entire component tree to it. Returns the
    /// [ComponentMap] of all drawn components.
    #[must_use]
    pub fn draw_all<T, Props>(
        frame: &'buf mut Frame,
        root: &T,
        props: Props,
    ) -> ComponentMap
    where
        T: Component + Draw<Props>,
    {
        Self::draw_all_area(frame, root, props, frame.area(), true)
    }

    /// [Self::draw_all], but the caller determines the area and focus of the
    /// root component. Called directly only for tests, where those need to be
    /// configured.
    #[must_use]
    pub fn draw_all_area<T, Props>(
        frame: &'buf mut Frame,
        root: &T,
        props: Props,
        area: Rect,
        has_focus: bool,
    ) -> ComponentMap
    where
        T: Component + Draw<Props>,
    {
        let mut canvas = Self::new(frame.buffer_mut());
        canvas.draw(root, props, area, has_focus);

        // Merge portaled buffers into the main buffer
        for portal_buffer in &canvas.portals {
            canvas.buffer.merge(portal_buffer);
        }

        canvas.components
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
        self.components.0.insert(component.id(), metadata);

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
        let main_buffer = mem::replace(self.buffer, Buffer::empty(area));
        self.draw(component, props, area, has_focus);
        // Swap the main buffer back into the frame
        let portal = mem::replace(self.buffer, main_buffer);
        // Store what we rendered, to be merged in later
        self.portals.push(portal);
    }

    /// Get the full screen area
    pub fn area(&self) -> Rect {
        self.buffer.area
    }

    /// Get a mutable reference to the internal screen buffer
    pub fn buffer_mut(&mut self) -> &mut Buffer {
        self.buffer
    }

    /// Render a [Widget] to the active buffer
    pub fn render_widget<W: Widget>(&mut self, widget: W, area: Rect) {
        widget.render(area, self.buffer);
    }

    /// Render a [StatefulWidget] to the active buffer
    pub fn render_stateful_widget<W>(
        &mut self,
        widget: W,
        area: Rect,
        state: &mut W::State,
    ) where
        W: StatefulWidget,
    {
        widget.render(area, self.buffer, state);
    }

    /// This is a shitty fix. To be reverted soon(tm)
    pub fn merge_components(&mut self, other: Canvas) {
        self.components.0.extend(other.components.0);
    }
}

/// All components that were drawn during the last draw phase. The purpose
/// of this is to allow each component to return an exhaustive list of its
/// children during event handling, then we can automatically filter that
/// list down to just the ones that are visible. This prevents the need to
/// duplicate visibility logic in both the draw and the children getters.
/// For each drawn component, this stores metadata related to its last
/// draw.
///
/// A new map is built for each [Canvas], which means a new map every draw
/// frame.
#[derive(Debug, Default)]
pub struct ComponentMap(HashMap<ComponentId, DrawMetadata>);

impl ComponentMap {
    /// Was this component drawn to the screen during the previous draw phase?
    pub fn is_visible<T: Component + ?Sized>(&self, component: &T) -> bool {
        self.0.contains_key(&component.id())
    }

    /// Get the area that the component was drawn to. Return `None` iff the
    /// component is not visible.
    pub fn area<T: Component + ?Sized>(&self, component: &T) -> Option<Rect> {
        self.0.get(&component.id()).map(|metadata| metadata.area())
    }

    /// Was this component in focus during the previous draw phase?
    fn has_focus<T: Component + ?Sized>(&self, component: &T) -> bool {
        let metadata = self.0.get(&component.id());
        metadata.is_some_and(|metadata| metadata.has_focus())
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

/// A wrapper for a dynamically dispatched [Component]
///
/// This is used to return a collection of event handlers from
/// [Component::children]. This serves two main purposes:
/// - Attach a static name to the component's trait object, for logging
/// - Support null children for optional components
///
/// Those may sound unimportant, but they're *very* useful and justify the
/// added abstraction. See [ToChild] as well.
pub enum Child<'a> {
    /// A null child, produced by an optional component. This is an ergonomic
    /// feature that makes it possible to call to_child_mut() on optional
    /// children.
    None,
    Borrowed {
        name: &'static str,
        component: &'a mut dyn Component,
    },
}

impl<'a> Child<'a> {
    /// Get a descriptive name for this component type
    pub fn name(&self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Borrowed { name, .. } => name,
        }
    }

    /// Get the contained component trait object. Return `None` iff this is
    /// [Child::None].
    pub fn component<'b>(&'b mut self) -> Option<&'b mut dyn Component>
    where
        // 'b is the given &self, 'a is the contained &dyn Component
        'a: 'b,
    {
        match self {
            Self::None => None,
            Self::Borrowed { component, .. } => Some(*component),
        }
    }
}

/// Abstraction to convert a component type into [Child], which is a wrapper for
/// a trait object.
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

impl<T: Component + Sized> ToChild for Option<T> {
    fn to_child_mut(&mut self) -> Child<'_> {
        match self {
            Some(component) => component.to_child_mut(),
            None => Child::None,
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
    // Keyboard input should only go to focused components. If a parent is
    // unfocused, then its children can't receive the event either. This is so
    // that parents don't have to propagate their focus state down the tree
    // manually
    if let Event::Input(InputEvent::Key { .. } | InputEvent::Paste) = &event
        && !context.component_map.has_focus(component)
    {
        return Some(event);
    }

    // If we have a child, send them the event. If not, eat it ourselves
    for mut child in component.children() {
        let name = child.name();
        let Some(component) = child.component() else {
            // If child is None, skip it
            continue;
        };
        // RECURSION
        let propagated = update_all(name, component, context, event);
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

    // At this point we know a few things about the event:
    // - A child didn't handle it
    // - IF it's a key event, we have focus (because of the gate above)
    // We need to check one more thing before handling the event: if it's a
    // mouse event, is the cursor within our area? We can't check this before
    // handling children because it's possible for an event to be over a child
    // without being over the parent (in the case of portals). In that case, the
    // child receives the event but the parent doesn't.
    let should_receive = match &event {
        Event::Input(
            InputEvent::Click { position, .. }
            | InputEvent::Scroll { position, .. },
        ) => component.contains(context, *position),
        _ => true,
    };
    if should_receive {
        // None of our children handled it, we'll take it ourselves. Event is
        // already traced in the root span, so don't dupe it.
        trace_span!("component.update").in_scope(|| {
            let propagated: Option<Event> =
                component.update(context, event).into();

            // Little bit a logging innit
            let status = if propagated.is_some() {
                "propagated"
            } else {
                "consumed"
            };
            trace!(status);

            propagated
        })
    } else {
        Some(event)
    }
}

/// Helper to recursively persist state in an entire component tree
fn persist_all(store: &mut PersistentStore, component: &mut dyn Component) {
    component.persist(store);
    for mut child in component.children() {
        if let Some(component) = child.component() {
            // Recursion!!
            persist_all(store, component);
        }
    }
}

/// Unique ID to refer to a single component
///
/// A component should generate a unique ID for itself upon construction (via
/// [ComponentId::new] or [ComponentId::default]) and use the same ID for its
/// entire lifespan. This ID should be returned from [Component::id].
#[derive(Copy, Clone, Debug, Display, Eq, Hash, PartialEq)]
pub struct ComponentId(u64);

impl ComponentId {
    /// Get a new unique component ID
    pub fn new() -> Self {
        // We use an incrementing integer because:
        // 1. They're more human-readable than UUIDs
        // 2. IDs are consistent across test runs (helpful for debugging)
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        Self(id)
    }
}

/// Generate a new unique component ID
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
    use Mode::*;
    use ratatui::layout::{Layout, Position};
    use rstest::{fixture, rstest};
    use slumber_config::Action;
    use terminput::{KeyCode, KeyModifiers};

    /// The root component. This exists just to push [Branch] down the tree
    /// one layer to enable tests that hide/unfocus the branch.
    #[derive(Debug, Default)]
    struct Root {
        id: ComponentId,
        branch: Branch,
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
            if props.branch_mode != Hidden {
                canvas.draw(
                    &self.branch,
                    props.branch_props,
                    metadata.area(),
                    props.branch_mode == Focused,
                );
            }
        }
    }

    struct RootProps {
        branch_mode: Mode,
        branch_props: BranchProps,
    }

    impl RootProps {
        fn new(branch: Mode, a: Mode, b: Mode, c: Mode) -> Self {
            Self {
                branch_mode: branch,
                branch_props: BranchProps { a, b, c },
            }
        }

        /// Create a common prop combination:
        ///
        /// - branch: Focused
        /// - a: Focused
        /// - b: Visible
        /// - c: Hidden
        fn fvh() -> Self {
            Self::new(Focused, Focused, Visible, Hidden)
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

    impl Branch {
        /// Assert that the component has received exactly one event (or zero
        /// for `Recipient::None`), and it went to the specified recipient.
        #[track_caller]
        fn assert_received(&self, recipient: Recipient) {
            let expected = match recipient {
                Recipient::None => [0, 0, 0, 0],
                Recipient::Branch => [1, 0, 0, 0],
                Recipient::A => [0, 1, 0, 0],
                Recipient::B => [0, 0, 1, 0],
                Recipient::C => [0, 0, 0, 1],
            };
            let actual = [self.count, self.a.count, self.b.count, self.c.count];
            assert_eq!(
                actual, expected,
                "Event count mismatch; expected recipient {recipient:?}"
            );
        }

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
                if mode != Hidden {
                    canvas.draw(component, (), area, mode == Focused);
                }
            }
        }
    }

    struct BranchProps {
        a: Mode,
        b: Mode,
        c: Mode,
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

    /// The recipient of an event
    #[derive(Debug, PartialEq)]
    enum Recipient {
        /// No one has received any events
        None,
        Branch,
        A,
        B,
        C,
    }

    /// Create a keyboard event
    fn key_event() -> Event {
        Event::Input(InputEvent::Key {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            action: Some(Action::Submit),
        })
    }

    /// Create a left click event
    fn click(x: u16, y: u16) -> Event {
        Event::Input(InputEvent::Click {
            position: Position { x, y },
        })
    }

    /// Get a testing component. This *doesn't* use `TestComponent` because
    /// we want to test logic internal to the component, so we need to do some
    /// wonky things unique to these tests that require calling the component
    /// methods directly.
    #[fixture]
    fn component() -> Root {
        Root::default()
    }

    /// Test the life cycle of a component tree, where individual components
    /// change state between focused/visible/hidden. In each state, **key**
    /// events should only go to focused components.
    #[rstest]
    fn test_life_cycle(
        harness: TestHarness,
        terminal: TestTerminal,
        mut component: Root,
    ) {
        let draw_update_assert =
            |component: &mut Root,
             props: Option<RootProps>,
             recipient: Recipient| {
                let mut component_map = ComponentMap::default();
                if let Some(props) = props {
                    terminal.draw(|frame| {
                        component_map =
                            Canvas::draw_all(frame, component, props);
                    });
                }

                let mut update_context = UpdateContext {
                    component_map: &component_map,
                    request_store: &mut harness.request_store.borrow_mut(),
                };

                component.update_all(&mut update_context, key_event());
                component.branch.assert_received(recipient);
                component.branch.reset(); // Reset for the next assertion
            };

        // Initial event handling - nothing is visible so nothing should consume
        draw_update_assert(&mut component, None, Recipient::None);

        // Visible components get events

        draw_update_assert(
            &mut component,
            Some(RootProps::fvh()),
            Recipient::A,
        );

        // Switch things up, make sure new state is reflected
        draw_update_assert(
            &mut component,
            Some(RootProps::new(Focused, Visible, Hidden, Focused)),
            Recipient::C,
        );

        // Hide all children, root should eat everything
        draw_update_assert(
            &mut component,
            Some(RootProps::new(Focused, Hidden, Hidden, Hidden)),
            Recipient::Branch,
        );
    }

    /// Render a simple component tree and test that events are propagated as
    /// expected, and that state updates as the visible and focused components
    /// change.
    ///
    /// For all these, the child states are:
    /// - a: Focused
    /// - b: Visible
    /// - c: Hidden
    #[rstest]
    // Keyboard event goes to the focused child
    #[case::keyboard(key_event(), RootProps::fvh(), Recipient::A)]
    // If the parent is unfocused but the child is focused, the child should
    // *not* receive focus-only events.
    #[case::keyboard_parent_unfocused(
        key_event(),
        RootProps::new(Visible, Focused, Focused, Focused,),
        Recipient::None
    )]
    // If the parent component is hidden, nobody gets to see events, even if
    // the children have been drawn. This is a very odd scenario and shouldn't
    // happen in the wild, but it's good to have it be well-defined.
    #[case::keyboard_parent_hidden(
        key_event(),
        RootProps::new(Hidden, Focused, Focused, Focused),
        Recipient::None
    )]
    #[case::mouse_focused(click(0, 0), RootProps::fvh(), Recipient::A)]
    // Mouse events can go to any visible component; don't have to be focused
    #[case::mouse_visible(click(0, 1), RootProps::fvh(), Recipient::B)]
    // If the clicked child is hidden, it goes through to the parent
    #[case::mouse_hidden(click(0, 2), RootProps::fvh(), Recipient::Branch)]
    fn test_event(
        harness: TestHarness,
        terminal: TestTerminal,
        mut component: Root,
        #[case] event: Event,
        #[case] props: RootProps,
        #[case] expected_recipient: Recipient,
    ) {
        let mut component_map = ComponentMap::default();
        terminal.draw(|frame| {
            component_map = Canvas::draw_all(frame, &component, props);
        });

        let mut update_context = UpdateContext {
            component_map: &component_map,
            request_store: &mut harness.request_store.borrow_mut(),
        };

        component.update_all(&mut update_context, event);
        component.branch.assert_received(expected_recipient);
    }
}
