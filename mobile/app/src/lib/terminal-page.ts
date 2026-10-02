// The page a host terminal is drawn in: xterm.js, and a `window.ket` the app
// calls to feed it. What is typed goes back through
// `window.ReactNativeWebView.postMessage`, which the WebView provides on a
// phone and `TerminalView.web` provides in a browser.

import { t } from './theme';
import { XTERM_CSS, XTERM_JS, XTERM_WEBGL_JS } from './xterm-assets';

export const page = `<!doctype html>
<html><head>
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no">
<style>${XTERM_CSS}
html { height: 100%; overflow: hidden; overscroll-behavior: none; touch-action: none; }
#terminal, .xterm, .xterm-viewport, .xterm-screen { touch-action: none; }
#hist { position: fixed; left: 50%; bottom: 12px; transform: translateX(-50%); display: none;
  padding: 7px 14px; border-radius: 16px; font: 500 13px -apple-system, system-ui, sans-serif;
  background: ${t.accent}; color: ${t.card}; box-shadow: 0 4px 14px rgba(0,0,0,.45); z-index: 10; }
html, body { margin: 0; padding: 0; background: ${t.card}; }
#terminal { padding: 6px 4px; display: inline-block; }
.xterm { overflow: hidden; }
.xterm-screen { will-change: transform; }
.xterm-viewport { overflow-y: auto !important; }
</style></head>
<body><div id="terminal"></div><div id="hist"></div>
<script>${XTERM_JS}</script>
<script>${XTERM_WEBGL_JS}</script>
<script>
(function () {
  var ALPHABET = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_';
  function decode(text) {
    var out = new Uint8Array(Math.floor(text.length * 3 / 4));
    var bits = 0, value = 0, at = 0;
    for (var i = 0; i < text.length; i++) {
      value = (value << 6) | ALPHABET.indexOf(text[i]);
      bits += 6;
      if (bits >= 8) { bits -= 8; out[at++] = (value >> bits) & 0xff; }
    }
    return out.subarray(0, at);
  }
  var term = null;
  function post(message) { window.ReactNativeWebView.postMessage(JSON.stringify(message)); }
  var page = document.scrollingElement || document.documentElement;

  // The page is pinned to its bottom: the host's screen can be taller than
  // the phone's, and the bottom is where the prompt is. Up and down is the
  // terminal's own history instead — every vertical swipe moves it, a line
  // per row of finger travel — while sideways still pans a screen wider than
  // the phone. Scrolling the page first and the history only past its top
  // left people scrolling a page with nothing new in it.
  function toEnd() { page.scrollTop = page.scrollHeight; }
  var lastX = 0, lastY = 0, axis = null;
  // A row's height in pixels, measured once per font size rather than on
  // every finger movement: reading layout mid-gesture forced the browser to
  // lay the page out again each time.
  var rowPx = 14;
  function measureRow() {
    var screen = document.querySelector('.xterm-screen');
    if (term && screen && screen.offsetHeight) rowPx = screen.offsetHeight / term.rows;
    else if (term) rowPx = term.options.fontSize * 1.2;
  }
  var startX = 0, startY = 0;
  // Finger travel not yet a whole line. The history itself moves a line at a
  // time, so the screen is slid by what is left over — the text follows the
  // finger to the pixel, and a line is scrolled each time a row's height has
  // gone by. At rest the slide settles on a line. Applied once a frame:
  // touchmove can fire faster than the screen draws.
  var carry = 0, framed = false;
  // A flick keeps going and slows, as a native list does. Its speed comes
  // from the last tenth of a second of the swipe.
  var samples = [], velocity = 0, gliding = false, settling = false, lastFrame = 0;
  function sample(y) {
    var now = performance.now();
    samples.push({ t: now, y: y });
    while (samples.length > 2 && now - samples[0].t > 100) samples.shift();
  }
  function schedule() {
    if (framed) return;
    framed = true;
    requestAnimationFrame(frame);
  }
  function frame(now) {
    framed = false;
    var dt = lastFrame ? Math.min(48, now - lastFrame) : 0;
    lastFrame = now;
    if (gliding) {
      carry += velocity * dt;
      velocity *= Math.pow(0.95, dt / 16);
      if (Math.abs(velocity) < 0.05) { gliding = false; settling = true; }
    }
    if (settling) {
      // To the nearer line, quickly and without overshoot.
      var target = Math.abs(carry) > rowPx / 2 ? (carry > 0 ? rowPx : -rowPx) : 0;
      carry += (target - carry) * Math.min(1, dt / 45);
      if (Math.abs(target - carry) < 0.5) { carry = target; settling = false; }
    }
    var lines = (carry / rowPx) | 0;
    if (lines !== 0) {
      carry -= lines * rowPx;
      if (!scrollBy(lines)) { gliding = false; carry = 0; }
    }
    clampCarry();
    place();
    if (gliding || settling) schedule();
  }
  // No slide past either end of the history.
  function clampCarry() {
    if (!term) return;
    var buffer = term.buffer.active;
    if (buffer.type === 'alternate') { carry = 0; return; }
    if (carry > 0 && buffer.viewportY >= buffer.baseY) { carry = 0; gliding = false; }
    if (carry < 0 && buffer.viewportY === 0) {
      if (carry < -4) note(buffer.baseY === 0 ? 'No earlier output here' : 'Start of history');
      carry = 0;
      gliding = false;
    }
  }
  var slid = 0;
  function place() {
    if (carry === slid) return;
    slid = carry;
    var screen = document.querySelector('.xterm-screen');
    if (screen) screen.style.transform = carry ? 'translate3d(0,' + (-carry) + 'px,0)' : '';
  }
  // A touch catches a glide or a settle where it is, as a native list does.
  function stopGlide() { gliding = false; settling = false; velocity = 0; }
  // Captured on the window, ahead of xterm's own handlers and the browser's:
  // this page owns every gesture on the terminal.
  window.addEventListener('touchstart', function (e) {
    if (e.touches.length !== 1) return;
    stopGlide();
    lastX = startX = e.touches[0].clientX; lastY = startY = e.touches[0].clientY; axis = null;
    samples = [];
    sample(lastY);
  }, { passive: true, capture: true });

  // Where the view is in the history, said on screen: a pill back to the
  // latest output while scrolled up, and a note when there is nothing above
  // — an agent drawing on the terminal's alternate screen keeps no history.
  var hist = document.getElementById('hist'), histTimer = null;
  function showHistory() {
    if (!term) return;
    var buffer = term.buffer.active;
    if (buffer.viewportY < buffer.baseY) {
      clearTimeout(histTimer);
      hist.textContent = '↓ Latest';
      hist.style.display = 'block';
    } else if (hist.textContent === '↓ Latest') {
      hist.style.display = 'none';
    }
  }
  function note(text) {
    hist.textContent = text;
    hist.style.display = 'block';
    clearTimeout(histTimer);
    histTimer = setTimeout(function () { hist.style.display = 'none'; }, 1600);
  }

  // A tap on a web address opens it. Nothing else would: the terminal draws
  // text, and xterm's own links wait for a mouse to hover first.
  var URL_PATTERN = /https?:\\/\\/[^\\s"'<>\\u0060]+/g;
  function linkAt(clientX, clientY) {
    var screen = document.querySelector('.xterm-screen');
    if (!term || !screen) return null;
    var rect = screen.getBoundingClientRect();
    var col = Math.floor((clientX - rect.left) / (rect.width / term.cols));
    var row = Math.floor((clientY - rect.top) / (rect.height / term.rows));
    if (col < 0 || row < 0 || col >= term.cols || row >= term.rows) return null;
    var buffer = term.buffer.active;
    // The whole logical line: back to where a wrap began, on to where it ends.
    var first = buffer.viewportY + row;
    while (first > 0 && buffer.getLine(first) && buffer.getLine(first).isWrapped) first--;
    var text = '', at = first, offset = 0;
    for (var line = buffer.getLine(at); line; line = buffer.getLine(++at)) {
      if (at > first && !line.isWrapped) break;
      if (at < buffer.viewportY + row) offset += term.cols;
      text += line.translateToString(false);
    }
    var target = offset + col, match;
    URL_PATTERN.lastIndex = 0;
    while ((match = URL_PATTERN.exec(text))) {
      var url = match[0].replace(/[.,;:!?)\\]}'"]+$/, '');
      if (target >= match.index && target < match.index + url.length) return url;
    }
    return null;
  }
  window.addEventListener('touchend', function () {
    if (!term || term.buffer.active.type === 'alternate') return;
    var first = samples[0], last = samples[samples.length - 1];
    // Pixels of history per millisecond; positive is toward newer output.
    velocity = axis === 'y' && first && last.t - first.t >= 8
      ? (first.y - last.y) / (last.t - first.t) : 0;
    if (Math.abs(velocity) >= 0.3) gliding = true;
    else { velocity = 0; settling = carry !== 0; }
    lastFrame = 0;
    if (gliding || settling) schedule();
  }, { passive: true, capture: true });
  window.addEventListener('touchend', function (e) {
    if (axis !== null || e.changedTouches.length !== 1) return;
    var t = e.changedTouches[0];
    if (Math.abs(t.clientX - startX) > 8 || Math.abs(t.clientY - startY) > 8) return;
    if (e.target === hist) {
      e.preventDefault();
      if (term) term.scrollToBottom();
      hist.style.display = 'none';
      return;
    }
    var url = linkAt(t.clientX, t.clientY);
    if (url) { e.preventDefault(); post({ type: 'open', url: url }); }
  }, { passive: false, capture: true });
  window.addEventListener('touchmove', function (e) {
    if (!term || e.touches.length !== 1) return;
    e.preventDefault();
    var x = e.touches[0].clientX, y = e.touches[0].clientY;
    var dx = lastX - x, dy = lastY - y;
    // The gesture's axis is decided once, by its first clear movement.
    if (axis === null) {
      if (Math.abs(dx) < 4 && Math.abs(dy) < 4) return;
      axis = Math.abs(dy) >= Math.abs(dx) ? 'y' : 'x';
    }
    lastX = x; lastY = y;
    // Sideways pans a screen wider than the phone.
    if (axis === 'x') { page.scrollLeft += dx; return; }
    sample(y);
    carry += dy;
    schedule();
  }, { passive: false, capture: true });

  // Moves the history by whole lines. False when it cannot go further, which
  // ends a glide.
  function scrollBy(lines) {
    // An agent drawing on the alternate screen — Codex, OpenCode, a full-
    // screen Claude — keeps its history itself, and the terminal has none.
    // The swipe goes to the agent instead, as a scroll wheel on the desktop
    // would: wheel events when it listens for the mouse, else Page Up/Down.
    if (term.buffer.active.type === 'alternate') { scrollAgent(lines); return true; }
    var buffer = term.buffer.active;
    if (lines < 0 && buffer.viewportY === 0) {
      note(buffer.baseY === 0 ? 'No earlier output here' : 'Start of history');
      return false;
    }
    if (lines > 0 && buffer.viewportY >= buffer.baseY) return false;
    term.scrollLines(lines);
    showHistory();
    return true;
  }

  var ESC = String.fromCharCode(27);
  var paged = 0;
  function scrollAgent(lines) {
    if (term.modes.mouseTrackingMode !== 'none') {
      // SGR wheel reports at the middle of the screen: 64 is up, 65 down.
      var at = ';' + Math.ceil(term.cols / 2) + ';' + Math.ceil(term.rows / 2) + 'M';
      var one = ESC + '[<' + (lines < 0 ? 64 : 65) + at;
      var out = '';
      for (var i = 0; i < Math.abs(lines); i++) out += one;
      post({ type: 'input', data: out });
    } else {
      // A page per half-screen of finger travel, so a flick is not ten pages.
      paged += lines;
      var step = Math.max(1, Math.floor(term.rows / 2));
      while (Math.abs(paged) >= step) {
        post({ type: 'input', data: ESC + (paged < 0 ? '[5~' : '[6~') });
        paged -= paged < 0 ? -step : step;
      }
    }
  }

  // The font follows the page's width: full screen, rotation.
  window.addEventListener('resize', function () {
    if (!term) return;
    term.options.fontSize = fontFor(term.cols);
    requestAnimationFrame(measureRow);
    toEnd();
    reportSize();
  });
  // Fitted: the host's terminal is resized to this page, so the type stays
  // at a size that reads comfortably and the page reports how many cells of
  // it fit. Otherwise the host's columns are squeezed across the screen.
  var fitted = false;
  var FIT_FONT = 13.5;
  function measure() {
    var width = window.innerWidth - 8, height = window.innerHeight - 12;
    return {
      cols: Math.max(20, Math.floor(width / (FIT_FONT * 0.602))),
      rows: Math.max(5, Math.floor(height / Math.ceil(FIT_FONT * 1.2))),
    };
  }
  var sizeTimer = null;
  function reportSize() {
    if (!fitted) return;
    clearTimeout(sizeTimer);
    sizeTimer = setTimeout(function () {
      var size = measure();
      post({ type: 'size', cols: size.cols, rows: size.rows });
    }, 150);
  }
  function fontFor(cols) {
    if (fitted) return FIT_FONT;
    // Menlo's advance is about 0.6 of its size: fit the host's columns across
    // the screen, but never below a size that can still be read.
    var fit = Math.floor((window.innerWidth - 8) / (cols * 0.602) * 10) / 10;
    return Math.max(6.5, Math.min(14, fit));
  }
  window.ket = function (message) {
    if (message.type === 'reset') {
      if (term) term.dispose();
      term = new Terminal({
        // Room for history beyond what the checkpoint carried, which can be
        // little or none; what arrives after it scrolls into this.
        cols: message.cols, rows: message.rows, scrollback: Math.max(message.scrollback || 0, 10000),
        fontFamily: 'Menlo, monospace', fontSize: fontFor(message.cols),
        theme: { background: '${t.card}', foreground: '${t.text}', cursor: '${t.accent}',
                 selectionBackground: '${t.selection}' },
        disableStdin: !window.ketInteractive, allowProposedApi: true, convertEol: false,
      });
      term.open(document.getElementById('terminal'));
      // Drawn on the GPU where the phone allows it: the DOM renderer rebuilt
      // every row on each line scrolled, which flashed and lagged the finger.
      // Without WebGL, or when the phone takes the context back, the DOM
      // renderer carries on.
      try {
        var webgl = new WebglAddon.WebglAddon();
        webgl.onContextLoss(function () { webgl.dispose(); });
        term.loadAddon(webgl);
      } catch (error) {}
      term.onData(function (data) { post({ type: 'input', data: data }); });
      term.onScroll(showHistory);
      stopGlide();
      carry = 0;
      slid = 0;
      term.write(decode(message.ansi), function () { measureRow(); toEnd(); });
    } else if (message.type === 'write' && term) {
      // Not pinned again: output does not change the page's height, and
      // setting the page's scroll on every chunk moved it under the finger.
      term.write(decode(message.bytes));
    } else if (message.type === 'interactive') {
      window.ketInteractive = message.on;
      // Control only lets the terminal take keys; it does not take focus.
      // Control is usually taken by focusing the message line, and the
      // terminal grabbing focus back pulled the keyboard away from it.
      if (term) { term.options.disableStdin = !message.on; if (!message.on) term.blur(); }
    } else if (message.type === 'fit') {
      fitted = message.on;
      if (term) { term.options.fontSize = fontFor(term.cols); requestAnimationFrame(measureRow); toEnd(); }
      reportSize();
    } else if (message.type === 'focus' && term) {
      term.focus();
    }
  };
  post({ type: 'ready' });
})();
</script></body></html>`;
