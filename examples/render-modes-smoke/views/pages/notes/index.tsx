import type { NotesIndexProps } from "../../generated/contracts.js";

export default function NotesIndex({ title, notes }: NotesIndexProps) {
  return (
    <main className="mode-card" data-smoke-mode="notes">
      <p className="eyebrow">SQLITE SMOKE</p>
      <h1>{title}</h1>
      <p>Notes are loaded from `storage/app.sqlite` on every request.</p>
      {notes.length === 0 ? (
        <p data-smoke-notes-empty="true">No notes yet. POST /notes to create one.</p>
      ) : (
        <ul data-smoke-notes-list="true">
          {notes.map((note) => (
            <li key={note.id} data-smoke-note-id={note.id}>
              {note.name}
            </li>
          ))}
        </ul>
      )}
      <nav aria-label="Rendering modes">
        <a href="/">Home</a>
        <a href="/spa">SPA</a>
        <a href="/islands">Islands</a>
        <a href="/ssr">SSR</a>
      </nav>
    </main>
  );
}
