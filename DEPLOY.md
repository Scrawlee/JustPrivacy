# Déploiement sur Dokploy

Le serveur Rust (`server/`) rend lui-même les pages du site (gabarits embarqués dans le binaire, articles lus dans git,
résultat gardé en mémoire), l'API de comptes (`/api`) et le dépôt git des articles (`/git/articles.git`). PostgreSQL tourne à côté, dans le même `docker-compose.yml`.

1. Dokploy → *Create Service* → **Compose** → source GitHub `Scrawlee/JustPrivacy`, fichier `docker-compose.yml`.
2. Onglet *Environment* : renseigner les variables de `.env.example`
   (`PUBLIC_URL`, `POSTGRES_PASSWORD`, `CAP_SECRET`, facultatif `APP_SECRET`).
3. Onglet *Domains* : domaine du site → service `app`, port `8080`, HTTPS activé.
4. Côté serveur Cap (`check.scrawlee.com`) : autoriser l'origine du site (sinon CORS bloque le widget)
   et récupérer le **secret** de la clé de site `5c7f97279d` pour `CAP_SECRET`.
5. Deploy. Au premier démarrage : migrations SQL, création du dépôt git (importé depuis `content/articles`)
   et premier rendu des pages à la demande.

## Rôles et git

- Tout compte peut cloner : `git clone https://<domaine>/git/articles.git`.
- Pousser : créer un jeton dans *Compte → Accès git*, l'utiliser comme mot de passe (identifiant = nom d'utilisateur).
- Compte simple : push uniquement vers `users/<nom>/…`. Éditeur/admin : toutes les branches (`main` en avance rapide).
- Promouvoir (terminal du conteneur `app`) : `justprivacy-server promote <utilisateur> editor`
- Les pages sont rendues à la première demande puis servies depuis la mémoire (`ETag`, revalidation du navigateur).
  Quand `main` change (éditeur web, `git push`, suppression), le serveur compare les articles de l'ancien et du nouveau
  commit et ne purge que les pages concernées : l'article, la liste, l'accueil et les articles qui le citent en « voir aussi ».
  Plus rien à reconstruire : la modification est visible dès la requête suivante (`GET /api/site` : commit courant et compteurs).

## Éditeur web (`/editor`)

Page réservée aux comptes connectés, qui lit et écrit dans le même dépôt git (`/api/articles`, `/api/proposals`) :

- **Éditeur / administrateur** : « Publier » commit directement sur `main` (détection de conflit sur la version ouverte,
  purge ciblée du cache de pages). Peut aussi supprimer un article, consulter l'historique et recharger une ancienne
  version, publier ou rejeter les propositions des autres comptes (auteur du commit = le proposant, validateur = l'éditeur).
- **Compte simple** : « Proposer » commit sur `users/<nom>/<slug>` (30 propositions ouvertes au plus), à relire par un éditeur ;
  c'est la même branche que celle qu'on obtient avec `git push origin HEAD:users/<nom>/<slug>`.
- Les articles sont validés à l'enregistrement (en-tête, langues, `slug`, liens `https`/`mailto`/internes) : pas de HTML brut
  (`<…>` refusé), et le rendu échappe de toute façon le texte. Les écritures sont limitées à 120 par heure et par compte.

## Sauvegardes

Volumes à sauvegarder : `db-data` (comptes) et `app-data` (dépôt git, `app.key` qui chiffre les secrets OTP
si `APP_SECRET` n'est pas défini).
