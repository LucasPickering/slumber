use crate::{
    context::TuiContext,
    util::PersistentStore,
    view::{
        Generate, UpdateContext,
        common::{
            Pane,
            select::{Select, SelectListProps},
        },
        component::{
            Canvas, Child, Component, ComponentId, Draw, DrawMetadata, ToChild,
        },
        event::{Emitter, Event, EventMatch, ToEmitter},
    },
};
use ratatui::text::Text;
use slumber_config::Action;

/// TODO
#[derive(Debug)]
pub struct PrimaryList<State: PrimaryListState> {
    id: ComponentId,
    emitter: Emitter<PrimaryListEvent>,
    title: String,
    select: Select<State::Item>,
    /// TODO
    state: State,
    /// TODO
    last_submitted: Option<usize>,
}

impl<State: PrimaryListState> PrimaryList<State> {
    /// TODO
    pub fn new(state: State) -> Self {
        let title = TuiContext::get()
            .input_engine
            .add_hint(State::TITLE, State::ACTION);
        // TODO persist
        let select = Select::builder(state.items()).build();
        let last_submitted = select.selected_index();
        Self {
            id: ComponentId::default(),
            emitter: Emitter::default(),
            title,
            select,
            state,
            last_submitted,
        }
    }

    pub fn selected(&self) -> Option<&State::Item> {
        self.select.selected()
    }
}

impl<Item: PrimaryListState> Component for PrimaryList<Item> {
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
                // We can't check for our own action to open here because we
                // won't have focus while closed
                _ => propagate.set(),
            })
    }

    fn persist(&self, _store: &mut PersistentStore) {
        // TODO
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.select.to_child_mut()]
    }
}

impl<State> Draw<PrimaryListProps> for PrimaryList<State>
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
            Format::List if self.select.is_empty() => {
                canvas.render_widget(self.state.empty_text(), area);
            }
            Format::List => {
                canvas.draw(&self.select, SelectListProps::pane(), area, true);
            }
        }
    }
}

impl<State> ToEmitter<PrimaryListEvent> for PrimaryList<State>
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
pub trait PrimaryListState {
    /// TODO
    const TITLE: &str;
    /// TODO
    const ACTION: Action;

    /// TODO
    type Item;

    /// TODO
    fn items(&self) -> Vec<Self::Item>;

    /// TODO
    fn empty_text(&self) -> Text<'static>;
}
