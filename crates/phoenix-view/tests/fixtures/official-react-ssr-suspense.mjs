import { Suspense, createElement, lazy } from "react";
import { startRenderer } from "@phoenix/react-ssr";

const Deferred = lazy(async () => {
  await new Promise((resolve) => setTimeout(resolve, 25));
  return {
    default: () => createElement("strong", { id: "resolved" }, "ready"),
  };
});

function SuspensePage() {
  return createElement(
    "main",
    null,
    createElement("h1", null, "shell"),
    createElement(
      Suspense,
      { fallback: createElement("span", { id: "fallback" }, "loading") },
      createElement(Deferred),
    ),
  );
}

startRenderer({
  pages: { "tests/suspense": SuspensePage },
});
