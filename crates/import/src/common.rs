use indexmap::IndexMap;
use slumber_core::{collection::QueryParameterValue, template::Template};

/// Convert a list of query parameters pairs into a map. Most formats store
/// query parameters in a list where keys can be duplicated. Slumber uses a
/// map format where keys are unique but values can be scalar or a vector. This
/// will group duplicates keys together to form a list of values.
pub fn build_query_parameters(
    parameters: impl IntoIterator<Item = (String, Template)>,
) -> IndexMap<String, QueryParameterValue> {
    // Group by parameter
    let grouped: IndexMap<String, Vec<Template>> = parameters.into_iter().fold(
        IndexMap::default(),
        |mut acc, (name, value)| {
            acc.entry(name).or_default().push(value);
            acc
        },
    );

    // Flatten 1-length values
    grouped
        .into_iter()
        .map(|(param, mut values)| {
            // If a param only has one value, flatten the vec
            let value = if values.len() == 1 {
                QueryParameterValue::One(values.remove(0))
            } else {
                QueryParameterValue::Many(values)
            };
            (param, value)
        })
        .collect()
}
