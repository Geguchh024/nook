//! Phosphor icons by name, for plugins.

use super::icon_names::ICONS;

/// The glyph for a Phosphor name (`"cloud-sun"`, `"cloud_sun"` and
/// `"CloudSun"` all work). Unknown names get a question mark.
pub fn glyph(name: &str) -> &'static str {
    lookup(name).unwrap_or(egui_phosphor::regular::QUESTION)
}

pub fn lookup(name: &str) -> Option<&'static str> {
    let mut key = String::with_capacity(name.len() + 4);
    for (i, c) in name.trim().chars().enumerate() {
        match c {
            '_' | ' ' => key.push('-'),
            c if c.is_ascii_uppercase() => {
                if i > 0 && !key.ends_with('-') {
                    key.push('-');
                }
                key.push(c.to_ascii_lowercase());
            }
            c => key.push(c),
        }
    }
    ICONS
        .binary_search_by(|(n, _)| (*n).cmp(key.as_str()))
        .ok()
        .map(|i| ICONS[i].1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_resolve_in_any_case() {
        let sun = egui_phosphor::regular::CLOUD_SUN;
        assert_eq!(lookup("cloud-sun"), Some(sun));
        assert_eq!(lookup("cloud_sun"), Some(sun));
        assert_eq!(lookup("CloudSun"), Some(sun));
        assert_eq!(lookup("no-such-icon"), None);
        assert!(
            ICONS.windows(2).all(|w| w[0].0 < w[1].0),
            "table not sorted"
        );
    }
}
