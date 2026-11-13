# Déploiement sur Dokploy

Le serveur Rust (`server/`) sert le site statique, l'API de comptes (`/api`) et le dépôt git des
articles (`/git/articles.git`). PostgreSQL tourne à côté, dans le même `docker-compose.yml`.

1. Dokploy → *Create Service* → **Compose** → source GitHub `Scrawlee/JustPrivacy`, fichier `docker-compose.yml`.
2. Onglet *Environment* : renseigner les variables de `.env.example`
   (`PUBLIC_URL`, `POSTGRES_PASSWORD`, `CAP_SECRET`, facultatif `APP_SECRET`).
3. Onglet *Domains* : domaine du site → service `app`, port `8080`, HTTPS activé.
4. Côté serveur Cap (`check.scrawlee.com`) : autoriser l'origine du site (sinon CORS bloque le widget)
   et récupérer le **secret** de la clé de site `5c7f97279d` pour `CAP_SECRET`.
5. Deploy. Au premier démarrage : migrations SQL, création du dépôt git (importé depuis `content/articles`)
   et construction du site.

## Rôles et git

- Tout compte peut cloner : `git clone https://<domaine>/git/articles.git`.
- Pousser : créer un jeton dans *Compte → Accès git*, l'utiliser comme mot de passe (identifiant = nom d'utilisateur).
- Compte simple : push uniquement vers `users/<nom>/…`. Éditeur/admin : toutes les branches (`main` en avance rapide).
- Promouvoir (terminal du conteneur `app`) : `justprivacy-server promote <utilisateur> editor`
- Un push sur `main` reconstruit le site automatiquement (état : `GET /api/site`). Si un article est invalide,
  la construction échoue et l'ancienne version reste en ligne.

## Sauvegardes

Volumes à sauvegarder : `db-data` (comptes) et `app-data` (dépôt git, `app.key` qui chiffre les secrets OTP
si `APP_SECRET` n'est pas défini).
