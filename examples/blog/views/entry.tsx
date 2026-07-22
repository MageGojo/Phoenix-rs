import { startPhoenix } from "@phoenix/react";

import LikeButton from "./islands/like-button.js";
import ArticleShow from "./pages/articles/show.js";
import "./styles.css";

startPhoenix({
  pages: {
    "articles/show": ArticleShow,
  },
  islands: { "like-button": LikeButton },
});
