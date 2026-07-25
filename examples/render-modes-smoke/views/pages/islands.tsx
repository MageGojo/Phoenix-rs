import type { IslandsProps } from "../generated/contracts.js";
import type {} from "@apizero/react";
import Counter from "../islands/counter.js";

export default function Islands({ title }: IslandsProps) {
  return (
    <main className="mode-card" data-smoke-mode="islands">
      <p className="eyebrow">SERVER HTML + TARGETED HYDRATION</p>
      <h1>{title}</h1>
      <p>Only this generated counter island hydrates in the browser.</p>
      <Counter client:load initialCount={7} />
      <nav aria-label="Rendering modes">
        <a href="/spa">SPA</a>
        <a href="/islands">Islands</a>
        <a href="/ssr">SSR</a>
      </nav>
    </main>
  );
}
