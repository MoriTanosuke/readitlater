// Schwebender „Zur Liste“-Link im Lesemodus.
//
// Oben steht der Link ganz normal im Text. Scrollt der Benutzer weiter, erscheint er zusätzlich
// als Bubble oben links über dem Text, damit er jederzeit zurück zur Liste kommt. Ganz unten am
// Seitenende steht ein zweiter normaler Link; sobald der zu sehen ist, verschwindet die Bubble.
//
// Die Bubble ist sichtbar, wenn der Link oben aus dem Bild gescrollt ist und der Link unten noch
// nicht im Bild ist. Gemessen wird bei jedem Scrollen (höchstens einmal pro Bild), nicht per
// IntersectionObserver: Der meldet nur Änderungen und kennt den Zustand nicht neu, wenn die
// Ansicht erneut geöffnet wird oder die Position nach dem Öffnen per Skript springt.

function isRendered(rect) {
  return rect.height > 0 || rect.width > 0;
}

/**
 * Soll die Bubble zu sehen sein? `topBottom` ist die Unterkante des Links oben, `bottomTop` die
 * Oberkante des Links unten (beides Fensterkoordinaten), `viewport` die Höhe des Fensters.
 */
export function floatVisible({ topBottom, bottomTop, viewport }) {
  const topGone = topBottom <= 0; // der Link oben ist nach oben aus dem Bild gescrollt
  const bottomShown = bottomTop < viewport; // der Link am Seitenende ist (teilweise) zu sehen
  return topGone && !bottomShown;
}

/**
 * `bubble`: schwebender Link, `top`/`bottom`: normale Links oben und unten, `content`: der
 * Artikeltext (nachladende Bilder verschieben das Seitenende), `isActive()`: ist der Lesemodus
 * gerade zu sehen?
 */
export function createBackBubble({ bubble, top, bottom, content, isActive }) {
  let frame = null;

  function update() {
    frame = null;
    let show = false;
    if (isActive()) {
      const topRect = top.getBoundingClientRect();
      const bottomRect = bottom.getBoundingClientRect();
      if (isRendered(topRect) && isRendered(bottomRect)) {
        const viewport = document.documentElement.clientHeight || window.innerHeight;
        show = floatVisible({ topBottom: topRect.bottom, bottomTop: bottomRect.top, viewport });
      }
    }
    if (bubble.hidden === show) bubble.hidden = !show;
  }

  /** Misst beim nächsten Bild neu; viele Scroll-Ereignisse pro Bild kosten nur einen Aufruf. */
  function schedule() {
    if (frame !== null || !isActive()) return;
    frame = window.requestAnimationFrame(update);
  }

  window.addEventListener('scroll', schedule, { passive: true });
  window.addEventListener('resize', schedule);
  content.addEventListener('load', schedule, true); // Bilder im Text

  return {
    schedule,
    update,

    /**
     * Beim Wechsel der Ansicht: Die Bubble verschwindet sofort (sonst blitzt sie beim erneuten
     * Öffnen kurz an der alten Stelle auf) und wird nach der nächsten Messung gezeigt, falls nötig.
     */
    reset() {
      if (frame !== null) window.cancelAnimationFrame(frame);
      frame = null;
      bubble.hidden = true;
      schedule();
    },
  };
}
