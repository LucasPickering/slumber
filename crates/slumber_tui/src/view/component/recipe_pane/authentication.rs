use crate::view::{
    common::{table::Table, template_preview::TemplatePreview},
    draw::{Draw, DrawMetadata, Generate},
};
use ratatui::{prelude::Constraint, Frame};
use slumber_core::{
    collection::{Authentication, ProfileId},
    template::Template,
};

/// Display authentication settings for a recipe
#[derive(Debug)]
pub enum AuthenticationDisplay {
    Basic {
        username: TemplatePreview,
        password: Option<TemplatePreview>,
    },
    Bearer(TemplatePreview),
}

impl AuthenticationDisplay {
    pub fn new(
        authentication: &Authentication<Template>,
        selected_profile_id: Option<&ProfileId>,
    ) -> Self {
        match authentication {
            Authentication::Basic { username, password } => {
                AuthenticationDisplay::Basic {
                    username: TemplatePreview::new(
                        username.clone(),
                        selected_profile_id.cloned(),
                        None,
                    ),
                    password: password.clone().map(|password| {
                        TemplatePreview::new(
                            password,
                            selected_profile_id.cloned(),
                            None,
                        )
                    }),
                }
            }
            Authentication::Bearer(token) => {
                AuthenticationDisplay::Bearer(TemplatePreview::new(
                    token.clone(),
                    selected_profile_id.cloned(),
                    None,
                ))
            }
        }
    }
}

impl Draw for AuthenticationDisplay {
    fn draw(&self, frame: &mut Frame, _: (), metadata: DrawMetadata) {
        match self {
            AuthenticationDisplay::Basic { username, password } => {
                let table = Table {
                    rows: vec![
                        ["Type".into(), "Basic".into()],
                        ["Username".into(), username.generate()],
                        [
                            "Password".into(),
                            password
                                .as_ref()
                                .map(Generate::generate)
                                .unwrap_or_default(),
                        ],
                    ],
                    column_widths: &[Constraint::Length(8), Constraint::Min(0)],
                    ..Default::default()
                };
                frame.render_widget(table.generate(), metadata.area())
            }
            AuthenticationDisplay::Bearer(token) => {
                let table = Table {
                    rows: vec![
                        ["Type".into(), "Bearer".into()],
                        ["Token".into(), token.generate()],
                    ],
                    column_widths: &[Constraint::Length(5), Constraint::Min(0)],
                    ..Default::default()
                };
                frame.render_widget(table.generate(), metadata.area())
            }
        }
    }
}
