//! Internal state for the [Component] struct. This defines common logic for
//! components., and exposes a small API for accessing both local and global
//! component state.

use crate::view::{
    context::UpdateContext,
    draw::{Draw, DrawMetadata},
    event::{Child, Emitter, Event, ToChild, Update},
};
use crossterm::event::MouseEvent;
use derive_more::Display;
use ratatui::{layout::Rect, Frame};
use std::{
    any,
    cell::{Cell, RefCell},
    collections::HashSet,
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
    static VISIBLE_COMPONENTS: RefCell<HashSet<ComponentId>> =
        Default::default();

    /// Track whichever components are *currently* being drawn. Whenever we
    /// draw a child, push it onto the stack. Pop off when done drawing it. This
    /// makes it easy to track when we're done with a draw phase.
    static STACK: RefCell<Vec<ComponentId>> = Default::default();
}

/// A wrapper around the various component types. The main job of this is to
/// automatically track the area that a component is drawn to, so that it can
/// be used during event handling to filter cursor events. This makes it easy
/// to have components automatically receive *only the cursor events* that
/// occurred within the bounds of that component. Generally every layer in the
/// component tree should be wrapped in one of these.
///
/// This intentionally does *not* implement `Deref` because that has led to bugs
/// in the past where the inner component is drawn without this pass through
/// layer, which means the component area isn't tracked. That means cursor
/// events aren't handled.
#[derive(Debug)]
pub struct Component<T> {
    /// Unique random identifier for this component, to reference it in global
    /// state
    id: ComponentId,
    /// Name of the component type, which is used just for tracing
    name: &'static str,
    inner: T,
    /// Draw metadata that affects event handling. This is updated on each draw
    /// call, hence the need for interior mutability.
    metadata: Cell<DrawMetadata>,
}

impl<T> Component<T> {
    pub fn new(inner: T) -> Self {
        Self {
            id: ComponentId::new(),
            name: any::type_name::<T>(),
            inner,
            metadata: Cell::default(),
        }
    }

    /// Name of the component type, which is used just for debugging/tracing
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Handle an event for this component *or* its children, starting at the
    /// lowest descendant. Recursively walk up the tree until a component
    /// consumes the event.
    #[instrument(level = "trace", skip_all, fields(component = self.name()))]
    pub fn update_all(
        &mut self,
        context: &mut UpdateContext,
        mut event: Event,
    ) -> Update
    where
        T: ToChild,
    {
        // If we can't handle the event, our children can't either
        if !self.should_handle(&event) {
            return Update::Propagate(event);
        }

        let mut self_dyn = self.data_mut().to_child_mut();

        // If we have a child, send them the event. If not, eat it ourselves
        for mut child in self_dyn.children() {
            // RECURSION
            let update = child.update_all(context, event);
            match update {
                Update::Propagate(returned) => {
                    // Keep going to the next child. The propagated event
                    // *should* just be whatever we passed in, but we have
                    // no way of verifying that
                    event = returned;
                }
                Update::Consumed => {
                    return update;
                }
            }
        }

        // None of our children handled it, we'll take it ourselves. Event is
        // already traced in the root span, so don't dupe it.
        (trace_span!("component.update")).in_scope(|| {
            let update = self_dyn.update(context, event);
            trace!(?update);
            update
        })
    }

    /// Should this component handle the given event? This is based on a few
    /// criteria:
    /// - Am I currently visible? I.e. was I drawn on the last draw phase?
    /// - If it's a non-mouse event, do I have focus?
    /// - If it's a mouse event, was it over me? Mouse events should always go
    ///   to the clicked element, even when unfocused, because that's intuitive.
    fn should_handle(&self, event: &Event) -> bool {
        // If this component isn't currently in the visible tree, it shouldn't
        // handle any events
        if !self.is_visible() {
            trace!("Skipping component, not visible");
            return false;
        }

        use crossterm::event::Event::*;
        if let Event::Input { event, .. } = event {
            match event {
                Key(_) | Paste(_) => self.metadata.get().has_focus(),

                Mouse(mouse_event) => {
                    // Check if the mouse is over the component
                    self.intersects(mouse_event)
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

    /// Was this component drawn to the screen during the previous draw phase?
    pub fn is_visible(&self) -> bool {
        VISIBLE_COMPONENTS.with_borrow(|tree| tree.contains(&self.id))
    }

    /// Did the given mouse event occur over/on this component?
    fn intersects(&self, mouse_event: &MouseEvent) -> bool {
        self.is_visible()
            && self.metadata.get().area().intersects(Rect {
                x: mouse_event.column,
                y: mouse_event.row,
                width: 1,
                height: 1,
            })
    }

    /// Get a `Component` wrapping a [Child], which holds an [EventHandler]
    /// trait object. Useful for returning from `[EventHandler::children]`.
    pub fn to_child_mut(&mut self) -> Component<Child<'_>>
    where
        T: ToChild,
    {
        Component {
            id: self.id,
            name: self.name,
            inner: self.inner.to_child_mut(),
            metadata: self.metadata.clone(),
        }
    }

    /// Get a reference to the inner component. This should only be used to
    /// access the contained *data*. Drawing should be routed through the
    /// wrapping component.
    pub fn data(&self) -> &T {
        &self.inner
    }

    /// Get a mutable reference to the inner component. This should only be used
    /// to access the contained *data*. Drawing should be routed through the
    /// wrapping component.
    pub fn data_mut(&mut self) -> &mut T {
        &mut self.inner
    }

    /// Move the inner component out
    pub fn into_data(self) -> T {
        self.inner
    }

    /// Forward to [Emitter::emitted]
    pub fn emitted<'a>(&self, event: &'a Event) -> Option<&'a T::Emitted>
    where
        T: Emitter,
    {
        self.data().emitted(event)
    }

    /// Forward to [Emitter::emitted_owned]
    /// TODO rename
    pub fn emitted_owned(&self, event: Event) -> Result<T::Emitted, Event>
    where
        T: Emitter,
    {
        self.data().emitted_owned(event)
    }

    /// Draw the component to the frame. This will update global state, then
    /// defer to the component's [Draw] implementation for the actual draw.
    pub fn draw<Props>(
        &self,
        frame: &mut Frame,
        props: Props,
        area: Rect,
        has_focus: bool,
    ) where
        T: Draw<Props>,
    {
        self.draw_inner(frame, props, area, has_focus, &self.inner);
    }

    fn draw_inner<D: Draw<Props>, Props>(
        &self,
        frame: &mut Frame,
        props: Props,
        area: Rect,
        has_focus: bool,
        inner: &D,
    ) {
        let guard = DrawGuard::new(self.id);

        // Update internal state for event handling
        let metadata = DrawMetadata::new_dangerous(area, has_focus);
        self.metadata.set(metadata);

        inner.draw(frame, props, metadata);
        drop(guard); // Make sure guard stays alive until here
    }
}

impl<T> Component<Option<T>> {
    /// For components with optional data, draw the contents if present
    pub fn draw_opt<Props>(
        &self,
        frame: &mut Frame,
        props: Props,
        area: Rect,
        has_focus: bool,
    ) where
        T: Draw<Props>,
    {
        if let Some(inner) = &self.inner {
            self.draw_inner(frame, props, area, has_focus, inner);
        }
    }
}

// Derive impl doesn't work because the constructor gets the correct name
impl<T: Default> Default for Component<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T> From<T> for Component<T> {
    fn from(inner: T) -> Self {
        Self::new(inner)
    }
}

/// Unique ID to refer to a single component. Generally components are persisent
/// throughout the lifespan of the view so this will be too, but it's only
/// *necessary* for it to be consistent from one draw phase to the next event
/// phase, because that's how we track if each component is visible or not.
#[derive(Copy, Clone, Debug, Display, Eq, Hash, PartialEq)]
struct ComponentId(Uuid);

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
    fn new(id: ComponentId) -> Self {
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
            visible_components.insert(id);
        });
        Self { id, is_root }
    }
}

impl Drop for DrawGuard {
    fn drop(&mut self) {
        let popped = STACK.with_borrow_mut(|stack| stack.pop());

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
                STACK.with_borrow(|stack| stack.is_empty()),
                "Render stack is not empty after popping root component"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{harness, terminal, TestHarness, TestTerminal},
        view::event::{EventHandler, Update},
    };
    use crossterm::event::{
        KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers,
        MouseButton, MouseEventKind,
    };
    use ratatui::layout::Layout;
    use rstest::{fixture, rstest};
    use slumber_config::Action;
    use slumber_core::assert_matches;

    #[derive(Debug, Default)]
    struct Branch {
        /// How many events have we consumed *ourselves*?
        count: u32,
        a: Component<Leaf>,
        b: Component<Leaf>,
        c: Component<Leaf>,
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
            self.a.data_mut().reset();
            self.b.data_mut().reset();
            self.c.data_mut().reset();
        }
    }

    impl EventHandler for Branch {
        fn update(&mut self, _: &mut UpdateContext, _: Event) -> Update {
            self.count += 1;
            Update::Consumed
        }

        fn children(&mut self) -> Vec<Component<Child<'_>>> {
            vec![
                self.a.to_child_mut(),
                self.b.to_child_mut(),
                self.c.to_child_mut(),
            ]
        }
    }

    impl Draw<Props> for Branch {
        fn draw(
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
        /// How many events have we consumed?
        count: u32,
    }

    impl Leaf {
        fn reset(&mut self) {
            self.count = 0;
        }
    }

    impl EventHandler for Leaf {
        fn update(&mut self, _: &mut UpdateContext, _: Event) -> Update {
            self.count += 1;
            Update::Consumed
        }
    }

    impl Draw for Leaf {
        fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
            frame.render_widget("hello!", metadata.area());
        }
    }

    fn keyboard_event() -> Event {
        Event::Input {
            event: crossterm::event::Event::Key(KeyEvent {
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
            event: crossterm::event::Event::Mouse(MouseEvent {
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
    fn component() -> Component<Branch> {
        Component::default()
    }

    /// Render a simple component tree and test that events are propagated as
    /// expected, and that state updates as the visible and focused components
    /// change.
    #[rstest]
    fn test_render_component_tree(
        harness: TestHarness,
        terminal: TestTerminal,
        mut component: Component<Branch>,
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
            |component: &mut Component<Branch>, expected_counts: [u32; 4]| {
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
                    component.data().count,
                    expected_root,
                    "count mismatch on root component"
                );
                assert_eq!(
                    component.data().a.data().count,
                    expected_a,
                    "count mismatch on component a"
                );
                assert_eq!(
                    component.data().b.data().count,
                    expected_b,
                    "count mismatch on component b"
                );
                assert_eq!(
                    component.data().c.data().count,
                    expected_c,
                    "count mismatch on component c"
                );

                // Reset state for the next assertion
                component.data_mut().reset();
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
        mut component: Component<Branch>,
    ) {
        terminal.draw(|frame| {
            let area = frame.area();
            component.data().a.draw(frame, (), area, true);
            component.data().b.draw(frame, (), area, true);
            component.data().c.draw(frame, (), area, true);
        });
        // Event should *not* be handled because the parent is hidden
        assert_matches!(
            component.update_all(
                &mut UpdateContext {
                    request_store: &mut harness.request_store.borrow_mut(),
                },
                keyboard_event()
            ),
            Update::Propagate(_)
        );
    }

    /// If the parent is unfocused but the child is focused, the child should
    /// *not* receive focus-only events.
    #[rstest]
    fn test_parent_unfocused(
        harness: TestHarness,
        terminal: TestTerminal,
        mut component: Component<Branch>,
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
            Update::Propagate(_)
        );
    }
}
