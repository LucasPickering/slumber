//! Internal state for the [Component] struct. This defines common logic for
//! components., and exposes a small API for accessing both local and global
//! component state.

use crate::tui::view::{
    draw::{Draw, DrawMetadata},
    event::{Event, EventHandler, Update},
};
use crossterm::event::MouseEvent;
use derive_more::Display;
use ratatui::{layout::Rect, Frame};
use std::{
    any,
    cell::{Cell, RefCell},
    collections::HashSet,
};
use tracing::{trace, trace_span, warn};
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
#[derive(Debug, Default)]
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

    /// Handle an event for this component *or* its children, starting at the
    /// lowest descendant. Recursively walk up the tree until a component
    /// consumes the event.
    pub fn update_all(&mut self, mut event: Event) -> Update
    where
        T: EventHandler,
    {
        // If we can't handle the event, our children can't either
        if !self.should_handle(&event) {
            return Update::Propagate(event);
        }

        // If we have a child, send them the event. If not, eat it ourselves
        for mut child in self.data_mut().children() {
            // Don't propgate to children that aren't visible or not in focus
            if child.should_handle(&event) {
                // RECURSION
                let update = child.update_all(event);
                match update {
                    Update::Propagate(returned) => {
                        // Keep going to the next child. The propgated event
                        // *should* just be whatever we passed in, but we have
                        // no way of verifying that
                        event = returned;
                    }
                    Update::Consumed => {
                        return update;
                    }
                }
            }
        }

        // None of our children handled it, we'll take it ourselves. Event is
        // already traced in the root span, so don't dupe it.
        let span = trace_span!("Component handling", component = self.name);
        span.in_scope(|| {
            let update = self.data_mut().update(event);
            trace!(?update);
            update
        })
    }

    /// Should this component handle the given event? This is based on a few
    /// criteria:
    /// - Am I currently visible? I.e. was I drawn on the last draw phase?
    /// - If it's a non-mouse event, do I have focus?
    /// - If it's a mouse event, was it over me? Mouse events should always go
    /// to the clicked element, even when unfocused, because that's intuitive.
    fn should_handle(&self, event: &Event) -> bool {
        // If this component isn't currently in the visible tree, it shouldn't
        // handle any events
        if !self.is_visible() {
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

    /// Get a mutable reference to the inner value, but as a trait object.
    /// Useful for returning from `[EventHandler::children]`.
    pub fn as_child(&mut self) -> Component<&mut dyn EventHandler>
    where
        T: EventHandler,
    {
        Component {
            id: self.id,
            name: self.name,
            inner: &mut self.inner,
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
        let guard = DrawGuard::new(self.id);

        // Update internal state for event handling
        let metadata = DrawMetadata::new_dangerous(area, has_focus);
        self.metadata.set(metadata);

        self.inner.draw(frame, props, metadata);
        drop(guard); // Make sure guard stays alive until here
    }
}

/// Test-only helpers
#[cfg(test)]
impl<T> Component<T> {
    /// Draw this component onto the terminal. Prefer this over calling
    /// [Component::draw] directly, because that won't update Ratatui's internal
    /// buffers correctly. The entire frame area will be used to draw the
    /// component.
    pub fn draw_term<Props>(
        &self,
        terminal: &mut ratatui::Terminal<ratatui::backend::TestBackend>,
        props: Props,
    ) where
        T: Draw<Props>,
    {
        terminal
            .draw(|frame| self.draw(frame, props, frame.size(), true))
            .unwrap();
    }

    /// Drain events from the event queue, and handle them one-by-one. We expect
    /// each event to be consumed, so panic if it's propagated.
    pub fn drain_events(&mut self)
    where
        T: EventHandler,
    {
        use crate::tui::view::ViewContext;

        // Safety check, prevent annoying bugs
        assert!(
            self.is_visible(),
            "Component {} is not visible, it can't handle events",
            self.name
        );

        while let Some(event) = ViewContext::pop_event() {
            match self.update_all(event) {
                Update::Consumed => {}
                Update::Propagate(event) => {
                    panic!("Event was not consumed: {event:?}")
                }
            }
        }
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
        test_util::*,
        tui::{input::Action, view::event::Update},
    };
    use crossterm::event::{
        KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers,
        MouseButton, MouseEventKind,
    };
    use ratatui::{backend::TestBackend, layout::Layout, Terminal};
    use rstest::{fixture, rstest};

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
        fn update(&mut self, _: Event) -> Update {
            self.count += 1;
            Update::Consumed
        }

        fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
            vec![self.a.as_child(), self.b.as_child(), self.c.as_child()]
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
        fn update(&mut self, _: Event) -> Update {
            self.count += 1;
            Update::Consumed
        }
    }

    impl Draw for Leaf {
        fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
            frame.render_widget("hello!", metadata.area());
        }
    }

    #[fixture]
    fn component() -> Component<Branch> {
        Component::default()
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

    /// Render a simple component tree and test that events are propagated as
    /// expected, and that state updates as the visible and focused components
    /// change.
    #[rstest]
    fn test_render_component_tree(
        _messages: MessageQueue,
        mut terminal: Terminal<TestBackend>,
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
                for event in events {
                    component.update_all(event);
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
        component.draw(
            &mut terminal.get_frame(),
            Props {
                a: Mode::Focused,
                b: Mode::Visible,
                c: Mode::Hidden,
            },
            area,
            true,
        );
        // Root - inherited mouse event from c, which is hidden
        // a - keyboard + mouse
        // b - mouse
        // c - hidden
        assert_events(&mut component, [1, 2, 1, 0]);

        // Switch things up, make sure new state is reflected
        component.draw(
            &mut terminal.get_frame(),
            Props {
                a: Mode::Visible,
                b: Mode::Hidden,
                c: Mode::Focused,
            },
            area,
            true,
        );
        // Root - inherited mouse event from b, which is hidden
        // a - mouse
        // b - hidden
        // c - keyboard + mouse
        assert_events(&mut component, [1, 1, 0, 2]);

        // Hide all children, root should eat everything
        component.draw(
            &mut terminal.get_frame(),
            Props {
                a: Mode::Hidden,
                b: Mode::Hidden,
                c: Mode::Hidden,
            },
            area,
            true,
        );
        assert_events(&mut component, [4, 0, 0, 0]);
    }

    /// If the parent component is hidden, nobody gets to see events, even if
    /// the children have been drawn. This is a very odd scenario and shouldn't
    /// happen in the wild, but it's good to have it be well-defined.
    #[rstest]
    fn test_parent_hidden(
        mut terminal: Terminal<TestBackend>,
        mut component: Component<Branch>,
    ) {
        component.data().a.draw_term(&mut terminal, ());
        component.data().b.draw_term(&mut terminal, ());
        component.data().c.draw_term(&mut terminal, ());
        // Event should *not* be handled because the parent is hidden
        assert_matches!(
            component.update_all(keyboard_event()),
            Update::Propagate(_)
        );
    }

    /// If the parent is unfocused but the child is focused, the child should
    /// *not* receive focus-only events.
    #[rstest]
    fn test_parent_unfocused(
        mut terminal: Terminal<TestBackend>,
        mut component: Component<Branch>,
    ) {
        // We are visible but *not* in focus
        terminal
            .draw(|frame| {
                component.draw(
                    frame,
                    Props {
                        a: Mode::Focused,
                        b: Mode::Visible,
                        c: Mode::Visible,
                    },
                    frame.size(),
                    false,
                )
            })
            .unwrap();
        // Event should *not* be handled because the parent is unfocused
        assert_matches!(
            component.update_all(keyboard_event()),
            Update::Propagate(_)
        );
    }
}
