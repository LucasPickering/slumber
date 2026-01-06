use crate::{
    context::TuiContext,
    message::Message,
    util::ResultReported,
    view::{
        ToStringGenerate, UpdateContext, ViewContext,
        common::{
            select::{Select, SelectEvent, SelectEventType, SelectListProps},
            text_box::{TextBox, TextBoxEvent, TextBoxProps},
        },
        component::{
            Canvas, Child, Component, ComponentId, Draw, DrawMetadata, ToChild,
        },
        event::{Event, EventMatch, ToEmitter},
    },
};
use derive_more::Display;
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::Span,
    widgets::{Block, Clear},
};
use slumber_config::Action;
use slumber_core::database::{CollectionDatabase, CollectionId};
use slumber_util::ResultTraced;
use std::path::PathBuf;

/// Display the current collection and select a different collection from a list
///
/// This manages its own open/close state and actions.
#[derive(Debug)]
pub struct CollectionSelect {
    id: ComponentId,
    select: Select<CollectionSelectItem>,
    /// Text box to filter contents. Always in focus
    filter: TextBox,
    /// When open, the full list of collections is displayed and the user can
    /// filter them by text and select a different collection. When closed,
    /// just show the current collection
    open: bool,
}

impl CollectionSelect {
    pub fn new() -> Self {
        Self {
            id: ComponentId::default(),
            select: build_select(""),
            filter: TextBox::default().subscribe([TextBoxEvent::Change]),
            open: false,
        }
    }

    /// Label text for the current collection. This is exposed so the parent can
    /// use it for sizing
    pub fn text(&self) -> String {
        // We need to grab the active collection, not just what's selected in
        // the list. This shouldn't change while the list is open
        let collection_name =
            ViewContext::with_database(CollectionDatabase::metadata)
                .map(|metadata| metadata.display_name())
                .traced()
                .unwrap_or_default();
        TuiContext::get()
            .input_engine
            .add_hint(collection_name, Action::SelectCollection)
    }
}

impl Default for CollectionSelect {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for CollectionSelect {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            .action(|action, propagate| match action {
                Action::SelectCollection if !self.open => self.open = true,
                Action::Cancel if self.open => {
                    self.open = false;
                    self.filter.set_text(String::new()); // Reset filter text box
                }
                _ => propagate.set(),
            })
            .emitted(self.select.to_emitter(), |event| {
                // The ol' Tennessee Switcharoo
                if let SelectEvent::Submit(index) = event {
                    let item = &self.select[index];
                    ViewContext::send_message(Message::CollectionSelect(
                        item.path.clone(),
                    ));
                }
            })
            .emitted(self.filter.to_emitter(), |event| {
                // Rebuild the list with the filter applied
                if let TextBoxEvent::Change = event {
                    self.select = build_select(self.filter.text());
                }
            })
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        // Select gets priority because it handles submission
        vec![self.select.to_child_mut(), self.filter.to_child_mut()]
    }
}

impl Draw for CollectionSelect {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        if self.open {
            // Open - show the full list
            // We're going to expand outside of our given area and overlay over
            // the main panes
            let filter_area = Rect {
                // Filter fills the entire bottom row
                x: 0,
                width: canvas.area().width,
                ..metadata.area()
            };
            let select_height = self.select.len().min(5) as u16;
            let select_area = Rect {
                height: select_height,
                y: filter_area.y - select_height,
                ..filter_area
            };

            // Clear previous styling
            canvas.render_widget(Clear, select_area.union(filter_area));

            // Select with background to provide contrast
            canvas.render_widget(
                Block::new().style(Style::new().bg(Color::DarkGray)),
                select_area,
            );
            canvas.draw(
                &self.select,
                SelectListProps::modal(),
                select_area,
                true,
            );

            canvas.draw(
                &self.filter,
                TextBoxProps::default(),
                filter_area,
                true,
            );
        } else {
            // Closed - just show the selected collection
            let styles = &TuiContext::get().styles;
            let text = Span::styled(self.text(), styles.text.highlight);
            canvas.render_widget(text, metadata.area());
        }
    }
}

#[derive(Debug, Display)]
#[display("{display_name}")]
struct CollectionSelectItem {
    id: CollectionId,
    display_name: String,
    path: PathBuf,
}

// Persistence
impl PartialEq<CollectionId> for CollectionSelectItem {
    fn eq(&self, id: &CollectionId) -> bool {
        &self.id == id
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
