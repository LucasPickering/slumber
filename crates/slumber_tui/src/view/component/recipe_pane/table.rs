use crate::{
    context::TuiContext,
    util::ResultReported,
    view::{
        common::{
            table::{Table, ToggleRow},
            template_preview::TemplatePreview,
            text_box::TextBox,
        },
        component::{misc::TextBoxModal, Component},
        context::{Persisted, PersistedKey, PersistedLazy},
        draw::{Draw, DrawMetadata, Generate},
        event::{Event, EventHandler, Update},
        state::select::SelectState,
        ViewContext,
    },
};
use itertools::Itertools;
use ratatui::{
    layout::Constraint,
    text::Span,
    widgets::{Row, TableState},
    Frame,
};
use slumber_config::Action;
use slumber_core::{
    collection::{HasId, ProfileId},
    http::{BuildFieldOverride, BuildFieldOverrides},
    template::Template,
};
use std::str::FromStr;

/// A table of key-value mappings. This is used in a new places in the recipe
/// pane, and provides some common functionality:
/// - Persist selected toggle
/// - Allow toggling rows, and persist toggled state
/// - Render values as template previwws
/// - Allow editing values for temporary overrides
///
/// Generic params define the keys to use for persisting state
#[derive(Debug)]
pub struct RecipeFieldTable<RowSelectKey, RowToggleKey>
where
    RowSelectKey: PersistedKey<Value = Option<String>>,
    RowToggleKey: PersistedKey<Value = bool>,
{
    select: Component<
        PersistedLazy<
            RowSelectKey,
            SelectState<RowState<RowToggleKey>, TableState>,
        >,
    >,
    /// Needed for template previews
    selected_profile_id: Option<ProfileId>,
}

impl<RowSelectKey, RowToggleKey> RecipeFieldTable<RowSelectKey, RowToggleKey>
where
    RowSelectKey: PersistedKey<Value = Option<String>>,
    RowToggleKey: 'static + PersistedKey<Value = bool>,
{
    pub fn new(
        select_key: RowSelectKey,
        selected_profile_id: Option<ProfileId>,
        rows: impl IntoIterator<Item = (String, Template, RowToggleKey)>,
    ) -> Self {
        let items = rows
            .into_iter()
            .enumerate()
            .map(|(i, (key, value, toggle_key))| RowState {
                index: i, // This will be the unique ID for the row
                key,
                value: value.clone(),
                preview: TemplatePreview::new(
                    value,
                    selected_profile_id.clone(),
                    None,
                ),
                overridden: false,
                enabled: Persisted::new(toggle_key, true),
            })
            .collect();
        let select = SelectState::builder(items)
            .on_toggle(RowState::toggle)
            .build();
        Self {
            select: PersistedLazy::new(select_key, select).into(),
            selected_profile_id,
        }
    }

    /// Get the set of disabled/overriden rows for this table
    pub fn to_build_overrides(&self) -> BuildFieldOverrides {
        self.select
            .data()
            .items()
            .filter_map(|row| {
                row.to_build_override().map(|ovr| (row.index, ovr))
            })
            .collect()
    }
}

impl<RowSelectKey, RowToggleKey> EventHandler
    for RecipeFieldTable<RowSelectKey, RowToggleKey>
where
    RowSelectKey: PersistedKey<Value = Option<String>>,
    RowToggleKey: PersistedKey<Value = bool>,
{
    fn update(&mut self, event: Event) -> Update {
        if let Some(Action::Edit) = event.action() {
            if let Some(selected_row) = self.select.data().selected() {
                selected_row.open_edit_modal();
            }
            // Consume the event even if we have no rows, for consistency
        } else if let Some(SaveOverride { row_index, value }) = event.local() {
            // The row we're modifying *should* still be the selected row,
            // because it shouldn't be possible to change the selection while
            // the edit modal is open. It's safer to re-grab the modal by index
            // though, just to be sure we've got the right one.
            self.select.data_mut().items_mut()[*row_index]
                .value
                .set_override(self.selected_profile_id.clone(), value);
        } else {
            return Update::Propagate(event);
        }
        Update::Consumed
    }

    fn children(&mut self) -> Vec<Component<&mut dyn EventHandler>> {
        vec![self.select.as_child()]
    }
}

impl<'a, RowSelectKey, RowToggleKey> Draw<RecipeFieldTableProps<'a>>
    for RecipeFieldTable<RowSelectKey, RowToggleKey>
where
    RowSelectKey: PersistedKey<Value = Option<String>>,
    RowToggleKey: PersistedKey<Value = bool>,
{
    fn draw(
        &self,
        frame: &mut Frame,
        props: RecipeFieldTableProps<'a>,
        metadata: DrawMetadata,
    ) {
        let table = Table {
            rows: self
                .select
                .data()
                .items()
                .map(Generate::generate)
                .collect_vec(),
            header: Some(["", props.key_header, props.value_header]),
            column_widths: &[
                Constraint::Min(3),
                Constraint::Percentage(50),
                Constraint::Percentage(50),
            ],
            ..Default::default()
        };
        self.select
            .draw(frame, table.generate(), metadata.area(), true);
    }
}

#[derive(Clone)]
pub struct RecipeFieldTableProps<'a> {
    /// Label for the left column in the table
    pub key_header: &'a str,
    /// Label for the right column in the table
    pub value_header: &'a str,
}

/// One row in the query/header table. Generic param is the persistence key to
/// use for toggle state
#[derive(Debug)]
struct RowState<K: PersistedKey<Value = bool>> {
    /// Index of this row in the table. This is the unique ID for this row
    /// **in the context of a single session**. Rows can be added/removed
    /// during a collection reload, so we can't persist this.
    index: usize,
    /// Persistent (but not unique) identifier for this row. Keys can be
    /// duplicated within one table (e.g. query params), but this is how we
    /// link instances of a row across collection reloads.
    key: String,
    /// We hang onto the source template so we can edit it
    value: Template,
    preview: TemplatePreview,
    /// Has the user modified the template? If so we'll provide this as an
    /// override when generating build options
    overridden: bool,
    /// Is the row enabled/included? This is persisted by row *key* rather than
    /// index, which **may not be unique**. E.g. a query param could be
    /// duplicated. This means duplicated keys will all get the same persisted
    /// toggle state. This is a bug but it's hard to fix, because if we persist
    /// by index (the actual unique key), then adding/removing any field to the
    /// table will mess with persistence.
    enabled: Persisted<K>,
}

impl<K: PersistedKey<Value = bool>> Generate for &RowState<K> {
    type Output<'this> = Row<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        let styles = &TuiContext::get().styles;
        let mut preview_text = self.preview.generate();
        if self.overridden {
            preview_text.push_span(Span::styled(" (edited)", styles.text.hint));
        }
        ToggleRow::new([self.key.as_str().into(), preview_text], *self.enabled)
            .generate()
    }
}

impl<K: PersistedKey<Value = bool>> RowState<K> {
    fn toggle(&mut self) {
        *self.enabled.borrow_mut() ^= true;
    }

    /// Open a modal to create or edit the value's temporary override
    fn open_edit_modal(&self) {
        let index = self.index;
        ViewContext::open_modal(TextBoxModal::new(
            format!("Edit value for {}", self.key),
            TextBox::default()
                // Edit as a raw template
                .default_value(self.value.display().into_owned())
                .validator(|value| Template::from_str(value).is_ok()),
            move |value| {
                // Defer the state update into an event, so it can get &mut
                ViewContext::push_event(Event::new_local(SaveOverride {
                    row_index: index,
                    value,
                }))
            },
        ));
    }

    /// Override the value template and re-render the preview
    fn set_override(
        &mut self,
        selected_profile_id: Option<ProfileId>,
        override_value: &str,
    ) {
        // The validator on the override text box enforces that it's a valid
        // template, so we expect this parse to succeed
        if let Some(template) = override_value
            .parse::<Template>()
            .reported(&ViewContext::messages_tx())
        {
            self.value = template.clone();
            self.preview =
                TemplatePreview::new(template, selected_profile_id, None);
            self.overridden = true;
        }
    }

    /// Get the disabled/override state of this row
    fn to_build_override(&self) -> Option<BuildFieldOverride> {
        if !*self.enabled {
            Some(BuildFieldOverride::Omit)
        } else if self.overridden {
            Some(BuildFieldOverride::Override(self.value.clone()))
        } else {
            None
        }
    }
}

/// Needed for SelectState persistence
impl<K: PersistedKey<Value = bool>> HasId for RowState<K> {
    type Id = String;

    fn id(&self) -> &Self::Id {
        &self.key
    }

    fn set_id(&mut self, id: Self::Id) {
        self.key = id;
    }
}

/// Needed for SelectState persistence
impl<K> PartialEq<RowState<K>> for String
where
    K: PersistedKey<Value = bool>,
{
    fn eq(&self, row_state: &RowState<K>) -> bool {
        self == &row_state.key
    }
}

/// Local event to modify a row's override template. Triggered from the edit
/// modal
#[derive(Debug)]
struct SaveOverride {
    row_index: usize,
    value: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{harness, terminal, TestHarness, TestTerminal},
        view::test_util::{TestComponent, WithModalQueue},
    };
    use crossterm::event::KeyCode;
    use rstest::rstest;
    use serde::Serialize;
    use slumber_core::{collection::RecipeId, test_util::Factory};

    #[derive(Debug, Serialize, persisted::PersistedKey)]
    #[persisted(Option<String>)]
    struct TestRowKey(RecipeId);

    #[derive(Debug, Serialize, persisted::PersistedKey)]
    #[persisted(bool)]
    struct TestRowToggleKey {
        recipe_id: RecipeId,
        key: String,
    }

    /// User can hide a row from the recipe
    #[rstest]
    fn test_disabled_row(_harness: TestHarness, terminal: TestTerminal) {
        let recipe_id = RecipeId::factory(());
        let rows = [
            (
                "row0".into(),
                "value0".into(),
                TestRowToggleKey {
                    recipe_id: recipe_id.clone(),
                    key: "row0".into(),
                },
            ),
            (
                "row1".into(),
                "value1".into(),
                TestRowToggleKey {
                    recipe_id: recipe_id.clone(),
                    key: "row1".into(),
                },
            ),
        ];
        let mut component = TestComponent::new(
            &terminal,
            RecipeFieldTable::new(TestRowKey(recipe_id.clone()), None, rows),
            RecipeFieldTableProps {
                key_header: "Key",
                value_header: "Value",
            },
        );

        // Check initial state
        assert_eq!(
            component.data().to_build_overrides(),
            BuildFieldOverrides::default()
        );

        // Disable the second row
        component.send_key(KeyCode::Down).assert_empty();
        component.send_key(KeyCode::Char(' ')).assert_empty();
        let selected_row = component.data().select.data().selected().unwrap();
        assert_eq!(&selected_row.key, "row1");
        assert!(!*selected_row.enabled);
        assert_eq!(
            component.data().to_build_overrides(),
            [(1, BuildFieldOverride::Omit)].into_iter().collect(),
        );

        // Re-enable the row
        component.send_key(KeyCode::Char(' ')).assert_empty();
        let selected_row = component.data().select.data().selected().unwrap();
        assert!(*selected_row.enabled);
        assert_eq!(
            component.data().to_build_overrides(),
            BuildFieldOverrides::default(),
        );
    }

    /// User can edit the value for a row
    #[rstest]
    fn test_override_row(_harness: TestHarness, terminal: TestTerminal) {
        let recipe_id = RecipeId::factory(());
        let rows = [
            (
                "row0".into(),
                "value0".into(),
                TestRowToggleKey {
                    recipe_id: recipe_id.clone(),
                    key: "row0".into(),
                },
            ),
            (
                "row1".into(),
                "value1".into(),
                TestRowToggleKey {
                    recipe_id: recipe_id.clone(),
                    key: "row1".into(),
                },
            ),
        ];
        let mut component = TestComponent::new(
            &terminal,
            // We'll need a modal queue to handle the edit box
            WithModalQueue::new(RecipeFieldTable::new(
                TestRowKey(recipe_id.clone()),
                None,
                rows,
            )),
            RecipeFieldTableProps {
                key_header: "Key",
                value_header: "Value",
            },
        );

        // Check initial state
        assert_eq!(
            component.data().inner().to_build_overrides(),
            BuildFieldOverrides::default()
        );

        // Edit the second row
        component.send_key(KeyCode::Down).assert_empty();
        component.send_key(KeyCode::Char('e')).assert_empty(); // Open the modal
        component.send_text("!!!").assert_empty();
        component.send_key(KeyCode::Enter).assert_empty();

        let selected_row =
            component.data().inner().select.data().selected().unwrap();
        assert_eq!(&selected_row.key, "row1");
        assert!(selected_row.overridden);
        assert_eq!(selected_row.value.display(), "value1!!!");
        assert_eq!(
            component.data().inner().to_build_overrides(),
            [(1, BuildFieldOverride::Override("value1!!!".into()))]
                .into_iter()
                .collect(),
        );
    }
}
