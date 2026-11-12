#!/usr/bin/env python3
"""Generates the JustPrivacy static site.

Usage:
    python3 build.py

Outputs dist/ — one self-contained HTML file per URL.
Each page embeds the CSS and fonts as base64 so the browser
needs only a single request to fully render the page.

Compatibility: the pages work without JavaScript (language is switched with
sibling selectors, see style.css). Declarations that use var() get a literal
fallback value injected at build time, for browsers without custom properties.
"""

import base64
import glob
import os
import re
import shutil
import sys

ROOT = os.path.dirname(os.path.abspath(__file__))
CONTENT_DIR = os.path.join(ROOT, 'content', 'articles')
TEMPLATES_DIR = os.path.join(ROOT, 'templates')
FONTS_DIR = os.path.join(ROOT, 'fonts')
DIST_DIR = os.path.join(ROOT, 'dist')
STATIC_VARS = {}


# ---------------------------------------------------------------------------
# File helpers
# ---------------------------------------------------------------------------

def read_file(path):
    with open(path, 'r', encoding='utf-8') as f:
        return f.read()


def write_file(path, content):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, 'w', encoding='utf-8') as f:
        f.write(content)


def encode_font_b64(path):
    with open(path, 'rb') as f:
        return base64.b64encode(f.read()).decode('ascii')


# ---------------------------------------------------------------------------
# YAML front-matter parser (subset: single-line key: value pairs)
# ---------------------------------------------------------------------------

def parse_yaml(text):
    data = {}
    for line in text.strip().split('\n'):
        line = line.strip()
        if not line or line.startswith('#'):
            continue
        m = re.match(r'^([A-Za-z_][A-Za-z0-9_]*)\s*:\s*(.*)$', line)
        if m:
            data[m.group(1).strip()] = m.group(2).strip()
    return data


# ---------------------------------------------------------------------------
# Markdown to HTML (subset: ##, paragraphs, ul/li, blockquotes, links, bold)
# ---------------------------------------------------------------------------

def slugify(text):
    t = text.lower()
    for old, new in {'à': 'a', 'â': 'a', 'ä': 'a', 'é': 'e', 'è': 'e', 'ê': 'e', 'ë': 'e',
                     'î': 'i', 'ï': 'i', 'ô': 'o', 'ö': 'o', 'ù': 'u', 'û': 'u', 'ü': 'u',
                     'ç': 'c', 'ñ': 'n', '’': '', '‘': '', '\'': '',
                     '«': '', '»': '', '“': '', '”': '', '"': ''}.items():
        t = t.replace(old, new)
    t = re.sub(r'[^a-z0-9]+', '-', t)
    return t.strip('-')


def inline_md(text):
    text = re.sub(r'\[([^\]]+)\]\(([^)]+)\)', r'<a href="\2">\1</a>', text)
    text = re.sub(r'\*\*([^*]+)\*\*', r'<strong>\1</strong>', text)
    text = re.sub(r'(?<!\*)\*([^*]+)\*(?!\*)', r'<em>\1</em>', text)
    return text


def md_to_html(text):
    if not text:
        return ''
    lines = text.split('\n')
    parts = []
    in_list = False
    in_bq = False
    para = []

    def flush_para():
        nonlocal para
        if para:
            parts.append('<p>' + inline_md(' '.join(para)) + '</p>')
            para = []

    def flush_list():
        nonlocal in_list
        if in_list:
            parts.append('</ul>')
            in_list = False

    def flush_bq():
        nonlocal in_bq
        if in_bq:
            parts.append('</blockquote>')
            in_bq = False

    for line in lines:
        s = line.strip()
        if not s:
            flush_para()
            continue
        if s.startswith('## '):
            flush_para(); flush_list(); flush_bq()
            h = s[3:]
            parts.append('<h2 id="%s">%s</h2>' % (slugify(h), inline_md(h)))
            continue
        if s.startswith('- ') or s.startswith('* '):
            flush_para(); flush_bq()
            if not in_list:
                parts.append('<ul>')
                in_list = True
            parts.append('<li>' + inline_md(s[2:]) + '</li>')
            continue
        if s.startswith('> '):
            flush_para(); flush_list()
            if not in_bq:
                parts.append('<blockquote class="modele">')
                in_bq = True
            parts.append('<p>' + inline_md(s[2:]) + '</p>')
            continue
        flush_list(); flush_bq()
        para.append(s)

    flush_para(); flush_list(); flush_bq()
    return '\n'.join(parts)


def extract_toc(md_text):
    headings = re.findall(r'^## (.+)$', md_text, re.MULTILINE)
    if not headings:
        return ''
    return ''.join('<li><a href="#%s">%s</a></li>' % (slugify(h), h) for h in headings)


# ---------------------------------------------------------------------------
# Article loading
# ---------------------------------------------------------------------------

def parse_article(path):
    content = read_file(path)
    if content.startswith('---'):
        parts = content.split('---', 2)
        meta = parse_yaml(parts[1])
        body = parts[2]
    else:
        meta = {}
        body = content

    lang = meta.get('lang', 'both')
    fr_md = ''
    en_md = ''

    if lang == 'both':
        chunks = re.split(r'^:::\s*(fr|en)\s*$', body, flags=re.MULTILINE)
        i = 1
        while i < len(chunks) - 1:
            block_lang = chunks[i]
            block_text = chunks[i + 1]
            end = block_text.find('\n:::')
            if end >= 0:
                block_text = block_text[:end]
            if block_lang == 'fr':
                fr_md = block_text.strip()
            elif block_lang == 'en':
                en_md = block_text.strip()
            i += 2
    elif lang == 'fr':
        fr_md = body.strip()
    elif lang == 'en':
        en_md = body.strip()

    meta['body_fr'] = md_to_html(fr_md)
    meta['body_en'] = md_to_html(en_md)
    meta['toc_fr'] = extract_toc(fr_md)
    meta['toc_en'] = extract_toc(en_md)
    return meta


def load_articles():
    articles = []
    for path in sorted(glob.glob(os.path.join(CONTENT_DIR, '*.md'))):
        articles.append(parse_article(path))
    articles.sort(key=lambda a: int(a.get('order', '999')))
    return articles


# ---------------------------------------------------------------------------
# var() fallbacks — for browsers without CSS custom properties
# ---------------------------------------------------------------------------

VAR_RE = re.compile(r'var\((--[\w-]+)\)')


def static_vars(css):
    """Static (light, non-clamp) values of the first top-level :root block."""
    m = re.search(r'(?m)^:root\{(.*?)\}', css, re.DOTALL)
    values = {}
    if m:
        for decl in m.group(1).split(';'):
            if ':' in decl:
                k, v = decl.split(':', 1)
                values[k.strip()] = v.strip()
    return values


def decl_fallbacks(decls, values):
    """'a:var(--x);b:2' -> 'a:<literal>;a:var(--x);b:2' (only where resolvable)."""
    out = []
    for decl in decls.split(';'):
        if not decl.strip():
            continue
        name, _, value = decl.partition(':')
        if not name.strip().startswith('--') and 'var(' in value:
            names = VAR_RE.findall(value)
            if names and all(n in values for n in names):
                literal = VAR_RE.sub(lambda m: values[m.group(1)], value)
                out.append('%s:%s' % (name, literal))
        out.append(decl)
    return ';'.join(out)


def add_css_fallbacks(css, values):
    def block(m):
        selector, body = m.group(1), m.group(2)
        if selector.strip().startswith((':root', '@font-face')):
            return m.group(0)
        return '%s{%s}' % (selector, decl_fallbacks(body, values))
    return re.sub(r'([^{}]+)\{([^{}]*)\}', block, css)


def add_inline_fallbacks(html, values):
    def attr(m):
        return 'style="%s"' % decl_fallbacks(m.group(1), values)
    return re.sub(r'style="([^"]*var\([^"]*)"', attr, html)


# ---------------------------------------------------------------------------
# CSS — fonts embedded as base64 data URIs
# ---------------------------------------------------------------------------

def strip_css_comments(css):
    return re.sub(r'/\*.*?\*/', '', css, flags=re.DOTALL)


def strip_js_comments(js):
    # Only /* ... */ blocks (the script uses no // comments).
    return re.sub(r'/\*.*?\*/', '', js, flags=re.DOTALL).strip()


def build_css():
    latin = encode_font_b64(os.path.join(FONTS_DIR, 'inter-latin.woff2'))
    latin_ext = encode_font_b64(os.path.join(FONTS_DIR, 'inter-latin-ext.woff2'))

    font_css = (
        '@font-face{font-family:"Inter";font-style:normal;font-weight:400 800;font-display:swap;'
        'src:url(data:font/woff2;base64,%s) format("woff2");'
        'unicode-range:U+0100-02BA,U+02BD-02C5,U+02C7-02CC,U+02CE-02D7,U+02DD-02FF,'
        'U+0304,U+0308,U+0329,U+1D00-1DBF,U+1E00-1E9F,U+1EF2-1EFF,U+2020,U+20A0-20AB,'
        'U+20AD-20C4,U+2113,U+2C60-2C7F,U+A720-A7FF}\n'
        '@font-face{font-family:"Inter";font-style:normal;font-weight:400 800;font-display:swap;'
        'src:url(data:font/woff2;base64,%s) format("woff2");'
        'unicode-range:U+0000-00FF,U+0131,U+0152-0153,U+02BB-02BC,U+02C6,U+02DA,U+02DC,'
        'U+0304,U+0308,U+0329,U+2000-206F,U+20AC,U+2122,U+2191,U+2193,U+2212,U+2215,'
        'U+FEFF,U+FFFD}\n'
    ) % (latin_ext, latin)

    raw_css = strip_css_comments(read_file(os.path.join(TEMPLATES_DIR, 'style.css')))
    return font_css + add_css_fallbacks(raw_css, static_vars(raw_css))


# ---------------------------------------------------------------------------
# Validation
# ---------------------------------------------------------------------------

def validate():
    errors = []
    for fname in ('inter-latin.woff2', 'inter-latin-ext.woff2'):
        if not os.path.exists(os.path.join(FONTS_DIR, fname)):
            errors.append('Missing font: fonts/%s' % fname)
    for tname in ('style.css', 'reglages.js', 'home.html', 'articles.html',
                  'article.html', 'benchmark.html', 'about.html', 'legal.html', 'privacy.html'):
        if not os.path.exists(os.path.join(TEMPLATES_DIR, tname)):
            errors.append('Missing template: templates/%s' % tname)
    if not os.path.isdir(CONTENT_DIR):
        errors.append('Missing content directory: content/articles/')
    if errors:
        for e in errors:
            print('ERROR: ' + e, file=sys.stderr)
        sys.exit(1)

    articles = load_articles()
    slugs = set()
    for a in articles:
        slug = a.get('slug', '').strip()
        if not slug:
            errors.append('Article missing slug field: %s' % (a.get('title_fr') or a.get('title_en') or '?'))
            continue
        if slug in slugs:
            errors.append('Duplicate slug: %s' % slug)
        else:
            slugs.add(slug)
        lang = a.get('lang', 'both')
        if lang in ('both', 'fr') and not a.get('title_fr'):
            errors.append('Article %r: missing title_fr' % slug)
        if lang in ('both', 'en') and not a.get('title_en'):
            errors.append('Article %r: missing title_en' % slug)
    if errors:
        for e in errors:
            print('ERROR: ' + e, file=sys.stderr)
        sys.exit(1)


# ---------------------------------------------------------------------------
# HTML minification
# ---------------------------------------------------------------------------

def minify_html(html):
    # Strip HTML comments (preserve IE conditionals: <!--[if ...)
    html = re.sub(r'<!--(?!\[if\b).*?-->', '', html, flags=re.DOTALL)
    # Strip leading/trailing whitespace from each line and remove blank lines
    lines = [l.strip() for l in html.split('\n')]
    html = '\n'.join(l for l in lines if l)
    return html


# ---------------------------------------------------------------------------
# Page rendering
# ---------------------------------------------------------------------------

def render_page(template_name, replacements):
    page = read_file(os.path.join(TEMPLATES_DIR, template_name))
    # Strip dev CSS block, replace with build placeholder
    page = re.sub(r'<!-- \[dev-css\] -->.*?<!-- \[/dev-css\] -->', '<style>{{CSS}}</style>', page, flags=re.DOTALL)
    # Strip named dev content blocks, replace with build placeholders
    page = re.sub(r'<!-- \[dev-start:([A-Z_]+)\] -->.*?<!-- \[dev-end:\1\] -->', r'{{\1}}', page, flags=re.DOTALL)
    # Fill all placeholders
    for key, val in replacements.items():
        page = page.replace('{{%s}}' % key, val)
    # Clear unresolved nav placeholders
    page = page.replace('{{NAV_ARTICLES}}', '')
    page = page.replace('{{NAV_BENCHMARK}}', '')
    page = page.replace('{{NAV_APROPOS}}', '')
    page = add_inline_fallbacks(page, STATIC_VARS)
    return minify_html(page)


def build_article_body(article, articles_map):
    """Build the HTML body of an article (bilingual or single-language)."""
    lang = article.get('lang', 'both')
    parts = []

    if lang == 'both':
        # French version
        parts.append('      <div class="lecture corps fr" lang="fr">')
        parts.append('        <p class="chapeau" style="margin-bottom:var(--e4)">%s</p>' % article.get('chapeau_fr', ''))
        if article['toc_fr']:
            parts.append('        <nav class="sommaire" aria-label="Sommaire"><h2>Sommaire</h2>')
            parts.append('          <ul style="list-style:none;padding:0;margin:0">%s</ul></nav>' % article['toc_fr'])
        parts.append(article['body_fr'])
        parts.append(_build_see_also(article, articles_map, 'fr'))
        parts.append('      </div>')

        # English version
        parts.append('      <div class="lecture corps en" lang="en">')
        parts.append('        <p class="chapeau" style="margin-bottom:var(--e4)">%s</p>' % article.get('chapeau_en', ''))
        if article['toc_en']:
            parts.append('        <nav class="sommaire" aria-label="Contents"><h2>Contents</h2>')
            parts.append('          <ul style="list-style:none;padding:0;margin:0">%s</ul></nav>' % article['toc_en'])
        parts.append(article['body_en'])
        parts.append(_build_see_also(article, articles_map, 'en'))
        parts.append('      </div>')

    elif lang == 'fr':
        parts.append('      <div class="lecture corps">')
        parts.append('        <p class="chapeau" style="margin-bottom:var(--e4)">%s</p>' % article.get('chapeau_fr', ''))
        if article['toc_fr']:
            parts.append('        <nav class="sommaire" aria-label="Sommaire"><h2>Sommaire</h2>')
            parts.append('          <ul style="list-style:none;padding:0;margin:0">%s</ul></nav>' % article['toc_fr'])
        parts.append(article['body_fr'])
        parts.append(_build_see_also(article, articles_map, 'fr'))
        parts.append('      </div>')

    elif lang == 'en':
        parts.append('      <div class="lecture corps" lang="en">')
        parts.append('        <p class="chapeau" style="margin-bottom:var(--e4)">%s</p>' % article.get('chapeau_en', ''))
        if article['toc_en']:
            parts.append('        <nav class="sommaire" aria-label="Contents"><h2>Contents</h2>')
            parts.append('          <ul style="list-style:none;padding:0;margin:0">%s</ul></nav>' % article['toc_en'])
        parts.append(article['body_en'])
        parts.append(_build_see_also(article, articles_map, 'en'))
        parts.append('      </div>')

    return '\n'.join(parts)


def _build_see_also(article, articles_map, display_lang):
    see_also = article.get('see_also', '').strip()
    if not see_also:
        return ''
    slugs = [s.strip() for s in see_also.split(',') if s.strip()]
    links = []
    for s in slugs:
        ref = articles_map.get(s)
        if not ref:
            continue
        if display_lang == 'fr':
            label = ref.get('title_fr', ref.get('title_en', s))
            prefix = 'Voir aussi : '
        else:
            label = ref.get('title_en', ref.get('title_fr', s))
            prefix = 'See also: '
        links.append('<p><a href="/articles/%s">%s%s</a></p>' % (s, prefix, label))

    back = (
        '<p><a href="/articles"><span aria-hidden="true">←</span> '
        '<span class="fr" lang="fr">Tous les articles</span>'
        '<span class="en" lang="en">All articles</span></a></p>'
    )
    return '<div class="voir">%s%s</div>' % (''.join(links), back)


def build_article_title(article):
    lang = article.get('lang', 'both')
    if lang == 'both':
        return '<span class="fr" lang="fr">%s</span><span class="en" lang="en">%s</span>' % (
            article.get('title_fr', ''), article.get('title_en', ''))
    return article.get('title_fr' if lang == 'fr' else 'title_en', '')


def build_home_preview(articles):
    items = []
    for a in articles:
        lang = a.get('lang', 'both')
        lang_class = ' fr-seul' if lang == 'fr' else (' en-seul' if lang == 'en' else '')
        cat = '<span class="fr" lang="fr">%s</span><span class="en" lang="en">%s</span>' % (
            a.get('category_fr', ''), a.get('category_en', ''))

        if lang == 'both':
            title = ('<a href="/articles/%s"><span class="fr" lang="fr">%s</span>'
                     '<span class="en" lang="en">%s</span></a>') % (
                a['slug'], a.get('title_fr', ''), a.get('title_en', ''))
            desc = '<span class="fr" lang="fr">%s</span><span class="en" lang="en">%s</span>' % (
                a.get('description_fr', ''), a.get('description_en', ''))
        elif lang == 'fr':
            title = '<a href="/articles/%s">%s</a>' % (a['slug'], a.get('title_fr', ''))
            desc = a.get('description_fr', '')
            cat += '<span class="tag"> · FR</span>'
        else:
            title = '<a href="/articles/%s">%s</a>' % (a['slug'], a.get('title_en', ''))
            desc = a.get('description_en', '')
            cat += '<span class="tag"> · EN</span>'

        tag = ' lang="%s"' % lang if lang in ('fr', 'en') else ''
        items.append(
            '          <li class="entree%s"><p class="meta">%s</p>'
            '<div><h3%s>%s</h3><p%s>%s</p></div></li>' % (
                lang_class, cat, tag, title, tag, desc))

    return '\n'.join(items)


def build_articles_by_category(articles, articles_map):
    categories = []
    seen = set()
    for a in articles:
        cat_key = a.get('category_fr', '') or a.get('category_en', '')
        if cat_key not in seen:
            seen.add(cat_key)
            categories.append({'fr': a.get('category_fr', ''), 'en': a.get('category_en', ''), 'articles': []})
        for c in categories:
            if c['fr'] == a.get('category_fr', '') or c['en'] == a.get('category_en', ''):
                c['articles'].append(a)
                break

    parts = []
    for i, cat in enumerate(categories):
        mb = ' style="margin-bottom:var(--e5)"' if i < len(categories) - 1 else ''
        parts.append(
            '      <h2 class="moyen" style="margin-bottom:var(--e3)">'
            '<span class="fr" lang="fr">%s</span><span class="en" lang="en">%s</span></h2>' % (cat['fr'], cat['en']))
        parts.append('      <ul%s>' % mb)

        for a in cat['articles']:
            lang = a.get('lang', 'both')
            lang_class = ' fr-seul' if lang == 'fr' else (' en-seul' if lang == 'en' else '')
            if lang == 'both':
                title = '<span class="fr" lang="fr">%s</span><span class="en" lang="en">%s</span>' % (
                    a.get('title_fr', ''), a.get('title_en', ''))
                desc = '<span class="fr" lang="fr">%s</span><span class="en" lang="en">%s</span>' % (
                    a.get('description_fr', ''), a.get('description_en', ''))
            elif lang == 'fr':
                title = '%s<span class="tag"> · FR</span>' % a.get('title_fr', '')
                desc = a.get('description_fr', '')
            else:
                title = '%s<span class="tag"> · EN</span>' % a.get('title_en', '')
                desc = a.get('description_en', '')

            tag = ' lang="%s"' % lang if lang in ('fr', 'en') else ''
            read_btn = '<span class="fr" lang="fr">Lire</span><span class="en" lang="en">Read</span>'
            parts.append(
                '        <li class="theme%s"><h3%s>%s</h3><p%s>%s</p>'
                '<a href="/articles/%s">%s</a></li>' % (
                    lang_class, tag, title, tag, desc, a['slug'], read_btn))

        parts.append('      </ul>')

    return '\n'.join(parts)


# ---------------------------------------------------------------------------
# Main build
# ---------------------------------------------------------------------------

def build():
    validate()

    if os.path.exists(DIST_DIR):
        shutil.rmtree(DIST_DIR)

    css = build_css()
    STATIC_VARS.update(static_vars(strip_css_comments(read_file(os.path.join(TEMPLATES_DIR, 'style.css')))))
    js = strip_js_comments(read_file(os.path.join(TEMPLATES_DIR, 'reglages.js')))
    articles = load_articles()
    articles_map = {a['slug']: a for a in articles}

    count_fr_only = sum(1 for a in articles if a.get('lang') == 'fr')
    count_en_only = sum(1 for a in articles if a.get('lang') == 'en')

    base_vars = {'CSS': css, 'JS': js, 'NAV_ARTICLES': '', 'NAV_BENCHMARK': '', 'NAV_APROPOS': ''}

    # Homepage
    home_vars = dict(base_vars)
    home_vars['TITLE'] = 'JustPrivacy'
    home_vars['META_DESCRIPTION'] = (
        'JustPrivacy: reference articles on privacy and digital rights, '
        'and an independent benchmark of free AI assistants.')
    home_vars['ARTICLES_PREVIEW'] = build_home_preview(articles)
    write_file(os.path.join(DIST_DIR, 'index.html'), render_page('home.html', home_vars))

    # Articles listing
    art_vars = dict(base_vars)
    art_vars['TITLE'] = 'Articles – JustPrivacy'
    art_vars['META_DESCRIPTION'] = 'Reference articles on privacy and digital rights.'
    art_vars['NAV_ARTICLES'] = ' aria-current="page"'
    art_vars['COUNT_FR_ONLY'] = str(count_fr_only)
    art_vars['COUNT_EN_ONLY'] = str(count_en_only)
    art_vars['ARTICLES_BY_CATEGORY'] = build_articles_by_category(articles, articles_map)
    write_file(os.path.join(DIST_DIR, 'articles', 'index.html'), render_page('articles.html', art_vars))

    # Individual articles
    for a in articles:
        a_vars = dict(base_vars)
        a_vars['NAV_ARTICLES'] = ' aria-current="page"'
        lang = a.get('lang', 'both')
        if lang == 'both':
            a_vars['TITLE'] = '%s – %s – JustPrivacy' % (a.get('title_fr', ''), a.get('title_en', ''))
            a_vars['META_DESCRIPTION'] = a.get('description_en', a.get('description_fr', ''))
        elif lang == 'fr':
            a_vars['TITLE'] = '%s – JustPrivacy' % a.get('title_fr', '')
            a_vars['META_DESCRIPTION'] = a.get('description_fr', '')
        else:
            a_vars['TITLE'] = '%s – JustPrivacy' % a.get('title_en', '')
            a_vars['META_DESCRIPTION'] = a.get('description_en', '')
        a_vars['ARTICLE_LANG'] = ' lang="%s"' % lang if lang in ('fr', 'en') else ''
        a_vars['ARTICLE_TITLE'] = build_article_title(a)
        a_vars['ARTICLE_BODY'] = build_article_body(a, articles_map)
        write_file(
            os.path.join(DIST_DIR, 'articles', a['slug'], 'index.html'),
            render_page('article.html', a_vars))

    # Benchmark
    bench_vars = dict(base_vars)
    bench_vars['TITLE'] = 'Benchmark – JustPrivacy'
    bench_vars['META_DESCRIPTION'] = 'Free AI assistants benchmark: tested the way you use them, on everyday tasks.'
    bench_vars['NAV_BENCHMARK'] = ' aria-current="page"'
    write_file(os.path.join(DIST_DIR, 'benchmark', 'index.html'), render_page('benchmark.html', bench_vars))

    # About
    about_vars = dict(base_vars)
    about_vars['TITLE'] = 'À propos – About – JustPrivacy'
    about_vars['META_DESCRIPTION'] = 'JustPrivacy is an independent resource on privacy and digital rights.'
    about_vars['NAV_APROPOS'] = ' aria-current="page"'
    write_file(os.path.join(DIST_DIR, 'apropos', 'index.html'), render_page('about.html', about_vars))

    # Legal notice
    legal_vars = dict(base_vars)
    legal_vars['TITLE'] = 'Mentions légales – Legal notice – JustPrivacy'
    legal_vars['META_DESCRIPTION'] = 'Legal notice for JustPrivacy.'
    write_file(os.path.join(DIST_DIR, 'mentions', 'index.html'), render_page('legal.html', legal_vars))

    # Privacy policy & terms
    priv_vars = dict(base_vars)
    priv_vars['TITLE'] = 'Confidentialité et CGU – Privacy & terms – JustPrivacy'
    priv_vars['META_DESCRIPTION'] = 'Privacy policy and terms of use for JustPrivacy.'
    write_file(os.path.join(DIST_DIR, 'privacy', 'index.html'), render_page('privacy.html', priv_vars))

    total = sum(1 for _, _, files in os.walk(DIST_DIR) for f in files if f.endswith('.html'))
    print('Build complete: %d pages in dist/' % total)


if __name__ == '__main__':
    build()