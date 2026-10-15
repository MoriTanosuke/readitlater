// Frontend-Ablauf: Passkey einrichten, mit Passkey ohne E-Mail anmelden, umbenennen und löschen.
// Das Gerät ist nachgebaut (soft-authenticator.mjs), Backend und App sind echt.
import { after, test } from 'node:test';
import assert from 'node:assert/strict';

import { createSoftAuthenticator } from './soft-authenticator.mjs';
import { setup, steps } from './helpers.mjs';

const ORIGIN = 'https://lesen.test';
const PASSWORD = 'passwort-12345';

const { $, visible, until, teardown } = await setup(
  {},
  { env: { PASSKEY_ORIGIN: ORIGIN } },
);
after(teardown);

const device = createSoftAuthenticator({ origin: ORIGIN, rpId: 'lesen.test' });

const waitForText = (id, pattern, label) =>
  until(() => pattern.test($(id).textContent), label);

async function logout() {
  $('btn-logout').click();
  await until(() => visible('view-auth'), 'Anmeldemaske nach Abmelden');
}

test('Passkey einrichten, ohne E-Mail anmelden, umbenennen und löschen', async (t) => {
  const ok = steps(t);

  // Ohne Unterstützung des Browsers erscheint der Knopf nicht, obwohl der Server Passkeys kann.
  await until(() => visible('view-auth'), 'Anmeldemaske');
  await new Promise((resolve) => setTimeout(resolve, 150)); // features-Antwort abwarten
  assert.equal(visible('passkey-login'), false, 'Knopf fehlt ohne WebAuthn im Browser');
  ok('ohne WebAuthn im Browser bleibt der Passkey-Knopf aus');

  // Konto anlegen
  $('tab-register').click();
  const auth = $('form-auth');
  auth.elements.email.value = 'passkey@example.org';
  auth.elements.password.value = PASSWORD;
  auth.requestSubmit();
  await until(() => visible('view-home'), 'Startseite');

  // Seite öffnen: ohne Gerät mit WebAuthn gibt es einen Hinweis und der Knopf ist gesperrt
  window.location.hash = '#/account/passkeys';
  await until(() => visible('view-passkeys'), 'Passkey-Seite');
  assert.equal(document.title, 'Passkeys – Leseliste');
  await until(() => !$('passkeys-unavailable').hidden, 'Hinweis ohne Browser-Unterstützung');
  assert.equal($('passkeys-add').disabled, true);
  assert.match($('passkeys-unavailable').textContent, /kann keine Passkeys/);
  ok('ohne Browser-Unterstützung erklärt die Seite, warum nichts geht');

  // Jetzt mit Gerät
  device.install(window);
  window.location.hash = '#/';
  await until(() => visible('view-home'), 'Liste');
  window.location.hash = '#/account/passkeys';
  await until(() => visible('view-passkeys'), 'Passkey-Seite erneut');
  await until(() => $('passkeys-unavailable').hidden && !$('passkeys-add').disabled, 'Formular freigegeben');
  await until(() => !$('passkeys-empty').hidden, 'Hinweis ohne Passkeys');
  ok('mit Gerät ist das Formular freigeschaltet, die Liste leer');

  // Falsches Passwort: Fehler, das Gerät wurde nicht gefragt
  const form = $('form-passkeys');
  form.elements.name.value = 'Mein Testgerät';
  form.elements.password.value = 'falsches-passwort';
  form.requestSubmit();
  await until(() => !$('passkeys-error').hidden, 'Fehler bei falschem Passwort');
  assert.equal($('passkeys-error').textContent, 'Passwort ist falsch');
  assert.equal(device.calls.create, 0, 'Gerät wurde nicht gefragt');
  ok('falsches Passwort wird abgelehnt, bevor das Gerät gefragt wird');

  // Abbrechen im Gerät: verständliche Meldung, nichts gespeichert
  device.failNextWith('NotAllowedError');
  form.elements.name.value = 'Mein Testgerät';
  form.elements.password.value = PASSWORD;
  form.requestSubmit();
  await until(() => /abgebrochen/.test($('passkeys-error').textContent), 'Meldung beim Abbrechen');
  assert.equal(document.querySelectorAll('#passkeys-list li').length, 0);
  device.clearFailure();
  ok('Abbruch im Gerät zeigt eine Meldung und speichert nichts');

  // Einrichten
  form.elements.name.value = 'Mein Testgerät';
  form.elements.password.value = PASSWORD;
  form.requestSubmit();
  await until(() => document.querySelectorAll('#passkeys-list li').length === 1, 'Passkey in der Liste');
  assert.equal($('passkeys-error').hidden, true);
  assert.match($('passkeys-notice').textContent, /Mein Testgerät.*eingerichtet/);
  assert.equal(device.count(), 1);
  const item = document.querySelector('#passkeys-list li');
  assert.equal(item.querySelector('strong').textContent, 'Mein Testgerät');
  assert.match(item.textContent, /noch nicht benutzt/);
  assert.equal(form.elements.password.value, '', 'Passwort wird nach dem Einrichten geleert');
  ok('Passkey wird eingerichtet und erscheint in der Liste');

  // Dasselbe Gerät kann keinen zweiten Passkey für dieses Konto einrichten
  form.elements.name.value = 'Doppelt';
  form.elements.password.value = PASSWORD;
  form.requestSubmit();
  await until(() => /schon einen Passkey/.test($('passkeys-error').textContent), 'Meldung bei doppeltem Gerät');
  assert.equal(document.querySelectorAll('#passkeys-list li').length, 1);
  ok('ein bereits eingerichtetes Gerät wird vom Browser abgelehnt');

  // Abmelden, dann zeigt die Anmeldeseite den Passkey-Knopf
  await logout();
  await until(() => visible('passkey-login'), 'Passkey-Knopf');
  $('tab-register').click();
  assert.equal(visible('passkey-login'), false, 'Knopf nur beim Anmelden');
  $('tab-login').click();
  assert.equal(visible('passkey-login'), true);
  ok('Passkey-Knopf erscheint nur im Reiter „Anmelden“');

  // Anmelden ohne E-Mail
  $('btn-passkey-login').click();
  await until(() => visible('view-home'), 'Startseite nach Passkey-Anmeldung');
  assert.equal($('user-email').textContent, 'passkey@example.org');
  assert.equal(device.calls.get, 1);
  assert.equal($('auth-error').hidden, true);
  ok('Anmeldung nur mit Passkey, ohne E-Mail und Passwort');

  // Die Sitzung ist echt
  const me = await (await fetch('/api/me')).json();
  assert.equal(me.email, 'passkey@example.org');

  // Letzte Nutzung wird angezeigt
  window.location.hash = '#/account/passkeys';
  await until(() => visible('view-passkeys'), 'Passkey-Seite');
  await until(
    () => /zuletzt benutzt am \d\d\.\d\d\.\d{4}/.test(document.querySelector('#passkeys-list li')?.textContent ?? ''),
    'letzte Nutzung',
  );
  ok('die Liste zeigt die letzte Nutzung');

  // Umbenennen
  window.prompt = () => '  Neuer   Name ';
  [...document.querySelectorAll('#passkeys-list button')].find((b) => b.textContent === 'Umbenennen').click();
  await waitForText('passkeys-list', /Neuer Name/, 'neuer Name');
  ok('Umbenennen ändert den Namen');

  // Löschen braucht das Passwort im Feld
  const remove = () => [...document.querySelectorAll('#passkeys-list button')].find((b) => b.textContent === 'Löschen');
  remove().click();
  await until(() => /Passwort unten eintragen/.test($('passkeys-error').textContent), 'Hinweis auf Passwort');
  assert.equal(document.querySelectorAll('#passkeys-list li').length, 1);
  form.elements.password.value = 'falsches-passwort';
  remove().click();
  await until(() => $('passkeys-error').textContent === 'Passwort ist falsch', 'Fehler bei falschem Passwort');
  assert.equal(document.querySelectorAll('#passkeys-list li').length, 1);
  form.elements.password.value = PASSWORD;
  remove().click();
  await until(() => document.querySelectorAll('#passkeys-list li').length === 0, 'Liste leer');
  assert.match($('passkeys-notice').textContent, /wurde gelöscht/);
  assert.equal(form.elements.password.value, '');
  ok('Löschen verlangt das Passwort und entfernt den Passkey');

  // Der gelöschte Passkey meldet nicht mehr an, das Passwort aber schon
  await logout();
  $('btn-passkey-login').click();
  await until(() => !$('auth-error').hidden, 'Fehler bei gelöschtem Passkey');
  assert.equal($('auth-error').textContent, 'Anmeldung mit Passkey fehlgeschlagen');
  assert.equal(visible('view-auth'), true);
  auth.elements.email.value = 'passkey@example.org';
  auth.elements.password.value = PASSWORD;
  auth.requestSubmit();
  await until(() => visible('view-home'), 'Anmeldung mit Passwort');
  ok('nach dem Löschen geht nur noch das Passwort');
});

test('Abbrechen bei der Anmeldung zeigt eine ruhige Meldung', async (t) => {
  const ok = steps(t);
  await logout();
  device.failNextWith('NotAllowedError');
  $('btn-passkey-login').click();
  await until(() => /abgebrochen/.test($('auth-error').textContent), 'Meldung beim Abbrechen');
  assert.equal(visible('view-auth'), true);
  assert.equal($('btn-passkey-login').disabled, false, 'Knopf ist wieder bedienbar');
  device.clearFailure();
  ok('Abbruch lässt die Anmeldeseite unverändert bedienbar');
});
