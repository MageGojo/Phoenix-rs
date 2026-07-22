import { startPhoenix } from "@phoenix/react";

import MemberDirectory from "./islands/member-directory.js";
import "./styles.css";

startPhoenix({
  pages: {},
  islands: { "member-directory": MemberDirectory },
});
