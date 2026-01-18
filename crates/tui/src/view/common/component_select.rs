use crate::{
    context::TuiContext,
    view::{
        UpdateContext,
        common::select::{Select, SelectItem, SelectState},
        component::{
            Canvas, Child, Component, ComponentId, Draw, DrawMetadata, ToChild,
        },
        event::{Event, EventMatch},
    },
};
use derive_more::derive::{Deref, DerefMut};
use itertools::Itertools;
use ratatui::{
    buffer::{Buffer, Cell},
    layout::{Constraint, Layout, Rect, Spacing},
    style::{Color, Style},
    widgets::Block,
};
use std::cmp;

/// A wrapper around [Select] for a list of items that implement [Component].
/// This provides some additional functionality:
/// - Items are treated as children in the component tree, allowing them to
///   receive events
/// - The [Draw] implementation uses each item's own [Draw] impl, allowing for
///   complex rendering beyond just generating `Text`
#[derive(Debug, Deref, DerefMut)]
pub struct ComponentSelect<Item> {
    #[deref]
    select: Select<Item, ComponentSelectState>,
}

impl<Item: 'static> ComponentSelect<Item> {
    /// Construct a new [ComponentSelect] from a [Select]
    pub fn new(select: Select<Item, ComponentSelectState>) -> Self {
        Self { select }
    }

    /// Move the inner [Select] value out
    pub fn into_select(self) -> Select<Item, ComponentSelectState> {
        self.select
    }

    /// Compute the view window from the item list and view height
    ///
    /// This will include any item that is at least partially visible. Items
    /// can be cut off at the top and bottom. The selected item will always be
    /// fully visible, unless the view height is less than that item's height.
    ///
    /// ## Params
    ///
    /// - `item_props`: A function that takes `(item, is_selected)` and produces
    ///   the item's draw props and height
    /// - `view_height`: Height of the area to which the list will be drawn
    fn window<Props>(
        &self,
        item_props: impl Fn(&Item, bool) -> (Props, u16),
        view_height: u16,
    ) -> Vec<Friend<'_, Item, Props>> {
        let Some(selected_index) = self.selected_index() else {
            // Shortcut! If the list is empty, we can skip all that math shit
            return vec![];
        };

        // Compute props, height, and pixel offset for all items first
        let mut item_offset = 0; // Accumulator for pixel offset from the top
        let friends = self
            .items_with_metadata()
            .enumerate()
            .map(|(i, item)| {
                let is_selected = i == selected_index;
                // Each item reports its own height
                let (props, height) = item_props(&item.value, is_selected);
                let offset = item_offset;
                item_offset += height;
                Friend {
                    item,
                    props,
                    is_selected,
                    offset,
                    height,
                }
            })
            .collect_vec();
        let list_height = item_offset; // Total height of all items

        // Safety: friends is same len as items
        let selected = &friends[selected_index];
        // Bound how far up we can scroll. This ensures the entire selected item
        // is visible. Order of operations matters here because the bounded
        // subtraction makes it non-commutative
        let lower_bound =
            (selected.offset + selected.height).saturating_sub(view_height);
        // Bound how far down we can scroll. The selected item has to be
        // visible, and we don't want to scroll past the bottom of the list.
        // Over-scrolling can happen implicitly when the view grows, so we need
        // to scroll back up in that case.
        let upper_bound =
            cmp::min(selected.offset, list_height.saturating_sub(view_height));
        // Clamp the offset into our bounds
        let offset = self.with_state(|state| {
            state.offset = if lower_bound > upper_bound {
                // The view is smaller than the selected item. All we can do is
                // show the top portion of it
                selected.offset
            } else {
                state.offset.clamp(lower_bound, upper_bound)
            };
            state.offset
        });

        // Slice the list down to just items that are partially or fully visible
        friends
            .into_iter()
            // Skip any item that ends above the top of the window
            .skip_while(|friend| friend.offset + friend.height <= offset)
            // Take any item that starts before the bottom of the window
            .take_while(|friend| friend.offset < offset + view_height)
            .collect()
    }
}

impl<Item: 'static> Default for ComponentSelect<Item> {
    fn default() -> Self {
        Self {
            select: Select::default(),
        }
    }
}

impl<Item: 'static + Component> Component for ComponentSelect<Item> {
    fn id(&self) -> ComponentId {
        self.select.id()
    }

    fn update(
        &mut self,
        context: &mut UpdateContext,
        event: Event,
    ) -> EventMatch {
        // Since this is a wrapper for Select, we pass events directly to it
        // instead of treating it as a child. This allows us to include all its
        // items as children while avoiding multiple uses of the mutable ref
        self.select.update(context, event)
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        self.select.items_mut().map(ToChild::to_child_mut).collect()
    }
}

/// Draw a list of components with configurable sizes and styling
impl<Item, Props> Draw<ComponentSelectProps<Item, Props>>
    for ComponentSelect<Item>
where
    Props: Clone,
    Item: 'static + Component + Draw<Props>,
{
    fn draw(
        &self,
        canvas: &mut Canvas,
        props: ComponentSelectProps<Item, Props>,
        metadata: DrawMetadata,
    ) {
        // Grab the subset of items in the viewport
        let window = self.window(props.item_props, metadata.area().height);
        if window.is_empty() {
            // Either the list is empty or the draw area is zero. Either way,
            // there's nothing for us to do
            return;
        }

        let first_offset = window[0].offset; // We'll need this area
        let target_area = metadata.area(); // Area we'll eventually render into
        let width = target_area.width;

        // It's possible some items are only partially visible. We have 3
        // options:
        //
        // 1. Render those entire items, which will overflow onto our neighbors
        // 2. Only render items that are fully visible
        // 3. Render partial items
        //
        // (3) is obviously the best but it requires rendering to a separate
        // buffer then copying back into the main buffer. This prevents
        // overwriting neighbors that have already been drawn. The items
        // themselves are arbitrary Component implementations so it's not
        // possible to tell them to only draw themselves partially.

        let mut empty_cell = Cell::default();
        empty_cell.set_bg(props.styles.background_color);

        // Build a new buffer that's large enough to fit the entire window
        let mut virtual_buffer = Buffer::filled(
            Rect {
                x: 0,
                y: 0,
                width,
                // Height of the virtual buffer is either the height of all visible
                // elements or, if the view is big enough to fit the entire list,
                // the height of the view
                height: cmp::max(
                    window.iter().map(|friend| friend.height).sum(),
                    target_area.height,
                ),
            },
            empty_cell,
        );
        let mut virtual_canvas = Canvas::new(&mut virtual_buffer);

        // Render each complete item into the virtual buffer. We know the buffer
        // is large enough to fit all items in the window.
        let item_areas = Layout::vertical(
            window
                .iter()
                .map(|friend| Constraint::Length(friend.height)),
        )
        .spacing(props.spacing)
        .split(virtual_canvas.area());
        for (friend, area) in window.into_iter().zip(&*item_areas) {
            // Apply styling before the render
            let mut style = Style::default();
            if !friend.item.enabled() {
                style = style.patch(props.styles.disabled);
            }
            if friend.is_selected {
                style = style.patch(props.styles.highlight);
            }
            virtual_canvas.render_widget(Block::new().style(style), *area);

            virtual_canvas.draw(
                &friend.item.value,
                friend.props,
                *area,
                friend.is_selected,
            );
        }

        // Copy the virtual buffer back to the canvas. An item can be partially
        // visible at the top *or* bottom of the list, so we need to slice down
        // the buffer content to just what will fit into the view
        let content = &mut virtual_canvas.buffer_mut().content;
        // Drop the first y rows, where y is the distance between the top of the
        // first item and the top of the view window
        let y = self.with_state(|state| state.offset) - first_offset;
        let start = usize::from(width * y);
        // Drop lines overhanging below the view
        let end = usize::from(width * (y + target_area.height));
        // We need to copy this into a *third* buffer so we can call
        // Buffer::merge
        let buffer = Buffer {
            area: target_area,
            content: content.drain(start..end).collect(),
        };
        // Safety first!
        debug_assert_eq!(
            buffer.area().area() as usize,
            buffer.content.len(),
            "Source buffer content length does not match area"
        );
        canvas.buffer_mut().merge(&buffer);
        canvas.merge_components(virtual_canvas);
    }
}

impl<Item> From<Select<Item, ComponentSelectState>> for ComponentSelect<Item>
where
    Item: 'static,
{
    fn from(select: Select<Item, ComponentSelectState>) -> Self {
        Self::new(select)
    }
}

/// Props for rendering a [ComponentSelect] as a list
pub struct ComponentSelectProps<Item, Props> {
    pub styles: SelectStyles,
    pub spacing: Spacing,
    /// Function to generate props and height for an item. Takes `(item,
    /// has_focus)` and returns `(props, height)`. Each item has to
    /// preemptively declare its height so the parent can allocate space
    /// correctly.
    #[expect(clippy::type_complexity)]
    pub item_props: Box<dyn Fn(&Item, bool) -> (Props, u16)>,
}

impl<Item, Props> Default for ComponentSelectProps<Item, Props>
where
    Props: Default,
{
    fn default() -> Self {
        Self {
            styles: SelectStyles::table(),
            spacing: Spacing::default(),
            item_props: Box::new(|_, _| (Props::default(), 1)),
        }
    }
}

/// Styling to apply to each [ComponentSelect] item
pub struct SelectStyles {
    pub disabled: Style,
    pub highlight: Style,
    pub background_color: Color,
}

impl SelectStyles {
    /// Apply no extra styling to each item
    pub fn none() -> Self {
        let styles = &TuiContext::get().styles.table;
        Self {
            disabled: Style::default(),
            highlight: Style::default(),
            background_color: styles.background_color,
        }
    }

    /// Apply table styling to each item
    pub fn table() -> Self {
        let styles = &TuiContext::get().styles.table;
        Self {
            disabled: styles.disabled,
            highlight: styles.highlight,
            background_color: styles.background_color,
        }
    }
}

/// Helper struct to hold data+metadata for each item. Used within the draw.
/// I wasn't sure what to name it.
#[derive(Debug)]
struct Friend<'a, Item, Props> {
    item: &'a SelectItem<Item>,
    props: Props,
    is_selected: bool,
    /// Number of pixels from the top of the list to the top of this item.
    /// This is the sum of the heights of all items before this in the
    /// list.
    offset: u16,
    height: u16,
}

/// [SelectState] implementation for [ComponentSelectState]
///
/// This uses its own state instead of reusing `ListState` because the scroll
/// offset is defined in terms of pixels rather than indexes
#[derive(Debug, Default)]
pub struct ComponentSelectState {
    /// Index of the selected item in the list
    selected: Option<usize>,
    /// Vertical offset of the view window, in **pixels**. This is the number
    /// of pixels that should be skipped before the first draw
    offset: u16,
}

impl SelectState for ComponentSelectState {
    fn selected(&self) -> Option<usize> {
        self.selected
    }

    fn select(&mut self, index: Option<usize>) {
        self.selected = index;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestHarness, TestTerminal, harness, terminal},
        view::test_util::TestComponent,
    };
    use proptest::{collection, test_runner::TestRunner};
    use ratatui::{
        style::Styled,
        text::{Line, Text},
    };
    use rstest::rstest;
    use std::iter;

    /// Make sure there are no faulty assumptions around the list being empty
    #[rstest]
    fn test_empty_list(harness: TestHarness, terminal: TestTerminal) {
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            ComponentSelect::<Item>::default(),
        );
        assert!(component.is_empty());
        // This shouldn't panic!
        component.int().drain_draw().assert_empty();
    }

    /// View window calculation and scrolling with variable-height items
    #[rstest]
    #[case::initial("a", 0, 0, &["a", "b", "c", "d"])]
    // e is only partially visible at the bottom
    #[case::partial("b", 1, 1, &["b", "c", "d", "e"])]
    #[case::scroll_up("a", 1, 0, &["a", "b", "c", "d"])]
    // c is only partially visible at the top
    #[case::scroll_down("e", 0, 5, &["c", "d", "e"])]
    // If the selected item is visible but we can fit more items in by scrolling
    // up, do that. This is relevant for resizes, when the window grows
    #[case::resize_up("e", 6, 5, &["c", "d", "e"])]
    fn test_view_window(
        harness: TestHarness,
        #[with(1, 10)] terminal: TestTerminal,
        #[case] selected: &str,
        #[case] initial_offset: u16,
        #[case] expected_offset: u16,
        #[case] expected_visible: &[&str],
    ) {
        let items = vec![
            Item::new("a", 1),
            Item::new("b", 2),
            Item::new("c", 3),
            Item::new("d", 4),
            Item::new("e", 5),
        ];
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            ComponentSelect::from(
                Select::builder(items).preselect(&selected).build(),
            ),
        );
        // Initialize state and draw
        component.with_state(|state| state.offset = initial_offset);
        let item_props = |item: &Item, _| ((), item.height);
        component
            .int_props(|| ComponentSelectProps {
                item_props: Box::new(item_props),
                ..Default::default()
            })
            .drain_draw()
            .assert_empty();

        // Check the state directly first
        let view_height = terminal.area().height;
        let window = component.window(item_props, view_height);
        let offset = component.with_state(|state| state.offset);
        assert_eq!(offset, expected_offset);
        assert_eq!(
            window
                .iter()
                .map(|friend| friend.item.value.name)
                .collect_vec(),
            expected_visible
        );

        // Now check what was rendered. We know the calculated window is correct
        // because we checked it against the expected list of items, so we can
        // use it generate the expected buffer
        let highlight_style = TuiContext::get().styles.table.highlight;
        // Generate the visible lines for *all* items
        let selected_index = component.selected_index().unwrap();
        let all_lines = component.items().enumerate().flat_map(|(i, item)| {
            let style = if i == selected_index {
                highlight_style
            } else {
                Style::default()
            };
            item.lines(style)
        });
        // Cut that down to just what's visible
        let expected_lines =
            all_lines.skip(offset.into()).take(view_height.into());
        terminal.assert_buffer_lines(expected_lines);
    }

    /// Test some properties of the scrolling view window:
    /// - Selected item is always fully in view
    /// - Render doesn't panic
    ///   - There's a lot of finicking math in the render, so just making sure
    ///     it doesn't crash with edge cases is useful
    #[rstest]
    fn test_view_window_prop(harness: TestHarness) {
        // I think the proptest! macro is annoying so I prefer manual
        type Params = (Vec<u16>, u16, usize, u16);
        let test = |(items, view_height, selected_index, offset): Params| {
            // Bound the values to the list len without biasing the distribution
            let selected_index = if items.is_empty() {
                0
            } else {
                selected_index % items.len()
            };

            // Initialize state
            let items = items
                .into_iter()
                .map(|height| Item::new("", height))
                .collect();
            let select: ComponentSelect<Item> = Select::builder(items)
                .preselect_index(selected_index)
                .build()
                .into();
            let terminal = TestTerminal::new(1, view_height);
            let mut component = TestComponent::new(&harness, &terminal, select);
            let item_props = |item: &Item, _| ((), item.height);
            component.with_state(|state| state.offset = offset);

            // Render to make sure it doesn't panic
            component
                .int_props(|| ComponentSelectProps {
                    item_props: Box::new(item_props),
                    ..Default::default()
                })
                .drain_draw()
                .assert_empty();

            // At this point, if the list is empty there isn't anything to test
            if component.is_empty() {
                return Ok(());
            }

            // This will scroll state to include selected item
            let visible = component.window(item_props, view_height);

            if component.is_empty() {
                // Source list is empty - nothing to draw
                assert!(visible.is_empty());
                assert_eq!(component.selected_index(), None);
            } else if view_height == 0 {
                // Draw area is empty - nothing is visible
                assert!(visible.is_empty());
                assert_eq!(component.selected_index(), Some(selected_index));
            } else {
                // ===== Main case - list is visible =====

                // Selected item is in the list
                let Some(selected) =
                    visible.iter().find(|friend| friend.is_selected)
                else {
                    panic!(
                        "selected item must be in window list; \
                    selected_index={selected_index} visible={visible:?}"
                    )
                };
                // Selected item is visible based on the scroll view
                let offset = component.with_state(|state| state.offset);
                if view_height >= selected.height {
                    // If the view is big enough to fit the selected item, it
                    // should be entirely in view

                    assert!(
                        offset <= selected.offset
                            && selected.offset + selected.height
                                <= offset + view_height,
                        "selected item must be entirely in view"
                    );
                } else {
                    // Selected item is bigger than the
                    assert!(
                        offset <= selected.offset
                            && selected.offset < offset + view_height,
                        "selected item must be partially in view"
                    );
                }
            }

            Ok(())
        };

        let mut runner = TestRunner::default();
        let item_height = 1..=8u16;
        let list_len = 0..=10usize;
        let view_height = 0..=15u16;
        let offset = 0..=12u16;
        // selected index has to be bounded by the list len. I don't know how to
        // derive one param from another, so the test fn has to normalize this
        let selected_index = 0..100usize;
        runner
            .run(
                &(
                    collection::vec(item_height, list_len),
                    view_height,
                    selected_index,
                    offset,
                ),
                test,
            )
            .unwrap();
    }

    #[derive(Debug)]
    struct Item {
        id: ComponentId,
        name: &'static str,
        height: u16,
    }

    impl Item {
        fn new(name: &'static str, height: u16) -> Self {
            Self {
                id: ComponentId::new(),
                name,
                height,
            }
        }

        /// Get the drawn lines for this item
        fn lines(&self, style: Style) -> impl Iterator<Item = Line<'static>> {
            iter::repeat_n(
                Line::from(self.name).set_style(style),
                self.height.into(),
            )
        }
    }

    impl PartialEq<&str> for Item {
        fn eq(&self, other: &&str) -> bool {
            &self.name == other
        }
    }

    impl Component for Item {
        fn id(&self) -> ComponentId {
            self.id
        }
    }

    impl Draw for Item {
        fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
            let text: Text = self.lines(Style::default()).collect();
            canvas.render_widget(text, metadata.area());
        }
    }
}
