//! The built-in theme library (`themes/ghostty/`, embedded by `build.rs`).

include!(concat!(env!("OUT_DIR"), "/builtin.rs"));

pub fn all() -> &'static [(&'static str, &'static str)] {
    BUILTIN
}

pub fn find(name: &str) -> Option<(&'static str, &'static str)> {
    BUILTIN.binary_search_by(|(n, _)| (*n).cmp(name)).ok().map(|i| BUILTIN[i])
}

pub fn find_ignore_case(name: &str) -> Option<(&'static str, &'static str)> {
    let lower = name.to_lowercase();
    BUILTIN.iter().copied().find(|(n, _)| n.to_lowercase() == lower)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_is_embedded_and_sorted() {
        assert!(all().len() > 400, "{}", all().len());
        assert!(all().windows(2).all(|w| w[0].0 < w[1].0));
        let (name, text) = find("Catppuccin Mocha").unwrap();
        assert_eq!(name, "Catppuccin Mocha");
        assert!(text.contains("background"));
        assert!(find("catppuccin mocha").is_none());
        assert_eq!(find_ignore_case("catppuccin MOCHA").unwrap().0, "Catppuccin Mocha");
        assert!(all().iter().all(|(n, _)| !n.starts_with('.') && *n != "LICENSE"));
    }

    #[test]
    fn every_built_in_theme_parses() {
        for (name, text) in all() {
            crate::parse::parse(text).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }
}
