/* Éditeur d'articles : écrit dans le dépôt git du serveur (API /api/articles et /api/proposals).
   ES5, aucune dépendance. Les éditeurs et administrateurs publient directement sur main ;
   les autres comptes enregistrent des propositions (branche users/<nom>/<slug>) relues par un éditeur. */
(function () {
  var d = document, w = window;
  function $(id) { return d.getElementById(id); }
  var app = $('app'), pre = $('prerequis');
  if (!app || !pre) { return; }
  if (!(w.XMLHttpRequest && w.JSON && d.querySelector && w.addEventListener && d.createElement)) { return; }
  pre.hidden = true;
  pre.style.display = 'none';
  app.hidden = false;
  app.removeAttribute('hidden');

  function fr() { return (d.documentElement.lang || '').slice(0, 2) === 'fr'; }
  function T(a, b) { return fr() ? a : b; }
  function each(list, fn) { for (var i = 0; i < list.length; i++) { fn(list[i], i); } }
  function on(el, ev, fn) { if (el) { el.addEventListener(ev, fn, false); } }

  // ---- Messages d'erreur -------------------------------------------------------
  var ERR = {
    unauthenticated: ['Votre session a expiré : reconnectez-vous.', 'Your session expired: log in again.'],
    not_an_editor: ["Seuls les éditeurs peuvent publier directement.", 'Only editors can publish directly.'],
    not_your_proposal: ["Cette proposition n'est pas la vôtre.", 'This suggestion is not yours.'],
    edit_conflict: ["L'article a changé depuis que vous l'avez ouvert.", 'The article changed since you opened it.'],
    not_found: ['Introuvable.', 'Not found.'],
    rate_limited: ['Trop de modifications : patientez un moment.', 'Too many changes: please wait a moment.'],
    too_many_proposals: ['Trop de propositions en attente : retirez-en une avant d\'en créer une autre.', 'Too many pending suggestions: remove one before creating another.'],
    no_changes: ['Aucune modification par rapport à la version publiée.', 'No change compared with the published version.'],
    too_large: ["L'article est trop volumineux (200 Ko maximum).", 'The article is too large (200 KB maximum).'],
    invalid_slug: ["Identifiant invalide : minuscules, chiffres et tirets (64 caractères maximum).", 'Invalid identifier: lowercase letters, digits and dashes (64 characters maximum).'],
    slug_mismatch: ["L'identifiant ne correspond pas à l'article.", 'The identifier does not match the article.'],
    missing_front_matter: ["L'en-tête de l'article est manquant.", 'The article header is missing.'],
    unterminated_front_matter: ["L'en-tête de l'article n'est pas refermé.", 'The article header is not closed.'],
    bad_front_matter: ["L'en-tête de l'article est mal formé (« --- » interdit dans un champ).", 'The article header is malformed ("---" is not allowed in a field).'],
    bad_front_matter_line: ["Une ligne de l'en-tête est invalide.", 'A header line is invalid.'],
    field_too_long: ['Un champ est trop long (600 caractères maximum).', 'A field is too long (600 characters maximum).'],
    invalid_characters: ['Caractères interdits dans le texte.', 'Forbidden characters in the text.'],
    html_not_allowed: ["Le HTML n'est pas accepté : retirez les balises (le Markdown suffit).", 'HTML is not accepted: remove the tags (Markdown is enough).'],
    invalid_order: ["L'ordre doit être un nombre entre 0 et 9999.", 'The order must be a number between 0 and 9999.'],
    invalid_lang: ['Langue invalide.', 'Invalid language.'],
    missing_title_fr: ['Le titre français est obligatoire.', 'The French title is required.'],
    missing_title_en: ['Le titre anglais est obligatoire.', 'The English title is required.'],
    invalid_see_also: ['« Voir aussi » : identifiants séparés par des virgules (minuscules, chiffres, tirets).', '"See also": identifiers separated by commas (lowercase, digits, dashes).'],
    empty_body: ["Le texte de l'article est vide.", 'The article text is empty.'],
    missing_language_block: ['Un article bilingue doit avoir un texte français et un texte anglais.', 'A bilingual article needs both a French and an English text.'],
    unsafe_link: ['Lien refusé : https, mailto, chemins du site (/…) et ancres (#…) seulement.', 'Link refused: https, mailto, site paths (/…) and anchors (#…) only.'],
    bad_origin: ['Requête refusée (origine).', 'Request refused (origin).'],
    internal: ['Erreur du serveur. Réessayez.', 'Server error. Please try again.'],
    network: ['Le serveur est injoignable.', 'The server cannot be reached.']
  };
  function errText(code) { var e = ERR[code] || ERR.internal; return T(e[0], e[1]); }

  var msgBox = $('msg'), okBox = $('ok');
  function fail(code) {
    okBox.hidden = true;
    msgBox.textContent = errText(code);
    msgBox.hidden = false; msgBox.removeAttribute('hidden');
  }
  function failText(text) { okBox.hidden = true; msgBox.textContent = text; msgBox.hidden = false; msgBox.removeAttribute('hidden'); }
  function ok(text) {
    msgBox.hidden = true;
    okBox.textContent = text; okBox.hidden = false; okBox.removeAttribute('hidden');
  }
  function clearMsg() { msgBox.hidden = true; okBox.hidden = true; $('ed-conflit').hidden = true; }

  // ---- API ---------------------------------------------------------------------
  function api(method, url, body, cb) {
    var x = new XMLHttpRequest();
    x.open(method, url, true);
    x.setRequestHeader('Accept', 'application/json');
    var has = body !== null && body !== undefined;
    if (has) { x.setRequestHeader('Content-Type', 'application/json'); }
    x.onreadystatechange = function () {
      if (x.readyState !== 4) { return; }
      var data = null;
      try { data = JSON.parse(x.responseText); } catch (e) { data = null; }
      if (x.status >= 200 && x.status < 300) { cb(null, data || {}); }
      else { cb({ status: x.status, code: (data && data.error) || (x.status ? 'internal' : 'network') }, data); }
    };
    x.send(has ? JSON.stringify(body) : null);
  }

  // ---- Format des articles (même découpage que build.py) -------------------------
  var KEYS = ['order', 'slug', 'lang', 'category_fr', 'category_en', 'title_fr', 'title_en',
    'description_fr', 'description_en', 'chapeau_fr', 'chapeau_en', 'see_also'];

  function trimNl(s) { return s.replace(/^\s*\n/, '').replace(/\s+$/, ''); }

  function splitBlocks(body) {
    var out = { fr: '', en: '' }, cur = null, buf = [], lines = body.split(/\r?\n/);
    function close() { if (cur) { out[cur] = trimNl(buf.join('\n')); } cur = null; buf = []; }
    for (var i = 0; i < lines.length; i++) {
      var m = /^:::\s*(fr|en)\s*$/.exec(lines[i]);
      if (m) { close(); cur = m[1]; continue; }
      if (cur && /^:::/.test(lines[i])) { close(); continue; }
      if (cur) { buf.push(lines[i]); }
    }
    close();
    return out;
  }

  function parseArticle(text) {
    var meta = {}, extra = [], body = text;
    if (text.indexOf('---') === 0) {
      var rest = text.slice(3).replace(/^\r?\n/, '');
      var m = /(^|\n)---[ \t]*(\r?\n|$)/.exec(rest);
      if (m) {
        var fm = rest.slice(0, m.index), lines = fm.split(/\r?\n/);
        body = rest.slice(m.index + m[0].length);
        for (var i = 0; i < lines.length; i++) {
          var kv = /^\s*([A-Za-z_][A-Za-z0-9_]*)\s*:\s*(.*?)\s*$/.exec(lines[i]);
          if (!kv) { continue; }
          var known = false;
          for (var k = 0; k < KEYS.length; k++) { if (KEYS[k] === kv[1]) { known = true; } }
          if (known) { meta[kv[1]] = kv[2]; } else { extra.push([kv[1], kv[2]]); }
        }
      }
    }
    var lang = meta.lang || 'both';
    meta.lang = lang;
    var bodies = { fr: '', en: '' };
    if (lang === 'both') { bodies = splitBlocks(body); }
    else { bodies[lang === 'en' ? 'en' : 'fr'] = trimNl(body); }
    return { meta: meta, extra: extra, bodies: bodies };
  }

  function oneLine(v) { return String(v).replace(/[\r\n]+/g, ' ').replace(/^\s+|\s+$/g, ''); }

  function serialize(c) {
    var lines = ['---'], i;
    for (i = 0; i < KEYS.length; i++) {
      var v = c.meta[KEYS[i]];
      if (v !== undefined && v !== null && oneLine(v) !== '') { lines.push(KEYS[i] + ': ' + oneLine(v)); }
    }
    for (i = 0; i < c.extra.length; i++) { lines.push(c.extra[i][0] + ': ' + oneLine(c.extra[i][1])); }
    lines.push('---', '');
    var lang = c.meta.lang || 'both', body;
    if (lang === 'both') {
      body = '::: fr\n' + c.bodies.fr.replace(/\s+$/, '') + '\n:::\n\n::: en\n' + c.bodies.en.replace(/\s+$/, '') + '\n:::\n';
    } else {
      body = c.bodies[lang === 'en' ? 'en' : 'fr'].replace(/\s+$/, '') + '\n';
    }
    return lines.join('\n') + '\n' + body;
  }

  // ---- Aperçu : même Markdown que build.py (sous-ensemble), HTML toujours échappé ------
  function esc(t) { return t.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;'); }
  function safeHref(u) {
    u = u.replace(/^\s+|\s+$/g, '');
    if (!/^(https?:\/\/|mailto:|\/|#)/i.test(u) || u.indexOf('//') === 0) { return '#'; }
    return u.replace(/"/g, '&quot;');
  }
  function inline(t) {
    t = esc(t);
    t = t.replace(/\[([^\]]+)\]\(([^)]+)\)/g, function (m, a, b) { return '<a href="' + safeHref(b) + '">' + a + '</a>'; });
    t = t.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
    t = t.replace(/(^|[^*])\*([^*]+)\*(?!\*)/g, '$1<em>$2</em>');
    return t;
  }
  var ACCENTS = { 'à': 'a', 'â': 'a', 'ä': 'a', 'é': 'e', 'è': 'e', 'ê': 'e', 'ë': 'e', 'î': 'i', 'ï': 'i', 'ô': 'o', 'ö': 'o', 'ù': 'u', 'û': 'u', 'ü': 'u', 'ç': 'c', 'ñ': 'n' };
  function slugify(t) {
    t = t.toLowerCase().replace(/[àâäéèêëîïôöùûüçñ]/g, function (c) { return ACCENTS[c]; });
    return t.replace(/[’‘'«»“”"]/g, '').replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '');
  }
  function mdToHtml(text) {
    if (!text) { return ''; }
    var lines = text.split('\n'), parts = [], inList = false, inBq = false, para = [];
    function flushPara() { if (para.length) { parts.push('<p>' + inline(para.join(' ')) + '</p>'); para = []; } }
    function flushList() { if (inList) { parts.push('</ul>'); inList = false; } }
    function flushBq() { if (inBq) { parts.push('</blockquote>'); inBq = false; } }
    for (var i = 0; i < lines.length; i++) {
      var s = lines[i].replace(/^\s+|\s+$/g, '');
      if (!s) { flushPara(); continue; }
      if (s.indexOf('## ') === 0) { flushPara(); flushList(); flushBq(); var h = s.slice(3); parts.push('<h2 id="' + slugify(h) + '">' + inline(h) + '</h2>'); continue; }
      if (s.indexOf('- ') === 0 || s.indexOf('* ') === 0) {
        flushPara(); flushBq();
        if (!inList) { parts.push('<ul>'); inList = true; }
        parts.push('<li>' + inline(s.slice(2)) + '</li>');
        continue;
      }
      if (s.indexOf('> ') === 0) {
        flushPara(); flushList();
        if (!inBq) { parts.push('<blockquote class="modele">'); inBq = true; }
        parts.push('<p>' + inline(s.slice(2)) + '</p>');
        continue;
      }
      flushList(); flushBq();
      para.push(s);
    }
    flushPara(); flushList(); flushBq();
    return parts.join('\n');
  }

  // ---- État ----------------------------------------------------------------------
  var S = { me: null, articles: [], props: [], cur: null, dirty: false, tab: 'fr', siteTimer: null };
  function isEditor() { return !!S.me && (S.me.role === 'editor' || S.me.role === 'admin'); }
  function setDirty(v) { S.dirty = v; renderState(); }

  w.addEventListener('beforeunload', function (e) {
    if (!S.dirty) { return; }
    e.preventDefault();
    e.returnValue = '';
    return '';
  });
  function confirmDiscard(next) {
    if (!S.dirty || w.confirm(T('Vous avez des modifications non enregistrées. Les abandonner ?', 'You have unsaved changes. Discard them?'))) { S.dirty = false; next(); }
  }

  // ---- Listes ----------------------------------------------------------------------
  function titleOf(a) {
    if (fr()) { return a.title_fr || a.title_en || a.slug; }
    return a.title_en || a.title_fr || a.slug;
  }
  function itemButton(label, sub, badge, active, onclick) {
    var li = d.createElement('li'), b = d.createElement('button');
    b.type = 'button';
    b.className = 'ed-item' + (active ? ' actif' : '');
    var t = d.createElement('span'); t.className = 'ed-item-titre'; t.textContent = label; b.appendChild(t);
    var s = d.createElement('span'); s.className = 'ed-item-sous'; s.textContent = sub; b.appendChild(s);
    if (badge) { var g = d.createElement('span'); g.className = 'ed-badge'; g.textContent = badge; b.appendChild(g); }
    on(b, 'click', onclick);
    li.appendChild(b);
    return li;
  }
  function renderLists() {
    var ul = $('l-articles'), i;
    ul.innerHTML = '';
    var mine = {};
    for (i = 0; i < S.props.length; i++) { if (S.me && S.props[i].user === S.me.username) { mine[S.props[i].slug] = true; } }
    for (i = 0; i < S.articles.length; i++) {
      (function (a) {
        var active = S.cur && S.cur.kind === 'main' && S.cur.slug === a.slug;
        ul.appendChild(itemButton(titleOf(a), a.slug, mine[a.slug] ? T('proposition', 'suggestion') : '', active, function () { openArticle(a.slug); }));
      })(S.articles[i]);
    }
    var pl = $('l-props');
    pl.innerHTML = '';
    $('t-props').hidden = !S.props.length;
    for (i = 0; i < S.props.length; i++) {
      (function (p) {
        var active = S.cur && S.cur.kind === 'proposal' && S.cur.slug === p.slug && S.cur.owner === p.user;
        var sub = (isEditor() ? p.user + ' · ' : '') + p.slug;
        pl.appendChild(itemButton(p.title || p.slug, sub, p.is_new ? T('nouveau', 'new') : '', active, function () { openProposal(p.user, p.slug); }));
      })(S.props[i]);
    }
    var dl = $('dl-articles');
    dl.innerHTML = '';
    for (i = 0; i < S.articles.length; i++) { var o = d.createElement('option'); o.value = S.articles[i].slug; dl.appendChild(o); }
  }
  function loadLists(cb) {
    api('GET', '/api/articles', null, function (err, data) {
      if (err) { fail(err.code); if (cb) { cb(false); } return; }
      S.articles = data.articles || [];
      api('GET', '/api/proposals', null, function (err2, data2) {
        S.props = err2 ? [] : (data2.proposals || []);
        renderLists();
        if (cb) { cb(true); }
      });
    });
  }

  // ---- Statut du site ----------------------------------------------------------------
  function showSite(st) {
    var el = $('ed-site');
    if (!st) { el.textContent = ''; return; }
    if (st.ok) { el.textContent = T('Site à jour', 'Site up to date') + (st.commit ? ' (' + st.commit.slice(0, 7) + ')' : ''); }
    else { el.textContent = T('La dernière construction du site a échoué : l\'ancienne version reste en ligne. ', 'The last site build failed: the previous version stays online. ') + (st.error || ''); }
  }
  function pollSite(commit, tries) {
    if (S.siteTimer) { w.clearTimeout(S.siteTimer); S.siteTimer = null; }
    api('GET', '/api/site', null, function (err, st) {
      if (err) { return; }
      showSite(st);
      var done = !commit || st.commit === commit || (!st.ok && st.error);
      if (!done && tries > 0) { S.siteTimer = w.setTimeout(function () { pollSite(commit, tries - 1); }, 2000); }
    });
  }

  // ---- Formulaire ---------------------------------------------------------------------
  function fields() { return $('f-meta').querySelectorAll('[data-k]'); }
  function applyLang() {
    var lang = S.cur.meta.lang || 'both';
    each($('f-meta').querySelectorAll('[data-l]'), function (el) {
      var l = el.getAttribute('data-l');
      el.hidden = !(lang === 'both' || lang === l);
    });
    var tabs = $('ed-langues');
    tabs.hidden = lang !== 'both';
    if (lang === 'fr') { S.tab = 'fr'; } else if (lang === 'en') { S.tab = 'en'; }
    each(tabs.querySelectorAll('[data-tab]'), function (b) { b.setAttribute('aria-selected', b.getAttribute('data-tab') === S.tab ? 'true' : 'false'); });
  }
  function fillForm() {
    var c = S.cur;
    each(fields(), function (el) {
      var k = el.getAttribute('data-k');
      el.value = c.meta[k] === undefined ? '' : c.meta[k];
    });
    $('m-slug').readOnly = c.kind !== 'new';
    applyLang();
    $('ed-texte').value = c.bodies[S.tab] || '';
    renderPreview();
  }
  function readForm() {
    each(fields(), function (el) { S.cur.meta[el.getAttribute('data-k')] = el.value; });
  }
  function renderPreview() {
    var c = S.cur, box = $('ed-apercu');
    if (!c) { return; }
    var tab = S.tab, h = '';
    var title = c.meta['title_' + tab] || '', lead = c.meta['chapeau_' + tab] || '';
    box.innerHTML = '';
    var t = d.createElement('h2'); t.className = 'moyen ed-apercu-titre'; t.textContent = title || T('(sans titre)', '(untitled)'); box.appendChild(t);
    if (lead) { var p = d.createElement('p'); p.className = 'chapeau'; p.textContent = lead; box.appendChild(p); }
    var holder = d.createElement('div');
    h = mdToHtml(c.bodies[tab] || '');
    holder.innerHTML = h;
    box.appendChild(holder);
  }

  function renderState() {
    var c = S.cur;
    if (!c) { return; }
    var el = $('ed-etat'), editor = isEditor(), txt;
    var mine = S.me && c.owner === S.me.username;
    if (c.kind === 'new') {
      txt = editor ? T('Nouvel article : sera publié sur main.', 'New article: will be published on main.')
        : T('Nouvel article : sera enregistré comme proposition, un éditeur le relira.', 'New article: saved as a suggestion, an editor will review it.');
    } else if (c.kind === 'main') {
      txt = editor ? T('Article publié (branche main). Vos enregistrements sont publiés immédiatement.', 'Published article (main branch). Your saves are published immediately.')
        : T('Article publié. Vos modifications seront enregistrées comme proposition à relire.', 'Published article. Your changes will be saved as a suggestion for review.');
    } else {
      txt = mine ? T('Votre proposition (branche users/', 'Your suggestion (branch users/') + c.owner + '/' + c.slug + T('), en attente de relecture.', '), awaiting review.')
        : T('Proposition de ', 'Suggestion by ') + c.owner + T(' (lecture seule : publiez-la ou modifiez-la avant).', ' (read-only: publish it or edit it first).');
    }
    el.textContent = txt + (S.dirty ? ' ' + T('● Modifications non enregistrées', '● Unsaved changes') : '');
    el.className = 'ed-etat' + (S.dirty ? ' modifie' : '');
    var save = $('b-save');
    var canSave = c.kind === 'new' || c.kind === 'main' || (c.kind === 'proposal' && mine);
    save.hidden = !canSave;
    if (c.kind === 'proposal') { save.textContent = T('Enregistrer la proposition', 'Save suggestion'); }
    else { save.textContent = editor ? T('Publier', 'Publish') : T('Proposer', 'Suggest'); }
    $('b-publish').hidden = !(c.kind === 'proposal' && editor);
    $('b-editmain').hidden = !(c.kind === 'proposal' && editor && !mine);
    $('b-history').hidden = c.kind !== 'main';
    var del = $('b-delete');
    if (c.kind === 'main' && editor) { del.hidden = false; del.textContent = T('Supprimer l\'article', 'Delete article'); }
    else if (c.kind === 'proposal') { del.hidden = false; del.textContent = mine ? T('Retirer ma proposition', 'Withdraw my suggestion') : T('Rejeter la proposition', 'Reject suggestion'); }
    else { del.hidden = true; }
    var ro = c.kind === 'proposal' && !mine;
    $('ed-texte').readOnly = ro;
    each(fields(), function (f) { if (f.id !== 'm-slug') { f.disabled = ro; } });
    $('ed-msg').disabled = ro;
  }

  function show() {
    $('ed-vide').hidden = true;
    $('ed-zone').hidden = false;
    $('d-histo').hidden = true;
    $('ed-msg').value = '';
    clearMsg();
    S.tab = (S.cur.meta.lang === 'en') ? 'en' : 'fr';
    fillForm();
    S.dirty = false;
    $('d-meta').open = S.cur.kind === 'new';
    renderState();
    renderLists();
    try { w.history.replaceState(null, '', S.cur.kind === 'new' ? '#' : '#' + S.cur.slug); } catch (e) {}
    if (w.innerWidth < 1024 && $('ed-zone').scrollIntoView) { $('ed-zone').scrollIntoView(); }
  }

  // ---- Ouvrir, créer ----------------------------------------------------------------------
  function fromContent(content) { return parseArticle(content); }

  function openArticle(slug, after) {
    confirmDiscard(function () {
      api('GET', '/api/articles/' + encodeURIComponent(slug), null, function (err, data) {
        if (err) { fail(err.code); return; }
        var c = fromContent(data.content);
        c.kind = 'main'; c.slug = slug; c.version = data.version; c.owner = null; c.mainVersion = data.version;
        S.cur = c;
        show();
        if (after) { after(); }
      });
    });
  }

  function openProposal(owner, slug) {
    confirmDiscard(function () {
      api('GET', '/api/proposals/' + encodeURIComponent(owner) + '/' + encodeURIComponent(slug), null, function (err, data) {
        if (err) { fail(err.code); return; }
        var c = fromContent(data.content);
        c.kind = 'proposal'; c.slug = slug; c.owner = owner; c.version = data.version; c.mainVersion = data.main_version;
        S.cur = c;
        show();
      });
    });
  }

  function newArticle() {
    confirmDiscard(function () {
      var max = 0;
      for (var i = 0; i < S.articles.length; i++) { if (S.articles[i].order > max && S.articles[i].order < 999) { max = S.articles[i].order; } }
      S.cur = {
        kind: 'new', slug: '', version: null, owner: null, mainVersion: null, extra: [],
        meta: { order: String(max + 1), lang: 'both', slug: '' },
        bodies: { fr: '## Titre de section\n\nVotre texte ici.', en: '## Section title\n\nYour text here.' }
      };
      show();
      $('m-slug').focus();
    });
  }

  // ---- Enregistrer ---------------------------------------------------------------------------
  var SLUG_RE = /^[a-z0-9][a-z0-9-]*$/;

  function save() {
    var c = S.cur;
    if (!c) { return; }
    readForm();
    clearMsg();
    var slug = c.kind === 'new' ? c.meta.slug : c.slug;
    if (!SLUG_RE.test(slug || '') || slug.length > 64) { fail('invalid_slug'); $('m-slug').focus(); return; }
    c.meta.slug = slug;
    var content = serialize(c), editor = isEditor(), url, body = { content: content, message: $('ed-msg').value };
    var asProposal = c.kind === 'proposal' || !editor;
    if (asProposal) { url = '/api/proposals/' + encodeURIComponent(slug); } else { url = '/api/articles/' + encodeURIComponent(slug); }
    if (c.kind !== 'new') { body.base = c.version; }
    var btn = $('b-save');
    btn.disabled = true;
    api('PUT', url, body, function (err, data) {
      btn.disabled = false;
      if (err) {
        if (err.code === 'edit_conflict') { $('ed-conflit').hidden = false; $('ed-conflit').removeAttribute('hidden'); return; }
        fail(err.code);
        return;
      }
      var before = c.kind;
      if (before === 'new') {
        c.slug = slug;
        if (asProposal) { c.kind = 'proposal'; c.owner = S.me.username; c.mainVersion = null; } else { c.kind = 'main'; c.mainVersion = data.version; }
      } else if (before === 'main' && asProposal) {
        c.kind = 'proposal'; c.owner = S.me.username;
      } else if (before === 'main') {
        c.mainVersion = data.version;
      }
      c.version = data.version;
      S.dirty = false;
      $('m-slug').readOnly = true;
      renderState();
      if (asProposal) { ok(T('Proposition enregistrée. Un éditeur la relira avant publication.', 'Suggestion saved. An editor will review it before publication.')); }
      else { ok(T('Publié sur main (commit ', 'Published on main (commit ') + data.commit.slice(0, 7) + T('). Le site se reconstruit…', '). The site is rebuilding…')); pollSite(data.commit, 15); }
      loadLists();
      try { w.history.replaceState(null, '', '#' + slug); } catch (e) {}
    });
  }

  // Conflit : recharger ou écraser.
  function latest(cb) {
    var c = S.cur;
    if (c.kind === 'proposal') {
      api('GET', '/api/proposals/' + encodeURIComponent(c.owner) + '/' + encodeURIComponent(c.slug), null, cb);
    } else {
      api('GET', '/api/articles/' + encodeURIComponent(c.kind === 'new' ? (c.meta.slug || '') : c.slug), null, cb);
    }
  }
  function reload() {
    var c = S.cur;
    S.dirty = false;
    if (c.kind === 'proposal') { openProposal(c.owner, c.slug); } else { openArticle(c.slug); }
  }
  function overwrite() {
    var c = S.cur;
    latest(function (err, data) {
      if (err && err.code !== 'not_found') { fail(err.code); return; }
      c.version = err ? null : data.version;
      if (c.kind === 'main') { c.mainVersion = c.version; }
      $('ed-conflit').hidden = true;
      save();
    });
  }

  // ---- Publier / rejeter / supprimer ---------------------------------------------------------------
  function publish() {
    var c = S.cur;
    if (!c || c.kind !== 'proposal') { return; }
    if (S.dirty) { failText(T('Enregistrez ou abandonnez vos modifications avant de publier.', 'Save or discard your changes before publishing.')); return; }
    api('POST', '/api/proposals/' + encodeURIComponent(c.owner) + '/' + encodeURIComponent(c.slug) + '/publish', { main_base: c.mainVersion || null }, function (err, data) {
      if (err) {
        if (err.code === 'edit_conflict') { failText(T('L\'article publié a changé depuis la proposition : rouvrez-la (« Modifier avant de publier ») pour fusionner.', 'The published article changed since the suggestion: reopen it ("Edit before publishing") to merge.')); }
        else { fail(err.code); }
        return;
      }
      ok(T('Proposition publiée sur main (commit ', 'Suggestion published on main (commit ') + data.commit.slice(0, 7) + T('). Le site se reconstruit…', '). The site is rebuilding…'));
      pollSite(data.commit, 15);
      var slug = c.slug;
      S.cur = null; $('ed-zone').hidden = true; $('ed-vide').hidden = false;
      loadLists();
      try { w.history.replaceState(null, '', '#'); } catch (e) {}
      slug = null;
    });
  }
  function editBeforePublish() {
    var c = S.cur;
    if (!c || c.kind !== 'proposal') { return; }
    api('GET', '/api/articles/' + encodeURIComponent(c.slug), null, function (err, data) {
      if (err && err.code !== 'not_found') { fail(err.code); return; }
      c.kind = 'main'; c.owner = null; c.version = err ? null : data.version; c.mainVersion = c.version;
      if (err) { c.kind = 'new'; }
      show();
      S.dirty = true; renderState();
      ok(T('Vous modifiez la version publiée avec le texte de la proposition : « Publier » l\'enregistre sur main.', 'You are editing the published version with the suggestion\'s text: "Publish" saves it on main.'));
    });
  }
  function remove() {
    var c = S.cur;
    if (!c) { return; }
    if (c.kind === 'proposal') {
      if (!w.confirm(T('Retirer cette proposition ? Elle sera supprimée.', 'Remove this suggestion? It will be deleted.'))) { return; }
      api('DELETE', '/api/proposals/' + encodeURIComponent(c.owner) + '/' + encodeURIComponent(c.slug), null, function (err) {
        if (err) { fail(err.code); return; }
        S.dirty = false; S.cur = null; $('ed-zone').hidden = true; $('ed-vide').hidden = false;
        ok(T('Proposition supprimée.', 'Suggestion deleted.'));
        loadLists();
      });
    } else if (c.kind === 'main') {
      if (!w.confirm(T('Supprimer définitivement cet article du site ? L\'historique git le conserve.', 'Delete this article from the site? The git history keeps it.'))) { return; }
      api('DELETE', '/api/articles/' + encodeURIComponent(c.slug) + '?base=' + encodeURIComponent(c.version), null, function (err, data) {
        if (err) { if (err.code === 'edit_conflict') { $('ed-conflit').hidden = false; } else { fail(err.code); } return; }
        S.dirty = false; S.cur = null; $('ed-zone').hidden = true; $('ed-vide').hidden = false;
        ok(T('Article supprimé (commit ', 'Article deleted (commit ') + data.commit.slice(0, 7) + ').');
        pollSite(data.commit, 15);
        loadLists();
      });
    }
  }

  // ---- Historique ----------------------------------------------------------------------------------------
  function when(t) {
    var dt = new Date(t * 1000);
    try { return dt.toLocaleString(fr() ? 'fr-FR' : 'en-GB', { dateStyle: 'medium', timeStyle: 'short' }); }
    catch (e) { return dt.toISOString().slice(0, 16).replace('T', ' '); }
  }
  function history() {
    var c = S.cur;
    if (!c || c.kind !== 'main') { return; }
    api('GET', '/api/articles/' + encodeURIComponent(c.slug) + '/history', null, function (err, data) {
      if (err) { fail(err.code); return; }
      var ul = $('l-histo'), revs = data.revisions || [];
      ul.innerHTML = '';
      for (var i = 0; i < revs.length; i++) {
        (function (r) {
          var li = d.createElement('li');
          var info = d.createElement('span');
          info.textContent = when(r.time) + ' · ' + r.author + ' · ' + (r.message || '') + ' · ' + r.commit.slice(0, 7);
          var b = d.createElement('button'); b.type = 'button'; b.className = 'bouton secondaire petit';
          b.textContent = T('Charger cette version', 'Load this version');
          on(b, 'click', function () { loadRevision(r.commit); });
          li.appendChild(info); li.appendChild(b); ul.appendChild(li);
        })(revs[i]);
      }
      if (!revs.length) { var e = d.createElement('li'); e.textContent = T('Aucune révision.', 'No revisions.'); ul.appendChild(e); }
      $('d-histo').hidden = false; $('d-histo').open = true;
    });
  }
  function loadRevision(commit) {
    var c = S.cur;
    api('GET', '/api/articles/' + encodeURIComponent(c.slug) + '/revisions/' + commit, null, function (err, data) {
      if (err) { fail(err.code); return; }
      var r = fromContent(data.content);
      c.meta = r.meta; c.extra = r.extra; c.bodies = r.bodies; c.meta.slug = c.slug;
      S.tab = (c.meta.lang === 'en') ? 'en' : 'fr';
      fillForm();
      S.dirty = true; renderState();
      ok(T('Version ' + commit.slice(0, 7) + ' chargée : enregistrez pour la rétablir.', 'Version ' + commit.slice(0, 7) + ' loaded: save to restore it.'));
    });
  }

  // ---- Barre d'outils Markdown --------------------------------------------------------------------------------
  function onText() {
    if (!S.cur) { return; }
    S.cur.bodies[S.tab] = $('ed-texte').value;
    setDirty(true);
    renderPreview();
  }
  function wrap(before, after, placeholder) {
    var t = $('ed-texte'), s = t.selectionStart, e = t.selectionEnd, v = t.value;
    var sel = v.slice(s, e) || placeholder;
    t.value = v.slice(0, s) + before + sel + after + v.slice(e);
    t.focus();
    t.setSelectionRange(s + before.length, s + before.length + sel.length);
    onText();
  }
  function linePrefix(prefix) {
    var t = $('ed-texte'), v = t.value, s = t.selectionStart, e = t.selectionEnd;
    var a = v.lastIndexOf('\n', s - 1) + 1, b = v.indexOf('\n', e);
    if (b < 0) { b = v.length; }
    var lines = v.slice(a, b).split('\n'), all = true, i;
    for (i = 0; i < lines.length; i++) { if (lines[i].indexOf(prefix) !== 0) { all = false; } }
    for (i = 0; i < lines.length; i++) { lines[i] = all ? lines[i].slice(prefix.length) : prefix + lines[i]; }
    var out = lines.join('\n');
    t.value = v.slice(0, a) + out + v.slice(b);
    t.focus();
    t.setSelectionRange(a, a + out.length);
    onText();
  }
  function tool(act) {
    if (act === 'h2') { linePrefix('## '); }
    else if (act === 'bold') { wrap('**', '**', T('gras', 'bold')); }
    else if (act === 'italic') { wrap('*', '*', T('italique', 'italic')); }
    else if (act === 'link') { wrap('[', '](https://)', T('texte du lien', 'link text')); }
    else if (act === 'list') { linePrefix('- '); }
    else if (act === 'quote') { linePrefix('> '); }
  }

  // ---- Évènements ----------------------------------------------------------------------------------------------------
  on($('b-nouveau'), 'click', newArticle);
  on($('b-save'), 'click', save);
  on($('b-publish'), 'click', publish);
  on($('b-editmain'), 'click', editBeforePublish);
  on($('b-history'), 'click', history);
  on($('b-delete'), 'click', remove);
  on($('b-reload'), 'click', reload);
  on($('b-overwrite'), 'click', overwrite);
  on($('ed-texte'), 'input', onText);
  on($('f-meta'), 'input', function (e) {
    if (!S.cur) { return; }
    var el = e.target, k = el && el.getAttribute && el.getAttribute('data-k');
    if (!k) { return; }
    S.cur.meta[k] = el.value;
    if (k === 'lang') { applyLang(); $('ed-texte').value = S.cur.bodies[S.tab] || ''; }
    renderPreview();
    setDirty(true);
  });
  each($('ed-langues').querySelectorAll('[data-tab]'), function (b) {
    on(b, 'click', function () {
      S.tab = b.getAttribute('data-tab');
      each($('ed-langues').querySelectorAll('[data-tab]'), function (o) { o.setAttribute('aria-selected', o === b ? 'true' : 'false'); });
      $('ed-texte').value = S.cur.bodies[S.tab] || '';
      renderPreview();
    });
  });
  each($('ed-vues').querySelectorAll('[data-vue]'), function (b) {
    on(b, 'click', function () {
      var v = b.getAttribute('data-vue');
      $('ed-ecran').setAttribute('data-vue', v);
      each($('ed-vues').querySelectorAll('[data-vue]'), function (o) { o.setAttribute('aria-pressed', o === b ? 'true' : 'false'); });
    });
  });
  each(d.querySelectorAll('.ed-outil'), function (b) { on(b, 'click', function () { tool(b.getAttribute('data-act')); }); });
  on($('ed-texte'), 'keydown', function (e) {
    if (!(e.ctrlKey || e.metaKey)) { return; }
    var k = (e.key || '').toLowerCase();
    if (k === 'b') { e.preventDefault(); tool('bold'); }
    else if (k === 'i') { e.preventDefault(); tool('italic'); }
    else if (k === 'k') { e.preventDefault(); tool('link'); }
  });
  d.addEventListener('keydown', function (e) {
    if ((e.ctrlKey || e.metaKey) && (e.key || '').toLowerCase() === 's' && S.cur && !$('b-save').hidden) { e.preventDefault(); save(); }
  }, false);
  w.addEventListener('hashchange', function () {
    var h = (w.location.hash || '').replace(/^#/, '');
    if (h && (!S.cur || S.cur.slug !== h)) { openArticle(h); }
  }, false);

  // ---- Démarrage -------------------------------------------------------------------------------------------------------------
  api('GET', '/api/me', null, function (err, me) {
    if (err) {
      if (err.status === 401) { $('ed-garde').hidden = false; $('ed-garde').removeAttribute('hidden'); }
      else { fail(err.code); }
      return;
    }
    S.me = me;
    $('ed-qui').textContent = T('Connecté en tant que ', 'Logged in as ') + me.username + ' · ' + (isEditor() ? T('éditeur', 'editor') : T('compte simple (vos modifications sont des propositions)', 'regular account (your changes are suggestions)'));
    $('ed-grille').hidden = false; $('ed-grille').removeAttribute('hidden');
    loadLists(function (okLists) {
      if (!okLists) { return; }
      var h = (w.location.hash || '').replace(/^#/, '');
      if (h === 'nouveau' || h === 'new') { newArticle(); }
      else if (h) { openArticle(h); }
    });
    api('GET', '/api/site', null, function (e2, st) { if (!e2) { showSite(st); } });
  });
})();
