use crate::{
    message::Message,
    util::ResultReported,
    view::{
        ToStringGenerate, UpdateContext, ViewContext,
        common::{
            list::List,
            modal::Modal,
            text_box::{TextBox, TextBoxEvent, TextBoxProps},
        },
        component::{
            Child, Component, ComponentExt, ComponentId, Draw, DrawMetadata,
            ToChild,
        },
        event::{Event, OptionEvent, ToEmitter},
        state::select::{SelectState, SelectStateEvent, SelectStateEventType},
    },
};
use derive_more::Display;
use ratatui::{
    Frame,
    layout::Layout,
    prelude::{Constraint, Line},
};
use slumber_core::database::CollectionId;
use std::path::PathBuf;

/// A modal to list all collections in the DB, allowing the user to switch to a
/// different one
#[derive(Debug)]
pub struct CollectionSelect {
    id: ComponentId,
    select: SelectState<CollectionSelectItem>,
    /// Text box to filter contents. Always in focus
    filter: TextBox,
}

impl CollectionSelect {
    pub fn new() -> Self {
        Self {
            id: ComponentId::default(),
            select: build_select_state(""),
            filter: TextBox::default(),
        }
    }
}

impl Modal for CollectionSelect {
    fn title(&self) -> Line<'_> {
        "Collections".into()
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        let footer_height = 1;
        let height = u16::min(self.select.len() as u16 + footer_height, 10);
        (Constraint::Percentage(60), Constraint::Length(height))
    }
}

impl Component for CollectionSelect {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event
            .opt()
            .emitted(self.select.to_emitter(), |event| {
                // The ol' Tennessee Switcharoo
                if let SelectStateEvent::Submit(index) = event {
                    let item = &self.select[index];
                    self.close(true);
                    ViewContext::send_message(Message::CollectionSelect(
                        item.path.clone(),
                    ));
                }
            })
            .emitted(self.filter.to_emitter(), |event| match event {
                TextBoxEvent::Change => {
                    // Rebuild the list with the filter applied
                    self.select = build_select_state(self.filter.text());
                }
                TextBoxEvent::Cancel => self.close(false),
                TextBoxEvent::Focus | TextBoxEvent::Submit => {}
            })
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        // Select gets priority because it handles submission
        vec![self.select.to_child_mut(), self.filter.to_child_mut()]
    }
}

impl Draw for CollectionSelect {
    fn draw_impl(&self, frame: &mut Frame, (): (), metadata: DrawMetadata) {
        let [select_area, filter_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)])
                .areas(metadata.area());
        self.select
            .draw(frame, List::from(&self.select), select_area, true);
        self.filter
            .draw(frame, TextBoxProps::default(), filter_area, true);
    }
}

#[derive(Debug, Display)]
#[display("{display_name}")]
struct CollectionSelectItem {
    id: CollectionId,
    display_name: String,
    path: PathBuf,
}

impl PartialEq<CollectionSelectItem> for CollectionId {
    fn eq(&self, other: &CollectionSelectItem) -> bool {
        self == &other.id
    }
}

impl ToStringGenerate for CollectionSelectItem {}

/// Build/rebuild the list selection
fn build_select_state(filter: &str) -> SelectState<CollectionSelectItem> {
    // Build the collection list from the DB's collections table. Preselect
    // the current collection. Current collection ID is only None if the query
    // fails , which would be... odd.
    let Some((collections, current_collection_id)) =
        ViewContext::with_database(|db| {
            let collections = db.root().get_collections()?;
            let current_collection_id = db.metadata()?.id;
            Ok::<_, anyhow::Error>((collections, current_collection_id))
        })
        .reported(&ViewContext::messages_tx())
    else {
        // If we fail to load anything from the DB, we can't show anything
        return SelectState::default();
    };

    SelectState::builder(
        collections
            .into_iter()
            .map(|collection| CollectionSelectItem {
                id: collection.id,
                display_name: collection.display_name(),
                path: collection.path,
            })
            .collect(),
    )
    // Filter *before* preselection so we don't present a value that will
    // disappear
    .filter(filter)
    .preselect(&current_collection_id)
    .subscribe([SelectStateEventType::Submit])
    .build()
}
