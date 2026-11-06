//! The forwarding page's markup. Every `__PLACEHOLDER__` is filled by `listen_handler`, escaped.

pub(super) const TEMPLATE: &str = r##"<!doctype html>

<html lang="en">

<head>

<meta charset="utf-8">

<meta name="viewport" content="width=device-width, initial-scale=1">

<meta name="robots" content="noindex, nofollow">

<meta name="referrer" content="no-referrer">

<title>__TITLE__</title>

__PREVIEW_META__

<style>
  :root {
    --bg: #000000;
    --surface: #121212;
    --surface-hover: #1c1c1c;
    --border: #262626;
    --text-main: #f2f2f2;
    --text-muted: #7e7e7e;
    --accent: #ffffff;
    --on-accent: #000000;
  }
  * { box-sizing: border-box; margin: 0; padding: 0; }
  body {
    min-height: 100vh; min-height: 100dvh;
    background-color: var(--bg);
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Oxygen, Ubuntu, Cantarell, "Fira Sans", "Droid Sans", "Helvetica Neue", sans-serif;
    color: var(--text-main); display: flex; align-items: center; justify-content: center; padding: 1.25rem;
    -webkit-font-smoothing: antialiased;
  }
  .container {
    width: 100%; max-width: 380px; background: var(--surface);
    border: 1px solid var(--border); border-radius: 6px; padding: 1.5rem; display: flex; flex-direction: column;
  }
  .tag { font-size: 0.7rem; font-weight: 700; letter-spacing: 0.08em; text-transform: uppercase; color: var(--text-muted); margin-bottom: 0.5rem; }
  h1 { font-size: 1.15rem; font-weight: 600; color: var(--text-main); letter-spacing: -0.01em; margin-bottom: 0.25rem; }
  .subtitle { font-size: 0.82rem; color: var(--text-muted); line-height: 1.4; margin-bottom: 1.4rem; }
  .actions { display: flex; flex-direction: column; gap: 0.6rem; }
  .btn {
    display: inline-flex; align-items: center; justify-content: center; gap: 0.5rem; height: 42px; border-radius: 4px;
    font-size: 0.88rem; font-weight: 600; text-decoration: none; cursor: pointer; transition: background-color 0.1s ease, border-color 0.1s ease;
  }
  .btn-primary { background: var(--accent); color: var(--on-accent); border: 1px solid var(--accent); }
  .btn-primary:active { background: #d4d4d4; }
  .btn-secondary { background: transparent; color: var(--text-main); border: 1px solid var(--border); }
  .btn-secondary:hover { background: var(--surface-hover); }
  .btn-secondary:active { background: #242424; }
  .progress-box { margin-top: 1.2rem; display: flex; flex-direction: column; gap: 0.4rem; }
  .progress-track { width: 100%; height: 2px; background: var(--border); overflow: hidden; }
  .progress-bar { height: 100%; width: 100%; background: var(--accent); transition: width 0.08s linear; }
  .progress-info { display: flex; justify-content: space-between; align-items: center; font-size: 0.75rem; color: var(--text-muted); }
  .btn-cancel { background: none; border: none; color: var(--text-main); cursor: pointer; font-size: 0.75rem; text-decoration: underline; padding: 0; }
  .target-url { font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace; font-size: 0.7rem; color: #555555; word-break: break-all; margin-top: 1rem; line-height: 1.3; }

</style>

</head>

<body>

<main class="container">
  <div class="tag">Wanda &middot; Link Gateway</div>
  <h1>__HEADING__</h1>
  <p class="subtitle">__SUBHEADING__</p>
  <div class="actions">
    <a id="btnWanda" class="btn btn-primary" href="wanda://listen__SEARCH_PARAMS__">Open in Wanda</a>
    <a id="btnWeb" class="btn btn-secondary" href="__TARGET__" rel="noreferrer">Open in Browser</a>
  </div>
  <div class="progress-box" id="progressWrap">
    <div class="progress-track"><div class="progress-bar" id="progressFill"></div></div>
    <div class="progress-info">
      <span id="countdownText">Forwarding in 2s&hellip;</span>
      <button class="btn-cancel" id="btnPause">Cancel</button>
    </div>
  </div>
  <p class="target-url">__TARGET__</p>

</main>

<script>
  (function() {
    var target = __TARGET_JSON__;
    var search = __SEARCH_JSON__;
    var isAndroid = /android/i.test(navigator.userAgent);
    if (isAndroid) {
      var intent = "intent://listen" + search + "#Intent;scheme=wanda;package=com.wander.android;S.browser_fallback_url=" + encodeURIComponent(target) + ";end";
      var btn = document.getElementById("btnWanda");
      if (btn) btn.href = intent;
      try { window.location.href = intent; } catch(e) {}
    }
    var duration = 2000, start = Date.now(), isPaused = false;
    var fill = document.getElementById("progressFill");
    var txt = document.getElementById("countdownText");
    var btnPause = document.getElementById("btnPause");
    if (btnPause) {
      btnPause.onclick = function() {
        isPaused = true;
        var wrap = document.getElementById("progressWrap");
        if (wrap) wrap.style.display = "none";
      };
    }
    function tick() {
      if (isPaused) return;
      var remaining = Math.max(0, duration - (Date.now() - start));
      if (fill) fill.style.width = ((remaining / duration) * 100) + "%";
      if (txt) txt.textContent = "Forwarding in " + (remaining / 1000).toFixed(1) + "s…";
      if (remaining > 0) {
        requestAnimationFrame(tick);
      } else if (target && target.length > 0) {
        window.location.replace(target);
      }
    }
    requestAnimationFrame(tick);
  })();

</script>

</body>

</html>"##;
