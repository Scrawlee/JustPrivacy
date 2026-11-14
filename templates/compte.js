/* Pages de compte (connexion, inscription, réglages). Seules pages du site qui exigent JavaScript.
   ES5 : si le navigateur est trop ancien, le message « mettez à jour / activez JavaScript » reste affiché. */
(function () {
  var d = document, w = window;
  function $(id) { return d.getElementById(id); }
  var app = $('app');
  if (!app) { return; }
  app.hidden = false;
  app.removeAttribute('hidden');

  function fr() { return (d.documentElement.lang || '').slice(0, 2) === 'fr'; }
  function T(a, b) { return fr() ? a : b; }

  var ERR = {
    invalid_username: ["Nom d'utilisateur invalide : 3 à 32 caractères (lettres, chiffres, point, tiret, tiret bas), commençant par une lettre ou un chiffre.", 'Invalid username: 3 to 32 characters (letters, digits, dot, dash, underscore), starting with a letter or digit.'],
    username_reserved: ["Ce nom d'utilisateur est réservé.", 'This username is reserved.'],
    username_taken: ["Ce nom d'utilisateur est déjà pris.", 'This username is already taken.'],
    username_cooldown: ["Vous avez déjà changé de nom d'utilisateur il y a moins de 30 jours.", 'You changed your username less than 30 days ago.'],
    password_too_short: ['Mot de passe trop court (10 caractères minimum).', 'Password too short (10 characters minimum).'],
    password_too_long: ['Mot de passe trop long (256 caractères maximum).', 'Password too long (256 characters maximum).'],
    password_is_username: ["Le mot de passe ne doit pas être le nom d'utilisateur.", 'The password must not be the username.'],
    passwords_differ: ['Les deux mots de passe ne correspondent pas.', 'The two passwords do not match.'],
    invalid_credentials: ["Nom d'utilisateur ou mot de passe incorrect.", 'Wrong username or password.'],
    account_locked: ['Trop d\'échecs : compte verrouillé 10 minutes.', 'Too many failures: account locked for 10 minutes.'],
    rate_limited: ['Trop de tentatives, réessayez dans quelques minutes.', 'Too many attempts, try again in a few minutes.'],
    captcha_required: ['Terminez d\'abord la vérification anti-robot.', 'Please complete the anti-bot check first.'],
    captcha_failed: ['La vérification anti-robot a échoué, réessayez.', 'The anti-bot check failed, please try again.'],
    captcha_unavailable: ['Le service de vérification est indisponible, réessayez plus tard.', 'The verification service is unavailable, try again later.'],
    captcha_load: ['La vérification anti-robot (Cap) n\'a pas pu se charger. Vérifiez votre connexion ou désactivez votre bloqueur pour cette page, puis rechargez.', 'The anti-bot check (Cap) could not load. Check your connection or disable your blocker for this page, then reload.'],
    ticket_expired: ['Connexion expirée ou trop d\'essais : recommencez.', 'Login expired or too many attempts: start again.'],
    invalid_code: ['Code incorrect.', 'Wrong code.'],
    invalid_passkey: ['Passkey refusée.', 'Passkey rejected.'],
    no_passkey: ['Aucune passkey enregistrée.', 'No passkey registered.'],
    wrong_password: ['Mot de passe actuel incorrect.', 'Current password is wrong.'],
    password_required: ['Saisissez votre mot de passe.', 'Enter your password.'],
    totp_already_enabled: ['Le code OTP est déjà activé.', 'OTP is already enabled.'],
    totp_not_started: ['Recommencez l\'activation.', 'Start the setup again.'],
    too_many_passkeys: ['Nombre maximal de passkeys atteint.', 'Maximum number of passkeys reached.'],
    too_many_tokens: ['Nombre maximal de jetons atteint.', 'Maximum number of tokens reached.'],
    ceremony_expired: ['Délai dépassé, recommencez.', 'Timed out, please try again.'],
    no_second_factor: ['Activez d\'abord un second facteur.', 'Enable a second factor first.'],
    invalid_name: ['Nom invalide.', 'Invalid name.'],
    unauthenticated: ['Session expirée.', 'Session expired.'],
    bad_origin: ['Requête refusée (origine).', 'Request refused (origin).'],
    network: ['Serveur injoignable.', 'Server unreachable.'],
    cancelled: ['Opération annulée.', 'Operation cancelled.']
  };
  function errText(code) { var e = ERR[code]; return e ? T(e[0], e[1]) : T('Une erreur est survenue (' + code + ').', 'Something went wrong (' + code + ').'); }

  var msg = $('msg'), okBox = $('ok'), msgTimer = null;
  function fail(code) {
    msg.textContent = errText(code);
    msg.hidden = false; msg.removeAttribute('hidden');
    if (okBox) { okBox.hidden = true; }
    if (msgTimer) { w.clearTimeout(msgTimer); }
    if (app.querySelector('.cpt')) { msgTimer = w.setTimeout(clearMsg, 10000); }
  }
  function clearMsg() { msg.hidden = true; if (okBox) { okBox.hidden = true; } }
  function good(text) {
    if (!okBox) { return; }
    msg.hidden = true;
    okBox.textContent = text; okBox.hidden = false; okBox.removeAttribute('hidden');
    if (msgTimer) { w.clearTimeout(msgTimer); }
    if (app.querySelector('.cpt')) { msgTimer = w.setTimeout(clearMsg, 6000); }
  }

  function api(method, path, body, cb) {
    var x = new XMLHttpRequest();
    x.open(method, path, true);
    x.setRequestHeader('Accept', 'application/json');
    if (body !== null) { x.setRequestHeader('Content-Type', 'application/json'); }
    x.onreadystatechange = function () {
      if (x.readyState !== 4) { return; }
      var data = null;
      try { data = JSON.parse(x.responseText); } catch (e) {}
      if (x.status >= 200 && x.status < 300) { cb(null, data || {}); }
      else if (x.status === 0) { cb('network', null); }
      else { cb((data && data.error) || 'internal', data, x.status); }
    };
    x.send(body === null ? null : JSON.stringify(body));
  }

  function el(tag, text, cls) {
    var e = d.createElement(tag);
    if (text !== undefined && text !== null) { e.appendChild(d.createTextNode(text)); }
    if (cls) { e.className = cls; }
    return e;
  }
  function busy(form, on) {
    var bs = form.querySelectorAll('button');
    for (var i = 0; i < bs.length; i++) { bs[i].disabled = on; }
  }
  function on(id, ev, fn) { var e = $(id); if (e) { e.addEventListener(ev, fn, false); } }

  // ---- Cap -----------------------------------------------------------------
  // Interface de la case anti-robot : la nôtre. Le calcul est celui de Cap (script servi par check.scrawlee.com).
  var capToken = '', capEl = $('cap'), capObj = null, capBusy = false;
  var CAPT = {
    idle: ['Je suis humain', 'I am human'],
    work: ['Vérification…', 'Verifying…'],
    done: ['Vérifié', 'Verified'],
    fail: ['Échec, réessayer', 'Failed, try again']
  };
  function capEvent(type, detail) {
    var ev;
    try { ev = new w.CustomEvent(type, { detail: detail }); }
    catch (e) { ev = d.createEvent('CustomEvent'); ev.initCustomEvent(type, false, false, detail); }
    capEl.dispatchEvent(ev);
  }
  var CAP_CIRC = 56.55; // 2 * pi * 9 : longueur de l'anneau de progression
  function capRing(p) {
    var ring = capEl.querySelector('.cap-anneau');
    if (ring) { ring.style.strokeDashoffset = String(CAP_CIRC * (1 - p)); }
  }
  function capShow(state) {
    capEl.setAttribute('data-state', state);
    capEl.querySelector('.cap-bouton').setAttribute('aria-checked', state === 'done' ? 'true' : 'false');
    capEl.querySelector('.cap-etat .fr').textContent = CAPT[state][0];
    capEl.querySelector('.cap-etat .en').textContent = CAPT[state][1];
    // Anneau : vide au repos, rempli quand c'est fini ou en échec, suit la progression pendant le calcul.
    capRing(state === 'work' ? 0.12 : (state === 'idle' ? 0 : 1));
  }
  function capSolve() {
    if (capBusy || capEl.getAttribute('data-state') === 'done') { return; }
    capBusy = true; capToken = ''; capShow('work');
    try {
      if (!capObj) {
        capObj = new w.Cap({ apiEndpoint: capEl.getAttribute('data-endpoint') });
        capObj.addEventListener('progress', function (ev) {
          var n = (ev && ev.detail && ev.detail.progress) || 0;
          if (capBusy) { capRing(Math.max(0.12, Math.min(1, n / 100))); }
        });
      }
      capObj.solve().then(function (r) {
        capBusy = false;
        if (r && r.token) { capRing(1); capShow('done'); capEvent('solve', { token: r.token }); }
        else { capShow('idle'); }
      }, function () {
        capBusy = false; capShow('fail'); capEvent('error', {});
      });
    } catch (err) { capBusy = false; capShow('fail'); fail('captcha_load'); }
  }
  if (capEl) {
    // The check script (served by check.scrawlee.com) did not load: send the visitor to the dedicated page.
    if (!w.Cap) { w.location.replace('/compatibilite'); return; }
    capEl.reset = function () {
      capToken = ''; capBusy = false;
      if (capObj && typeof capObj.reset === 'function') { try { capObj.reset(); } catch (err) {} }
      capShow('idle'); capEvent('reset', {});
    };
    capEl.querySelector('.cap-bouton').addEventListener('click', capSolve, false);
    capEl.addEventListener('solve', function (e) { capToken = (e.detail && e.detail.token) || ''; }, false);
    capEl.addEventListener('reset', function () { capToken = ''; }, false);
    capEl.addEventListener('error', function () { capToken = ''; }, false);
  }
  function capReset() { capToken = ''; if (capEl && typeof capEl.reset === 'function') { try { capEl.reset(); } catch (e) {} } }
  function capValue() {
    if (capToken) { return capToken; }
    var h = d.querySelector('input[name="cap-token"]');
    return h ? h.value : '';
  }

  // ---- WebAuthn (base64url <-> ArrayBuffer) ----------------------------------
  var hasPasskeys = !!(w.PublicKeyCredential && navigator.credentials && w.Promise && w.Uint8Array && w.atob);
  function b2buf(s) {
    s = s.replace(/-/g, '+').replace(/_/g, '/');
    while (s.length % 4) { s += '='; }
    var bin = w.atob(s), a = new Uint8Array(bin.length);
    for (var i = 0; i < bin.length; i++) { a[i] = bin.charCodeAt(i); }
    return a.buffer;
  }
  function buf2b(b) {
    var a = new Uint8Array(b), s = '';
    for (var i = 0; i < a.length; i++) { s += String.fromCharCode(a[i]); }
    return w.btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  }
  function ext(c) { try { return c.getClientExtensionResults(); } catch (e) { return {}; } }
  function waErr(e) { return (e && e.name === 'NotAllowedError') ? 'cancelled' : 'invalid_passkey'; }

  // ============================== CONNEXION =====================================
  var fLogin = $('f-login');
  if (fLogin) {
    var ticket = '';
    var fMfa = $('f-mfa');
    fLogin.addEventListener('submit', function (e) {
      e.preventDefault(); clearMsg();
      var u = $('username').value, p = $('password').value;
      if (!u || !p) { fail('invalid_credentials'); return; }
      var tok = capValue();
      if (!tok) { fail('captcha_required'); return; }
      busy(fLogin, true);
      api('POST', '/api/auth/login', { username: u, password: p, cap_token: tok }, function (err, data) {
        capReset(); busy(fLogin, false);
        if (err) { fail(err); return; }
        if (data.mfa) {
          ticket = data.mfa.ticket;
          fLogin.hidden = true; fLogin.style.display = 'none';
          fMfa.hidden = false; fMfa.removeAttribute('hidden');
          $('mfa-passkey').hidden = !(data.mfa.passkey && hasPasskeys);
          $('code').focus();
          return;
        }
        w.location.href = '/compte';
      });
    }, false);
    fMfa.addEventListener('submit', function (e) {
      e.preventDefault(); clearMsg();
      busy(fMfa, true);
      api('POST', '/api/auth/mfa/totp', { ticket: ticket, code: $('code').value }, function (err) {
        busy(fMfa, false);
        if (err) { fail(err); if (err === 'ticket_expired') { w.location.reload(); } return; }
        w.location.href = '/compte';
      });
    }, false);
    on('b-passkey', 'click', function () {
      clearMsg();
      api('POST', '/api/auth/mfa/passkey/start', { ticket: ticket }, function (err, data) {
        if (err) { fail(err); return; }
        var pk = data.publicKey;
        pk.challenge = b2buf(pk.challenge);
        if (pk.allowCredentials) {
          for (var i = 0; i < pk.allowCredentials.length; i++) { pk.allowCredentials[i].id = b2buf(pk.allowCredentials[i].id); }
        }
        navigator.credentials.get({ publicKey: pk }).then(function (c) {
          var r = c.response;
          var cred = { id: c.id, rawId: buf2b(c.rawId), type: c.type, extensions: ext(c), response: {
            authenticatorData: buf2b(r.authenticatorData), clientDataJSON: buf2b(r.clientDataJSON),
            signature: buf2b(r.signature), userHandle: r.userHandle ? buf2b(r.userHandle) : null } };
          api('POST', '/api/auth/mfa/passkey/finish', { ticket: ticket, credential: cred }, function (err2) {
            if (err2) { fail(err2); return; }
            w.location.href = '/compte';
          });
        }, function (e2) { fail(waErr(e2)); });
      });
    });
  }

  // ============================== INSCRIPTION ===================================
  var fSignup = $('f-signup');
  if (fSignup) {
    fSignup.addEventListener('submit', function (e) {
      e.preventDefault(); clearMsg();
      var u = $('username').value, p = $('password').value;
      if (p !== $('password2').value) { fail('passwords_differ'); return; }
      if (p.length < 10) { fail('password_too_short'); return; }
      var tok = capValue();
      if (!tok) { fail('captcha_required'); return; }
      busy(fSignup, true);
      api('POST', '/api/auth/signup', { username: u, password: p, cap_token: tok }, function (err) {
        capReset(); busy(fSignup, false);
        if (err) { fail(err); return; }
        w.location.href = '/compte';
      });
    }, false);
  }

  // ================================ COMPTE ======================================
  if (!$('f-password')) { return; }
  var me = null;

  function after(err, okText) {
    if (err) {
      fail(err);
      if (err === 'unauthenticated') { w.location.href = '/connexion'; }
      return false;
    }
    good(okText);
    load();
    return true;
  }
  function date(s) { if (!s) { return ''; } try { return new Date(s).toLocaleDateString(fr() ? 'fr-FR' : 'en-GB'); } catch (e) { return s; } }
  function clear(node) { while (node.firstChild) { node.removeChild(node.firstChild); } }
  function trim(v) { return String(v).replace(/^\s+|\s+$/g, ''); }

  // ---- Copy to clipboard -------------------------------------------------------------------------
  function copyText(text, done) {
    function fallback() {
      var t = d.createElement('textarea'), okc = false;
      t.value = text; t.setAttribute('readonly', ''); t.style.position = 'fixed'; t.style.opacity = '0';
      d.body.appendChild(t); t.select();
      try { okc = d.execCommand('copy'); } catch (e) {}
      d.body.removeChild(t); done(okc);
    }
    if (w.navigator.clipboard && w.navigator.clipboard.writeText) { w.navigator.clipboard.writeText(text).then(function () { done(true); }, fallback); }
    else { fallback(); }
  }
  function wireCopy(id, getText) {
    on(id, 'click', function () {
      var b = $(id);
      copyText(getText(), function (okc) {
        b.textContent = okc ? T('Copié ✓', 'Copied ✓') : T('Copie impossible', 'Could not copy');
        w.setTimeout(labels, 2000);
      });
    });
  }
  wireCopy('b-copy-token', function () { return $('tk-value').textContent; });
  wireCopy('b-copy-codes', function () {
    var out = [], li = $('codes-list').querySelectorAll('li');
    for (var i = 0; i < li.length; i++) { out.push(li[i].textContent); }
    return out.join('\n');
  });

  function showCodes(codes) {
    var ul = $('codes-list'); clear(ul);
    for (var i = 0; i < codes.length; i++) { var li = el('li'); li.appendChild(el('code', codes[i])); ul.appendChild(li); }
    $('codes').hidden = false; $('codes').removeAttribute('hidden');
  }

  // ---- Show / hide password -----------------------------------------------------------------------
  var eyes = [];
  function addEye(id) {
    var input = $(id), b = el('button', '', 'cpt-oeil');
    b.type = 'button';
    b.addEventListener('click', function () { input.type = input.type === 'password' ? 'text' : 'password'; labels(); input.focus(); }, false);
    input.parentNode.appendChild(b);
    eyes.push({ input: input, btn: b });
  }
  addEye('newpw'); addEye('newpw2'); addEye('pw-champ');

  // ---- Text that depends on the language (placeholders, toggles, copy buttons) -----------------------
  function labels() {
    $('pk-name').placeholder = T('Nom (ex. : téléphone)', 'Name (e.g. phone)');
    $('tk-name').placeholder = T('Nom (ex. : ordinateur)', 'Name (e.g. laptop)');
    for (var i = 0; i < eyes.length; i++) {
      eyes[i].btn.textContent = eyes[i].input.type === 'password' ? T('Afficher', 'Show') : T('Masquer', 'Hide');
    }
    $('b-copy-token').textContent = T('Copier le jeton', 'Copy token');
    $('b-copy-codes').textContent = T('Copier les codes', 'Copy codes');
  }

  // ---- Confirmation sheet: asks for the password only when an action needs it ----------------------------
  var sheet = { run: null, last: null };
  function openSheet(o, run) {
    $('pw-titre').textContent = o.title;
    $('pw-texte').textContent = o.text || ''; $('pw-texte').hidden = !o.text;
    $('pw-label').textContent = T('Mot de passe', 'Password');
    $('pw-non').textContent = T('Annuler', 'Cancel');
    var yes = $('pw-oui'); yes.textContent = o.ok; yes.className = 'bouton' + (o.destructive ? ' danger-plein' : '');
    $('pw-user').value = me ? me.username : '';
    $('pw-champ').value = ''; $('pw-champ').type = 'password';
    $('pw-erreur').hidden = true;
    labels();
    sheet.run = run; sheet.last = d.activeElement;
    $('pw-voile').hidden = false; $('pw-voile').removeAttribute('hidden');
    w.setTimeout(function () { $('pw-champ').focus(); }, 30);
  }
  function closeSheet() {
    $('pw-voile').hidden = true; sheet.run = null;
    if (sheet.last && sheet.last.focus) { try { sheet.last.focus(); } catch (e) {} }
  }
  $('f-reauth').addEventListener('submit', function (e) {
    e.preventDefault();
    var pw = $('pw-champ').value, box = $('pw-erreur');
    if (!pw) { box.textContent = errText('password_required'); box.hidden = false; $('pw-champ').focus(); return; }
    if (!sheet.run) { return; }
    busy($('f-reauth'), true);
    sheet.run(pw, function (code) {
      busy($('f-reauth'), false);
      if (code === 'wrong_password') { box.textContent = errText('wrong_password'); box.hidden = false; $('pw-champ').value = ''; $('pw-champ').focus(); return; }
      closeSheet();
    });
  }, false);
  on('pw-non', 'click', closeSheet);
  on('pw-voile', 'mousedown', function (e) { if (e.target === $('pw-voile')) { closeSheet(); } });
  d.addEventListener('keydown', function (e) { if ((e.key === 'Escape' || e.keyCode === 27) && !$('pw-voile').hidden) { closeSheet(); } }, false);

  // `call(password, done)` runs the request and then calls `done(err, successText, afterSuccess)`.
  function secure(o, call) {
    clearMsg();
    openSheet(o, function (pw, finish) {
      call(pw, function (err, okText, extra) {
        finish(err);
        if (err === 'wrong_password') { return; }
        if (err) { fail(err); if (err === 'unauthenticated') { w.location.href = '/connexion'; } return; }
        if (extra) { extra(); }
        if (okText) { good(okText); }
        load();
      });
    });
  }

  // ---- Live validation of the forms -------------------------------------------------------------------
  function validate() {
    if (!me) { return; }
    var cur = me.display_name || me.username, nv = trim($('newname').value);
    var locked = nv.toLowerCase() !== me.username && !!me.username_changeable_at;
    $('b-name-save').disabled = !nv || nv === cur || locked;
    var p1 = $('newpw').value, p2 = $('newpw2').value, hint = $('pw-hint'), ready = false;
    if (!p1 && !p2) { hint.textContent = T('10 caractères minimum. Vos autres appareils seront déconnectés.', '10 characters minimum. Your other devices will be logged out.'); }
    else if (p1.length < 10) { hint.textContent = T('Encore ' + (10 - p1.length) + ' caractère(s) au minimum.', (10 - p1.length) + ' more character(s) needed.'); }
    else if (p1 !== p2) { hint.textContent = p2 ? T('Les deux mots de passe ne correspondent pas.', 'The two passwords do not match.') : T('Confirmez le mot de passe.', 'Confirm the password.'); }
    else { hint.textContent = T('Prêt. Vos autres appareils seront déconnectés.', 'Ready. Your other devices will be logged out.'); ready = true; }
    $('b-pw-save').disabled = !ready;
  }
  on('newname', 'input', validate); on('newpw', 'input', validate); on('newpw2', 'input', validate);

  function render() {
    var shown = me.display_name || me.username;
    $('who').textContent = shown;
    $('avatar').textContent = shown.charAt(0);
    $('pw-user2').value = me.username;
    if (d.activeElement !== $('newname')) { $('newname').value = shown; }
    $('name-hint').textContent = me.username_changeable_at
      ? T('Vous pourrez changer de nom d\'utilisateur à partir du ', 'You can change your username again from ') + date(me.username_changeable_at) + T('. Changer seulement les majuscules reste possible à tout moment.', '. Changing only the capitals is always possible.')
      : T('Le nom peut être changé une fois tous les 30 jours. Changer seulement les majuscules est libre et ne compte pas.', 'The username can be changed once every 30 days. Changing only the capitals is free and does not count.');
    $('role').textContent = ({ user: T('Compte', 'Member'), editor: T('Éditeur', 'Editor'), admin: 'Admin' }[me.role] || me.role);
    $('totp-state').textContent = me.totp_enabled ? T('Activé.', 'Enabled.') : T('Désactivé.', 'Disabled.');
    $('b-totp-setup').hidden = me.totp_enabled;
    $('b-totp-disable').hidden = !me.totp_enabled;
    if (me.totp_enabled) { $('totp-setup').hidden = true; }

    var pl = $('pk-list'); clear(pl);
    if (!hasPasskeys) { $('pk-unsupported').hidden = false; $('f-passkey').hidden = true; }
    for (var i = 0; i < me.passkeys.length; i++) {
      (function (p) {
        var li = el('li');
        var t = el('span', '', 'cpt-titre'); t.appendChild(el('span', p.name)); t.appendChild(el('span', T('Ajoutée le ', 'Added on ') + date(p.created_at), 'cpt-sous'));
        li.appendChild(t);
        var b = el('button', T('Retirer', 'Remove'), 'bouton secondaire danger'); b.type = 'button';
        b.addEventListener('click', function () {
          secure({ title: T('Retirer cette passkey ?', 'Remove this passkey?'), text: p.name, ok: T('Retirer', 'Remove'), destructive: true }, function (pw, done) {
            api('POST', '/api/account/passkeys/' + p.id + '/delete', { password: pw }, function (err) { done(err, T('Passkey retirée.', 'Passkey removed.')); });
          });
        }, false);
        li.appendChild(b); pl.appendChild(li);
      })(me.passkeys[i]);
    }

    var has2fa = me.totp_enabled || me.passkeys.length > 0;
    $('rec-state').textContent = has2fa
      ? T(me.recovery_remaining + ' code(s) de secours restant(s). Sans e-mail, ils sont votre seul recours si vous perdez votre second facteur.', me.recovery_remaining + ' recovery code(s) left. Without e-mail, they are your only way back in if you lose your second factor.')
      : T('Disponibles dès qu\'un second facteur est activé.', 'Available once a second factor is enabled.');
    $('rec-actions').hidden = !has2fa;

    // A short, single nudge at the top when the account is weakly protected.
    var tip = $('cpt-conseil');
    if (!has2fa) {
      $('conseil-titre').textContent = T('Votre compte n\'est protégé que par un mot de passe', 'Your account is protected by a password only');
      $('conseil-texte').textContent = T('Ajoutez une application d\'authentification ou une passkey : sans e-mail, c\'est ce qui protège votre compte.', 'Add an authenticator app or a passkey: with no e-mail, this is what protects your account.');
      $('conseil-lien').textContent = T('Renforcer', 'Add protection');
      tip.hidden = false;
    } else if (me.recovery_remaining === 0) {
      $('conseil-titre').textContent = T('Plus aucun code de secours', 'No recovery codes left');
      $('conseil-texte').textContent = T('Générez-en de nouveaux pour ne pas perdre l\'accès si vous perdez votre second facteur.', 'Generate new ones so you do not lose access if you lose your second factor.');
      $('conseil-lien').textContent = T('Générer', 'Generate');
      tip.hidden = false;
    } else { tip.hidden = true; }

    $('git-url').textContent = 'git clone ' + me.git_url;
    $('git-rights').textContent = (me.role === 'user')
      ? T('Avec un compte simple, les modifications envoyées sont enregistrées comme propositions, relues par un éditeur.', 'With a regular account, changes you send are saved as suggestions, reviewed by an editor.')
      : T('Les modifications envoyées sont publiées directement (sans réécriture de l\'historique).', 'Changes you send are published directly (history is never rewritten).');
    var tl = $('tk-list'); clear(tl);
    for (var j = 0; j < me.tokens.length; j++) {
      (function (t) {
        var li = el('li');
        var tt = el('span', '', 'cpt-titre'); tt.appendChild(el('span', t.name));
        tt.appendChild(el('span', date(t.created_at) + (t.expires_at ? ' → ' + date(t.expires_at) : ''), 'cpt-sous'));
        li.appendChild(tt);
        var b = el('button', T('Révoquer', 'Revoke'), 'bouton secondaire danger'); b.type = 'button';
        b.addEventListener('click', function () {
          api('POST', '/api/account/tokens/' + t.id + '/delete', {}, function (err) { after(err, T('Jeton révoqué.', 'Token revoked.')); });
        }, false);
        li.appendChild(b); tl.appendChild(li);
      })(me.tokens[j]);
    }
    $('sess-state').textContent = T(me.sessions + ' session(s) active(s).', me.sessions + ' active session(s).');
    $('b-revoke').hidden = me.sessions < 2;
    labels(); validate();
  }

  function load() {
    api('GET', '/api/me', null, function (err, data) {
      if (err === 'unauthenticated') { w.location.href = '/connexion'; return; }
      if (err) { fail(err); return; }
      me = data; render();
    });
  }

  on('b-logout', 'click', function () { api('POST', '/api/auth/logout', {}, function () { w.location.href = '/'; }); });

  $('f-username').addEventListener('submit', function (e) {
    e.preventDefault();
    secure({ title: T('Confirmer le changement', 'Confirm the change'), text: T('Entrez votre mot de passe pour modifier votre nom d\'utilisateur.', 'Enter your password to change your username.'), ok: T('Enregistrer', 'Save') }, function (pw, done) {
      api('POST', '/api/account/username', { username: trim($('newname').value), password: pw }, function (err) { done(err, T('Nom d\'utilisateur enregistré.', 'Username saved.')); });
    });
  }, false);

  $('f-password').addEventListener('submit', function (e) {
    e.preventDefault(); clearMsg();
    if ($('newpw').value !== $('newpw2').value) { fail('passwords_differ'); return; }
    secure({ title: T('Changer le mot de passe', 'Change your password'), text: T('Entrez votre mot de passe actuel. Vos autres appareils seront déconnectés.', 'Enter your current password. Your other devices will be logged out.'), ok: T('Changer', 'Change') }, function (pw, done) {
      api('POST', '/api/account/password', { current_password: pw, new_password: $('newpw').value }, function (err) {
        done(err, T('Mot de passe modifié. Vos autres appareils ont été déconnectés.', 'Password changed. Your other devices were logged out.'), function () { $('newpw').value = ''; $('newpw2').value = ''; });
      });
    });
  }, false);

  on('b-totp-setup', 'click', function () {
    secure({ title: T('Activer l\'application d\'authentification', 'Enable the authenticator app'), text: T('Confirmez avec votre mot de passe pour afficher le QR code.', 'Confirm with your password to show the QR code.'), ok: T('Continuer', 'Continue') }, function (pw, done) {
      api('POST', '/api/account/totp/setup', { password: pw }, function (err, data) {
        done(err, null, function () {
          $('qr').innerHTML = data.qr_svg;
          $('totp-secret').textContent = data.secret;
          $('totp-setup').hidden = false; $('totp-setup').removeAttribute('hidden');
          w.setTimeout(function () { $('totp-code').focus(); }, 50);
        });
      });
    });
  });
  $('f-totp').addEventListener('submit', function (e) {
    e.preventDefault(); clearMsg();
    api('POST', '/api/account/totp/enable', { code: $('totp-code').value }, function (err, data) {
      if (!err) { $('totp-code').value = ''; $('totp-setup').hidden = true; if (data.recovery_codes) { showCodes(data.recovery_codes); } }
      after(err, T('Code OTP activé.', 'OTP enabled.'));
    });
  }, false);
  on('b-totp-disable', 'click', function () {
    secure({ title: T('Désactiver l\'application d\'authentification ?', 'Disable the authenticator app?'), text: T('Votre compte sera moins bien protégé.', 'Your account will be less protected.'), ok: T('Désactiver', 'Disable'), destructive: true }, function (pw, done) {
      api('POST', '/api/account/totp/disable', { password: pw }, function (err) { done(err, T('Code OTP désactivé.', 'OTP disabled.')); });
    });
  });

  $('f-passkey').addEventListener('submit', function (e) {
    e.preventDefault(); clearMsg();
    api('POST', '/api/account/passkeys/start', {}, function (err, data) {
      if (err) { fail(err); return; }
      var pk = data.publicKey;
      pk.challenge = b2buf(pk.challenge);
      pk.user.id = b2buf(pk.user.id);
      if (pk.excludeCredentials) {
        for (var i = 0; i < pk.excludeCredentials.length; i++) { pk.excludeCredentials[i].id = b2buf(pk.excludeCredentials[i].id); }
      }
      navigator.credentials.create({ publicKey: pk }).then(function (c) {
        var r = c.response, tr = [];
        try { tr = r.getTransports ? r.getTransports() : []; } catch (e3) {}
        var cred = { id: c.id, rawId: buf2b(c.rawId), type: c.type, extensions: ext(c),
          response: { attestationObject: buf2b(r.attestationObject), clientDataJSON: buf2b(r.clientDataJSON), transports: tr } };
        api('POST', '/api/account/passkeys/finish', { credential: cred, name: $('pk-name').value }, function (err2, res) {
          if (!err2) { $('pk-name').value = ''; if (res.recovery_codes) { showCodes(res.recovery_codes); } }
          after(err2, T('Passkey ajoutée.', 'Passkey added.'));
        });
      }, function (e2) { fail(waErr(e2)); });
    });
  }, false);

  on('b-recovery', 'click', function () {
    secure({ title: T('Générer de nouveaux codes ?', 'Generate new codes?'), text: T('Les anciens codes ne fonctionneront plus.', 'The old codes will stop working.'), ok: T('Générer', 'Generate') }, function (pw, done) {
      api('POST', '/api/account/recovery', { password: pw }, function (err, data) {
        done(err, T('Nouveaux codes générés ; les anciens ne fonctionnent plus.', 'New codes generated; the old ones no longer work.'), function () { showCodes(data.recovery_codes); });
      });
    });
  });
  $('f-token').addEventListener('submit', function (e) {
    e.preventDefault(); clearMsg();
    secure({ title: T('Créer un jeton', 'Create a token'), text: T('Entrez votre mot de passe pour créer le jeton.', 'Enter your password to create the token.'), ok: T('Créer', 'Create') }, function (pw, done) {
      api('POST', '/api/account/tokens', { password: pw, name: $('tk-name').value, expires_days: parseInt($('tk-days').value, 10) }, function (err, data) {
        done(err, T('Jeton créé.', 'Token created.'), function () { $('tk-name').value = ''; $('tk-value').textContent = data.token; $('tk-new').hidden = false; $('tk-new').removeAttribute('hidden'); });
      });
    });
  }, false);

  on('b-revoke', 'click', function () {
    api('POST', '/api/account/sessions/revoke-others', {}, function (err) { after(err, T('Autres appareils déconnectés.', 'Other devices logged out.')); });
  });
  on('b-delete', 'click', function () {
    secure({ title: T('Supprimer définitivement votre compte ?', 'Permanently delete your account?'), text: T('Votre compte, vos passkeys, jetons et sessions seront supprimés. Cette action est irréversible.', 'Your account, passkeys, tokens and sessions will be deleted. This cannot be undone.'), ok: T('Supprimer', 'Delete'), destructive: true }, function (pw, done) {
      api('POST', '/api/account/delete', { password: pw }, function (err) { done(err, null, function () { w.location.href = '/'; }); });
    });
  });

  // ---- Section navigation: highlight the section being read ----------------------------------------------
  (function () {
    if (!w.IntersectionObserver) { return; }
    var links = d.querySelectorAll('.cpt-nav a'), map = {}, i;
    for (i = 0; i < links.length; i++) { map[links[i].getAttribute('href').slice(1)] = links[i]; }
    var io = new w.IntersectionObserver(function (entries) {
      for (var k = 0; k < entries.length; k++) {
        if (!entries[k].isIntersecting) { continue; }
        for (var id in map) { if (map.hasOwnProperty(id)) { map[id].removeAttribute('aria-current'); } }
        var a = map[entries[k].target.id]; if (a) { a.setAttribute('aria-current', 'true'); }
      }
    }, { rootMargin: '-15% 0px -75% 0px' });
    for (var id2 in map) { if (map.hasOwnProperty(id2) && $(id2)) { io.observe($(id2)); } }
  })();

  on('msg', 'click', clearMsg); on('ok', 'click', clearMsg);
  on('l-fr', 'change', function () { if (me) { render(); } else { labels(); } });
  on('l-en', 'change', function () { if (me) { render(); } else { labels(); } });
  labels();
  load();
})();
