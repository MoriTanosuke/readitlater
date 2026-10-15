// Nachgebautes Gerät für Passkeys: ersetzt `navigator.credentials` im Test und antwortet wie ein
// Authenticator (ES256, Anmeldung ohne E-Mail mit Benutzerkennung). So läuft der ganze Ablauf
// aus js/passkeys.js und js/app.js gegen das echte Backend, ohne Browser und ohne Hardware.

import { createHash, generateKeyPairSync, randomBytes, sign } from 'node:crypto';

const b64u = (buffer) => Buffer.from(buffer).toString('base64url');
const sha256 = (data) => createHash('sha256').update(data).digest();
const toArrayBuffer = (buffer) => buffer.buffer.slice(buffer.byteOffset, buffer.byteOffset + buffer.byteLength);

// --- minimales CBOR: nur das, was für Attestation „none“ und den COSE-Schlüssel nötig ist ---
const head = (major, length) => {
  if (length < 24) return Buffer.from([(major << 5) | length]);
  if (length < 256) return Buffer.from([(major << 5) | 24, length]);
  return Buffer.from([(major << 5) | 25, length >> 8, length & 0xff]);
};
const cText = (text) => Buffer.concat([head(3, Buffer.byteLength(text)), Buffer.from(text)]);
const cBytes = (bytes) => Buffer.concat([head(2, bytes.length), bytes]);
const cInt = (n) => (n >= 0 ? head(0, n) : head(1, -1 - n));
const cMap = (entries) => Buffer.concat([head(5, entries.length), ...entries.flat()]);

export class DomError extends Error {
  constructor(name, message = name) {
    super(message);
    this.name = name;
  }
}

/** origin und rpId müssen zur Einstellung PASSKEY_ORIGIN des Backends passen. */
export function createSoftAuthenticator({ origin, rpId }) {
  const stored = []; // { id: Buffer, privateKey, userHandle: Buffer, counter }
  const calls = { create: 0, get: 0 };
  let nextError = null;

  const clientData = (type, challenge) =>
    Buffer.from(JSON.stringify({ type, challenge: b64u(challenge), origin, crossOrigin: false }));

  const credentials = {
    async create({ publicKey }) {
      calls.create += 1;
      if (nextError) throw nextError;
      const selection = publicKey.authenticatorSelection ?? {};
      if (selection.residentKey !== 'required') throw new DomError('NotSupportedError', 'Passkey muss auffindbar sein');
      for (const excluded of publicKey.excludeCredentials ?? []) {
        if (stored.some((entry) => entry.id.equals(Buffer.from(excluded.id)))) {
          throw new DomError('InvalidStateError');
        }
      }
      const { publicKey: pub, privateKey } = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
      const jwk = pub.export({ format: 'jwk' });
      const id = randomBytes(32);
      stored.push({ id, privateKey, userHandle: Buffer.from(publicKey.user.id), counter: 0 });

      const coseKey = cMap([
        [cInt(1), cInt(2)],
        [cInt(3), cInt(-7)],
        [cInt(-1), cInt(1)],
        [cInt(-2), cBytes(Buffer.from(jwk.x, 'base64url'))],
        [cInt(-3), cBytes(Buffer.from(jwk.y, 'base64url'))],
      ]);
      const authData = Buffer.concat([
        sha256(rpId),
        Buffer.from([0x01 | 0x04 | 0x40]),
        Buffer.alloc(4),
        Buffer.alloc(16),
        Buffer.from([id.length >> 8, id.length & 0xff]),
        id,
        coseKey,
      ]);
      const attestationObject = cMap([
        [cText('fmt'), cText('none')],
        [cText('attStmt'), cMap([])],
        [cText('authData'), cBytes(authData)],
      ]);
      return {
        id: b64u(id),
        rawId: toArrayBuffer(id),
        type: 'public-key',
        response: {
          attestationObject: toArrayBuffer(attestationObject),
          clientDataJSON: toArrayBuffer(clientData('webauthn.create', Buffer.from(publicKey.challenge))),
        },
        getClientExtensionResults: () => ({ credProps: { rk: true } }),
      };
    },

    async get({ publicKey }) {
      calls.get += 1;
      if (nextError) throw nextError;
      const allowed = (publicKey.allowCredentials ?? []).map((entry) => Buffer.from(entry.id));
      const entry = stored.find((e) => allowed.length === 0 || allowed.some((id) => id.equals(e.id)));
      if (!entry) throw new DomError('NotAllowedError');
      entry.counter += 1;
      const counter = Buffer.alloc(4);
      counter.writeUInt32BE(entry.counter);
      const authData = Buffer.concat([sha256(rpId), Buffer.from([0x01 | 0x04]), counter]);
      const data = clientData('webauthn.get', Buffer.from(publicKey.challenge));
      const signature = sign('sha256', Buffer.concat([authData, sha256(data)]), entry.privateKey);
      return {
        id: b64u(entry.id),
        rawId: toArrayBuffer(entry.id),
        type: 'public-key',
        response: {
          authenticatorData: toArrayBuffer(authData),
          clientDataJSON: toArrayBuffer(data),
          signature: toArrayBuffer(signature),
          userHandle: toArrayBuffer(entry.userHandle),
        },
        getClientExtensionResults: () => ({}),
      };
    },
  };

  return {
    credentials,
    calls,
    /** Wie viele Passkeys das Gerät hält. */
    count: () => stored.length,
    /** Der nächste Aufruf scheitert mit diesem Fehler (z. B. NotAllowedError für „Abbrechen“). */
    failNextWith: (name) => {
      nextError = new DomError(name);
    },
    clearFailure: () => {
      nextError = null;
    },
    /** Hängt das Gerät als `navigator.credentials` in das jsdom-Fenster. */
    install(window) {
      window.PublicKeyCredential = function PublicKeyCredential() {};
      Object.defineProperty(window.navigator, 'credentials', { value: credentials, configurable: true });
    },
  };
}
