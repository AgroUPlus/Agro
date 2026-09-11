import React from 'react';

export default function AgroLogo({ size = 32, className = '' }) {
  return (
    <svg
      xmlns="http://www.w3.org/2000/svg"
      viewBox="0 0 108 108"
      width={size}
      height={size}
      className={className}
      style={{ display: 'inline-block', verticalAlign: 'middle', flexShrink: 0, borderRadius: `${size * (36 / 108)}px` }}
    >
      <rect width="108" height="108" rx="36" fill="#D0BCFF" />
      <path
        d="M16 63 C16 57 20 48 24 43 L48 14 C50 12 55 12 57 15 L63 24 L71 28 C74 29 76 33 74 36 L70 42 L80 46 C84 48 87 52 86 56 C84 62 78 68 70 70 L48 88 C45 90 40 90 38 87 L22 69 C18 67 16 65 16 63 Z"
        fill="#381E72"
      />
      <path d="M38 32 L44 26 L48 34 Z" fill="#D0BCFF" />
      <path d="M46 42 L52 36 L56 44 Z" fill="#D0BCFF" />
      <circle cx="64" cy="38" r="3" fill="#D0BCFF" />
      <circle cx="78" cy="52" r="2.2" fill="#D0BCFF" />
    </svg>
  );
}
