// M3 Expressive cursor: a dot that sits exactly on the pointer and a ring that follows on a spring,
// then morphs into the shape of whatever clickable thing it is over. Mouse-like pointers only;
// touch and reduced-motion users keep the native cursor.
(() => {
  const fine = window.matchMedia('(hover: hover) and (pointer: fine)').matches;
  const calm = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  if (!fine || calm) return;

  // Action buttons only: not images, cards, list rows or inline text links.
  const INTERACTIVE = 'button:not(:disabled), .btn, .nav-link, .menu-item, [role="button"]';
  const REST = 32;

  const dot = document.createElement('div');
  const ring = document.createElement('div');
  dot.className = 'cursor-dot';
  ring.className = 'cursor-ring';
  document.body.append(ring, dot);
  document.documentElement.classList.add('has-custom-cursor');

  const pointer = { x: -100, y: -100 };
  // Each channel is a damped spring: position follows quickly, size/shape morph a touch looser.
  const keys = ['x', 'y', 'w', 'h', 'r'];
  const now = { x: -100, y: -100, w: REST, h: REST, r: REST / 2 };
  const speed = { x: 0, y: 0, w: 0, h: 0, r: 0 };
  const SPRING = { x: [620, 40], y: [620, 40], w: [420, 31], h: [420, 31], r: [420, 31] };

  let target = null;
  let goal = null; // geometry of `target`, refreshed every frame while there is one
  let pressed = false;

  function measure() {
    if (!target || !target.isConnected) {
      goal = null;
      return;
    }
    const box = target.getBoundingClientRect();
    // A state layer sitting exactly on the button: same shape, no outline, no overhang.
    const radius = parseFloat(getComputedStyle(target).borderTopLeftRadius) || 8;
    const pad = 0;
    const w = box.width + pad * 2;
    const h = box.height + pad * 2;
    goal = { x: box.left + box.width / 2, y: box.top + box.height / 2, w, h, r: Math.min(radius + pad, Math.min(w, h) / 2) };
  }

  function step(prev) {
    return (time) => {
      const dt = Math.min((time - prev) / 1000, 1 / 30);
      // Re-read the target every frame: buttons move under the cursor (navbar shrinking, hover
      // scale, entrance animations) and a cached rect would leave the ring on their old spot.
      if (target) measure();
      const aim = goal || { x: pointer.x, y: pointer.y, w: REST, h: REST, r: REST / 2 };
      const squish = pressed ? 0.88 : 1;
      for (const key of keys) {
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
      requestAnimationFrame(step(time));
    };
  }

  window.addEventListener('pointermove', (e) => {
    pointer.x = e.clientX;
    pointer.y = e.clientY;
    dot.style.transform = `translate3d(${e.clientX}px, ${e.clientY}px, 0)`;
    const next = e.target instanceof Element ? e.target.closest(INTERACTIVE) : null;
    if (next !== target) {
      target = next;
      measure();
      ring.classList.toggle('on-target', Boolean(next));
      dot.classList.toggle('hidden', Boolean(next));
    }
  }, { passive: true });
  window.addEventListener('pointerdown', () => { pressed = true; ring.classList.add('pressed'); });
  window.addEventListener('pointerup', () => { pressed = false; ring.classList.remove('pressed'); });
  document.addEventListener('pointerleave', () => { ring.classList.add('away'); dot.classList.add('hidden'); });
  document.addEventListener('pointerenter', () => { ring.classList.remove('away'); dot.classList.toggle('hidden', Boolean(target)); });

  requestAnimationFrame(step(performance.now()));
})();
