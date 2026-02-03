use crate::view::{
    common::{
        Checkbox,
        component_select::{
            ComponentSelect, ComponentSelectProps, SelectStyles,
        },
        select::{Select, SelectEventKind},
    },
    component::{
        Canvas, Component, ComponentId, Draw, DrawMetadata, ToChild,
        editable_template::EditableTemplate, internal::Child,
    },
    context::{UpdateContext, ViewContext},
    event::{Event, EventMatch, ToEmitter},
    persistent::{PersistentKey, PersistentStore, SessionKey},
};
use indexmap::IndexMap;
use ratatui::{
    layout::{Constraint, Layout, Spacing},
    style::Styled,
    widgets::Block,
};
use serde::{Serialize, de::DeserializeOwned};
use slumber_core::{collection::RecipeId, http::BuildFieldOverride};
use slumber_template::Template;
use std::{any, fmt::Debug, hash::Hash, iter, marker::PhantomData};
use unicode_width::UnicodeWidthStr;

/// A table of key-value mappings. This is used in a new places in the recipe
/// pane, and provides some common functionality:
/// - Persist selected toggle
/// - Allow toggling rows, and persist toggled state
/// - Render values as template previwws
/// - Allow editing values for temporary overrides
///
/// See [RecipeTableKind] for a description of the generic param.
#[derive(Debug)]
pub struct RecipeTable<Kind: RecipeTableKind> {
    id: ComponentId,
    /// Persistent key for storing selected row key
    selected_row_key: SelectedRowKey<Kind>,
    /// Selectable rows
    select: ComponentSelect<RecipeTableRow<Kind>>,
}

impl<Kind: RecipeTableKind> RecipeTable<Kind> {
    pub fn new(
        noun: &'static str,
        recipe_id: RecipeId,
        rows: impl IntoIterator<Item = (Kind::Key, Template)>,
        can_stream: bool,
    ) -> Self {
        let rows: Vec<RecipeTableRow<Kind>> = rows
            .into_iter()
            .map(|(key, template)| {
                RecipeTableRow::new(
                    recipe_id.clone(),
                    noun,
                    key,
                    template,
                    can_stream,
                )
            })
            .collect();

        let selected_row_key = SelectedRowKey::new(recipe_id);
        let select = Select::builder(rows)
            .persisted(&selected_row_key)
            .subscribe([SelectEventKind::Select, SelectEventKind::Toggle])
            .build();

        Self {
            id: Default::default(),
            selected_row_key,
            select: ComponentSelect::new(select),
        }
    }

    /// Get the set of disabled/overridden rows for this table
    pub fn to_build_overrides(
        &self,
    ) -> IndexMap<Kind::Key, BuildFieldOverride> {
        self.select
            .items()
            .filter_map(|row| {
                let ovr = row.to_build_override()?;
                Some((row.key.clone(), ovr))
            })
            .collect()
    }
}

impl<Kind: RecipeTableKind> Component for RecipeTable<Kind> {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> EventMatch {
        event
            .m()
            .emitted(self.select.to_emitter(), |event| match event.kind {
                SelectEventKind::Select => {
                    // When changing selection, stop editing the previous item
                    for row in self.select.items_mut() {
                        row.value.submit_edit();
                    }
                }
                SelectEventKind::Toggle => {
                    self.select[event].toggle();
                }
                SelectEventKind::Submit => {}
            })
    }

    fn persist(&self, store: &mut PersistentStore) {
        // Persist selected row
        store.set_opt(
            &self.selected_row_key,
            self.select.selected().map(|row| &row.key),
        );
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.select.to_child_mut()]
    }
}

impl<'a, Kind> Draw<RecipeTableProps<'a>> for RecipeTable<Kind>
where
    Kind: RecipeTableKind,
{
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
            .chain(self.select.items().map(|row| Kind::key_as_str(&row.key)))
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
        let header_style = ViewContext::styles().table.header;
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

/// One row in the query/header table. Generic param is the persistence key to
/// use for toggle/override state
#[derive(Debug)]
struct RecipeTableRow<Kind: RecipeTableKind> {
    id: ComponentId,
    /// **Non-unique** identifier for this row. Keys can be duplicated within
    /// one table (e.g. query params). This should be consistent across reloads
    /// though because this is the *value* persisted to identify which row is
    /// selected.
    key: Kind::Key,
    /// Value template. This includes functionality to make it editable, and
    /// persist the edited value within the current session
    value: EditableTemplate<RowPersistentKey<Kind>>,
    /// Persistence key to store the toggle and override state for this row
    persistent_key: RowPersistentKey<Kind>,
    /// Is the row enabled/included? This is persisted by row *key* rather than
    /// index, which **may not be unique**. E.g. a query param could be
    /// duplicated. This means duplicated keys will all get the same persisted
    /// toggle state. This is a bug but it's hard to fix, because if we persist
    /// by index (the actual unique key), then adding/removing any field to the
    /// table will mess with persistence.
    enabled: bool,
}

impl<Kind: RecipeTableKind> RecipeTableRow<Kind> {
    fn new(
        recipe_id: RecipeId,
        noun: &'static str,
        key: Kind::Key,
        template: Template,
        can_stream: bool,
    ) -> Self {
        let persistent_key = RowPersistentKey::new(recipe_id, key.clone());
        let value = EditableTemplate::new(
            noun,
            persistent_key.clone(),
            template.clone(),
            can_stream,
            false,
        );
        Self {
            id: ComponentId::default(),
            key,
            value,
            enabled: PersistentStore::get(&persistent_key).unwrap_or(true),
            persistent_key,
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

impl<Kind: RecipeTableKind> Component for RecipeTableRow<Kind> {
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

impl<Kind: RecipeTableKind> Draw<RecipeTableRowProps> for RecipeTableRow<Kind> {
    fn draw(
        &self,
        canvas: &mut Canvas,
        props: RecipeTableRowProps,
        metadata: DrawMetadata,
    ) {
        if !self.enabled {
            let styles = ViewContext::styles();
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
        canvas.render_widget(Kind::key_as_str(&self.key), key_area);
        canvas.draw(&self.value, (), value_area, true);
    }
}

// Needed for toggle persistence
impl<Kind: RecipeTableKind> PartialEq<Kind::Key> for RecipeTableRow<Kind> {
    fn eq(&self, key: &Kind::Key) -> bool {
        &self.key == key
    }
}

#[derive(Copy, Clone)]
struct RecipeTableRowProps {
    key_column_width: u16,
}

/// A trait to define a single usage of [RecipeTable], e.g. headers, query
/// params, etc. This serves two purposes:
///
/// - Define the key type for the table
/// - Provide a unique type to attach to each table's persisted state
pub trait RecipeTableKind: 'static {
    /// Type of the key column for this table. The key value **must be unique**
    /// within each table. For most tables this is just `String`, but for tables
    /// that don't have a single unique value (e.g. query parameters), it will
    /// be something more specific to make it unique.
    type Key: 'static
        + Clone // Override persistence (session store)
        + Debug // Override persistence (session store)
        + Eq // Override map key
        + Hash // Override map key
        + PartialEq // Override persistence (session store)
        + Serialize // For selected/toggle persistence
        + DeserializeOwned; // For selected row persistence

    /// Convert the key to a `&str` so it can be displayed
    fn key_as_str(key: &Self::Key) -> &str;
}

/// Persistent key for which row is selected in a single table.
///
/// This key is unique to (table kind, recipe). Each table in each recipe
/// persists its selected row separately.
#[derive(Debug, Serialize)]
#[serde(bound = "")]
struct SelectedRowKey<Kind> {
    /// Which table is this key from? Form, header, etc.
    kind: TypeName<Kind>,
    /// Recipe being edited
    recipe_id: RecipeId,
}

impl<Kind> SelectedRowKey<Kind> {
    fn new(recipe_id: RecipeId) -> Self {
        Self {
            kind: TypeName(PhantomData),
            recipe_id,
        }
    }
}

impl<Kind: RecipeTableKind> PersistentKey for SelectedRowKey<Kind> {
    /// Persist the key of the selected row, which must be unique within the
    /// table
    type Value = Kind::Key;
}

/// Persistent key for data specific to a single row in a single table. This is
/// used to persist:
/// - Toggle state in the persistent store
/// - Override template in the session store
///
/// This key is unique to (table kind, recipe, row). Each row in each table of
/// each recipe has its own state.
#[derive(derive_more::Debug, derive_more::PartialEq, Serialize)]
#[serde(bound = "Kind::Key: Serialize")]
struct RowPersistentKey<Kind: RecipeTableKind> {
    /// Which table is this row from? Form, header, etc.
    kind: TypeName<Kind>,
    /// Recipe being edited
    recipe_id: RecipeId,
    /// Unique row identifier
    row_key: Kind::Key,
}

// Remove `Kind: Clone` bound
impl<Kind: RecipeTableKind> Clone for RowPersistentKey<Kind> {
    fn clone(&self) -> Self {
        Self {
            kind: self.kind.clone(),
            recipe_id: self.recipe_id.clone(),
            row_key: self.row_key.clone(),
        }
    }
}

impl<Kind: RecipeTableKind> RowPersistentKey<Kind> {
    fn new(recipe_id: RecipeId, row_key: Kind::Key) -> Self {
        Self {
            kind: TypeName(PhantomData),
            recipe_id,
            row_key,
        }
    }
}

impl<Kind: RecipeTableKind> PersistentKey for RowPersistentKey<Kind> {
    type Value = bool;
}

impl<Kind: RecipeTableKind> SessionKey for RowPersistentKey<Kind> {
    type Value = Template;
}

/// Serialize `T` as just its fully qualified path. Allows the type to make a
/// unique serialization value for persistence without needing a value of that
/// type.
#[derive(derive_more::Debug, derive_more::PartialEq)]
struct TypeName<T>(PhantomData<T>);

// Remove `T: Clone` bound
impl<T> Clone for TypeName<T> {
    fn clone(&self) -> Self {
        Self(PhantomData)
    }
}

impl<T> Serialize for TypeName<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        any::type_name::<T>().serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestTerminal, terminal},
        view::test_util::{TestComponent, TestHarness, harness},
    };
    use rstest::rstest;
    use slumber_core::collection::RecipeId;
    use slumber_util::Factory;
    use terminput::KeyCode;

    #[derive(Debug)]
    struct TestKey;

    impl RecipeTableKind for TestKey {
        type Key = String;

        fn key_as_str(key: &Self::Key) -> &str {
            key.as_str()
        }
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
        assert_eq!(component.to_build_overrides(), IndexMap::new());

        // Disable the second row
        component
            .int_props(props_factory)
            .send_keys([KeyCode::Down, KeyCode::Char(' ')])
            .assert()
            .empty();
        let selected_row = component.select.selected().unwrap();
        assert_eq!(&selected_row.key, "row1");
        assert!(!selected_row.enabled);
        assert_eq!(
            component.to_build_overrides(),
            IndexMap::<_, _>::from_iter([(
                "row1".to_owned(),
                BuildFieldOverride::Omit
            )]),
        );

        // Re-enable the row
        component
            .int_props(props_factory)
            .send_key(KeyCode::Char(' '))
            .assert()
            .empty();
        let selected_row = component.select.selected().unwrap();
        assert!(selected_row.enabled);
        assert_eq!(component.to_build_overrides(), IndexMap::new());
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
        assert_eq!(component.to_build_overrides(), IndexMap::new());

        // Edit the second row
        component
            .int_props(props_factory)
            // Open the modal
            .send_keys([KeyCode::Down, KeyCode::Char('e')])
            .send_text("!!!")
            .send_key(KeyCode::Enter)
            .assert()
            .empty();

        let selected_row = component.select.selected().unwrap();
        assert_eq!(&selected_row.key, "row1");
        assert!(selected_row.value.is_overridden());
        assert_eq!(selected_row.value.template().display(), "value1!!!");
        assert_eq!(
            component.to_build_overrides(),
            IndexMap::<_, _>::from_iter([(
                "row1".to_owned(),
                "value1!!!".into()
            )]),
        );

        // Reset edited state
        component
            .int_props(props_factory)
            .send_key(KeyCode::Char('z'))
            .assert()
            .empty();
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
            .action(&["Edit Row"])
            .send_keys([KeyCode::Char('!'), KeyCode::Enter])
            .assert()
            .empty();

        let selected_row = component.select.selected().unwrap();
        assert_eq!(selected_row.value.template().display(), "value0!");
    }

    /// Override templates should be loaded from the store on init
    #[rstest]
    fn test_persisted_override(harness: TestHarness, terminal: TestTerminal) {
        let recipe_id = RecipeId::factory(());
        harness.persistent_store().set_session(
            RowPersistentKey::<TestKey>::new(
                recipe_id.clone(),
                "row0".to_owned(),
            ),
            "p0".into(),
        );
        harness.persistent_store().set_session(
            RowPersistentKey::<TestKey>::new(
                recipe_id.clone(),
                "row1".to_owned(),
            ),
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
            IndexMap::<_, _>::from_iter([
                ("row0".to_owned(), "p0".into()),
                ("row1".to_owned(), "p1".into()),
            ]),
        );
    }

    fn props_factory() -> RecipeTableProps<'static> {
        RecipeTableProps {
            key_header: "Key",
            value_header: "Value",
        }
    }
}
