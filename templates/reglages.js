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

  function liaison(r, nom) {
    if (saved[nom] && r.id === saved[nom]) { r.checked = true; }
    r.onchange = function () { saved[nom] = r.id; memo(); lang(); };
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