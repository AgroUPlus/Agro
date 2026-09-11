import React from 'react';

export default function AgroLogo({ size = 32, className = '' }) {
  return (
    <img
      src="/agro-logo.png"
      alt="Agro"
      width={size}
      height={size}
      className={className}
      style={{
        display: 'inline-block',
        verticalAlign: 'middle',
        flexShrink: 0,
        borderRadius: `${size * 0.28}px`,
        objectFit: 'contain',
      }}
    />
  );
}
