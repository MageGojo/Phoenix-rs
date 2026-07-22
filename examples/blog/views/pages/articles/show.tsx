import { island } from "@phoenix/react";

import LikeButton from "../../islands/like-button.js";

const LikeButtonIsland = island("like-button", LikeButton);

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
      <LikeButtonIsland islandId="article-like" initialLikes={7} />
    </main>
  );
}
