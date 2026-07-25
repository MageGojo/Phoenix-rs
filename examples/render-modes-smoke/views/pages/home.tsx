import type { HomeProps } from "../generated/contracts.js";

export default function Home({ title, description }: HomeProps) {
  return (
    <main className="welcome">
      <p className="eyebrow">PHOENIX-RS / RUST + REACT</p>
      <h1>{title}</h1>
      <p>{description}</p>
      <nav className="mode-grid" aria-label="Choose a rendering mode">
        <a href="/spa">
          <strong>SPA</strong>
          <span>Client renders the first view.</span>
        </a>
        <a href="/islands">
          <strong>Islands</strong>
          <span>Server HTML with one interactive island.</span>
        </a>
        <a href="/ssr">
          <strong>SSR</strong>
          <span>Server renders and hydrates the whole page.</span>
        </a>
      </nav>
      <code>Open each link to compare the render mode.</code>
    </main>
  );
}
