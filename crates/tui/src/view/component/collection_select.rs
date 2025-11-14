use crate::{
    message::Message,
    util::ResultReported,
    view::{
        ToStringGenerate, UpdateContext, ViewContext,
        common::{
            modal::Modal,
            text_box::{TextBox, TextBoxEvent, TextBoxProps},
        },
        component::{
            Canvas, Child, Component, ComponentId, Draw, DrawMetadata, ToChild,
        },
        event::{Event, EventMatch, ToEmitter},
        state::select::{
            Select, SelectEvent, SelectEventType, SelectListProps,
        },
    },
};
use derive_more::Display;
use ratatui::{layout::Layout, prelude::Constraint, text::Line};
use slumber_core::database::CollectionId;
use std::path::PathBuf;

/// A modal to list all collections in the DB, allowing the user to switch to a
/// different one
#[derive(Debug)]
pub struct CollectionSelect {
    id: ComponentId,
    select: Select<CollectionSelectItem>,
    /// Text box to filter contents. Always in focus
    filter: TextBox,
}

impl CollectionSelect {
    pub fn new() -> Self {
        Self {
            id: ComponentId::default(),
            select: build_select(""),
            filter: TextBox::default().subscribe([TextBoxEvent::Change]),
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

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            .emitted(self.select.to_emitter(), |event| {
                // The ol' Tennessee Switcharoo
                if let SelectEvent::Submit(index) = event {
                    let item = &self.select[index];
                    ViewContext::send_message(Message::CollectionSelect(
                        item.path.clone(),
                    ));
                }
            })
            .emitted(self.filter.to_emitter(), |event| match event {
                TextBoxEvent::Change => {
                    // Rebuild the list with the filter applied
                    self.select = build_select(self.filter.text());
                }
                TextBoxEvent::Cancel | TextBoxEvent::Submit => {}
            })
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        // Select gets priority because it handles submission
        vec![self.select.to_child_mut(), self.filter.to_child_mut()]
    }
}

impl Draw for CollectionSelect {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        let [select_area, filter_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)])
                .areas(metadata.area());
        canvas.draw(&self.select, SelectListProps::modal(), select_area, true);
        canvas.draw(&self.filter, TextBoxProps::default(), filter_area, true);
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
fn build_select(filter: &str) -> Select<CollectionSelectItem> {
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
        return Select::default();
    };

    Select::builder(
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
    .subscribe([SelectEventType::Submit])
    .build()
}
