import React, { useMemo } from 'react';

const PALETTES = [
  { bgStart: '#FFB4A2', bgEnd: '#FFCDB2', blush: '#E56B6F', feature: '#4A2828', acc: '#B5838D' },
  { bgStart: '#B7E4C7', bgEnd: '#D8F3DC', blush: '#74C69D', feature: '#1B4332', acc: '#52B788' },
  { bgStart: '#D8B4E2', bgEnd: '#E8D7F1', blush: '#C77DFF', feature: '#3C096C', acc: '#9D4EDD' },
  { bgStart: '#A2D2FF', bgEnd: '#BDE0FE', blush: '#FFC8DD', feature: '#1D3557', acc: '#457B9D' },
  { bgStart: '#FFB703', bgEnd: '#FFD166', blush: '#EF476F', feature: '#3D2600', acc: '#FB8500' },
  { bgStart: '#FFC6FF', bgEnd: '#BDB2FF', blush: '#FF70A6', feature: '#3A0CA3', acc: '#7209B7' },
  { bgStart: '#CCE3DE', bgEnd: '#EAF4F4', blush: '#A4C3B2', feature: '#283618', acc: '#6B9080' },
  { bgStart: '#FDE4CF', bgEnd: '#FFCAD4', blush: '#F4ACB7', feature: '#4A3E3D', acc: '#9D8189' },
  { bgStart: '#C8B6FF', bgEnd: '#E7C6FF', blush: '#FF85A1', feature: '#240046', acc: '#7B2CBF' },
  { bgStart: '#90E0EF', bgEnd: '#CAF0F8', blush: '#FF9EAA', feature: '#03045E', acc: '#0077B6' },
  { bgStart: '#F4A261', bgEnd: '#E76F51', blush: '#D62828', feature: '#2B2D42', acc: '#E9C46A' },
  { bgStart: '#D4E09B', bgEnd: '#F6F4D2', blush: '#90A955', feature: '#132A13', acc: '#31572C' }
];

export default function Avatar({ username, size = 32, className = '' }) {
  const seed = (username || 'wanderer').trim().toLowerCase();

  const { palette, eyeStyle, mouthStyle, accStyle, hasFreckles, gradId } = useMemo(() => {
    let hash = 0;
    for (let i = 0; i < seed.length; i++) {
      hash = (hash * 37 + seed.charCodeAt(i)) >>> 0;
    }
    const p = PALETTES[hash % PALETTES.length];
    const e = (hash >> 4) % 5;
    const m = (hash >> 7) % 4;
    const a = (hash >> 10) % 5;
    const f = (hash >> 13) % 3 === 0;
    const gid = `av-grad-${seed.replace(/[^a-z0-9]/g, '')}-${hash % 9999}`;
    return { palette: p, eyeStyle: e, mouthStyle: m, accStyle: a, hasFreckles: f, gradId: gid };
  }, [seed]);

  const cx = 50;
  const cy = 50;

  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 100 100"
      className={className}
      style={{ borderRadius: '50%', flexShrink: 0, display: 'inline-block', verticalAlign: 'middle' }}
    >
      <defs>
        <linearGradient id={gradId} x1="0%" y1="0%" x2="100%" y2="100%">
          <stop offset="0%" stopColor={palette.bgStart} />
          <stop offset="100%" stopColor={palette.bgEnd} />
        </linearGradient>
      </defs>

      {/* Background circle */}
      <circle cx={cx} cy={cy} r="48" fill={`url(#${gradId})`} />

      {/* Accessories (Ears, Sprout, Star, Halo) */}
      {accStyle === 0 && (
        <>
          <circle cx="24" cy="22" r="12" fill={palette.acc} opacity="0.6" />
          <circle cx="76" cy="22" r="12" fill={palette.acc} opacity="0.6" />
          <circle cx="24" cy="22" r="6" fill="#FFF" opacity="0.4" />
          <circle cx="76" cy="22" r="6" fill="#FFF" opacity="0.4" />
        </>
      )}
      {accStyle === 1 && (
        <>
          <path d="M50 25 Q48 15 50 8" stroke={palette.acc} strokeWidth="4" strokeLinecap="round" fill="none" />
          <path d="M50 9 Q66 4 63 14 Q54 14 50 9 Z" fill={palette.acc} />
        </>
      )}
      {accStyle === 2 && (
        <path d="M78 18 Q78 24 84 24 Q78 24 78 30 Q78 24 72 24 Q78 24 78 18 Z" fill={palette.acc} opacity="0.85" />
      )}
      {accStyle === 3 && (
        <ellipse cx="50" cy="14" rx="16" ry="6" stroke={palette.acc} strokeWidth="3.5" fill="none" opacity="0.75" />
      )}
      {accStyle === 4 && (
        <>
          <path d="M22 18 Q22 22 26 22 Q22 22 22 26 Q22 22 18 22 Q22 22 22 18 Z" fill={palette.acc} opacity="0.75" />
          <path d="M78 18 Q78 22 82 22 Q78 22 78 26 Q78 22 74 22 Q78 22 78 18 Z" fill={palette.acc} opacity="0.75" />
        </>
      )}

      {/* Rosy blush cheeks */}
      <circle cx="26" cy="58" r="9" fill={palette.blush} opacity="0.65" />
      <circle cx="74" cy="58" r="9" fill={palette.blush} opacity="0.65" />

      {/* Freckles */}
      {hasFreckles && (
        <>
          <circle cx="28" cy="54" r="1.5" fill={palette.feature} opacity="0.35" />
          <circle cx="32" cy="56" r="1.5" fill={palette.feature} opacity="0.35" />
          <circle cx="72" cy="54" r="1.5" fill={palette.feature} opacity="0.35" />
          <circle cx="68" cy="56" r="1.5" fill={palette.feature} opacity="0.35" />
        </>
      )}

      {/* Eyes */}
      {eyeStyle === 0 && (
        <>
          <circle cx="33" cy="44" r="7" fill={palette.feature} />
          <circle cx="67" cy="44" r="7" fill={palette.feature} />
          <circle cx="35" cy="42" r="2.5" fill="#FFF" />
          <circle cx="69" cy="42" r="2.5" fill="#FFF" />
          <circle cx="31" cy="46" r="1.2" fill="#FFF" />
          <circle cx="65" cy="46" r="1.2" fill="#FFF" />
        </>
      )}
      {eyeStyle === 1 && (
        <>
          <path d="M26 46 Q33 37 40 46" stroke={palette.feature} strokeWidth="5.5" strokeLinecap="round" fill="none" />
          <path d="M60 46 Q67 37 74 46" stroke={palette.feature} strokeWidth="5.5" strokeLinecap="round" fill="none" />
        </>
      )}
      {eyeStyle === 2 && (
        <>
          <path d="M26 42 L33 45 L26 48" stroke={palette.feature} strokeWidth="5.5" strokeLinecap="round" strokeLinejoin="round" fill="none" />
          <circle cx="67" cy="44" r="7" fill={palette.feature} />
          <circle cx="69" cy="42" r="2.5" fill="#FFF" />
          <circle cx="65" cy="46" r="1.2" fill="#FFF" />
        </>
      )}
      {eyeStyle === 3 && (
        <>
          <path d="M33 36 Q33 44 41 44 Q33 44 33 52 Q33 44 25 44 Q33 44 33 36 Z" fill={palette.feature} />
          <path d="M67 36 Q67 44 75 44 Q67 44 67 52 Q67 44 59 44 Q67 44 67 36 Z" fill={palette.feature} />
          <circle cx="35" cy="42" r="2" fill="#FFF" />
          <circle cx="69" cy="42" r="2" fill="#FFF" />
        </>
      )}
      {eyeStyle === 4 && (
        <>
          <rect x="28" y="38" width="9" height="12" rx="4.5" fill={palette.feature} />
          <rect x="63" y="38" width="9" height="12" rx="4.5" fill={palette.feature} />
          <circle cx="34" cy="41" r="2.2" fill="#FFF" />
          <circle cx="69" cy="41" r="2.2" fill="#FFF" />
          <circle cx="31" cy="46" r="1.2" fill="#FFF" />
          <circle cx="66" cy="46" r="1.2" fill="#FFF" />
        </>
      )}

      {/* Mouth */}
      {mouthStyle === 0 && (
        <path d="M39 58 Q50 65 61 58" stroke={palette.feature} strokeWidth="5" strokeLinecap="round" fill="none" />
      )}
      {mouthStyle === 1 && (
        <>
          <path d="M38 58 Q50 74 62 58 Z" fill={palette.feature} />
          <path d="M43 63 Q50 72 57 63 Z" fill="#FF758F" />
        </>
      )}
      {mouthStyle === 2 && (
        <>
          <path d="M37 58 Q43.5 63 50 58" stroke={palette.feature} strokeWidth="5" strokeLinecap="round" fill="none" />
          <path d="M50 58 Q56.5 63 63 58" stroke={palette.feature} strokeWidth="5" strokeLinecap="round" fill="none" />
        </>
      )}
      {mouthStyle === 3 && (
        <path d="M37 57 Q43 65 50 65 Q57 65 63 57" stroke={palette.feature} strokeWidth="5" strokeLinecap="round" fill="none" />
      )}
    </svg>
  );
}
