/* Réglages (langue, thème, filtres) : amélioration progressive, ES5, aucune dépendance.
   Le site fonctionne sans ce script ; il ne fait que mémoriser les choix et
   détecter le français au premier passage. */
(function () {
  var d = document, K = 'justprivacy-reglages', saved = {};
  function $(id) { return d.getElementById(id); }
  function each(list, fn) { for (var i = 0; i < list.length; i++) { fn(list[i]); } }
  function memo() { try { localStorage.setItem(K, JSON.stringify(saved)); } catch (e) {} }
  function lang() {
    var fr = $('l-fr');
    d.documentElement.lang = (fr && fr.checked) ? 'fr' : 'en';
  }
  try { saved = JSON.parse(localStorage.getItem(K)) || {}; } catch (e) { saved = {}; }
  var pref = (navigator.languages && navigator.languages[0]) || navigator.language || navigator.userLanguage || '';
  if (!saved.langue) { saved.langue = /^fr/i.test(pref) ? 'l-fr' : 'l-en'; }

  // Menu de langue (pied de page) : se referme après un choix ou Échap, et ses entrées se commandent au clavier.
  function fermer() { each(d.querySelectorAll ? d.querySelectorAll('.menu-langue') : [], function (m) { m.removeAttribute('open'); }); }
  each(d.querySelectorAll ? d.querySelectorAll('.menu-langue label') : [], function (l) {
    l.onkeydown = function (e) {
      var k = e.key || e.keyCode;
      if (k === 'Enter' || k === ' ' || k === 13 || k === 32) { if (e.preventDefault) { e.preventDefault(); } l.click(); }
    };
  });
  d.addEventListener('keydown', function (e) { if (e.key === 'Escape' || e.keyCode === 27) { fermer(); } }, false);
  d.addEventListener('click', function (e) {
    var t = e.target;
    while (t && t !== d) { if (t.className === 'menu-langue') { return; } t = t.parentNode; }
    fermer();
  }, false);

  function liaison(r, nom) {
    if (saved[nom] && r.id === saved[nom]) { r.checked = true; }
    r.onchange = function () { saved[nom] = r.id; memo(); lang(); fermer(); };
  }
  var groupes = ['langue', 'theme'];
  for (var g = 0; g < groupes.length; g++) {
    (function (nom) { each(d.getElementsByName(nom), function (r) { liaison(r, nom); }); })(groupes[g]);
  }
  each(['masquer-en', 'afficher-fr'], function (id) {
    var c = $(id);
    if (!c) { return; }
    if (saved[id] !== undefined) { c.checked = !!saved[id]; }
    c.onchange = function () { saved[id] = c.checked; memo(); };
  });
  lang();
})();