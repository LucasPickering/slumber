use crate::view::{
    Component,
    common::{fixed_select::FixedSelect, tabs::Tabs},
    component::{
        Canvas, ComponentId, Draw, DrawMetadata,
        internal::{Child, ToChild},
        recipe::{
            authentication::AuthenticationDisplay,
            body::RecipeBodyDisplay,
            table::{RecipeTable, RecipeTableKind, RecipeTableProps},
            url::UrlDisplay,
        },
    },
    persistent::PersistentKey,
};
use derive_more::Display;
use ratatui::{layout::Layout, prelude::Constraint, widgets::Paragraph};
use serde::{Deserialize, Serialize};
use slumber_core::{
    collection::Recipe,
    http::{BuildOptions, HttpMethod},
};
use std::iter;
use strum::{EnumCount, EnumIter};

/// Display a recipe. Not a recipe *node*, this is for genuine bonafide recipe.
/// This maintains internal state specific to a recipe, so it should be
/// recreated every time the recipe/profile changes.
#[derive(Debug)]
pub struct RecipeDisplay {
    id: ComponentId,
    tabs: Tabs<RecipeTabKey, Tab>,
    method: HttpMethod,
    url: UrlDisplay,
    query: RecipeTable<QueryTableKind>,
    headers: RecipeTable<HeaderTableKind>,
    body: Option<RecipeBodyDisplay>,
    authentication: Option<AuthenticationDisplay>,
}

impl RecipeDisplay {
    /// Initialize new recipe state. Should be called whenever the recipe or
    /// profile changes
    pub fn new(recipe: &Recipe) -> Self {
        // Disable tabs that have no content
        let disabled_tabs = iter::empty()
            .chain(recipe.body.is_none().then_some(Tab::Body))
            .chain(
                recipe
                    .authentication
                    .is_none()
                    .then_some(Tab::Authentication),
            );
        let tabs = Tabs::new(
            RecipeTabKey,
            FixedSelect::builder().disabled(disabled_tabs),
        );

        Self {
            id: ComponentId::default(),
            tabs,
            method: recipe.method,
            url: UrlDisplay::new(recipe.id.clone(), recipe.url.clone()),
            query: RecipeTable::new(
                "Parameter",
                recipe.id.clone(),
                recipe.query_iter().map(|(param, index, value)| {
                    ((param.to_owned(), index), value.clone())
                }),
                false,
            ),
            headers: RecipeTable::new(
                "Header",
                recipe.id.clone(),
                recipe
                    .headers
                    .iter()
                    .map(|(header, value)| (header.clone(), value.clone())),
                false,
            ),
            body: recipe
                .body
                .as_ref()
                .map(|body| RecipeBodyDisplay::new(body, recipe)),
            // Map authentication type
            authentication: recipe.authentication.as_ref().map(
                |authentication| {
                    AuthenticationDisplay::new(
                        recipe.id.clone(),
                        authentication.clone(),
                    )
                },
            ),
        }
    }

    /// Generate a [BuildOptions] instance based on current UI state
    pub fn build_options(&self) -> BuildOptions {
        let url = self.url.override_value();
        let authentication = self.authentication.as_ref().and_then(
            super::authentication::AuthenticationDisplay::override_value,
        );
        let form_fields = self
            .body
            .as_ref()
            .and_then(|body| match body {
                RecipeBodyDisplay::Raw(_) | RecipeBodyDisplay::Json(_) => None,
                RecipeBodyDisplay::Form(form) => {
                    Some(form.to_build_overrides())
                }
            })
            .unwrap_or_default();
        let body = self
            .body
            .as_ref()
            .and_then(RecipeBodyDisplay::override_value);

        BuildOptions {
            url,
            authentication,
            headers: self.headers.to_build_overrides(),
            query_parameters: self.query.to_build_overrides(),
            form_fields,
            body,
        }
    }
}

impl Component for RecipeDisplay {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn children(&mut self) -> Vec<Child<'_>> {
        vec![
            self.url.to_child_mut(),
            self.body.to_child_mut(),
            self.query.to_child_mut(),
            self.headers.to_child_mut(),
            self.authentication.to_child_mut(),
            // Tabs last so edit text boxes can use left/right if needed
            self.tabs.to_child_mut(),
        ]
    }
}

impl Draw for RecipeDisplay {
    fn draw(&self, canvas: &mut Canvas, (): (), metadata: DrawMetadata) {
        // Render request contents
        let method = self.method.to_string();

        let [metadata_area, tabs_area, content_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(metadata.area());

        let [method_area, url_area] = Layout::horizontal(
            // Method gets just as much as it needs, URL gets the rest
            [Constraint::Max(method.len() as u16 + 1), Constraint::Min(0)],
        )
        .areas(metadata_area);

        // First line: Method + URL
        canvas.render_widget(Paragraph::new(method), method_area);
        canvas.render_widget(self.url.preview(), url_area);

        // Navigation tabs
        canvas.draw(&self.tabs, (), tabs_area, true);

        // Recipe content
        match self.tabs.selected() {
            Tab::Url => canvas.draw(&self.url, (), content_area, true),
            Tab::Body => {
                if let Some(body) = &self.body {
                    canvas.draw(body, (), content_area, true);
                }
            }
            Tab::Query => canvas.draw(
                &self.query,
                RecipeTableProps {
                    key_header: "Parameter",
                    value_header: "Value",
                },
                content_area,
                true,
            ),
            Tab::Headers => canvas.draw(
                &self.headers,
                RecipeTableProps {
                    key_header: "Header",
                    value_header: "Value",
                },
                content_area,
                true,
            ),
            Tab::Authentication => {
                if let Some(authentication) = &self.authentication {
                    canvas.draw(authentication, (), content_area, true);
                }
            }
        }
    }
}

/// Persistence key for selected tab
#[derive(Debug, Default, Serialize)]
struct RecipeTabKey;

impl PersistentKey for RecipeTabKey {
    type Value = Tab;
}

#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    Display,
    EnumCount,
    EnumIter,
    PartialEq,
    Serialize,
    Deserialize,
)]
enum Tab {
    #[default]
    #[display("URL")]
    Url,
    Body,
    Query,
    Headers,
    Authentication,
}

/// [RecipeTableKind] for the query parameter table
#[derive(Debug)]
struct QueryTableKind;

impl RecipeTableKind for QueryTableKind {
    /// Query parameters can be repeated, so the parameter name alone isn't
    /// unique. The index makes each key unique. These are pulled directly from
    /// [Recipe::query_iter].
    type Key = (String, usize);

    fn key_as_str(key: &Self::Key) -> &str {
        key.0.as_str()
    }
}

/// [RecipeTableKind] for the header table
#[derive(Debug)]
struct HeaderTableKind;

impl RecipeTableKind for HeaderTableKind {
    type Key = String;

    fn key_as_str(key: &Self::Key) -> &str {
        key.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_util::{TestTerminal, terminal},
        view::test_util::{TestComponent, TestHarness, harness},
    };
    use indexmap::{IndexMap, indexmap};
    use rstest::rstest;
    use slumber_core::http::BuildFieldOverride;
    use slumber_util::Factory;
    use terminput::KeyCode;

    /// Override query parameters, including persistence. Query param keys are
    /// not unique on their own, so this ensures the index-based uniqueness is
    /// working correctly.
    #[rstest]
    fn test_override_query(harness: TestHarness, terminal: TestTerminal) {
        let recipe = Recipe {
            query: indexmap! {
                "p0".into() => "v0".into(),
                "p1".into() => ["v0", "v1", "v2"].into(),
            },
            ..Recipe::factory(())
        };
        let mut component = TestComponent::new(
            &harness,
            &terminal,
            RecipeDisplay::new(&recipe),
        );

        // Select query tab
        component
            .int()
            .drain_draw() // Drain initial events
            .send_key(KeyCode::Right)
            .assert()
            .empty();
        assert_eq!(component.tabs.selected(), Tab::Query);

        // Test persistence of both disable and override state, with a mixture
        // of row ordering to make sure higher rows don't overwrite lower ones,
        // or vice versa.
        component
            .int()
            .send_keys([KeyCode::Down, KeyCode::Char(' ')]) // Disable (p1,v0)
            .send_keys([KeyCode::Down, KeyCode::Char('e')]) // Override (p1,v1)
            .send_text("www")
            // Disable+override (p1,v2)
            .send_keys([KeyCode::Down, KeyCode::Char(' '), KeyCode::Char('e')])
            .send_text("xxx")
            .assert()
            .empty();

        let expected = IndexMap::<_, _>::from_iter([
            (("p1".to_owned(), 0), BuildFieldOverride::Omit),
            (("p1".to_owned(), 1), "v1www".into()),
            (("p1".to_owned(), 2), BuildFieldOverride::Omit),
        ]);
        assert_eq!(component.query.to_build_overrides(), expected);

        // Rebuild the component and make sure state was persisted+reloaded
        let component = TestComponent::new(
            &harness,
            &terminal,
            RecipeDisplay::new(&recipe),
        );
        assert_eq!(component.query.to_build_overrides(), expected);
    }
}
