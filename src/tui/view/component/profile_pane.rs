use crate::{
    collection::{Profile, ProfileId},
    tui::view::{
        common::{table::Table, template_preview::TemplatePreview, Pane},
        draw::{Draw, Generate},
        state::StateCell,
    },
};
use itertools::Itertools;
use ratatui::{layout::Rect, Frame};

/// Display the contents of a profile
#[derive(Debug, Default)]
pub struct ProfilePane {
    fields: StateCell<ProfileId, Vec<(String, TemplatePreview)>>,
}

pub struct ProfilePaneProps<'a> {
    pub profile: &'a Profile,
}

impl<'a> Draw<ProfilePaneProps<'a>> for ProfilePane {
    fn draw(&self, frame: &mut Frame, props: ProfilePaneProps<'a>, area: Rect) {
        // Whenever the selected profile changes, rebuild the internal state.
        // This is needed because the template preview rendering is async.
        let fields =
            self.fields.get_or_update(props.profile.id.clone(), || {
                props
                    .profile
                    .data
                    .iter()
                    .map(|(key, template)| {
                        (
                            key.clone(),
                            TemplatePreview::new(
                                template.clone(),
                                Some(props.profile.id.clone()),
                            ),
                        )
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
                .map(|(key, value)| [key.as_str().into(), value.generate()])
                .collect_vec(),
            alternate_row_style: true,
            ..Default::default()
        };
        frame.render_widget(table.generate().block(pane.generate()), area);
    }
}
