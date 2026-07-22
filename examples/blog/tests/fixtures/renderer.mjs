import { createInterface } from "node:readline";

createInterface({ input: process.stdin, crlfDelay: Infinity }).on("line", (line) => {
  const request = JSON.parse(line);
  let html = "";
  let islands = [];
  if (request.kind === "render" && request.envelope.page === "articles/show") {
    html = `<main><h1>${request.envelope.props.title}</h1></main>`;
  }
  if (request.kind === "render" && request.envelope.page === "members/index") {
    html = `<main><h1>团队成员目录</h1><div data-phoenix-island="member-creator" data-component="member-creator"><form><strong>新增成员</strong></form></div><table id="member-table"><tr><td>${request.envelope.props.members[0].email}</td></tr></table></main>`;
    islands = [{ id: "member-creator", component: "member-creator", props: { initialTotal: 100 } }];
  }
  process.stdout.write(`${JSON.stringify({
    protocol: 2,
    id: request.id,
    ok: true,
    html,
    islands,
    head: [],
  })}\n`);
});
