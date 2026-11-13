-- Comptes : un nom d'utilisateur et un mot de passe, rien d'autre (pas d'e-mail, pas d'IP).
CREATE TABLE users (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    username           TEXT NOT NULL UNIQUE CHECK (username ~ '^[a-z0-9][a-z0-9._-]{2,31}$'),
    password_hash      TEXT NOT NULL,
    role               TEXT NOT NULL DEFAULT 'user' CHECK (role IN ('user', 'editor', 'admin')),
    -- TOTP : secret chiffré (ChaCha20-Poly1305) ; totp_pending sert pendant l'activation.
    totp_secret        BYTEA,
    totp_pending       BYTEA,
    totp_enabled       BOOLEAN NOT NULL DEFAULT FALSE,
    totp_last_step     BIGINT NOT NULL DEFAULT 0,
    failed_attempts    INTEGER NOT NULL DEFAULT 0,
    locked_until       TIMESTAMPTZ,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    password_changed_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Sessions : seul le hachage SHA-256 du jeton est stocké. Aucune IP, aucun user-agent.
CREATE TABLE sessions (
    id          BYTEA PRIMARY KEY,
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at  TIMESTAMPTZ NOT NULL
);
CREATE INDEX sessions_user_idx ON sessions(user_id);

-- Connexion en deux temps : mot de passe validé, second facteur attendu.
CREATE TABLE pending_logins (
    id          BYTEA PRIMARY KEY,
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    attempts    INTEGER NOT NULL DEFAULT 0,
    expires_at  TIMESTAMPTZ NOT NULL
);

CREATE TABLE passkeys (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id      UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    credential   JSONB NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ
);
CREATE INDEX passkeys_user_idx ON passkeys(user_id);

-- Codes de secours (sans e-mail, ils sont le seul recours si l'on perd son second facteur).
CREATE TABLE recovery_codes (
    id        BIGSERIAL PRIMARY KEY,
    user_id   UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_hash BYTEA NOT NULL,
    used_at   TIMESTAMPTZ
);
CREATE INDEX recovery_user_idx ON recovery_codes(user_id);

-- Jetons d'accès personnels, pour pousser des articles avec git.
CREATE TABLE api_tokens (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id      UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    token_hash   BYTEA NOT NULL UNIQUE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ,
    expires_at   TIMESTAMPTZ
);
CREATE INDEX api_tokens_user_idx ON api_tokens(user_id);
