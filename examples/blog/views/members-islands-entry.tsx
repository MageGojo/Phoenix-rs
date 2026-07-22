import { startPhoenix } from "@phoenix/react";

import MemberDirectory from "./islands/member-directory.js";
import "./styles.css";

startPhoenix({
  islands: [MemberDirectory],
});
