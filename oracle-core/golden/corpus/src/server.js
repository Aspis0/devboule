/**
 * Oracle query server - handles incoming search requests.
 */
const http = require("http");
const url = require("url");

class QueryRouter {
  constructor(options = {}) {
    this.port = options.port || 8765;
    this.host = options.host || "127.0.0.1";
    this.routes = new Map();
    this.setupDefaultRoutes();
  }

  setupDefaultRoutes() {
    this.routes.set("/health", this.handleHealth.bind(this));
    this.routes.set("/context", this.handleContext.bind(this));
    this.routes.set("/ask", this.handleAsk.bind(this));
  }

  handleHealth(req, res) {
    return { status: "ready", phase: "phase1" };
  }

  handleContext(req, res) {
    const params = new URLSearchParams(url.parse(req.url).query);
    const query = params.get("q") || "";
    const limit = parseInt(params.get("limit") || "8", 10);
    return { query, results: [], limit };
  }

  handleAsk(req, res) {
    return { mode: "oracle-qwen-local", answer: "Not implemented in stub." };
  }

  async start() {
    const server = http.createServer((req, res) => {
      const parsed = url.parse(req.url, true);
      const handler = this.routes.get(parsed.pathname);
      if (handler) {
        const result = handler(req, res);
        res.writeHead(200, { "Content-Type": "application/json" });
        res.end(JSON.stringify(result));
      } else {
        res.writeHead(404);
        res.end("Not found");
      }
    });
    return new Promise((resolve) => {
      server.listen(this.port, this.host, () => resolve(server));
    });
  }
}

const startServer = async (port = 8765) => {
  const router = new QueryRouter({ port });
  await router.start();
  console.log(`Oracle server running on http://127.0.0.1:${port}`);
};

module.exports = { QueryRouter, startServer };
