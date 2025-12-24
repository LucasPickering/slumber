use crate::{
    context::TuiContext,
    view::{
        common::{
            Checkbox,
            component_select::{
                ComponentSelect, ComponentSelectProps, SelectStyles,
            },
            select::{Select, SelectEvent, SelectEventType},
        },
        component::{
            Canvas, Component, ComponentId, Draw, DrawMetadata, ToChild,
            internal::Child,
            override_template::{EditableTemplate, TemplateOverrideKey},
        },
        context::UpdateContext,
        event::{Event, EventMatch, ToEmitter},
        persistent::{PersistentKey, PersistentStore},
    },
};
use ratatui::{
    layout::{Constraint, Layout, Spacing},
    style::Styled,
    widgets::Block,
};
use slumber_core::{
    collection::RecipeId,
    http::{BuildFieldOverride, BuildFieldOverrides},
};
use slumber_template::Template;
use std::iter;
use unicode_width::UnicodeWidthStr;

/// A table of key-value mappings. This is used in a new places in the recipe
/// pane, and provides some common functionality:
/// - Persist selected toggle
/// - Allow toggling rows, and persist toggled state
/// - Render values as template previwws
/// - Allow editing values for temporary overrides
///
/// The generic param defines some common behavior via the trait
/// [RecipeTableKey].
#[derive(Debug)]
pub struct RecipeTable<K: RecipeTableKey> {
    id: ComponentId,
    /// Persistence key to store which row is selected
    select_persistent_key: K::SelectKey,
    /// Selectable rows
    select: ComponentSelect<RecipeTableRow<K::ToggleKey>>,
}

impl<K: RecipeTableKey> RecipeTable<K> {
    pub fn new(
        noun: &'static str,
        recipe_id: RecipeId,
        rows: impl IntoIterator<Item = (String, Template)>,
        can_stream: bool,
    ) -> Self {
        let rows: Vec<RecipeTableRow<K::ToggleKey>> = rows
            .into_iter()
            .enumerate()
            .map(|(i, (key, template))| {
                let toggle_key = K::toggle_key(recipe_id.clone(), key.clone());
                let override_key = K::override_key(recipe_id.clone(), i);
                RecipeTableRow::new(
                    i, // This will be the unique ID for the row
                    key,
                    EditableTemplate::new(
                        noun,
                        override_key,
                        template.clone(),
                        can_stream,
                        false,
                    ),
                    toggle_key,
                )
            })
            .collect();

        let select_key = K::select_key(recipe_id);
        let select = Select::builder(rows)
            .persisted(&select_key)
            .subscribe([SelectEventType::Select, SelectEventType::Toggle])
            .build();

        Self {
            id: Default::default(),
            select_persistent_key: select_key,
            select: ComponentSelect::new(select),
        }
    }

    /// Get the set of disabled/overridden rows for this table
    pub fn to_build_overrides(&self) -> BuildFieldOverrides {
        self.select
            .items()
            .filter_map(|row| {
                row.to_build_override().map(|ovr| (row.index, ovr))
            })
            .collect()
    }
}

impl<K: RecipeTableKey> Component for RecipeTable<K> {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            .emitted(self.select.to_emitter(), |event| match event {
                SelectEvent::Select(_) => {
                    // When changing selection, stop editing the previous item
                    for row in self.select.items_mut() {
                        row.value.submit_edit();
                    }
                }
                SelectEvent::Toggle(index) => {
                    self.select[index].toggle();
                }
                SelectEvent::Submit(_) => {}
            })
    }

    fn persist(&self, store: &mut PersistentStore) {
        // Persist selected row
        store.set_opt(
            &self.select_persistent_key,
            self.select.selected().map(|row| &row.key),
        );
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.select.to_child_mut()]
    }
}

impl<'a, K: RecipeTableKey> Draw<RecipeTableProps<'a>> for RecipeTable<K> {
    fn draw(
        &self,
        canvas: &mut Canvas,
        props: RecipeTableProps<'a>,
        metadata: DrawMetadata,
    ) {
        let [header_area, rows_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)])
                .areas(metadata.area());

        // Find the widest key so we know how to size the key column
        let key_column_width = iter::once(props.key_header)
            .chain(self.select.items().map(|row| row.key.as_str()))
            .map(UnicodeWidthStr::width)
            .max()
            .unwrap_or(0) as u16
            + 1; // Padding!
        let [_, key_header_area, value_header_area] = Layout::horizontal([
            Constraint::Length(4), // Checkbox padding
            Constraint::Length(key_column_width),
            Constraint::Min(1),
        ])
        .areas(header_area);

        // Draw header
        let header_style = TuiContext::get().styles.table.header;
        canvas.render_widget(
            props.key_header.set_style(header_style),
            key_header_area,
        );
        canvas.render_widget(
            props.value_header.set_style(header_style),
            value_header_area,
        );

        // Draw rows
        let item_props = RecipeTableRowProps { key_column_width };
        canvas.draw(
            &self.select,
            ComponentSelectProps {
                styles: SelectStyles::table(),
                spacing: Spacing::default(),
                item_props: Box::new(move |_, _| (item_props, 1)),
            },
            rows_area,
            true,
        );
    }
}

/// Draw props for [RecipeTable]
#[derive(Debug)]
pub struct RecipeTableProps<'a> {
    /// Label for the left column in the table
    pub key_header: &'a str,
    /// Label for the right column in the table
    pub value_header: &'a str,
}

/// Abstraction for row types in [RecipeTable]
pub trait RecipeTableKey {
    /// Persistent key to store the selected row in the table
    type SelectKey: PersistentKey<Value = String>;
    /// Persistent key to store toggle state for a single row
    type ToggleKey: PersistentKey<Value = bool>;

    /// Get the key under which row selection state is persisted for this table.
    /// Typically just a wrapper around the recipe ID.
    fn select_key(recipe_id: RecipeId) -> Self::SelectKey;

    /// Get the key under which toggle state for a single row is persisted
    fn toggle_key(recipe_id: RecipeId, key: String) -> Self::ToggleKey;

    /// Get the key under which the template override for a single row is
    /// persisted in the session store
    fn override_key(recipe_id: RecipeId, index: usize) -> TemplateOverrideKey;
}

/// One row in the query/header table. Generic param is the persistence key to
/// use for toggle state
#[derive(Debug)]
struct RecipeTableRow<RowToggleKey> {
    id: ComponentId,
    /// Index of this row in the table. This is the unique ID for this row
    /// **in the context of a single session**. Rows can be added/removed
    /// during a collection reload, so we can't persist this.
    index: usize,
    /// **Non-unique** identifier for this row. Keys can be duplicated within
    /// one table (e.g. query params). This should be consistent across reloads
    /// though because this is the *value* persisted to identify which row is
    /// selected.
    key: String,
    /// Value template. This includes functionality to make it editable, and
    /// persist the edited value within the current session
    value: EditableTemplate,
    /// Persistence key to store the toggle state for this row
    persistent_key: RowToggleKey,
    /// Is the row enabled/included? This is persisted by row *key* rather than
    /// index, which **may not be unique**. E.g. a query param could be
    /// duplicated. This means duplicated keys will all get the same persisted
    /// toggle state. This is a bug but it's hard to fix, because if we persist
    /// by index (the actual unique key), then adding/removing any field to the
    /// table will mess with persistence.
    enabled: bool,
}

impl<RowToggleKey: PersistentKey<Value = bool>> RecipeTableRow<RowToggleKey> {
    fn new(
        index: usize,
        key: String,
        value: EditableTemplate,
        toggle_persistent_key: RowToggleKey,
    ) -> Self {
        Self {
            id: ComponentId::default(),
            index,
            key,
            value,
            enabled: PersistentStore::get(&toggle_persistent_key)
                .unwrap_or(true),
            persistent_key: toggle_persistent_key,
        }
    }

    fn toggle(&mut self) {
        self.enabled ^= true;
    }

    /// Get the disabled/override state of this row
    fn to_build_override(&self) -> Option<BuildFieldOverride> {
        if !self.enabled {
            Some(BuildFieldOverride::Omit)
        } else if self.value.is_overridden() {
            Some(BuildFieldOverride::Override(self.value.template().clone()))
        } else {
            None
        }
    }
}

impl<RowToggleKey> Component for RecipeTableRow<RowToggleKey>
where
    RowToggleKey: PersistentKey<Value = bool>,
{
    fn id(&self) -> ComponentId {
        self.id
    }

    fn persist(&self, store: &mut PersistentStore) {
        // Persist toggle state
        store.set(&self.persistent_key, &self.enabled);
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.value.to_child_mut()]
    }
}

impl<RowToggleKey> Draw<RecipeTableRowProps> for RecipeTableRow<RowToggleKey>
where
    RowToggleKey: PersistentKey<Value = bool>,
{
    fn draw(
        &self,
        canvas: &mut Canvas,
        props: RecipeTableRowProps,
        metadata: DrawMetadata,
    ) {
        if !self.enabled {
            let styles = &TuiContext::get().styles;
            canvas.render_widget(
                Block::new().style(styles.table.disabled),
                metadata.area(),
            );
        }

        let [checkbox_area, key_area, value_area] = Layout::horizontal([
            Constraint::Length(4),
            Constraint::Length(props.key_column_width),
            Constraint::Min(1),
        ])
        .areas(metadata.area());

        // Render each cell
        canvas.render_widget(
            Checkbox {
                checked: self.enabled,
            },
            checkbox_area,
        );
        canvas.render_widget(self.key.as_str(), key_area);
        canvas.draw(&self.value, (), value_area, true);
    }
}

// Needed for toggle persistence
impl<RowToggleKey> PartialEq<String> for RecipeTableRow<RowToggleKey> {
    fn eq(&self, key: &String) -> bool {
        &self.key == key
    }
}

#[derive(Copy, Clone)]
struct RecipeTableRowProps {
    key_column_width: u16,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestHarness, TestTerminal, harness, terminal},
        view::test_util::TestComponent,
    };
    use rstest::rstest;
    use serde::Serialize;
    use slumber_core::collection::RecipeId;
    use slumber_util::Factory;
    use terminput::KeyCode;

    #[derive(Debug)]
    struct TestKey;

    impl RecipeTableKey for TestKey {
        type SelectKey = TestRowKey;
        type ToggleKey = TestRowToggleKey;

        fn select_key(recipe_id: RecipeId) -> Self::SelectKey {
            TestRowKey(recipe_id)
        }

        fn toggle_key(recipe_id: RecipeId, key: String) -> Self::ToggleKey {
            TestRowToggleKey { recipe_id, key }
        }

        fn override_key(
            recipe_id: RecipeId,
            index: usize,
        ) -> TemplateOverrideKey {
            TemplateOverrideKey::query_param(recipe_id, index)
        }
    }

    #[derive(Debug, Serialize)]
    struct TestRowKey(RecipeId);

    impl PersistentKey for TestRowKey {
        type Value = String;
    }

    #[derive(Debug, Serialize)]
    struct TestRowToggleKey {
        recipe_id: RecipeId,
        key: String,
    }

    impl PersistentKey for TestRowToggleKey {
        type Value = bool;
    }

    /// User can hide a row from the recipe
    #[rstest]
    fn test_disabled_row(harness: TestHarness, terminal: TestTerminal) {
        let recipe_id = RecipeId::factory(());
        let rows = [
            ("row0".into(), "value0".into()),
            ("row1".into(), "value1".into()),
        ];

        let mut component = TestComponent::builder(
            &harness,
            &terminal,
            RecipeTable::<TestKey>::new("Row", recipe_id, rows, false),
        )
        .with_props(props_factory())
        .build();

        // Check initial state
        assert_eq!(
            component.to_build_overrides(),
            BuildFieldOverrides::default()
        );

        // Disable the second row
        component
            .int_props(props_factory)
            .drain_draw() // Clear initial events
            .send_keys([KeyCode::Down, KeyCode::Char(' ')])
            .assert_empty();
        let selected_row = component.select.selected().unwrap();
        assert_eq!(&selected_row.key, "row1");
        assert!(!selected_row.enabled);
        assert_eq!(
            component.to_build_overrides(),
            [(1, BuildFieldOverride::Omit)].into_iter().collect(),
        );

        // Re-enable the row
        component
            .int_props(props_factory)
            .send_key(KeyCode::Char(' '))
            .assert_empty();
        let selected_row = component.select.selected().unwrap();
        assert!(selected_row.enabled);
        assert_eq!(
            component.to_build_overrides(),
            BuildFieldOverrides::default(),
        );
    }

    /// User can edit the value for a row
    #[rstest]
    fn test_override_row(harness: TestHarness, terminal: TestTerminal) {
        let recipe_id = RecipeId::factory(());
        let rows = [
            ("row0".into(), "value0".into()),
            ("row1".into(), "value1".into()),
        ];

        let mut component = TestComponent::builder(
            &harness,
            &terminal,
            RecipeTable::<TestKey>::new("Row", recipe_id, rows, false),
        )
        .with_props(props_factory())
        .build();

        // Check initial state
        assert_eq!(
            component.to_build_overrides(),
            BuildFieldOverrides::default()
        );

        // Edit the second row
        component
            .int_props(props_factory)
            .drain_draw() // Clear initial events
            // Open the modal
            .send_keys([KeyCode::Down, KeyCode::Char('e')])
            .send_text("!!!")
            .send_key(KeyCode::Enter)
            .assert_empty();

        let selected_row = component.select.selected().unwrap();
        assert_eq!(&selected_row.key, "row1");
        assert!(selected_row.value.is_overridden());
        assert_eq!(selected_row.value.template().display(), "value1!!!");
        assert_eq!(
            component.to_build_overrides(),
            [(1, BuildFieldOverride::Override("value1!!!".into()))]
                .into_iter()
                .collect(),
        );

        // Reset edited state
        component
            .int_props(props_factory)
            .send_key(KeyCode::Char('z'))
            .assert_empty();
        let selected_row = component.select.selected().unwrap();
        assert!(!selected_row.value.is_overridden());
    }

    /// Test Edit menu action
    #[rstest]
    fn test_edit_action(harness: TestHarness, terminal: TestTerminal) {
        let recipe_id = RecipeId::factory(());
        let rows = [("row0".into(), "value0".into())];

        let mut component = TestComponent::builder(
            &harness,
            &terminal,
            RecipeTable::<TestKey>::new("Row", recipe_id, rows, false),
        )
        .with_props(props_factory())
        .build();

        component
            .int_props(props_factory)
            .drain_draw() // Clear initial events
            .action(&["Edit Row"])
            .send_keys([KeyCode::Char('!'), KeyCode::Enter])
            .assert_empty();

        let selected_row = component.select.selected().unwrap();
        assert_eq!(selected_row.value.template().display(), "value0!");
    }

    /// Override templates should be loaded from the store on init
    #[rstest]
    fn test_persisted_override(harness: TestHarness, terminal: TestTerminal) {
        let recipe_id = RecipeId::factory(());
        harness.persistent_store().set_session(
            TemplateOverrideKey::query_param(recipe_id.clone(), 0),
            "p0".into(),
        );
        harness.persistent_store().set_session(
            TemplateOverrideKey::query_param(recipe_id.clone(), 1),
            "p1".into(),
        );
        let rows = [("row0".into(), "".into()), ("row1".into(), "".into())];
        let component = TestComponent::builder(
            &harness,
            &terminal,
            RecipeTable::<TestKey>::new("Row", recipe_id, rows, false),
        )
        .with_props(props_factory())
        .build();

        assert_eq!(
            component.to_build_overrides(),
            [
                (0, BuildFieldOverride::Override("p0".into())),
                (1, BuildFieldOverride::Override("p1".into()))
            ]
            .into_iter()
            .collect(),
        );
    }

    fn props_factory() -> RecipeTableProps<'static> {
        RecipeTableProps {
            key_header: "Key",
            value_header: "Value",
        }
    }
}
