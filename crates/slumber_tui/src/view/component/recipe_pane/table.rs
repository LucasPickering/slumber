use crate::view::{
    common::{
        table::{Table, ToggleRow},
        template_preview::TemplatePreview,
    },
    component::Component,
    context::{Persisted, PersistedKey, PersistedLazy},
    draw::{Draw, DrawMetadata, Generate},
    event::EventHandler,
    state::select::SelectState,
};
use itertools::Itertools;
use ratatui::{layout::Constraint, widgets::TableState, Frame};
use slumber_core::{
    collection::{HasId, ProfileId},
    template::Template,
};

/// A table of key-value mappings. This is used in a new places in the recipe
/// pane, and provides some common functionality:
/// - Persist selected togle
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
}

impl<RowSelectKey, RowToggleKey> RecipeFieldTable<RowSelectKey, RowToggleKey>
where
    RowSelectKey: PersistedKey<Value = Option<String>>,
    RowToggleKey: 'static + PersistedKey<Value = bool>,
{
    pub fn new(
        select_key: RowSelectKey,
        selected_profile_id: Option<&ProfileId>,
        rows: impl IntoIterator<Item = (String, Template, RowToggleKey)>,
    ) -> Self {
        let items = rows
            .into_iter()
            .map(|(key, value, toggle_key)| {
                RowState::new(
                    key,
                    TemplatePreview::new(
                        value,
                        selected_profile_id.cloned(),
                        None,
                    ),
                    toggle_key,
                )
            })
            .collect();
        let select = SelectState::builder(items)
            .on_toggle(RowState::toggle)
            .build();
        Self {
            select: PersistedLazy::new(select_key, select).into(),
        }
    }

    /// Get the set of disabled rows for this table
    pub fn to_disabled_indexes(&self) -> Vec<usize> {
        self.select
            .data()
            .items()
            .enumerate()
            .filter(|(_, row)| !*row.enabled)
            .map(|(i, _)| i)
            .collect()
    }
}

impl<RowSelectKey, RowToggleKey> EventHandler
    for RecipeFieldTable<RowSelectKey, RowToggleKey>
where
    RowSelectKey: PersistedKey<Value = Option<String>>,
    RowToggleKey: PersistedKey<Value = bool>,
{
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
                .map(|row| {
                    ToggleRow::new(
                        [row.key.as_str().into(), row.value.generate()],
                        *row.enabled,
                    )
                    .generate()
                })
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
    key: String,
    value: TemplatePreview,
    enabled: Persisted<K>,
}

impl<K: PersistedKey<Value = bool>> RowState<K> {
    fn new(key: String, value: TemplatePreview, persisted_key: K) -> Self {
        Self {
            key,
            value,
            enabled: Persisted::new(persisted_key, true),
        }
    }

    fn toggle(&mut self) {
        *self.enabled.borrow_mut() ^= true;
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
