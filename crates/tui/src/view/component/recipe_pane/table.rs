use crate::view::{
    Generate,
    common::{
        actions::MenuAction,
        table::{Table, ToggleRow},
    },
    component::{
        Component, ComponentExt, ComponentId, Draw, DrawMetadata, ToChild,
        internal::Child,
        recipe_pane::persistence::{RecipeOverrideKey, RecipeTemplate},
    },
    context::UpdateContext,
    event::{Emitter, Event, OptionEvent, ToEmitter},
    state::select::{SelectState, SelectStateEvent, SelectStateEventType},
    util::persistence::{Persisted, PersistedKey, PersistedLazy},
};
use itertools::Itertools;
use ratatui::{
    Frame,
    layout::Constraint,
    widgets::{Row, TableState},
};
use slumber_config::Action;
use slumber_core::{
    collection::HasId,
    http::{BuildFieldOverride, BuildFieldOverrides},
};
use slumber_template::Template;

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
    id: ComponentId,
    /// What kind of data we we storing? e.g. "Header"
    noun: &'static str,
    /// Emitter for the callback from editing a row
    override_emitter: Emitter<SaveRecipeTableOverride>,
    /// Emitter for menu actions
    actions_emitter: Emitter<RecipeTableMenuAction>,
    select: PersistedLazy<
        RowSelectKey,
        SelectState<RowState<RowToggleKey>, TableState>,
    >,
}

impl<RowSelectKey, RowToggleKey> RecipeFieldTable<RowSelectKey, RowToggleKey>
where
    RowSelectKey: PersistedKey<Value = Option<String>>,
    RowToggleKey: 'static + PersistedKey<Value = bool>,
{
    pub fn new(
        noun: &'static str,
        select_key: RowSelectKey,
        rows: impl IntoIterator<
            Item = (String, Template, RecipeOverrideKey, RowToggleKey),
        >,
        can_stream: bool,
    ) -> Self {
        let items = rows
            .into_iter()
            .enumerate()
            .map(|(i, (key, template, override_key, toggle_key))| RowState {
                index: i, // This will be the unique ID for the row
                key,
                value: RecipeTemplate::new(
                    override_key,
                    template.clone(),
                    None,
                    can_stream,
                ),
                enabled: Persisted::new(toggle_key, true),
            })
            .collect();
        let select = SelectState::builder(items)
            .subscribe([SelectStateEventType::Toggle])
            .build();
        Self {
            id: Default::default(),
            noun,
            override_emitter: Default::default(),
            actions_emitter: Default::default(),
            select: PersistedLazy::new(select_key, select),
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

    fn edit_selected_row(&self) {
        if let Some(selected_row) = self.select.selected() {
            selected_row.open_edit_modal(self.override_emitter);
        }
    }

    fn reset_selected_row(&mut self) {
        if let Some(selected_row) = self.select.get_mut().selected_mut() {
            selected_row.value.reset_override();
        }
    }
}

impl<RowSelectKey, RowToggleKey> Component
    for RecipeFieldTable<RowSelectKey, RowToggleKey>
where
    RowSelectKey: PersistedKey<Value = Option<String>>,
    RowToggleKey: 'static + PersistedKey<Value = bool>,
{
    fn id(&self) -> ComponentId {
        self.id
    }

    fn update(&mut self, _: &mut UpdateContext, event: Event) -> Option<Event> {
        event
            .opt()
            .action(|action, propagate| match action {
                // Consume the event even if we have no rows, for consistency
                Action::Edit => self.edit_selected_row(),
                Action::Reset => self.reset_selected_row(),
                _ => propagate.set(),
            })
            .emitted(self.select.to_emitter(), |event| {
                if let SelectStateEvent::Toggle(index) = event {
                    self.select.get_mut()[index].toggle();
                }
            })
            .emitted(
                self.override_emitter,
                |SaveRecipeTableOverride {
                     row_index,
                     template,
                 }| {
                    // The row we're modifying *should* still be the selected
                    // row, because it shouldn't be possible to change the
                    // selection while the edit modal is open. It's safer to
                    // re-grab the modal by index though, just to be sure we've
                    // got the right one.
                    self.select.get_mut().items_mut()[row_index]
                        .value
                        .value
                        .set_override(template);
                },
            )
            .emitted(self.actions_emitter, |menu_action| match menu_action {
                // The selected row can't change while the action menu is open,
                // so we don't need to plumb the index/key through
                RecipeTableMenuAction::Edit => self.edit_selected_row(),
                RecipeTableMenuAction::Reset => {
                    self.reset_selected_row();
                }
            })
    }

    fn menu_actions(&self) -> Vec<MenuAction> {
        let emitter = self.actions_emitter;
        let noun = self.noun;
        let selected = self.select.selected();
        vec![
            emitter
                .menu(RecipeTableMenuAction::Edit, format!("Edit {noun}"))
                .enable(selected.is_some())
                .shortcut(Some(Action::Edit)),
            emitter
                .menu(RecipeTableMenuAction::Reset, format!("Reset {noun}"))
                .enable(selected.is_some_and(|row| row.value.is_overridden()))
                .shortcut(Some(Action::Reset)),
        ]
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![self.select.to_child_mut()]
    }
}

impl<'a, RowSelectKey, RowToggleKey> Draw<RecipeFieldTableProps<'a>>
    for RecipeFieldTable<RowSelectKey, RowToggleKey>
where
    RowSelectKey: PersistedKey<Value = Option<String>>,
    RowToggleKey: 'static + PersistedKey<Value = bool>,
{
    fn draw_impl(
        &self,
        frame: &mut Frame,
        props: RecipeFieldTableProps<'a>,
        metadata: DrawMetadata,
    ) {
        let table = Table {
            rows: self.select.items().map(Generate::generate).collect_vec(),
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

/// Local event to modify a row's override template. Triggered from the edit
/// modal
#[derive(Debug)]
struct SaveRecipeTableOverride {
    row_index: usize,
    template: Template,
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
struct RowState<K: PersistedKey<Value = bool>> {
    /// Index of this row in the table. This is the unique ID for this row
    /// **in the context of a single session**. Rows can be added/removed
    /// during a collection reload, so we can't persist this.
    index: usize,
    /// Persistent (but not unique) identifier for this row. Keys can be
    /// duplicated within one table (e.g. query params), but this is how we
    /// link instances of a row across collection reloads.
    key: String,
    /// Value template. This includes functionality to make it editable, and
    /// persist the edited value within the current session
    value: RecipeTemplate,
    /// Is the row enabled/included? This is persisted by row *key* rather than
    /// index, which **may not be unique**. E.g. a query param could be
    /// duplicated. This means duplicated keys will all get the same persisted
    /// toggle state. This is a bug but it's hard to fix, because if we persist
    /// by index (the actual unique key), then adding/removing any field to the
    /// table will mess with persistence.
    enabled: Persisted<K>,
}

impl<K: PersistedKey<Value = bool>> Generate for &RowState<K> {
    type Output<'this>
        = Row<'this>
    where
        Self: 'this;

    fn generate<'this>(self) -> Self::Output<'this>
    where
        Self: 'this,
    {
        ToggleRow::new(
            [self.key.as_str().into(), self.value.preview().generate()],
            *self.enabled,
        )
        .generate()
    }
}

impl<K: PersistedKey<Value = bool>> RowState<K> {
    fn toggle(&mut self) {
        *self.enabled.get_mut() ^= true;
    }

    /// Open a modal to create or edit the value's temporary override
    fn open_edit_modal(&self, emitter: Emitter<SaveRecipeTableOverride>) {
        let index = self.index;
        self.value.open_edit_modal(
            format!("Edit value for {}", self.key),
            move |template| {
                // Defer the state update into an event, so it can get &mut
                emitter.emit(SaveRecipeTableOverride {
                    row_index: index,
                    template,
                });
            },
        );
    }

    /// Get the disabled/override state of this row
    fn to_build_override(&self) -> Option<BuildFieldOverride> {
        if !*self.enabled {
            Some(BuildFieldOverride::Omit)
        } else if self.value.is_overridden() {
            Some(BuildFieldOverride::Override(self.value.template().clone()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestHarness, TestTerminal, harness, terminal},
        view::{
            component::{
                RecipeOverrideStore,
                recipe_pane::persistence::RecipeOverrideValue,
            },
            test_util::TestComponent,
        },
    };
    use persisted::PersistedStore;
    use rstest::rstest;
    use serde::Serialize;
    use slumber_core::collection::RecipeId;
    use slumber_util::Factory;
    use terminput::KeyCode;

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
    fn test_disabled_row(harness: TestHarness, terminal: TestTerminal) {
        let recipe_id = RecipeId::factory(());
        let rows = [
            (
                "row0".into(),
                "value0".into(),
                RecipeOverrideKey::query_param(recipe_id.clone(), 0),
                TestRowToggleKey {
                    recipe_id: recipe_id.clone(),
                    key: "row0".into(),
                },
            ),
            (
                "row1".into(),
                "value1".into(),
                RecipeOverrideKey::query_param(recipe_id.clone(), 1),
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
            .send_keys([KeyCode::Down, KeyCode::Char(' ')])
            .assert_empty();
        let selected_row = component.select.selected().unwrap();
        assert_eq!(&selected_row.key, "row1");
        assert!(!*selected_row.enabled);
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
        assert!(*selected_row.enabled);
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
                RecipeOverrideKey::query_param(recipe_id.clone(), 0),
                TestRowToggleKey {
                    recipe_id: recipe_id.clone(),
                    key: "row0".into(),
                },
            ),
            (
                "row1".into(),
                "value1".into(),
                RecipeOverrideKey::query_param(recipe_id.clone(), 1),
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
            RecipeOverrideKey::query_param(recipe_id.clone(), 0),
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
            .action("Edit Row")
            .send_keys([KeyCode::Char('!'), KeyCode::Enter])
            .assert_empty();

        let selected_row = component.select.selected().unwrap();
        assert_eq!(selected_row.value.template().display(), "value0!");
    }

    /// Override templates should be loaded from the store on init
    #[rstest]
    fn test_persisted_override(harness: TestHarness, terminal: TestTerminal) {
        let recipe_id = RecipeId::factory(());
        RecipeOverrideStore::store_persisted(
            &RecipeOverrideKey::query_param(recipe_id.clone(), 0),
            &RecipeOverrideValue::Override("p0".into()),
        );
        RecipeOverrideStore::store_persisted(
            &RecipeOverrideKey::query_param(recipe_id.clone(), 1),
            &RecipeOverrideValue::Override("p1".into()),
        );
        let rows = [
            (
                "row0".into(),
                "".into(),
                RecipeOverrideKey::query_param(recipe_id.clone(), 0),
                TestRowToggleKey {
                    recipe_id: recipe_id.clone(),
                    key: "row0".into(),
                },
            ),
            (
                "row1".into(),
                "".into(),
                RecipeOverrideKey::query_param(recipe_id.clone(), 1),
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
