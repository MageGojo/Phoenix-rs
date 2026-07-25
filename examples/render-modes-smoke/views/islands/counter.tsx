import { useState } from "react";

export interface CounterProps {
  initialCount?: number;
}

export default function Counter({ initialCount = 0 }: CounterProps) {
  const [count, setCount] = useState(initialCount);
  return (
    <button
      data-smoke-counter
      type="button"
      onClick={() => setCount((value) => value + 1)}
    >
      Count: {count}
    </button>
  );
}
