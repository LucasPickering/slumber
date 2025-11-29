use crate::{
    context::TuiContext,
    util::{PersistentKey, PersistentStore},
    view::{
        Generate, UpdateContext,
        common::{
            Pane,
            select::{Select, SelectFilter, SelectListProps},
            text_box::{TextBox, TextBoxEvent, TextBoxProps},
        },
        component::{
            Canvas, Child, Component, ComponentId, Draw, DrawMetadata, ToChild,
        },
        event::{Emitter, Event, EventMatch, ToEmitter},
    },
};
use ratatui::{
    layout::{Constraint, Layout},
    text::Text,
};
use slumber_config::Action;
use slumber_core::collection::HasId;

/// TODO
#[derive(Debug)]
pub struct SidebarList<State: PrimaryListState> {
    id: ComponentId,
    emitter: Emitter<PrimaryListEvent>,
    title: String,
    select: Select<State::Item>,
    /// TODO
    state: State,
    /// TODO
    last_submitted: Option<usize>,

    /// Text box for filtering down items in the list
    filter: TextBox,
    /// Is the user typing in the filter box? User has to explicitly grab focus
    /// on the box to start typing
    filter_focused: bool,
}

impl<State: PrimaryListState> SidebarList<State> {
    /// TODO
    pub fn new(state: State) -> Self {
        let input_engine = &TuiContext::get().input_engine;
        let title = input_engine.add_hint(State::TITLE, State::ACTION);
        let select = Self::build_select(&state, "");
        let last_submitted = select.selected_index();
        let filter = TextBox::default()
            .placeholder(format!(
                "{binding} to filter",
                binding = input_engine.binding_display(Action::Search)
            ))
            .subscribe([
                TextBoxEvent::Cancel,
                TextBoxEvent::Change,
                TextBoxEvent::Submit,
            ]);

        Self {
            id: ComponentId::default(),
            emitter: Emitter::default(),
            title,
            select,
            state,
            last_submitted,
            filter,
            filter_focused: false,
        }
    }

    pub fn selected(&self) -> Option<&State::Item> {
        self.select.selected()
    }

    /// TODO
    fn collapse_selected(&mut self, action: Collapse) {
        if let Some(selected) = self.select.selected()
            && self.state.collapse(selected, action)
        {
            // If we changed the set of what is visible, rebuild the list state
            self.rebuild_select();
        }
    }

    /// Rebuild the select. Call this whenever the list of items may change
    fn rebuild_select(&mut self) {
        self.select = Self::build_select(&self.state, self.filter.text());
    }

    /// Build/rebuild a select based on the item list
    fn build_select(state: &State, filter: &str) -> Select<State::Item> {
        let items = state.items();
        Select::builder(items)
            .persisted(&state.persistent_key())
            .filter(filter)
            .build()
    }
}

impl<State: Default + PrimaryListState> Default for SidebarList<State> {
    fn default() -> Self {
        Self::new(State::default())
    }
}

impl<State: PrimaryListState> Component for SidebarList<State> {
    fn id(&self) -> super::ComponentId {
        self.id
    }

    fn update(
        &mut self,
        _context: &mut UpdateContext,
        event: Event,
    ) -> EventMatch {
        event
            .m()
            .click(|_, _| self.emitter.emit(PrimaryListEvent::Open))
            .action(|action, propagate| match action {
                Action::Submit => {
                    // Close with the current item selected. Checkpoint this
                    // item for the next time we're opened
                    self.last_submitted = self.select.selected_index();
                    self.emitter.emit(PrimaryListEvent::Close);
                }
                Action::Cancel => {
                    // Revert to whatever was selected when the list was opened,
                    // then close
                    if let Some(index) = self.last_submitted {
                        self.select.select_index(index);
                    }
                    self.emitter.emit(PrimaryListEvent::Close);
                }

                // For lists with collapsible groups, handle collapse/expand
                Action::Left => self.collapse_selected(Collapse::Collapse),
                Action::Right => self.collapse_selected(Collapse::Expand),
                Action::Toggle => self.collapse_selected(Collapse::Toggle),

                Action::Search => self.filter_focused = true,

                // We can't check for our own action to open here because we
                // won't have focus while closed
                _ => propagate.set(),
            })
            // Filter emitted events
            .emitted(self.filter.to_emitter(), |event| match event {
                TextBoxEvent::Change => self.rebuild_select(),
                TextBoxEvent::Cancel | TextBoxEvent::Submit => {
                    self.filter_focused = false;
                }
            })
    }

    fn persist(&self, store: &mut PersistentStore) {
        // Persist selected item
        store.set_opt(
            &self.state.persistent_key(),
            self.select.selected().map(State::Item::id),
        );
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![
            // State is a component so that it can persist and handle events
            self.state.to_child_mut(),
            self.select.to_child_mut(),
            self.filter.to_child_mut(),
        ]
    }
}

impl<State> Draw<PrimaryListProps> for SidebarList<State>
where
    State: PrimaryListState,
    for<'a> &'a State::Item: Generate,
    for<'a> <&'a State::Item as Generate>::Output<'a>: Into<Text<'a>>,
{
    fn draw(
        &self,
        canvas: &mut Canvas,
        props: PrimaryListProps,
        metadata: DrawMetadata,
    ) {
        // Both formats use a pane outline
        let block = Pane {
            title: &self.title,
            has_focus: metadata.has_focus(),
        }
        .generate();
        let area = block.inner(metadata.area());
        canvas.render_widget(block, metadata.area());

        match props.format {
            Format::Header => {
                let value: Text = self
                    .select
                    .selected()
                    .map(|item| item.generate().into())
                    .unwrap_or_else(|| "None".into());
                canvas.render_widget(value, area);
            }
            Format::List => {
                // Expanded sidebar
                let [filter_area, list_area] = Layout::vertical([
                    Constraint::Length(1),
                    Constraint::Min(0),
                ])
                .areas(area);
                canvas.draw(
                    &self.filter,
                    TextBoxProps::default(),
                    filter_area,
                    self.filter_focused,
                );
                canvas.draw(
                    &self.select,
                    SelectListProps::pane(),
                    list_area,
                    true,
                );
            }
        }
    }
}

impl<State> ToEmitter<PrimaryListEvent> for SidebarList<State>
where
    State: PrimaryListState,
{
    fn to_emitter(&self) -> Emitter<PrimaryListEvent> {
        self.emitter
    }
}

/// Draw props for [PrimaryList]
pub struct PrimaryListProps {
    pub format: Format,
}

/// Visual format of the list
#[derive(Debug)]
pub enum Format {
    /// List is collapsed and just visible as a header. Only the selected value
    /// is visible
    Header,
    /// List is open in the sidebar and the entire list is visible
    List,
}

/// Emitted event from [PrimaryList]
#[derive(Debug)]
pub enum PrimaryListEvent {
    Open,
    Close,
}

/// TODO
/// TODO CollapsibleListState
pub trait PrimaryListState: Component {
    /// TODO
    const TITLE: &str;
    /// TODO
    const ACTION: Action;

    /// TODO
    type Item: HasId
        // Compare to ID to restore from persistence
        + PartialEq<<Self::Item as HasId>::Id>
        // Compare to string for filtering
        + SelectFilter;
    /// TODO
    type PersistentKey: PersistentKey<Value = <Self::Item as HasId>::Id>;

    /// TODO
    fn persistent_key(&self) -> Self::PersistentKey;

    /// TODO
    fn items(&self) -> Vec<Self::Item>;

    /// TODO
    fn collapse(&mut self, _selected: &Self::Item, _action: Collapse) -> bool {
        // By default, lists aren't collapsible so nothing changes
        false
    }
}

/// Ternary action for modifying node collapse state
pub enum Collapse {
    /// If the selected node is collapsed, expand it
    Expand,
    /// If the selected node is expanded, collapse it
    #[expect(clippy::enum_variant_names)]
    Collapse,
    /// If the selected node is collapsed, expand it. If it's expanded, collapse
    /// it.
    Toggle,
}

// TODO tests
#[cfg(test)]
mod tests {

    /// Test the filter box
    #[test]
    fn test_filter() {
        todo!()
    }
}
