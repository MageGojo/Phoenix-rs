import type { SpaProps } from "../generated/contracts.js";

export default function Spa({ title }: SpaProps) {
  return (
    <main className="mode-card" data-smoke-mode="spa">
      <p className="eyebrow">CLIENT RENDERED</p>
      <h1>{title}</h1>
      <p>The browser owns the first render and subsequent navigation.</p>
      <ModeLinks />
    </main>
  );
}

function ModeLinks() {
  return (
    <nav aria-label="Rendering modes">
      <a href="/spa">SPA</a>
      <a href="/islands">Islands</a>
      <a href="/ssr">SSR</a>
    </nav>
  );
}
