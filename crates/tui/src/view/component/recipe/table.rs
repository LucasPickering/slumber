use crate::{
    context::TuiContext,
    view::{
        common::{
            Checkbox,
            actions::MenuItem,
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
        event::{Emitter, Event, EventMatch, ToEmitter},
        persistent::{PersistentKey, PersistentStore},
    },
};
use ratatui::{
    layout::{Constraint, Layout, Spacing},
    widgets::Block,
};
use slumber_config::Action;
use slumber_core::http::{BuildFieldOverride, BuildFieldOverrides};
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
/// Generic params define the keys to use for persisting state
#[derive(Debug)]
pub struct RecipeFieldTable<RowSelectKey, RowToggleKey> {
    id: ComponentId,
    /// What kind of data we we storing? e.g. "Header"
    noun: &'static str,
    /// Emitter for menu actions
    actions_emitter: Emitter<RecipeTableMenuAction>,
    /// Persistence key to store which row is selected
    select_persistent_key: RowSelectKey,
    /// Selectable rows
    select: ComponentSelect<RecipeFieldTableRow<RowToggleKey>>,
}

impl<RowSelectKey, RowToggleKey> RecipeFieldTable<RowSelectKey, RowToggleKey>
where
    RowSelectKey: PersistentKey<Value = String>,
    RowToggleKey: 'static + PersistentKey<Value = bool>,
{
    pub fn new(
        noun: &'static str,
        select_key: RowSelectKey,
        rows: impl IntoIterator<
            Item = (String, Template, TemplateOverrideKey, RowToggleKey),
        >,
        can_stream: bool,
    ) -> Self {
        let rows: Vec<RecipeFieldTableRow<RowToggleKey>> = rows
            .into_iter()
            .enumerate()
            .map(|(i, (key, template, override_key, toggle_key))| {
                RecipeFieldTableRow::new(
                    i, // This will be the unique ID for the row
                    key,
                    EditableTemplate::new(
                        override_key,
                        template.clone(),
                        can_stream,
                        false,
                    ),
                    toggle_key,
                )
            })
            .collect();

        let select = Select::builder(rows)
            .persisted(&select_key)
            .subscribe([SelectEventType::Select, SelectEventType::Toggle])
            .build();

        Self {
            id: Default::default(),
            noun,
            actions_emitter: Default::default(),
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

    /// Enter edit mode in the selected row
    fn edit_selected_row(&mut self) {
        if let Some(selected_row) = self.select.selected_mut() {
            selected_row.value.edit();
        }
    }

    /// Reset override on selected row
    fn reset_selected_row(&mut self) {
        if let Some(selected_row) = self.select.selected_mut() {
            selected_row.value.reset_override();
        }
    }
}

impl<RowSelectKey, RowToggleKey> Component
    for RecipeFieldTable<RowSelectKey, RowToggleKey>
where
    RowSelectKey: PersistentKey<Value = String>,
    RowToggleKey: 'static + PersistentKey<Value = bool>,
{
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
            .emitted(self.actions_emitter, |menu_action| match menu_action {
                // The selected row can't change while the action menu is open,
                // so we don't need to plumb the index/key through
                RecipeTableMenuAction::Edit => self.edit_selected_row(),
                RecipeTableMenuAction::Reset => {
                    self.reset_selected_row();
                }
            })
    }

    fn menu(&self) -> Vec<MenuItem> {
        let emitter = self.actions_emitter;
        let noun = self.noun;
        let selected = self.select.selected();
        vec![
            emitter
                .menu(RecipeTableMenuAction::Edit, format!("Edit {noun}"))
                .enable(selected.is_some())
                .shortcut(Some(Action::Edit))
                .into(),
            emitter
                .menu(RecipeTableMenuAction::Reset, format!("Reset {noun}"))
                .enable(selected.is_some_and(|row| row.value.is_overridden()))
                .shortcut(Some(Action::Reset))
                .into(),
        ]
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

impl<'a, RowSelectKey, RowToggleKey> Draw<RecipeFieldTableProps<'a>>
    for RecipeFieldTable<RowSelectKey, RowToggleKey>
where
    RowSelectKey: PersistentKey<Value = String>,
    RowToggleKey: 'static + PersistentKey<Value = bool>,
{
    fn draw(
        &self,
        canvas: &mut Canvas,
        props: RecipeFieldTableProps<'a>,
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
        canvas.render_widget(props.key_header, key_header_area);
        canvas.render_widget(props.value_header, value_header_area);

        // Draw rows
        let item_props = RecipeFieldTableRowProps { key_column_width };
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

#[derive(Debug)]
enum RecipeTableMenuAction {
    Edit,
    Reset,
}

#[derive(Debug)]
pub struct RecipeFieldTableProps<'a> {
    /// Label for the left column in the table
    pub key_header: &'a str,
    /// Label for the right column in the table
    pub value_header: &'a str,
}

/// One row in the query/header table. Generic param is the persistence key to
/// use for toggle state
#[derive(Debug)]
struct RecipeFieldTableRow<RowToggleKey> {
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

impl<RowToggleKey> RecipeFieldTableRow<RowToggleKey>
where
    RowToggleKey: PersistentKey<Value = bool>,
{
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

impl<RowToggleKey> Component for RecipeFieldTableRow<RowToggleKey>
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

impl<RowToggleKey> Draw<RecipeFieldTableRowProps>
    for RecipeFieldTableRow<RowToggleKey>
where
    RowToggleKey: PersistentKey<Value = bool>,
{
    fn draw(
        &self,
        canvas: &mut Canvas,
        props: RecipeFieldTableRowProps,
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
impl<RowToggleKey> PartialEq<String> for RecipeFieldTableRow<RowToggleKey> {
    fn eq(&self, key: &String) -> bool {
        &self.key == key
    }
}

#[derive(Copy, Clone)]
struct RecipeFieldTableRowProps {
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
            (
                "row0".into(),
                "value0".into(),
                TemplateOverrideKey::query_param(recipe_id.clone(), 0),
                TestRowToggleKey {
                    recipe_id: recipe_id.clone(),
                    key: "row0".into(),
                },
            ),
            (
                "row1".into(),
                "value1".into(),
                TemplateOverrideKey::query_param(recipe_id.clone(), 1),
                TestRowToggleKey {
                    recipe_id: recipe_id.clone(),
                    key: "row1".into(),
                },
            ),
        ];

        let mut component = TestComponent::builder(
            &harness,
            &terminal,
            RecipeFieldTable::new(
                "Row",
                TestRowKey(recipe_id.clone()),
                rows,
                false,
            ),
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
            (
                "row0".into(),
                "value0".into(),
                TemplateOverrideKey::query_param(recipe_id.clone(), 0),
                TestRowToggleKey {
                    recipe_id: recipe_id.clone(),
                    key: "row0".into(),
                },
            ),
            (
                "row1".into(),
                "value1".into(),
                TemplateOverrideKey::query_param(recipe_id.clone(), 1),
                TestRowToggleKey {
                    recipe_id: recipe_id.clone(),
                    key: "row1".into(),
                },
            ),
        ];

        let mut component = TestComponent::builder(
            &harness,
            &terminal,
            RecipeFieldTable::new(
                "Row",
                TestRowKey(recipe_id.clone()),
                rows,
                false,
            ),
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
        let rows = [(
            "row0".into(),
            "value0".into(),
            TemplateOverrideKey::query_param(recipe_id.clone(), 0),
            TestRowToggleKey {
                recipe_id: recipe_id.clone(),
                key: "row0".into(),
            },
        )];

        let mut component = TestComponent::builder(
            &harness,
            &terminal,
            RecipeFieldTable::new(
                "Row",
                TestRowKey(recipe_id.clone()),
                rows,
                false,
            ),
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
        let rows = [
            (
                "row0".into(),
                "".into(),
                TemplateOverrideKey::query_param(recipe_id.clone(), 0),
                TestRowToggleKey {
                    recipe_id: recipe_id.clone(),
                    key: "row0".into(),
                },
            ),
            (
                "row1".into(),
                "".into(),
                TemplateOverrideKey::query_param(recipe_id.clone(), 1),
                TestRowToggleKey {
                    recipe_id: recipe_id.clone(),
                    key: "row1".into(),
                },
            ),
        ];
        let component = TestComponent::builder(
            &harness,
            &terminal,
            RecipeFieldTable::new(
                "Row",
                TestRowKey(recipe_id.clone()),
                rows,
                false,
            ),
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

    fn props_factory() -> RecipeFieldTableProps<'static> {
        RecipeFieldTableProps {
            key_header: "Key",
            value_header: "Value",
        }
    }
}
