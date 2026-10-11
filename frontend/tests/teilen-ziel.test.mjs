// Web Share Target: Die Teilen-Funktion des Betriebssystems öffnet die App mit ?url=…&text=…
import { after, test } from 'node:test';
import assert from 'node:assert/strict';

import { ARTIKEL } from './fixtures.mjs';
import { setup, steps } from './helpers.mjs';

const { $, visible, until, teardown } = await setup(
  { '/artikel.html': ARTIKEL },
  {
    // Android teilt oft „Titel Adresse“ im Textfeld; die Adresse kennt der Test erst nach dem Start.
    url: (site) =>
      `http://app.test/?title=Lesenswert&text=${encodeURIComponent(`Schau mal ${site.url('/artikel.html')} lohnt sich`)}`,
  },
);
after(teardown);

test('Geteilte Adresse wird nach der Anmeldung gespeichert', async (t) => {
  const ok = steps(t);

  // Nicht angemeldet: Anmeldemaske, die Adresse wartet, die Adressleiste ist aufgeräumt.
  await until(() => visible('view-auth'), 'Anmeldemaske');
  assert.equal(window.location.search, '', 'Parameter sind aus der Adresszeile entfernt');
  ok('Parameter entfernt, Anmeldung verlangt');

  $('tab-register').click();
  $('form-auth').elements.email.value = 'teilen@example.org';
  $('form-auth').elements.password.value = 'passwort-12345';
  $('form-auth').requestSubmit();
  await until(() => visible('view-home'), 'Startseite');
  await until(() => /Gespeichert: E2E Testartikel/.test($('home-notice').textContent), 'Artikel gespeichert');
  await until(() => document.querySelectorAll('#article-list li').length === 1, 'Artikel in der Liste');
  assert.equal($('list-error').hidden, true);
  ok('Artikel nach der Anmeldung gespeichert');
});
