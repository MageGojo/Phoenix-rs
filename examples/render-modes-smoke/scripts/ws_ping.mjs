const url = process.argv[2] || "ws://127.0.0.1:3000/features/ws";
const ws = new WebSocket(url);
const timer = setTimeout(() => {
  console.error("timeout");
  process.exit(2);
}, 5000);
ws.addEventListener("open", () => ws.send("ping"));
ws.addEventListener("message", (event) => {
  clearTimeout(timer);
  const data = String(event.data);
  if (data === "pong") {
    process.exit(0);
  }
  console.error(`unexpected: ${data}`);
  process.exit(1);
});
ws.addEventListener("error", (event) => {
  clearTimeout(timer);
  console.error(event.message || "websocket error");
  process.exit(1);
});
