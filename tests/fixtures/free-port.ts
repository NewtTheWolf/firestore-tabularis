// Print a free localhost port (lets the OS pick one). Used by the integration-test
// recipe so parallel emulator runs don't collide on a fixed port.

const server = Bun.listen({
  hostname: "127.0.0.1",
  port: 0,
  socket: { data() {}, open() {}, close() {}, error() {} },
});
console.log(server.port);
server.stop();
