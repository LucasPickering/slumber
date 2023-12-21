use crate::{
    collection::{Profile, ProfileId, ProfileValue},
    tui::view::{
        common::{table::Table, template_preview::TemplatePreview, Pane},
        draw::{Draw, DrawContext, Generate},
        state::StateCell,
    },
};
use itertools::Itertools;
use ratatui::layout::Rect;

/// Display the contents of a profile
#[derive(Debug, Default)]
pub struct ProfilePane {
    fields: StateCell<ProfileId, Vec<(String, FieldValue)>>,
}

pub struct ProfilePaneProps<'a> {
    pub profile: &'a Profile,
}

#[derive(Debug)]
enum FieldValue {
    Raw(String),
    Template(TemplatePreview),
}

impl<'a> Draw<ProfilePaneProps<'a>> for ProfilePane {
    fn draw(
        &self,
        context: &mut DrawContext,
        props: ProfilePaneProps<'a>,
        area: Rect,
    ) {
        // Whenever the selected profile changes, rebuild the internal state.
        // This is needed because the template preview rendering is async.
        let fields =
            self.fields.get_or_update(props.profile.id.clone(), || {
                props
                    .profile
                    .data
                    .iter()
                    .map(|(key, value)| {
                        let value = match value {
                            ProfileValue::Raw(value) => {
                                FieldValue::Raw(value.clone())
                            }
                            ProfileValue::Template(template) => {
                                FieldValue::Template(TemplatePreview::new(
                                    template.clone(),
                                    Some(props.profile.id.clone()),
                                    context.config.preview_templates,
                                ))
                            }
                        };
                        (key.clone(), value)
                    })
                    .collect_vec()
            });

        let pane = Pane {
            title: "Profile",
            is_focused: false,
        };
        let table = Table {
            header: Some(["Field", "Value"]),
            rows: fields
                .iter()
                .map(|(key, value)| {
                    let value = match value {
                        FieldValue::Raw(value) => value.as_str().into(),
                        FieldValue::Template(preview) => preview.generate(),
                    };
                    [key.as_str().into(), value]
                })
                .collect_vec(),
            alternate_row_style: true,
            ..Default::default()
        };
        context
            .frame
            .render_widget(table.generate().block(pane.generate()), area);
    }
}
