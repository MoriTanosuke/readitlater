-- Migration 0009: Passkeys (WebAuthn) als zusätzliche Anmeldung neben dem Passwort.
-- Ein Benutzer kann mehrere Passkeys haben (je Gerät oder Passwortmanager einer).
-- Das Passwort bleibt Pflicht und Rückfallweg: Es gibt keine Konten nur mit Passkey.
--
-- `passkey` enthält die von webauthn-rs serialisierte Anmeldeinformation als JSON
-- (öffentlicher Schlüssel, Zähler, Flags). Geheimes Material liegt nie auf dem Server.
-- `credential_id` ist die ID, die der Browser bei der Anmeldung nennt (einmalig über alle Konten).
-- `user_handle` ist die zufällige, nichtssagende Kennung des Benutzers (16 Byte, UUID),
-- die der Passkey im Gerät speichert und bei der Anmeldung ohne E-Mail zurückliefert.

CREATE TABLE passkeys (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id       INTEGER NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name          TEXT    NOT NULL,
    credential_id BLOB    NOT NULL UNIQUE,
    user_handle   BLOB    NOT NULL,
    passkey       TEXT    NOT NULL,
    created_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    last_used_at  TEXT
);
CREATE INDEX idx_passkeys_user ON passkeys (user_id);
CREATE INDEX idx_passkeys_handle ON passkeys (user_handle);
