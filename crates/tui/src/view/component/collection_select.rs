use super::Component;
use crate::{
    context::TuiContext,
    message::Message,
    util::ResultReported,
    view::{
        UpdateContext, ViewContext,
        common::{list::List, modal::Modal},
        draw::{Draw, DrawMetadata, ToStringGenerate},
        event::{Child, Event, EventHandler, OptionEvent, ToEmitter},
        state::select::{SelectState, SelectStateEvent, SelectStateEventType},
    },
};
use ratatui::{
    Frame,
    layout::Layout,
    prelude::{Constraint, Line},
    text::Text,
};
use slumber_core::database::CollectionDatabase;
use slumber_util::ResultTraced;
use std::path::PathBuf;

/// A modal to list all collections in the DB, allowing the user to switch to a
/// different one
#[derive(Debug)]
pub struct CollectionSelect {
    select: Component<SelectState<CollectionSelectItem>>,
}

impl CollectionSelect {
    pub fn new() -> Self {
        // Build the collection list from the DB's collections table. Preselect
        // the current collection
        let collections =
            ViewContext::with_database(|db| db.root().collections())
                .reported(&ViewContext::messages_tx())
                .unwrap_or_default();
        let current_collection =
            ViewContext::with_database(CollectionDatabase::collection_path)
                .traced()
                .ok();

        let select = SelectState::builder(
            collections
                .into_iter()
                .map(|path| CollectionSelectItem { path })
                .collect(),
        )
        .preselect_opt(current_collection.as_ref())
        .subscribe([SelectStateEventType::Submit])
        .build();

        Self {
            select: select.into(),
        }
    }
}

impl Modal for CollectionSelect {
    fn title(&self) -> Line<'_> {
        "Collections".into()
    }

    fn dimensions(&self) -> (Constraint, Constraint) {
        let footer_height = 2;
        let height =
            u16::min(self.select.data().len() as u16 + footer_height, 10);
        (Constraint::Percentage(60), Constraint::Length(height))
    }
}

impl EventHandler for CollectionSelect {
    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event.opt().emitted(self.select.to_emitter(), |event| {
            // The ol' Tennessee Switcharoo
            if let SelectStateEvent::Submit(index) = event {
                let item = &self.select.data()[index];
                self.close(true);
                ViewContext::send_message(Message::CollectionSelect(
                    item.path.clone(),
                ));
            }
        })
    }

    fn children(&mut self) -> Vec<Component<Child<'_>>> {
        vec![self.select.to_child_mut()]
    }
}

impl Draw for CollectionSelect {
    fn draw(&self, frame: &mut Frame, (): (), metadata: DrawMetadata) {
        let styles = &TuiContext::get().styles;
        let [select_area, _, footer_area] = Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(metadata.area());
        self.select.draw(
            frame,
            List::from(self.select.data()),
            select_area,
            true,
        );
        frame.render_widget(
            Text::from(
                "Only collections that have been opened before are shown",
            )
            .style(styles.text.note),
            footer_area,
        );
    }
}

#[derive(Debug, derive_more::Display)]
#[display("{}", path.display())]
struct CollectionSelectItem {
    path: PathBuf,
}

impl PartialEq<CollectionSelectItem> for PathBuf {
    fn eq(&self, other: &CollectionSelectItem) -> bool {
        self == &other.path
    }
}

impl ToStringGenerate for CollectionSelectItem {}
