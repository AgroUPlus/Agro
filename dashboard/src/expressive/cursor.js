// The promo site's M3 Expressive cursor (docs/cursor.js), ported for the dashboard: a dot that sits
// exactly on the pointer and a ring that follows on a spring, then morphs into the shape of
// whatever control it is over. Mouse-like pointers only; touch and reduced-motion users keep the
// native cursor. Unlike the site, the dashboard is mostly forms, so over a text field the native
// caret comes back and the ring steps aside.

const INTERACTIVE =
  'button:not(:disabled), .btn, a.auth-secondary, a.auth-footer-link, [role="button"], [role="tab"], label.switch';
const TEXT_ENTRY = 'input, textarea, select, [contenteditable="true"]';
const REST = 32;
const KEYS = ['x', 'y', 'w', 'h', 'r'];
const SPRING = { x: [620, 40], y: [620, 40], w: [420, 31], h: [420, 31], r: [420, 31] };

/** Starts the cursor and returns a function that removes it again. */
export function startExpressiveCursor() {
  const fine = window.matchMedia('(hover: hover) and (pointer: fine)').matches;
  const calm = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  if (!fine || calm) return () => {};

  const dot = document.createElement('div');
  const ring = document.createElement('div');
  dot.className = 'cursor-dot';
  ring.className = 'cursor-ring';
  // Both start off-screen: until the pointer first moves, neither has anywhere to be.
  dot.style.transform = 'translate3d(-100px, -100px, 0)';
  document.body.append(ring, dot);
  document.documentElement.classList.add('has-custom-cursor');

  const pointer = { x: -100, y: -100 };
  const now = { x: -100, y: -100, w: REST, h: REST, r: REST / 2 };
  const speed = { x: 0, y: 0, w: 0, h: 0, r: 0 };
  let target = null;
  let goal = null;
  let pressed = false;
  let frame = 0;

  function measure() {
    if (!target || !target.isConnected) {
      goal = null;
      return;
    }
    const box = target.getBoundingClientRect();
    const radius = parseFloat(getComputedStyle(target).borderTopLeftRadius) || 8;
    goal = {
      x: box.left + box.width / 2,
      y: box.top + box.height / 2,
      w: box.width,
      h: box.height,
      r: Math.min(radius, Math.min(box.width, box.height) / 2),
    };
  }

  let prev = performance.now();
  function step(time) {
    const dt = Math.min((time - prev) / 1000, 1 / 30);
    prev = time;
    // Controls move under a still pointer (hover scale, entrance animations), so re-measure.
    if (target) measure();
    const aim = goal || { x: pointer.x, y: pointer.y, w: REST, h: REST, r: REST / 2 };
    const squish = pressed ? 0.88 : 1;
    for (const key of KEYS) {
      const want = key === 'x' || key === 'y' ? aim[key] : aim[key] * squish;
      const [stiffness, damping] = SPRING[key];
      speed[key] += ((want - now[key]) * stiffness - speed[key] * damping) * dt;
      now[key] += speed[key] * dt;
    }
    const radius = Math.max(0, Math.min(now.r, now.w / 2, now.h / 2));
    ring.style.transform = `translate3d(${now.x - now.w / 2}px, ${now.y - now.h / 2}px, 0)`;
    ring.style.width = `${Math.max(0, now.w)}px`;
    ring.style.height = `${Math.max(0, now.h)}px`;
    ring.style.borderRadius = `${radius}px`;
    frame = requestAnimationFrame(step);
  }

  const onMove = (e) => {
    pointer.x = e.clientX;
    pointer.y = e.clientY;
    dot.style.transform = `translate3d(${e.clientX}px, ${e.clientY}px, 0)`;
    const el = e.target instanceof Element ? e.target : null;
    const typing = Boolean(el?.closest(TEXT_ENTRY));
    ring.classList.toggle('typing', typing);
    dot.classList.toggle('typing', typing);
    const next = typing ? null : el?.closest(INTERACTIVE) ?? null;
    if (next !== target) {
      target = next;
      measure();
      ring.classList.toggle('on-target', Boolean(next));
      dot.classList.toggle('hidden', Boolean(next));
    }
  };
  const onDown = () => { pressed = true; ring.classList.add('pressed'); };
  const onUp = () => { pressed = false; ring.classList.remove('pressed'); };
  const onLeave = () => { ring.classList.add('away'); dot.classList.add('hidden'); };
  const onEnter = () => { ring.classList.remove('away'); dot.classList.toggle('hidden', Boolean(target)); };

  window.addEventListener('pointermove', onMove, { passive: true });
  window.addEventListener('pointerdown', onDown);
  window.addEventListener('pointerup', onUp);
  document.addEventListener('pointerleave', onLeave);
  document.addEventListener('pointerenter', onEnter);
  frame = requestAnimationFrame(step);

  return () => {
    cancelAnimationFrame(frame);
    window.removeEventListener('pointermove', onMove);
    window.removeEventListener('pointerdown', onDown);
    window.removeEventListener('pointerup', onUp);
    document.removeEventListener('pointerleave', onLeave);
    document.removeEventListener('pointerenter', onEnter);
    ring.remove();
    dot.remove();
    document.documentElement.classList.remove('has-custom-cursor');
  };
}
