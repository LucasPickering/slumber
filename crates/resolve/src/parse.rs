use crate::{Reference, ReferenceError, ReferencePath, ReferenceSource};

// TODO support root ref /

pub fn parse_reference(input: &str) -> Result<Reference, ReferenceError> {
    if let Some(rest) = input.strip_prefix("#/") {
        let path = ReferencePath(
            rest.split("/").map(String::from).collect::<Vec<_>>().into(),
        );
        Ok(Reference {
            source: ReferenceSource::Local,
            path,
        })
    } else {
        Err(ReferenceError::InvalidReference(input.to_owned()))
    }
}
