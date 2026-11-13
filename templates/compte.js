/* Pages de compte (connexion, inscription, réglages). Seules pages du site qui exigent JavaScript.
   ES5 : si le navigateur est trop ancien, le message « mettez à jour / activez JavaScript » reste affiché. */
(function () {
  var d = document, w = window;
  function $(id) { return d.getElementById(id); }
  var app = $('app'), pre = $('prerequis');
  if (!app || !pre) { return; }

  // Prérequis du navigateur : on ne révèle l'interface que s'il sait tout faire.
  var needsCap = !!$('cap');
  var capable = !!(w.XMLHttpRequest && w.JSON && d.querySelector && w.addEventListener && d.createElement);
  if (needsCap) { capable = capable && !!(w.WebAssembly && w.Worker && w.customElements); }
  if (!capable) { return; }
  pre.hidden = true;
  pre.style.display = 'none';
  app.hidden = false;
  app.removeAttribute('hidden');

  function fr() { return (d.documentElement.lang || '').slice(0, 2) === 'fr'; }
  function T(a, b) { return fr() ? a : b; }

  var ERR = {
    invalid_username: ["Nom d'utilisateur invalide : 3 à 32 caractères (lettres, chiffres, point, tiret, tiret bas), commençant par une lettre ou un chiffre.", 'Invalid username: 3 to 32 characters (letters, digits, dot, dash, underscore), starting with a letter or digit.'],
    username_reserved: ["Ce nom d'utilisateur est réservé.", 'This username is reserved.'],
    username_taken: ["Ce nom d'utilisateur est déjà pris.", 'This username is already taken.'],
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
    password_required: ['Saisissez votre mot de passe actuel dans « Confirmation ».', 'Enter your current password in "Confirmation".'],
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

  var msg = $('msg'), okBox = $('ok');
  function fail(code) {
    msg.textContent = errText(code);
    msg.hidden = false; msg.removeAttribute('hidden');
    if (okBox) { okBox.hidden = true; }
    try { msg.scrollIntoView(); } catch (e) {}
  }
  function clearMsg() { msg.hidden = true; if (okBox) { okBox.hidden = true; } }
  function good(text) {
    if (!okBox) { return; }
    msg.hidden = true;
    okBox.textContent = text; okBox.hidden = false; okBox.removeAttribute('hidden');
    try { okBox.scrollIntoView(); } catch (e) {}
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
  var capToken = '', capEl = $('cap');
  if (capEl) {
    if (!w.customElements.get('cap-widget')) { fail('captcha_load'); }
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

  function reauth() {
    var v = $('reauth').value;
    if (!v) { fail('password_required'); $('reauth').focus(); return null; }
    return v;
  }
  function after(err, okText) {
    if (err) {
      fail(err);
      if (err === 'unauthenticated') { w.location.href = '/connexion'; }
      return false;
    }
    good(okText);
    $('reauth').value = '';
    load();
    return true;
  }
  function date(s) { if (!s) { return ''; } try { return new Date(s).toLocaleDateString(fr() ? 'fr-FR' : 'en-GB'); } catch (e) { return s; } }
  function clear(node) { while (node.firstChild) { node.removeChild(node.firstChild); } }
  function showCodes(codes) {
    var ul = $('codes-list'); clear(ul);
    for (var i = 0; i < codes.length; i++) { var li = el('li'); li.appendChild(el('code', codes[i])); ul.appendChild(li); }
    $('codes').hidden = false; $('codes').removeAttribute('hidden');
  }

  function render() {
    $('who').textContent = me.username;
    $('role').textContent = '(' + ({ user: T('compte', 'member'), editor: T('éditeur', 'editor'), admin: 'admin' }[me.role] || me.role) + ')';
    $('totp-state').textContent = me.totp_enabled ? T('Activé.', 'Enabled.') : T('Désactivé.', 'Disabled.');
    $('b-totp-setup').hidden = me.totp_enabled;
    $('b-totp-disable').hidden = !me.totp_enabled;
    if (me.totp_enabled) { $('totp-setup').hidden = true; }

    var pl = $('pk-list'); clear(pl);
    if (!hasPasskeys) { $('pk-unsupported').hidden = false; $('f-passkey').hidden = true; }
    for (var i = 0; i < me.passkeys.length; i++) {
      (function (p) {
        var li = el('li');
        li.appendChild(el('span', p.name + ' · ' + date(p.created_at)));
        var b = el('button', T('Retirer', 'Remove'), 'bouton secondaire'); b.type = 'button';
        b.addEventListener('click', function () {
          var pw = reauth(); if (!pw) { return; }
          api('POST', '/api/account/passkeys/' + p.id + '/delete', { password: pw }, function (err) { after(err, T('Passkey retirée.', 'Passkey removed.')); });
        }, false);
        li.appendChild(b); pl.appendChild(li);
      })(me.passkeys[i]);
    }

    var has2fa = me.totp_enabled || me.passkeys.length > 0;
    $('rec-state').textContent = has2fa
      ? T(me.recovery_remaining + ' code(s) de secours restant(s). Sans e-mail, ils sont votre seul recours si vous perdez votre second facteur.', me.recovery_remaining + ' recovery code(s) left. Without e-mail, they are your only way back in if you lose your second factor.')
      : T('Disponibles dès qu\'un second facteur est activé.', 'Available once a second factor is enabled.');
    $('rec-actions').hidden = !has2fa;

    $('git-url').textContent = 'git clone ' + me.git_url;
    $('git-rights').textContent = (me.role === 'user')
      ? T('Votre compte peut pousser vers la branche users/' + me.username + '/… (propositions). La branche main est réservée aux éditeurs.', 'Your account can push to the branch users/' + me.username + '/… (proposals). The main branch is reserved for editors.')
      : T('Votre compte peut pousser vers toutes les branches (main en avance rapide uniquement).', 'Your account can push to any branch (fast-forward only on main).');
    var tl = $('tk-list'); clear(tl);
    for (var j = 0; j < me.tokens.length; j++) {
      (function (t) {
        var li = el('li');
        li.appendChild(el('span', t.name + ' · ' + date(t.created_at) + (t.expires_at ? ' → ' + date(t.expires_at) : '')));
        var b = el('button', T('Révoquer', 'Revoke'), 'bouton secondaire'); b.type = 'button';
        b.addEventListener('click', function () {
          api('POST', '/api/account/tokens/' + t.id + '/delete', {}, function (err) { after(err, T('Jeton révoqué.', 'Token revoked.')); });
        }, false);
        li.appendChild(b); tl.appendChild(li);
      })(me.tokens[j]);
    }
    $('sess-state').textContent = T(me.sessions + ' session(s) active(s).', me.sessions + ' active session(s).');
  }

  function load() {
    api('GET', '/api/me', null, function (err, data) {
      if (err === 'unauthenticated') { w.location.href = '/connexion'; return; }
      if (err) { fail(err); return; }
      me = data; render();
    });
  }

  on('b-logout', 'click', function () { api('POST', '/api/auth/logout', {}, function () { w.location.href = '/'; }); });

  $('f-password').addEventListener('submit', function (e) {
    e.preventDefault(); clearMsg();
    var pw = reauth(); if (!pw) { return; }
    if ($('newpw').value !== $('newpw2').value) { fail('passwords_differ'); return; }
    api('POST', '/api/account/password', { current_password: pw, new_password: $('newpw').value }, function (err) {
      if (!err) { $('newpw').value = ''; $('newpw2').value = ''; }
      after(err, T('Mot de passe modifié. Vos autres appareils ont été déconnectés.', 'Password changed. Your other devices were logged out.'));
    });
  }, false);

  on('b-totp-setup', 'click', function () {
    clearMsg();
    var pw = reauth(); if (!pw) { return; }
    api('POST', '/api/account/totp/setup', { password: pw }, function (err, data) {
      if (err) { fail(err); return; }
      $('qr').innerHTML = data.qr_svg;
      $('totp-secret').textContent = data.secret;
      $('totp-setup').hidden = false; $('totp-setup').removeAttribute('hidden');
      $('totp-code').focus();
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
    clearMsg();
    var pw = reauth(); if (!pw) { return; }
    api('POST', '/api/account/totp/disable', { password: pw }, function (err) { after(err, T('Code OTP désactivé.', 'OTP disabled.')); });
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
    clearMsg();
    var pw = reauth(); if (!pw) { return; }
    api('POST', '/api/account/recovery', { password: pw }, function (err, data) {
      if (!err) { showCodes(data.recovery_codes); }
      after(err, T('Nouveaux codes générés ; les anciens ne fonctionnent plus.', 'New codes generated; the old ones no longer work.'));
    });
  });

  $('f-token').addEventListener('submit', function (e) {
    e.preventDefault(); clearMsg();
    var pw = reauth(); if (!pw) { return; }
    api('POST', '/api/account/tokens', { password: pw, name: $('tk-name').value, expires_days: parseInt($('tk-days').value, 10) }, function (err, data) {
      if (!err) { $('tk-name').value = ''; $('tk-value').textContent = data.token; $('tk-new').hidden = false; $('tk-new').removeAttribute('hidden'); }
      after(err, T('Jeton créé.', 'Token created.'));
    });
  }, false);

  on('b-revoke', 'click', function () {
    api('POST', '/api/account/sessions/revoke-others', {}, function (err) { after(err, T('Autres appareils déconnectés.', 'Other devices logged out.')); });
  });
  on('b-delete', 'click', function () {
    var pw = reauth(); if (!pw) { return; }
    if (!w.confirm(T('Supprimer définitivement votre compte ?', 'Permanently delete your account?'))) { return; }
    api('POST', '/api/account/delete', { password: pw }, function (err) {
      if (err) { fail(err); return; }
      w.location.href = '/';
    });
  });

  load();
})();
