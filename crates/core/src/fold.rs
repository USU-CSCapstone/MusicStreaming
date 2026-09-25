//! Identity keys for artists, albums, and tags (`design/database.md` §3).
//!
//! Identity is a folded key; the displayed name is whatever spelling most
//! files use. The fold has to be byte-identical on the server and in
//! WebAssembly, so it uses only `char::to_lowercase`, which is Unicode-aware
//! and locale-free. It is deliberately *not* full Unicode case folding or
//! normalization: those need tables the client should not carry until the
//! offline design asks for them.

/// Fold a name for identity: trim outer whitespace, lowercase every
/// character. Interior spacing is kept, so "Sun Ra" and "Sunra" stay
/// distinct.
pub fn name_key(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.trim().chars() {
        out.extend(ch.to_lowercase());
    }
    out
}

/// Fold a tag for identity: case *and* spacing (`requirements/tags.md` §4),
/// so "Hip Hop", "hip-hop", and "HipHop" are one tag.
pub fn tag_key(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.trim().chars() {
        if ch.is_whitespace() || ch == '-' || ch == '_' {
            continue;
        }
        out.extend(ch.to_lowercase());
    }
    out
}

/// The album-artists half of album identity: each artist's `name_key`,
/// sorted so order does not matter, joined with `;`
/// (`design/database.md` §3).
pub fn artists_key<'a>(names: impl IntoIterator<Item = &'a str>) -> String {
    let mut keys: Vec<String> = names.into_iter().map(name_key).collect();
    keys.sort();
    keys.dedup();
    keys.join(";")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_fold_case_not_spacing() {
        assert_eq!(name_key("  Nirvana "), "nirvana");
        assert_eq!(name_key("SUN RA"), "sun ra");
        assert_ne!(name_key("Sun Ra"), name_key("Sunra"));
        assert_eq!(name_key("Björk"), "björk");
        assert_eq!(name_key("BJÖRK"), "björk");
    }

    #[test]
    fn tags_fold_spacing_too() {
        assert_eq!(tag_key("Hip Hop"), tag_key("hip-hop"));
        assert_eq!(tag_key("Hip Hop"), tag_key("HipHop"));
        assert_ne!(tag_key("Rock"), tag_key("Rocks"));
    }

    #[test]
    fn artists_key_is_order_independent() {
        assert_eq!(
            artists_key(["SZA", "Kendrick Lamar"]),
            artists_key(["kendrick lamar", "sza"])
        );
        assert_eq!(artists_key(["A", "A"]), "a");
        assert_eq!(artists_key(std::iter::empty()), "");
    }
}
