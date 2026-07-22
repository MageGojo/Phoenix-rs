import LikeButton from "../../islands/like-button.js";

export interface ArticleShowProps {
  title: string;
  summary: string;
}

export default function ArticleShow({ title, summary }: ArticleShowProps) {
  return (
    <main>
      <article>
        <h1>{title}</h1>
        <p>{summary}</p>
      </article>
      <LikeButton client:load initialLikes={7} />
    </main>
  );
}
