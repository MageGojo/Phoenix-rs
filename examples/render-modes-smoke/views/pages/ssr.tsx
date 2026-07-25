import type { SsrProps } from "../generated/contracts.js";

export default function Ssr({ title }: SsrProps) {
  return (
    <main className="mode-card" data-smoke-mode="ssr">
      <p className="eyebrow">SERVER RENDERED + FULL HYDRATION</p>
      <h1>{title}</h1>
      <p>The complete page is rendered on the server before hydration.</p>
      <nav aria-label="Rendering modes">
        <a href="/spa">SPA</a>
        <a href="/islands">Islands</a>
        <a href="/ssr">SSR</a>
      </nav>
    </main>
  );
}
