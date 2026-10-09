// Click a screenshot to maximise it: the image grows from where it sits to fill the viewport,
// and shrinks back to its place on close (click, or Escape).
(() => {
  const calm = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  const SPRING = 'cubic-bezier(0.34, 1.3, 0.64, 1)';
  const MS = calm ? 0 : 420;

  // Images that are links, buttons or chrome keep their own click.
  const zoomable = [...document.querySelectorAll('img')].filter(
    (img) => !img.closest('a, button, .navbar-wrapper, .modal, .chart-cover')
  );
  zoomable.forEach((img) => img.classList.add('zoomable'));

  let open = null;

  function fit(img) {
    const margin = 32;
    const ratio = img.naturalWidth / img.naturalHeight || 1;
    let w = Math.min(window.innerWidth - margin * 2, img.naturalWidth || Infinity);
    let h = w / ratio;
    const maxH = window.innerHeight - margin * 2;
    if (h > maxH) { h = maxH; w = h * ratio; }
    return { left: (window.innerWidth - w) / 2, top: (window.innerHeight - h) / 2, width: w, height: h };
  }

  function frame(box) {
    return { left: `${box.left}px`, top: `${box.top}px`, width: `${box.width}px`, height: `${box.height}px` };
  }

  function show(source) {
    if (open) return;
    const from = source.getBoundingClientRect();
    const backdrop = document.createElement('div');
    backdrop.className = 'lightbox-backdrop';
    backdrop.setAttribute('role', 'dialog');
    backdrop.setAttribute('aria-modal', 'true');
    backdrop.setAttribute('aria-label', source.alt || 'Image');
    const big = document.createElement('img');
    big.className = 'lightbox-img';
    big.src = source.currentSrc || source.src;
    big.alt = source.alt;
    Object.assign(big.style, frame(from), { borderRadius: getComputedStyle(source).borderRadius });
    backdrop.append(big);
    document.body.append(backdrop);
    source.style.visibility = 'hidden';
    open = { source, backdrop, big };

    backdrop.animate([{ opacity: 0 }, { opacity: 1 }], { duration: MS, fill: 'forwards' });
    big.animate([frame(from), frame(fit(source))], { duration: MS, easing: SPRING, fill: 'forwards' });
    backdrop.addEventListener('click', hide);
  }

  function hide() {
    if (!open) return;
    const { source, backdrop, big } = open;
    open = null;
    const back = frame(source.getBoundingClientRect());
    backdrop.animate([{ opacity: 1 }, { opacity: 0 }], { duration: MS, fill: 'forwards' });
    big.animate([frame(fit(source)), back], { duration: MS, easing: 'cubic-bezier(0.2, 0, 0, 1)', fill: 'forwards' })
      .finished.then(() => {
        source.style.visibility = '';
        backdrop.remove();
      });
  }

  document.addEventListener('click', (e) => {
    const img = e.target instanceof Element ? e.target.closest('img.zoomable') : null;
    if (img) show(img);
  });
  window.addEventListener('keydown', (e) => { if (e.key === 'Escape') hide(); });
})();
