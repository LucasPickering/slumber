use super::{Canvas, DrawMetadata};
use crate::{
    context::TuiContext,
    view::{
        Generate,
        common::Pane,
        component::{Component, ComponentId, Draw},
    },
};
use slumber_config::Action;

/// TODO
///
/// This needs to be a component instead of just a widget because it's
/// clickable.
#[derive(Debug)]
pub struct PrimaryHeader {
    id: ComponentId,
    title: String,
}

impl PrimaryHeader {
    /// TODO
    pub fn new(title: &str, action: Action) -> Self {
        let title = TuiContext::get().input_engine.add_hint(title, action);
        Self {
            id: ComponentId::default(),
            title,
        }
    }
}

impl Component for PrimaryHeader {
    fn id(&self) -> ComponentId {
        self.id
    }
}

impl Draw<PrimaryHeaderProps<'_>> for PrimaryHeader {
    fn draw(
        &self,
        canvas: &mut Canvas,
        props: PrimaryHeaderProps<'_>,
        metadata: DrawMetadata,
    ) {
        let block = Pane {
            title: &self.title,
            has_focus: false,
        }
        .generate();
        let inner_area = block.inner(metadata.area());
        canvas.render_widget(block, metadata.area());
        let value = props.value.unwrap_or("None");
        canvas.render_widget(value, inner_area);
    }
}

/// Draw props for [PrimaryHeader]
pub struct PrimaryHeaderProps<'a> {
    /// Value to display in the header, e.g. profile or recipe name
    pub value: Option<&'a str>,
}
