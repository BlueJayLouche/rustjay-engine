import { useState, useCallback } from 'react';

declare global {
  interface Window {
    rustjay?: {
      set_delay_r: (v: number) => void;
      set_delay_g: (v: number) => void;
      set_delay_b: (v: number) => void;
      set_mix: (v: number) => void;
    };
  }
}

export default function DelaySliders() {
  const [delayR, setDelayR] = useState(0);
  const [delayG, setDelayG] = useState(5);
  const [delayB, setDelayB] = useState(10);
  const [mix, setMix] = useState(0.5);

  const call = useCallback((fn: string, value: number) => {
    const wasm = window.rustjay;
    if (!wasm) return;
    const setter = wasm[fn as keyof typeof wasm];
    if (setter) setter(value);
  }, []);

  return (
    <div
      style={{
        background: 'rgba(0,0,0,0.7)',
        padding: '16px 20px',
        borderRadius: 8,
        color: '#fff',
        fontFamily: 'system-ui, -apple-system, sans-serif',
        minWidth: 220,
        backdropFilter: 'blur(4px)',
      }}
    >
      <h3
        style={{
          margin: '0 0 12px',
          fontSize: 14,
          textTransform: 'uppercase',
          letterSpacing: 1,
        }}
      >
        Delta Controls
      </h3>

      <Slider
        label="Red Delay"
        value={delayR}
        min={-64}
        max={64}
        onChange={(v) => {
          setDelayR(v);
          call('set_delay_r', v);
        }}
        color="#ff4444"
      />
      <Slider
        label="Green Delay"
        value={delayG}
        min={-64}
        max={64}
        onChange={(v) => {
          setDelayG(v);
          call('set_delay_g', v);
        }}
        color="#44ff44"
      />
      <Slider
        label="Blue Delay"
        value={delayB}
        min={-64}
        max={64}
        onChange={(v) => {
          setDelayB(v);
          call('set_delay_b', v);
        }}
        color="#4444ff"
      />
      <Slider
        label="Mix Amount"
        value={mix}
        min={0}
        max={1}
        step={0.01}
        onChange={(v) => {
          setMix(v);
          call('set_mix', v);
        }}
        color="#ffffff"
      />
    </div>
  );
}

function Slider({
  label,
  value,
  min,
  max,
  step = 1,
  onChange,
  color,
}: {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  onChange: (v: number) => void;
  color: string;
}) {
  return (
    <div style={{ marginBottom: 10 }}>
      <div
        style={{
          display: 'flex',
          justifyContent: 'space-between',
          fontSize: 12,
          marginBottom: 4,
        }}
      >
        <span>{label}</span>
        <span style={{ color, fontVariantNumeric: 'tabular-nums' }}>{value}</span>
      </div>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(e) => onChange(parseFloat(e.target.value))}
        style={{ width: '100%', accentColor: color }}
      />
    </div>
  );
}
