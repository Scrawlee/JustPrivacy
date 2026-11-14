//! Rendu des pages du site (port de l'ancien `build.py`) : Markdown → HTML, gabarits, CSS avec valeurs de repli.
//! Tout est calculé en mémoire à partir des gabarits embarqués dans le binaire et des articles lus dans git.
//!
//! Compatibilité : les pages fonctionnent sans JavaScript (la langue se change avec des sélecteurs frères,
//! voir style.css). Les déclarations qui utilisent `var()` reçoivent une valeur littérale de repli, pour les
//! navigateurs sans propriétés personnalisées.
use base64::Engine;
use std::{collections::BTreeMap, sync::LazyLock};

macro_rules! template {
    ($name:literal) => { include_str!(concat!("../../templates/", $name)) };
}

const HOME: &str = template!("home.html");
const ARTICLES: &str = template!("articles.html");
const ARTICLE: &str = template!("article.html");
const BENCHMARK: &str = template!("benchmark.html");
const ABOUT: &str = template!("about.html");
const LEGAL: &str = template!("legal.html");
const PRIVACY: &str = template!("privacy.html");
const LOGIN: &str = template!("connexion.html");
const SIGNUP: &str = template!("inscription.html");
const ACCOUNT: &str = template!("compte.html");
const EDITOR: &str = template!("editeur.html");
const COMPAT: &str = template!("compat.html");
const NOT_FOUND: &str = template!("notfound.html");
const STYLE: &str = template!("style.css");
const JS_SETTINGS: &str = template!("reglages.js");
const JS_ACCOUNT: &str = template!("compte.js");
const JS_EDITOR: &str = template!("editeur.js");
const FONT_LATIN: &[u8] = include_bytes!("../../fonts/inter-latin.woff2");
const FONT_LATIN_EXT: &[u8] = include_bytes!("../../fonts/inter-latin-ext.woff2");

type Vars = BTreeMap<&'static str, String>;

// ---------------------------------------------------------------------------- utilitaires de texte

/// Supprime les blocs `/* … */`.
fn strip_block_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        match rest[i + 2..].find("*/") {
            Some(j) => rest = &rest[i + 2 + j + 2..],
            None => { rest = ""; break; }
        }
    }
    out.push_str(rest);
    out
}

fn esc_text(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Échappe aussi les guillemets doubles (valeurs utilisées dans des attributs) ; l'apostrophe reste telle quelle.
fn esc_attr(s: &str) -> String {
    esc_text(s).replace('"', "&quot;")
}

// ---------------------------------------------------------------------------- CSS : valeurs de repli pour var()

/// `var(--nom)` : (début, fin, nom) de chaque occurrence.
fn find_vars(value: &str) -> Vec<(usize, usize, &str)> {
    let mut out = vec![];
    let mut from = 0;
    while let Some(i) = value[from..].find("var(--") {
        let start = from + i;
        let name_start = start + 4;
        let rest = &value[name_start..];
        let len = rest.bytes().take_while(|b| b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-').count();
        if len > 2 && rest[len..].starts_with(')') {
            out.push((start, name_start + len + 1, &value[name_start..name_start + len]));
            from = name_start + len + 1;
        } else {
            from = start + 4;
        }
    }
    out
}

/// `a:var(--x);b:2` → `a:<littéral>;a:var(--x);b:2` (seulement quand toutes les variables sont connues).
fn decl_fallbacks(decls: &str, values: &BTreeMap<String, String>) -> String {
    let mut out: Vec<String> = vec![];
    for decl in decls.split(';') {
        if decl.trim().is_empty() { continue; }
        let (name, value) = decl.split_once(':').unwrap_or((decl, ""));
        if !name.trim().starts_with("--") && value.contains("var(") {
            let vars = find_vars(value);
            if !vars.is_empty() && vars.iter().all(|(_, _, n)| values.contains_key(*n)) {
                let mut literal = String::new();
                let mut last = 0;
                for (s, e, n) in &vars {
                    literal.push_str(&value[last..*s]);
                    literal.push_str(&values[*n]);
                    last = *e;
                }
                literal.push_str(&value[last..]);
                out.push(format!("{name}:{literal}"));
            }
        }
        out.push(decl.to_string());
    }
    out.join(";")
}

/// Valeurs statiques (thème clair, sans `clamp`) du premier bloc `:root{…}` au début d'une ligne.
fn static_vars(css: &str) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    let start = css.match_indices(":root{").map(|(i, _)| i).find(|&i| i == 0 || css.as_bytes()[i - 1] == b'\n');
    if let Some(i) = start {
        let body_start = i + ":root{".len();
        if let Some(end) = css[body_start..].find('}') {
            for decl in css[body_start..body_start + end].split(';') {
                if let Some((k, v)) = decl.split_once(':') {
                    values.insert(k.trim().to_string(), v.trim().to_string());
                }
            }
        }
    }
    values
}

/// Applique `decl_fallbacks` à chaque bloc `sélecteur{déclarations}` sans accolade imbriquée.
fn add_css_fallbacks(css: &str, values: &BTreeMap<String, String>) -> String {
    let b = css.as_bytes();
    let mut out = String::with_capacity(css.len() + css.len() / 8);
    let (mut copied, mut last_brace) = (0usize, None::<usize>);
    let mut p = 0;
    while p < b.len() {
        match b[p] {
            b'}' => { last_brace = Some(p); p += 1; }
            b'{' => {
                let sel_start = last_brace.map_or(0, |i| i + 1);
                let next = css[p + 1..].find(['{', '}']).map(|i| p + 1 + i);
                match next {
                    Some(q) if b[q] == b'}' && sel_start < p => {
                        let selector = &css[sel_start..p];
                        out.push_str(&css[copied..sel_start]);
                        let t = selector.trim_start();
                        if t.starts_with(":root") || t.starts_with("@font-face") {
                            out.push_str(&css[sel_start..=q]);
                        } else {
                            out.push_str(selector);
                            out.push('{');
                            out.push_str(&decl_fallbacks(&css[p + 1..q], values));
                            out.push('}');
                        }
                        copied = q + 1;
                        last_brace = Some(q);
                        p = q + 1;
                    }
                    _ => { last_brace = Some(p); p += 1; }
                }
            }
            _ => p += 1,
        }
    }
    out.push_str(&css[copied..]);
    out
}

/// Attributs `style="…var(…)…"` : même traitement que pour la feuille de style.
fn add_inline_fallbacks(html: &str, values: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find("style=\"") {
        let content_start = i + 7;
        let Some(len) = rest[content_start..].find('"') else { break };
        let content = &rest[content_start..content_start + len];
        out.push_str(&rest[..content_start]);
        if content.contains("var(") { out.push_str(&decl_fallbacks(content, values)); } else { out.push_str(content); }
        out.push('"');
        rest = &rest[content_start + len + 1..];
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------- ressources (calculées une seule fois)

struct Assets {
    css: String,
    vars: BTreeMap<String, String>,
    js: String,
    js_account: String,
    js_editor: String,
}

static ASSETS: LazyLock<Assets> = LazyLock::new(|| {
    let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    let font_css = format!(
        "@font-face{{font-family:\"Inter\";font-style:normal;font-weight:400 800;font-display:swap;\
         src:url(data:font/woff2;base64,{ext}) format(\"woff2\");\
         unicode-range:U+0100-02BA,U+02BD-02C5,U+02C7-02CC,U+02CE-02D7,U+02DD-02FF,\
         U+0304,U+0308,U+0329,U+1D00-1DBF,U+1E00-1E9F,U+1EF2-1EFF,U+2020,U+20A0-20AB,\
         U+20AD-20C4,U+2113,U+2C60-2C7F,U+A720-A7FF}}\n\
         @font-face{{font-family:\"Inter\";font-style:normal;font-weight:400 800;font-display:swap;\
         src:url(data:font/woff2;base64,{latin}) format(\"woff2\");\
         unicode-range:U+0000-00FF,U+0131,U+0152-0153,U+02BB-02BC,U+02C6,U+02DA,U+02DC,\
         U+0304,U+0308,U+0329,U+2000-206F,U+20AC,U+2122,U+2191,U+2193,U+2212,U+2215,\
         U+FEFF,U+FFFD}}\n",
        ext = b64(FONT_LATIN_EXT),
        latin = b64(FONT_LATIN),
    );
    let raw = strip_block_comments(STYLE);
    let vars = static_vars(&raw);
    let css = font_css + &add_css_fallbacks(&raw, &vars);
    let js = |s: &str| strip_block_comments(s).trim().to_string();
    Assets { css, vars, js: js(JS_SETTINGS), js_account: js(JS_ACCOUNT), js_editor: js(JS_EDITOR) }
});

// ---------------------------------------------------------------------------- gabarits

/// Remplace `<!-- [dev-css] -->…<!-- [/dev-css] -->` et les blocs `[dev-start:NOM]…[dev-end:NOM]` par des marqueurs.
fn strip_dev_blocks(template: &str) -> String {
    let mut page = template.to_string();
    if let (Some(a), Some(end_tag)) = (page.find("<!-- [dev-css] -->"), page.find("<!-- [/dev-css] -->")) {
        if end_tag > a {
            page.replace_range(a..end_tag + "<!-- [/dev-css] -->".len(), "<style>{{CSS}}</style>");
        }
    }
    let mut out = String::with_capacity(page.len());
    let mut rest = page.as_str();
    while let Some(i) = rest.find("<!-- [dev-start:") {
        let name_start = i + "<!-- [dev-start:".len();
        let Some(name_len) = rest[name_start..].find("] -->") else { break };
        let name = &rest[name_start..name_start + name_len];
        let end_tag = format!("<!-- [dev-end:{name}] -->");
        let after_start = name_start + name_len + "] -->".len();
        let Some(end) = rest[after_start..].find(&end_tag) else { break };
        out.push_str(&rest[..i]);
        out.push_str(&format!("{{{{{name}}}}}"));
        rest = &rest[after_start + end + end_tag.len()..];
    }
    out.push_str(rest);
    out
}

/// Remplace chaque `{{NOM}}` en un seul passage : les valeurs insérées ne sont jamais réinterprétées.
fn fill(template: &str, vars: &Vars) -> String {
    let mut out = String::with_capacity(template.len() + 4096);
    let mut rest = template;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let name_len = after.bytes().take_while(|b| b.is_ascii_uppercase() || *b == b'_').count();
        if name_len > 0 && after[name_len..].starts_with("}}") {
            let name = &after[..name_len];
            match vars.get(name) {
                Some(v) => out.push_str(v),
                None if name.starts_with("NAV_") => {}
                None => out.push_str(&rest[i..i + 2 + name_len + 2]),
            }
            rest = &after[name_len + 2..];
        } else {
            out.push_str("{{");
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// Retire les commentaires HTML (sauf `<!--[if …`), les espaces de début et de fin de ligne et les lignes vides.
fn minify_html(html: &str) -> String {
    let mut stripped = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(i) = rest.find("<!--") {
        let after = &rest[i + 4..];
        let conditional = after.strip_prefix("[if").is_some_and(|r| !r.starts_with(|c: char| c.is_alphanumeric() || c == '_'));
        match after.find("-->") {
            Some(end) if !conditional => {
                stripped.push_str(&rest[..i]);
                rest = &after[end + 3..];
            }
            _ => {
                stripped.push_str(&rest[..i + 4]);
                rest = after;
            }
        }
    }
    stripped.push_str(rest);
    stripped.split('\n').map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

fn render_template(template: &str, mut vars: Vars) -> String {
    vars.insert("CSS", ASSETS.css.clone());
    vars.insert("JS", ASSETS.js.clone());
    let page = fill(&strip_dev_blocks(template), &vars);
    minify_html(&add_inline_fallbacks(&page, &ASSETS.vars))
}

// ---------------------------------------------------------------------------- Markdown (sous-ensemble)

const ACCENTS: [(char, char); 16] = [
    ('à', 'a'), ('â', 'a'), ('ä', 'a'), ('é', 'e'), ('è', 'e'), ('ê', 'e'), ('ë', 'e'), ('î', 'i'),
    ('ï', 'i'), ('ô', 'o'), ('ö', 'o'), ('ù', 'u'), ('û', 'u'), ('ü', 'u'), ('ç', 'c'), ('ñ', 'n'),
];

pub fn slugify(text: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in text.to_lowercase().chars() {
        let c = ACCENTS.iter().find(|(from, _)| *from == c).map_or(c, |(_, to)| *to);
        if matches!(c, '’' | '‘' | '\'' | '«' | '»' | '“' | '”' | '"') { continue; }
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if dash && !out.is_empty() { out.push('-'); }
            dash = false;
            out.push(c);
        } else {
            dash = true;
        }
    }
    out
}

/// Seuls http(s), mailto, les chemins du site et les ancres sont acceptés ; le reste devient « # ».
fn safe_href(url: &str) -> String {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    let ok = lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:")
        || lower.starts_with('#') || (lower.starts_with('/') && !lower.starts_with("//"));
    if ok { url.replace('"', "&quot;") } else { "#".to_string() }
}

fn replace_links(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut i = 0;
    while i < t.len() {
        let rest = &t[i..];
        if let Some(inner) = rest.strip_prefix('[') {
            if let Some(close) = inner.find(']').map(|j| j + 1) {
                if close > 1 && rest[close + 1..].starts_with('(') {
                    if let Some(end) = rest[close + 2..].find(')').map(|k| close + 2 + k) {
                        if end > close + 2 {
                            out.push_str(&format!("<a href=\"{}\">{}</a>", safe_href(&rest[close + 2..end]), &rest[1..close]));
                            i += end + 1;
                            continue;
                        }
                    }
                }
            }
        }
        let c = rest.chars().next().unwrap_or(' ');
        out.push(c);
        i += c.len_utf8();
    }
    out
}

fn replace_strong(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut i = 0;
    while i < t.len() {
        let rest = &t[i..];
        if let Some(after) = rest.strip_prefix("**") {
            let run = after.bytes().take_while(|b| *b != b'*').count();
            if run > 0 && after[run..].starts_with("**") {
                out.push_str(&format!("<strong>{}</strong>", &after[..run]));
                i += 2 + run + 2;
                continue;
            }
        }
        let c = rest.chars().next().unwrap_or(' ');
        out.push(c);
        i += c.len_utf8();
    }
    out
}

fn replace_em(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let mut prev_star = false;
    let mut i = 0;
    while i < t.len() {
        let rest = &t[i..];
        if rest.starts_with('*') && !prev_star {
            let after = &rest[1..];
            let run = after.bytes().take_while(|b| *b != b'*').count();
            if run > 0 && after[run..].starts_with('*') && !after[run + 1..].starts_with('*') {
                out.push_str(&format!("<em>{}</em>", &after[..run]));
                i += 1 + run + 1;
                prev_star = false;
                continue;
            }
        }
        let c = rest.chars().next().unwrap_or(' ');
        prev_star = c == '*';
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// Le texte des articles n'est jamais du HTML : tout est échappé avant la mise en forme.
fn inline_md(text: &str) -> String {
    replace_em(&replace_strong(&replace_links(&esc_text(text))))
}

pub fn md_to_html(text: &str) -> String {
    if text.is_empty() { return String::new(); }
    let (mut parts, mut para): (Vec<String>, Vec<&str>) = (vec![], vec![]);
    let (mut in_list, mut in_bq) = (false, false);
    fn flush_para(parts: &mut Vec<String>, para: &mut Vec<&str>) {
        if !para.is_empty() {
            parts.push(format!("<p>{}</p>", inline_md(&para.join(" "))));
            para.clear();
        }
    }
    for line in text.split('\n') {
        let s = line.trim();
        if s.is_empty() { flush_para(&mut parts, &mut para); continue; }
        if let Some(h) = s.strip_prefix("## ") {
            flush_para(&mut parts, &mut para);
            if in_list { parts.push("</ul>".into()); in_list = false; }
            if in_bq { parts.push("</blockquote>".into()); in_bq = false; }
            parts.push(format!("<h2 id=\"{}\">{}</h2>", slugify(h), inline_md(h)));
            continue;
        }
        if let Some(item) = s.strip_prefix("- ").or_else(|| s.strip_prefix("* ")) {
            flush_para(&mut parts, &mut para);
            if in_bq { parts.push("</blockquote>".into()); in_bq = false; }
            if !in_list { parts.push("<ul>".into()); in_list = true; }
            parts.push(format!("<li>{}</li>", inline_md(item)));
            continue;
        }
        if let Some(q) = s.strip_prefix("> ") {
            flush_para(&mut parts, &mut para);
            if in_list { parts.push("</ul>".into()); in_list = false; }
            if !in_bq { parts.push("<blockquote class=\"modele\">".into()); in_bq = true; }
            parts.push(format!("<p>{}</p>", inline_md(q)));
            continue;
        }
        if in_list { parts.push("</ul>".into()); in_list = false; }
        if in_bq { parts.push("</blockquote>".into()); in_bq = false; }
        para.push(s);
    }
    flush_para(&mut parts, &mut para);
    if in_list { parts.push("</ul>".into()); }
    if in_bq { parts.push("</blockquote>".into()); }
    parts.join("\n")
}

fn extract_toc(md: &str) -> String {
    md.lines()
        .filter_map(|l| l.strip_prefix("## ").filter(|h| !h.is_empty()))
        .map(|h| format!("<li><a href=\"#{}\">{}</a></li>", slugify(h), esc_text(h)))
        .collect()
}

// ---------------------------------------------------------------------------- articles

pub struct Art {
    pub slug: String,
    meta: BTreeMap<String, String>,
    lang: String,
    order: i64,
    body_fr: String,
    body_en: String,
    toc_fr: String,
    toc_en: String,
}

impl Art {
    fn m(&self, key: &str) -> &str { self.meta.get(key).map_or("", String::as_str) }
}

/// Même lecture que l'ancien `build.py` : tolérante (un article poussé en git peut ne pas passer la validation de l'éditeur).
pub fn parse_art(slug: &str, content: &str) -> Art {
    let mut meta = BTreeMap::new();
    let mut body = content;
    if let Some(rest) = content.strip_prefix("---") {
        if let Some(end) = rest.find("---") {
            for line in rest[..end].lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') { continue; }
                let Some((key, value)) = line.split_once(':') else { continue };
                let key = key.trim_end();
                let valid = key.bytes().next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
                    && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
                if valid { meta.insert(key.to_string(), value.trim().to_string()); }
            }
            body = &rest[end + 3..];
        }
    }
    // Les champs texte sont insérés tels quels dans les pages : on les échappe ici.
    for (k, v) in meta.iter_mut() {
        if !matches!(k.as_str(), "slug" | "order" | "lang" | "see_also") { *v = esc_attr(v); }
    }
    let lang = meta.get("lang").cloned().unwrap_or_else(|| "both".into());
    let (mut fr_md, mut en_md) = (String::new(), String::new());
    match lang.as_str() {
        "both" => {
            let (mut cur, mut buf): (Option<&str>, Vec<&str>) = (None, vec![]);
            let mut close = |cur: &mut Option<&str>, buf: &mut Vec<&str>| {
                match cur.take() {
                    Some("fr") => fr_md = buf.join("\n").trim().to_string(),
                    Some("en") => en_md = buf.join("\n").trim().to_string(),
                    _ => {}
                }
                buf.clear();
            };
            for line in body.split('\n') {
                let marker = line.trim_end().strip_prefix(":::").map(str::trim).filter(|l| matches!(*l, "fr" | "en"));
                if let Some(m) = marker {
                    close(&mut cur, &mut buf);
                    cur = Some(if m == "fr" { "fr" } else { "en" });
                } else if cur.is_some() && line.starts_with(":::") {
                    close(&mut cur, &mut buf);
                } else if cur.is_some() {
                    buf.push(line);
                }
            }
            close(&mut cur, &mut buf);
        }
        "fr" => fr_md = body.trim().to_string(),
        "en" => en_md = body.trim().to_string(),
        _ => {}
    }
    Art {
        slug: slug.to_string(),
        order: meta.get("order").and_then(|o| o.parse().ok()).unwrap_or(999),
        body_fr: md_to_html(&fr_md),
        body_en: md_to_html(&en_md),
        toc_fr: extract_toc(&fr_md),
        toc_en: extract_toc(&en_md),
        meta,
        lang,
    }
}

pub struct ArticleSet {
    pub list: Vec<Art>,
}

impl ArticleSet {
    /// `raw` : (slug, texte) par ordre alphabétique ; tri stable par champ `order`.
    pub fn new(raw: &[(String, String)]) -> ArticleSet {
        let mut list: Vec<Art> = raw.iter().map(|(slug, text)| parse_art(slug, text)).collect();
        list.sort_by_key(|a| a.order);
        ArticleSet { list }
    }
    pub fn get(&self, slug: &str) -> Option<&Art> { self.list.iter().find(|a| a.slug == slug) }
}

fn bi(fr: &str, en: &str) -> String {
    format!("<span class=\"fr\" lang=\"fr\">{fr}</span><span class=\"en\" lang=\"en\">{en}</span>")
}

fn article_title(a: &Art) -> String {
    match a.lang.as_str() {
        "both" => bi(a.m("title_fr"), a.m("title_en")),
        "fr" => a.m("title_fr").to_string(),
        _ => a.m("title_en").to_string(),
    }
}

fn see_also_block(a: &Art, set: &ArticleSet, display_lang: &str, tags: &mut Vec<String>) -> String {
    let see = a.m("see_also").trim();
    if see.is_empty() { return String::new(); }
    let mut links = String::new();
    for s in see.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        tags.push(format!("article:{s}"));
        let Some(r) = set.get(s) else { continue };
        let (label, prefix) = if display_lang == "fr" {
            (if r.m("title_fr").is_empty() { r.m("title_en") } else { r.m("title_fr") }, "Voir aussi\u{a0}: ")
        } else {
            (if r.m("title_en").is_empty() { r.m("title_fr") } else { r.m("title_en") }, "See also: ")
        };
        links.push_str(&format!("<p><a href=\"/articles/{s}\">{prefix}{label}</a></p>"));
    }
    let back = "<p><a href=\"/articles\"><span aria-hidden=\"true\">←</span> <span class=\"fr\" lang=\"fr\">Tous les articles</span><span class=\"en\" lang=\"en\">All articles</span></a></p>";
    format!("<div class=\"voir\">{links}{back}</div>")
}

fn article_body(a: &Art, set: &ArticleSet, tags: &mut Vec<String>) -> String {
    let mut parts: Vec<String> = vec![];
    let mut version = |open: String, chapeau: &str, toc: &str, toc_label: (&str, &str), body: &str, lang: &str, tags: &mut Vec<String>| {
        parts.push(open);
        parts.push(format!("        <p class=\"chapeau\" style=\"margin-bottom:var(--e4)\">{chapeau}</p>"));
        if !toc.is_empty() {
            parts.push(format!("        <nav class=\"sommaire\" aria-label=\"{}\"><h2>{}</h2>", toc_label.0, toc_label.1));
            parts.push(format!("          <ul style=\"list-style:none;padding:0;margin:0\">{toc}</ul></nav>"));
        }
        parts.push(body.to_string());
        parts.push(see_also_block(a, set, lang, tags));
        parts.push("      </div>".into());
    };
    match a.lang.as_str() {
        "both" => {
            version("      <div class=\"lecture corps fr\" lang=\"fr\">".into(), a.m("chapeau_fr"), &a.toc_fr, ("Sommaire", "Sommaire"), &a.body_fr, "fr", tags);
            version("      <div class=\"lecture corps en\" lang=\"en\">".into(), a.m("chapeau_en"), &a.toc_en, ("Contents", "Contents"), &a.body_en, "en", tags);
        }
        "fr" => version("      <div class=\"lecture corps\">".into(), a.m("chapeau_fr"), &a.toc_fr, ("Sommaire", "Sommaire"), &a.body_fr, "fr", tags),
        "en" => version("      <div class=\"lecture corps\" lang=\"en\">".into(), a.m("chapeau_en"), &a.toc_en, ("Contents", "Contents"), &a.body_en, "en", tags),
        _ => {}
    }
    parts.join("\n")
}

fn lang_class(lang: &str) -> &'static str {
    match lang { "fr" => " fr-seul", "en" => " en-seul", _ => "" }
}

fn home_preview(set: &ArticleSet) -> String {
    let mut items = vec![];
    for a in &set.list {
        let mut cat = bi(a.m("category_fr"), a.m("category_en"));
        let (title, desc) = match a.lang.as_str() {
            "both" => (
                format!("<a href=\"/articles/{}\"><span class=\"fr\" lang=\"fr\">{}</span><span class=\"en\" lang=\"en\">{}</span></a>", a.slug, a.m("title_fr"), a.m("title_en")),
                bi(a.m("description_fr"), a.m("description_en")),
            ),
            "fr" => { cat.push_str("<span class=\"tag\"> · FR</span>"); (format!("<a href=\"/articles/{}\">{}</a>", a.slug, a.m("title_fr")), a.m("description_fr").to_string()) }
            _ => { cat.push_str("<span class=\"tag\"> · EN</span>"); (format!("<a href=\"/articles/{}\">{}</a>", a.slug, a.m("title_en")), a.m("description_en").to_string()) }
        };
        let tag = if matches!(a.lang.as_str(), "fr" | "en") { format!(" lang=\"{}\"", a.lang) } else { String::new() };
        items.push(format!(
            "          <li class=\"entree{}\"><p class=\"meta\">{cat}</p><div><h3{tag}>{title}</h3><p{tag}>{desc}</p></div></li>",
            lang_class(&a.lang)
        ));
    }
    items.join("\n")
}

fn articles_by_category(set: &ArticleSet) -> String {
    struct Cat<'a> { fr: &'a str, en: &'a str, articles: Vec<&'a Art> }
    let mut cats: Vec<Cat> = vec![];
    let mut seen: Vec<&str> = vec![];
    for a in &set.list {
        let key = if a.m("category_fr").is_empty() { a.m("category_en") } else { a.m("category_fr") };
        if !seen.contains(&key) {
            seen.push(key);
            cats.push(Cat { fr: a.m("category_fr"), en: a.m("category_en"), articles: vec![] });
        }
        if let Some(c) = cats.iter_mut().find(|c| c.fr == a.m("category_fr") || c.en == a.m("category_en")) {
            c.articles.push(a);
        }
    }
    let mut parts: Vec<String> = vec![];
    for (i, cat) in cats.iter().enumerate() {
        let mb = if i + 1 < cats.len() { " style=\"margin-bottom:var(--e5)\"" } else { "" };
        parts.push(format!("      <h2 class=\"moyen\" style=\"margin-bottom:var(--e3)\"><span class=\"fr\" lang=\"fr\">{}</span><span class=\"en\" lang=\"en\">{}</span></h2>", cat.fr, cat.en));
        parts.push(format!("      <ul{mb}>"));
        for a in &cat.articles {
            let (title, desc) = match a.lang.as_str() {
                "both" => (bi(a.m("title_fr"), a.m("title_en")), bi(a.m("description_fr"), a.m("description_en"))),
                "fr" => (format!("{}<span class=\"tag\"> · FR</span>", a.m("title_fr")), a.m("description_fr").to_string()),
                _ => (format!("{}<span class=\"tag\"> · EN</span>", a.m("title_en")), a.m("description_en").to_string()),
            };
            let tag = if matches!(a.lang.as_str(), "fr" | "en") { format!(" lang=\"{}\"", a.lang) } else { String::new() };
            parts.push(format!(
                "        <li class=\"theme{}\"><h3{tag}>{title}</h3><p{tag}>{desc}</p><a href=\"/articles/{}\">{}</a></li>",
                lang_class(&a.lang), a.slug, bi("Lire", "Read")
            ));
        }
        parts.push("      </ul>".into());
    }
    parts.join("\n")
}

// ---------------------------------------------------------------------------- pages

pub struct Rendered {
    pub html: String,
    /// Étiquettes de dépendance : la page est invalidée quand l'une d'elles est purgée.
    pub tags: Vec<String>,
}

const CURRENT: &str = " aria-current=\"page\"";
const ACCOUNT_DESC: &str = "Compte JustPrivacy : un nom d'utilisateur et un mot de passe, rien d'autre.";

fn page(template: &str, title: &str, desc: &str, extra: Vec<(&'static str, String)>) -> String {
    let mut vars: Vars = BTreeMap::new();
    vars.insert("TITLE", title.to_string());
    vars.insert("META_DESCRIPTION", desc.to_string());
    for (k, v) in extra { vars.insert(k, v); }
    render_template(template, vars)
}

/// The dedicated 404 page (rendered once).
pub fn not_found_page() -> &'static str {
    static HTML: LazyLock<String> = LazyLock::new(|| page(NOT_FOUND, "Page introuvable – Page not found – JustPrivacy", "This page does not exist.", vec![]));
    &HTML
}

/// Pages qui ne dépendent d'aucun article : identiques tant que le binaire ne change pas.
fn static_page(path: &str) -> Option<String> {
    Some(match path {
        "/benchmark" => page(BENCHMARK, "Benchmark – JustPrivacy", "Free AI assistants benchmark: tested the way you use them, on everyday tasks.", vec![("NAV_BENCHMARK", CURRENT.into())]),
        "/about" => page(ABOUT, "À propos – About – JustPrivacy", "JustPrivacy is an independent resource on privacy and digital rights.", vec![("NAV_APROPOS", CURRENT.into())]),
        "/legal" => page(LEGAL, "Mentions légales – Legal notice – JustPrivacy", "Legal notice for JustPrivacy.", vec![]),
        "/privacy" => page(PRIVACY, "Confidentialité et CGU – Privacy & terms – JustPrivacy", "Privacy policy and terms of use for JustPrivacy.", vec![]),
        "/compatibility" => page(COMPAT, "Navigateur non compatible – Unsupported browser – JustPrivacy", "The anti-bot check could not load: what to do.", vec![]),
        "/login" => page(LOGIN, "Connexion – Log in – JustPrivacy", ACCOUNT_DESC, vec![("JS_COMPTE", ASSETS.js_account.clone())]),
        "/signup" => page(SIGNUP, "Inscription – Sign up – JustPrivacy", ACCOUNT_DESC, vec![("JS_COMPTE", ASSETS.js_account.clone())]),
        "/account" => page(ACCOUNT, "Mon compte – My account – JustPrivacy", ACCOUNT_DESC, vec![("JS_COMPTE", ASSETS.js_account.clone()), ("NAV_COMPTE", CURRENT.into())]),
        "/editor" => page(EDITOR, "Éditeur d'articles – Article editor – JustPrivacy", "Éditeur d'articles JustPrivacy.", vec![("JS_EDITEUR", ASSETS.js_editor.clone())]),
        _ => return None,
    })
}

/// `path` : chemin normalisé (sans barre finale, sauf la racine). `None` : page inconnue (404).
pub fn render_route(path: &str, set: &ArticleSet) -> Option<Rendered> {
    if let Some(html) = static_page(path) { return Some(Rendered { html, tags: vec![] }); }
    let articles_tag = vec!["articles".to_string()];
    match path {
        "/" => Some(Rendered {
            html: page(HOME, "JustPrivacy", "JustPrivacy: reference articles on privacy and digital rights, and an independent benchmark of free AI assistants.",
                vec![("ARTICLES_PREVIEW", home_preview(set))]),
            tags: articles_tag,
        }),
        "/articles" => {
            let count = |l: &str| set.list.iter().filter(|a| a.lang == l).count().to_string();
            Some(Rendered {
                html: page(ARTICLES, "Articles – JustPrivacy", "Reference articles on privacy and digital rights.", vec![
                    ("NAV_ARTICLES", CURRENT.into()),
                    ("COUNT_FR_ONLY", count("fr")),
                    ("COUNT_EN_ONLY", count("en")),
                    ("ARTICLES_BY_CATEGORY", articles_by_category(set)),
                ]),
                tags: articles_tag,
            })
        }
        _ => {
            let slug = path.strip_prefix("/articles/")?;
            let a = set.get(slug)?;
            let mut tags = vec![format!("article:{slug}")];
            let (title, desc) = match a.lang.as_str() {
                "both" => (
                    format!("{} – {} – JustPrivacy", a.m("title_fr"), a.m("title_en")),
                    if a.meta.contains_key("description_en") { a.m("description_en") } else { a.m("description_fr") }.to_string(),
                ),
                "fr" => (format!("{} – JustPrivacy", a.m("title_fr")), a.m("description_fr").to_string()),
                _ => (format!("{} – JustPrivacy", a.m("title_en")), a.m("description_en").to_string()),
            };
            let body = article_body(a, set, &mut tags);
            tags.sort();
            tags.dedup();
            let edit = format!("<span aria-hidden=\"true\">·</span> <a href=\"/editor#{slug}\">{}</a>", bi("Modifier", "Edit"));
            let lang_attr = if matches!(a.lang.as_str(), "fr" | "en") { format!(" lang=\"{}\"", a.lang) } else { String::new() };
            Some(Rendered {
                html: page(ARTICLE, &title, &desc, vec![
                    ("NAV_ARTICLES", CURRENT.into()),
                    ("ARTICLE_EDIT", edit),
                    ("ARTICLE_LANG", lang_attr),
                    ("ARTICLE_TITLE", article_title(a)),
                    ("ARTICLE_BODY", body),
                ]),
                tags,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(raw: &[(&str, &str)]) -> ArticleSet {
        ArticleSet::new(&raw.iter().map(|(s, t)| (s.to_string(), t.to_string())).collect::<Vec<_>>())
    }

    const ACCES: &str = "---\norder: 2\nslug: acces\nlang: both\ncategory_fr: Droits\ncategory_en: Rights\ntitle_fr: Le droit d'accès\ntitle_en: Access\ndescription_fr: Desc FR\ndescription_en: Desc EN\nchapeau_fr: Chapeau FR\nchapeau_en: Chapeau EN\nsee_also: perm\n---\n\n::: fr\n## Avant d'écrire\n\nTexte avec un [lien](https://example.org) et du **gras** et de l'*italique*.\n\n- un\n- deux\n\n> citation\n:::\n\n::: en\n## Before writing\n\nEnglish text.\n:::\n";
    const PERM: &str = "---\norder: 3\nslug: perm\nlang: fr\ncategory_fr: Droits\ntitle_fr: Permissions\ndescription_fr: Desc\n---\n\n## Section\n\nCorps.\n";

    #[test]
    fn markdown_subset() {
        let html = md_to_html("## Un titre\n\nTexte **gras** *it* [x](/a).\n\n- a\n- b\n\n> q");
        assert!(html.contains("<h2 id=\"un-titre\">Un titre</h2>"));
        assert!(html.contains("<strong>gras</strong> <em>it</em> <a href=\"/a\">x</a>."));
        assert!(html.contains("<ul>\n<li>a</li>\n<li>b</li>\n</ul>"));
        assert!(html.contains("<blockquote class=\"modele\">\n<p>q</p>\n</blockquote>"));
        assert_eq!(slugify("Ce qu'il faut — savoir : é à ç!"), "ce-quil-faut-savoir-e-a-c");
    }

    #[test]
    fn markdown_is_escaped_and_links_are_filtered() {
        let html = md_to_html("<script>alert(1)</script> & [x](javascript:alert(1)) [y](//evil.example) [z](https://a.b/?q=1&r=2) [q](https://a\" onmouseover=\"x)");
        assert!(!html.contains("<script"));
        assert!(html.contains("&lt;script&gt;") && html.contains("&amp; "));
        assert!(html.contains("<a href=\"#\">x</a>") && html.contains("<a href=\"#\">y</a>"));
        assert!(html.contains("href=\"https://a.b/?q=1&amp;r=2\""));
        assert!(!html.contains("\" onmouseover"));
    }

    #[test]
    fn emphasis_edge_cases_match_the_old_script() {
        assert_eq!(inline_md("**a*b**"), "**a*b**");
        assert_eq!(inline_md("a * b * c"), "a <em> b </em> c");
        assert_eq!(inline_md("**gras** puis *it*"), "<strong>gras</strong> puis <em>it</em>");
        assert_eq!(inline_md("[x](a)(b)"), "<a href=\"#\">x</a>(b)");
    }

    #[test]
    fn css_fallbacks() {
        let vars = static_vars(":root{--a:1rem;--b:#fff}\n@supports (x:y){\n  :root{--a:2rem}\n}\n");
        assert_eq!(vars["--a"], "1rem");
        let css = add_css_fallbacks(":root{--a:1rem}\n.x{margin:var(--a);color:var(--b);width:var(--zzz)}\n@media (min-width:5rem){.y{padding:var(--a)}}", &vars);
        assert!(css.contains(".x{margin:1rem;margin:var(--a);color:#fff;color:var(--b);width:var(--zzz)}"));
        assert!(css.contains("@media (min-width:5rem){.y{padding:1rem;padding:var(--a)}}"));
        assert!(css.starts_with(":root{--a:1rem}"));
        assert_eq!(add_inline_fallbacks("<p style=\"margin-bottom:var(--a)\">x</p>", &vars), "<p style=\"margin-bottom:1rem;margin-bottom:var(--a)\">x</p>");
    }

    #[test]
    fn templates_fill_in_one_pass() {
        let mut vars: Vars = BTreeMap::new();
        vars.insert("A", "{{B}}".into());
        vars.insert("B", "boom".into());
        assert_eq!(fill("x {{A}} {{NAV_X}} {{INCONNU}} {{ pas }}", &vars), "x {{B}}  {{INCONNU}} {{ pas }}");
        assert_eq!(minify_html("<!-- c -->\n  <p>a</p>  \n\n<!--[if IE]>x<![endif]-->\n"), "<p>a</p>\n<!--[if IE]>x<![endif]-->");
    }

    #[test]
    fn pages_render_with_tags() {
        let s = set(&[("acces", ACCES), ("perm", PERM)]);
        let home = render_route("/", &s).unwrap();
        assert_eq!(home.tags, vec!["articles"]);
        assert!(home.html.contains("href=\"/articles/acces\""));
        let list = render_route("/articles", &s).unwrap();
        assert!(list.html.contains("Droits") && list.html.contains("Lire"));
        let acces = render_route("/articles/acces", &s).unwrap();
        assert_eq!(acces.tags, vec!["article:acces", "article:perm"]);
        assert!(acces.html.contains("Voir aussi\u{a0}: Permissions") && acces.html.contains("href=\"/editor#acces\""));
        assert!(acces.html.contains("<h2 id=\"avant-decrire\">"));
        assert!(render_route("/articles/inconnu", &s).is_none());
        assert!(not_found_page().contains("404") && not_found_page().contains("noindex"));
        assert!(render_route("/benchmark", &s).unwrap().tags.is_empty());
        assert!(render_route("/editor", &s).unwrap().html.contains("ed-grille"));
        for p in ["/about", "/legal", "/privacy", "/compatibility", "/login", "/signup", "/account"] {
            assert!(render_route(p, &s).is_some(), "{p}");
        }
    }

    #[test]
    fn hostile_front_matter_cannot_break_out() {
        let evil = "---\nslug: evil\nlang: fr\ntitle_fr: </title><script>alert(1)</script>\ndescription_fr: \"><img src=x onerror=alert(1)>\n---\ncorps\n";
        let s = set(&[("evil", evil)]);
        let html = render_route("/articles/evil", &s).unwrap().html;
        assert!(!html.contains("<script>alert") && !html.contains("<img src=x"), "{}", &html[..html.len().min(200)]);
    }

    #[test]
    fn malformed_articles_are_tolerated() {
        let s = set(&[("b", "---\nlang: en\ntitle_en: B\n---\ntext\n"), ("a", "pas d'en-tête"), ("c", "---\norder: x\nslug: c\n---\nz")]);
        assert_eq!(s.list.len(), 3);
        assert!(render_route("/articles", &s).is_some());
        assert!(render_route("/articles/a", &s).is_some());
    }

    /// Développement : `GOLDEN_DIST=/chemin/vers/dist cargo test golden` compare le rendu avec la sortie de l'ancien build.py.
    #[test]
    fn golden_against_the_old_build() {
        let Ok(dir) = std::env::var("GOLDEN_DIST") else { return };
        let raw: Vec<(String, String)> = {
            let mut files: Vec<_> = std::fs::read_dir("../content/articles").unwrap().flatten().filter(|e| e.path().extension().is_some_and(|x| x == "md")).collect();
            files.sort_by_key(|e| e.file_name());
            files.iter().map(|e| (e.path().file_stem().unwrap().to_string_lossy().into_owned(), std::fs::read_to_string(e.path()).unwrap())).collect()
        };
        let set = ArticleSet::new(&raw);
        let mut routes: Vec<String> = ["/", "/articles", "/benchmark", "/about", "/legal", "/privacy", "/login", "/signup", "/account", "/editor"].iter().map(|s| s.to_string()).collect();
        routes.extend(raw.iter().map(|(s, _)| format!("/articles/{s}")));
        for route in routes {
            let file = if route == "/" { format!("{dir}/index.html") } else { format!("{dir}{route}/index.html") };
            let want = std::fs::read_to_string(&file).unwrap_or_else(|_| panic!("{file}"));
            let got = render_route(&route, &set).unwrap().html;
            if got != want {
                let i = got.bytes().zip(want.bytes()).position(|(a, b)| a != b).unwrap_or(got.len().min(want.len()));
                let around = |t: &str| t.get(i.saturating_sub(30)..(i + 60).min(t.len())).unwrap_or("").replace('\n', "⏎");
                if let Ok(out) = std::env::var("GOLDEN_OUT") { std::fs::write(format!("{out}/rust.html"), &got).unwrap(); std::fs::write(format!("{out}/python.html"), &want).unwrap(); }
                panic!("{route}: première différence octet {i} (rust {} o, python {} o)\n rust  : {}\n python: {}", got.len(), want.len(), around(&got), around(&want));
            }
        }
    }
}
