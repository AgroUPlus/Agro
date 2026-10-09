// Shrinks the floating navbar once the page is scrolled. The gap between the two thresholds stops
// it flickering when the resize itself nudges the scroll position.
(() => {
  const wrapper = document.getElementById('navbar-wrapper');
  window.addEventListener('scroll', () => {
    if (window.scrollY > 60) wrapper.classList.add('scrolled');
    else if (window.scrollY < 10) wrapper.classList.remove('scrolled');
  }, { passive: true });
})();
