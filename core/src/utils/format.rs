use std::collections::HashMap;

pub fn summarize_fetch_errors(errors: &HashMap<String, String>, max_items: usize) -> String {
    if errors.is_empty() {
        return String::new();
    }

    let mut details: Vec<String> = errors
        .iter()
        .map(|(name, err)| format!("{name}: {err}"))
        .collect();
    details.sort();

    details
        .into_iter()
        .take(max_items.max(1))
        .collect::<Vec<_>>()
        .join(" | ")
}
