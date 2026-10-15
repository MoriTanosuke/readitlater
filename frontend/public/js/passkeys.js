// Passkeys (WebAuthn) im Browser: Optionen des Servers in die Form der Browser-Schnittstelle
// bringen und die Antwort des Geräts als JSON zurückgeben. Bewusst ohne
// `PublicKeyCredential.parseCreationOptionsFromJSON()` und `toJSON()`: Beides fehlt in
// älteren Browsern (Safari vor 18.4, Chrome vor 129), die Umwandlung ist dafür nur wenige Zeilen.

export class PasskeyError extends Error {
  /** cancelled: Der Benutzer hat abgebrochen oder die Zeit lief ab, kein Fehler im eigentlichen Sinn. */
  constructor(message, { cancelled = false } = {}) {
    super(message);
    this.name = 'PasskeyError';
    this.cancelled = cancelled;
  }
}

const credentialsOf = () => window.navigator.credentials;

/** Kann dieser Browser Passkeys erstellen und verwenden? (Braucht HTTPS oder localhost.) */
export function passkeysSupported() {
  const credentials = credentialsOf();
  return (
    typeof window.PublicKeyCredential === 'function' &&
    typeof credentials?.create === 'function' &&
    typeof credentials?.get === 'function'
  );
}

export function toBase64Url(buffer) {
  let text = '';
  for (const byte of new Uint8Array(buffer)) text += String.fromCharCode(byte);
  return btoa(text).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

export function fromBase64Url(text) {
  const padded = text.replace(/-/g, '+').replace(/_/g, '/').padEnd(Math.ceil(text.length / 4) * 4, '=');
  const binary = atob(padded);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
  return bytes.buffer;
}

/** Entfernt null-Werte: Die Schnittstelle des Browsers behandelt sie anders als fehlende Felder. */
function withoutNulls(value) {
  if (Array.isArray(value)) return value.map(withoutNulls);
  if (value && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value)
        .filter(([, entry]) => entry !== null)
        .map(([key, entry]) => [key, withoutNulls(entry)]),
    );
  }
  return value;
}

/** Übersetzt die Fehler des Browsers in verständliche Meldungen. */
function explain(error, action) {
  const name = error?.name;
  if (name === 'NotAllowedError' || name === 'AbortError') {
    return new PasskeyError(
      action === 'create'
        ? 'Das Einrichten wurde abgebrochen oder hat zu lange gedauert.'
        : 'Die Anmeldung wurde abgebrochen oder hat zu lange gedauert.',
      { cancelled: true },
    );
  }
  if (name === 'InvalidStateError') {
    return new PasskeyError('Auf diesem Gerät gibt es für dieses Konto schon einen Passkey.');
  }
  if (name === 'SecurityError') {
    return new PasskeyError(
      'Passkeys funktionieren nur über HTTPS und unter der Adresse, für die sie eingerichtet sind.',
    );
  }
  if (name === 'NotSupportedError') {
    return new PasskeyError('Dieses Gerät oder dieser Browser unterstützt keine Passkeys dieser Art.');
  }
  return new PasskeyError(
    action === 'create' ? 'Der Passkey konnte nicht eingerichtet werden.' : 'Die Anmeldung mit Passkey ist fehlgeschlagen.',
  );
}

/** Richtet einen Passkey ein. options: Antwort des Servers ({ publicKey: … }). */
export async function createPasskey(options) {
  const publicKey = withoutNulls(options.publicKey);
  publicKey.challenge = fromBase64Url(publicKey.challenge);
  publicKey.user = { ...publicKey.user, id: fromBase64Url(publicKey.user.id) };
  if (publicKey.excludeCredentials) {
    publicKey.excludeCredentials = publicKey.excludeCredentials.map((entry) => ({
      ...entry,
      id: fromBase64Url(entry.id),
    }));
  }

  let credential;
  try {
    credential = await credentialsOf().create({ publicKey });
  } catch (error) {
    throw explain(error, 'create');
  }
  if (!credential) throw new PasskeyError('Es wurde kein Passkey eingerichtet.', { cancelled: true });

  return {
    id: credential.id,
    rawId: toBase64Url(credential.rawId),
    type: credential.type,
    response: {
      attestationObject: toBase64Url(credential.response.attestationObject),
      clientDataJSON: toBase64Url(credential.response.clientDataJSON),
    },
    // Die Ergebnisse der Erweiterungen braucht der Server nicht (er liest sie aus den signierten
    // Daten) und ihre Form unterscheidet sich zwischen den Browsern, darum werden sie nicht gesendet.
    clientExtensionResults: {},
  };
}

/** Meldet mit einem Passkey an, ohne dass ein Konto vorgegeben ist. options: Feld `options` des Servers. */
export async function getPasskey(options) {
  const publicKey = withoutNulls(options.publicKey);
  publicKey.challenge = fromBase64Url(publicKey.challenge);
  if (publicKey.allowCredentials) {
    publicKey.allowCredentials = publicKey.allowCredentials.map((entry) => ({
      ...entry,
      id: fromBase64Url(entry.id),
    }));
  }

  let credential;
  try {
    credential = await credentialsOf().get({ publicKey });
  } catch (error) {
    throw explain(error, 'get');
  }
  if (!credential) throw new PasskeyError('Es wurde kein Passkey gewählt.', { cancelled: true });

  const response = credential.response;
  return {
    id: credential.id,
    rawId: toBase64Url(credential.rawId),
    type: credential.type,
    response: {
      authenticatorData: toBase64Url(response.authenticatorData),
      clientDataJSON: toBase64Url(response.clientDataJSON),
      signature: toBase64Url(response.signature),
      userHandle: response.userHandle ? toBase64Url(response.userHandle) : null,
    },
    clientExtensionResults: {},
  };
}
