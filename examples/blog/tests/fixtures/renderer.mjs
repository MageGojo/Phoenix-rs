import { createInterface } from "node:readline";

createInterface({ input: process.stdin, crlfDelay: Infinity }).on("line", (line) => {
  const request = JSON.parse(line);
  let html = "";
  if (request.kind === "render" && request.envelope.page === "articles/show") {
    html = `<main><h1>${request.envelope.props.title}</h1></main>`;
  }
  if (request.kind === "render" && request.envelope.page === "members/index") {
    html = `<main><h1>团队成员目录</h1><p>${request.envelope.props.members[0].email}</p></main>`;
  }
  process.stdout.write(`${JSON.stringify({
    protocol: 1,
    id: request.id,
    ok: true,
    html,
    head: [],
  })}\n`);
});
