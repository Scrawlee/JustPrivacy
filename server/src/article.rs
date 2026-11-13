//! Format des articles (en-tête YAML plat + corps Markdown) : lecture de l'en-tête et validation.
//! Les mêmes règles sont appliquées par l'éditeur web et par `build.py` : un article accepté ici
//! ne doit jamais faire échouer la construction du site ni injecter de HTML.
use crate::gitrepo::valid_slug;
use std::collections::BTreeMap;

pub type Meta = BTreeMap<String, String>;

/// Sépare l'en-tête du corps. L'en-tête se termine à la première occurrence de `---`,
/// qui doit être seule sur sa ligne (c'est ce que fait `build.py`).
pub fn parse(content: &str) -> Result<(Meta, &str), &'static str> {
    let rest = content.strip_prefix("---").ok_or("missing_front_matter")?;
    let rest = rest.strip_prefix("\r\n").or_else(|| rest.strip_prefix('\n')).ok_or("bad_front_matter")?;
    let end = match rest.find("---") {
        Some(i) if i == 0 || rest[..i].ends_with('\n') => i,
        Some(_) => return Err("bad_front_matter"),
        None => return Err("unterminated_front_matter"),
    };
    let after = &rest[end + 3..];
    if !(after.is_empty() || after.starts_with('\n') || after.starts_with("\r\n")) {
        return Err("bad_front_matter");
    }
    let mut meta = Meta::new();
    for line in rest[..end].lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') { continue; }
        let (key, value) = line.split_once(':').ok_or("bad_front_matter_line")?;
        let key = key.trim();
        let valid_key = !key.is_empty()
            && key.bytes().next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
            && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
        if !valid_key { return Err("bad_front_matter_line"); }
        let value = value.trim();
        if value.chars().count() > 600 { return Err("field_too_long"); }
        meta.insert(key.to_string(), value.to_string());
    }
    Ok((meta, after))
}

pub fn lang_of(meta: &Meta) -> &str {
    meta.get("lang").map(String::as_str).unwrap_or("both")
}

fn safe_url(url: &str) -> bool {
    let u = url.trim().to_ascii_lowercase();
    (u.starts_with("https://") || u.starts_with("http://") || u.starts_with("mailto:") || u.starts_with('#')
        || (u.starts_with('/') && !u.starts_with("//")))
        && u.len() > 1
}

/// Lignes `::: fr` / `::: en` du corps (mode bilingue).
fn lang_markers(body: &str) -> (bool, bool) {
    let (mut fr, mut en) = (false, false);
    for line in body.lines() {
        if let Some(rest) = line.trim_end().strip_prefix(":::") {
            match rest.trim() { "fr" => fr = true, "en" => en = true, _ => {} }
        }
    }
    (fr, en)
}

pub fn validate(slug: &str, content: &str) -> Result<(), &'static str> {
    if content.contains('\0') { return Err("invalid_characters"); }
    // Jamais de HTML brut : le Markdown est le seul format.
    let b = content.as_bytes();
    for i in 0..b.len() {
        if b[i] == b'<' && b.get(i + 1).is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, b'/' | b'!' | b'?')) {
            return Err("html_not_allowed");
        }
    }
    let (meta, body) = parse(content)?;
    match meta.get("slug") {
        Some(s) if s == slug => {}
        _ => return Err("slug_mismatch"),
    }
    if let Some(o) = meta.get("order") {
        match o.parse::<u32>() { Ok(n) if n <= 9999 => {}, _ => return Err("invalid_order") }
    }
    let lang = lang_of(&meta);
    if !matches!(lang, "both" | "fr" | "en") { return Err("invalid_lang"); }
    if matches!(lang, "both" | "fr") && meta.get("title_fr").is_none_or(|t| t.is_empty()) { return Err("missing_title_fr"); }
    if matches!(lang, "both" | "en") && meta.get("title_en").is_none_or(|t| t.is_empty()) { return Err("missing_title_en"); }
    if let Some(see) = meta.get("see_also") {
        if see.split(',').map(str::trim).filter(|s| !s.is_empty()).any(|s| !valid_slug(s)) { return Err("invalid_see_also"); }
    }
    if body.trim().is_empty() { return Err("empty_body"); }
    if lang == "both" {
        let (fr, en) = lang_markers(body);
        if !fr || !en { return Err("missing_language_block"); }
    }
    // Liens Markdown : http(s), mailto, chemins du site et ancres seulement.
    let mut rest = content;
    while let Some(i) = rest.find("](") {
        let after = &rest[i + 2..];
        let url = after.split(')').next().unwrap_or("");
        if !safe_url(url) { return Err("unsafe_link"); }
        rest = after;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "---\norder: 2\nslug: demo\nlang: both\ntitle_fr: Démo\ntitle_en: Demo\nsee_also: acces, permissions\n---\n\n::: fr\n## Titre\n\nTexte [lien](https://example.org).\n:::\n\n::: en\n## Title\n\nText.\n:::\n";

    #[test]
    fn accepts_a_valid_article() {
        assert_eq!(validate("demo", GOOD), Ok(()));
        let (meta, body) = parse(GOOD).unwrap();
        assert_eq!(meta["title_fr"], "Démo");
        assert!(body.contains("::: fr"));
    }

    #[test]
    fn single_language_article() {
        let a = "---\nslug: solo\nlang: en\ntitle_en: Solo\n---\nHello.\n";
        assert_eq!(validate("solo", a), Ok(()));
        assert_eq!(validate("solo", &a.replace("title_en: Solo\n", "")), Err("missing_title_en"));
    }

    #[test]
    fn rejects_bad_front_matter() {
        assert_eq!(validate("demo", "pas d'en-tête"), Err("missing_front_matter"));
        assert_eq!(validate("demo", "---\nslug: demo\ntitle_fr: x\n"), Err("unterminated_front_matter"));
        assert_eq!(validate("demo", "---\nslug: demo\ntitle_fr: a --- b\n---\ncorps"), Err("bad_front_matter"));
        assert_eq!(validate("demo", "---\nce n'est pas une paire\n---\ncorps"), Err("bad_front_matter_line"));
        assert_eq!(validate("autre", GOOD), Err("slug_mismatch"));
        assert_eq!(validate("demo", &GOOD.replace("order: 2", "order: deux")), Err("invalid_order"));
        assert_eq!(validate("demo", &GOOD.replace("lang: both", "lang: de")), Err("invalid_lang"));
        assert_eq!(validate("demo", &GOOD.replace("acces, permissions", "../x")), Err("invalid_see_also"));
    }

    #[test]
    fn rejects_html_and_unsafe_links() {
        assert_eq!(validate("demo", &GOOD.replace("Texte", "<script>x</script>")), Err("html_not_allowed"));
        assert_eq!(validate("demo", &GOOD.replace("Texte", "a <b>gras</b>")), Err("html_not_allowed"));
        assert_eq!(validate("demo", &GOOD.replace("Texte", "5 < 6 et 7 > 3")), Ok(()));
        assert_eq!(validate("demo", &GOOD.replace("https://example.org", "javascript:alert(1)")), Err("unsafe_link"));
        assert_eq!(validate("demo", &GOOD.replace("https://example.org", "//evil.example")), Err("unsafe_link"));
        assert_eq!(validate("demo", &GOOD.replace("https://example.org", "/articles/acces")), Ok(()));
    }

    #[test]
    fn bilingual_needs_both_blocks() {
        let one = GOOD.replace("::: en", "::: xx");
        assert_eq!(validate("demo", &one), Err("missing_language_block"));
        assert_eq!(validate("demo", "---\nslug: demo\nlang: fr\ntitle_fr: x\n---\n\n"), Err("empty_body"));
    }
}
