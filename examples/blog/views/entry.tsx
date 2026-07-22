import { startPhoenix } from "@phoenix/react";

import LikeButton from "./islands/like-button.js";
import ArticleShow from "./pages/articles/show.js";
import MembersIndex from "./pages/members/index.js";
import "./styles.css";

startPhoenix({
  pages: {
    "articles/show": ArticleShow,
    "members/index": MembersIndex,
  },
  islands: { "like-button": LikeButton },
});
