/**
 * Material 3 Expressive shapes, from the shape library Wanda draws its shelves with. Each is a
 * radius that swells and dips `lobes` times around the circle, sampled finely enough that the
 * outline reads as smooth curves rather than a polygon. Paths are in a 100×100 box.
 */
export function cookiePath(lobes, depth, steps = 180) {
  const points = [];
  for (let i = 0; i < steps; i += 1) {
    const angle = (i / steps) * Math.PI * 2;
    const radius = 50 * (1 - depth + depth * Math.cos(lobes * angle));
    points.push(`${(50 + radius * Math.cos(angle)).toFixed(2)},${(50 + radius * Math.sin(angle)).toFixed(2)}`);
  }
  return `M${points.join('L')}Z`;
}

export const PATHS = {
  cookie: cookiePath(9, 0.07),
  clover: cookiePath(4, 0.16),
  sunny: cookiePath(8, 0.1),
  flower: cookiePath(6, 0.13),
  circle: cookiePath(1, 0),
};

/** A shape as a CSS mask, for cutting an image (a cover) to it at any size. */
export function shapeMask(name) {
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><path d="${PATHS[name]}"/></svg>`;
  return `url("data:image/svg+xml,${encodeURIComponent(svg)}") center / 100% 100% no-repeat`;
}
