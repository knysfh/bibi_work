import { createServer } from "node:http";
import { gunzipSync } from "node:zlib";

const port = Number(process.env.BIWORK_OTLP_FIXTURE_PORT ?? "4318");
const maxBodyBytes = 16 * 1024 * 1024;
const binaryPayloads = [];
const jsonTraces = new Map();

function visit(value, currentService = null) {
  if (!value || typeof value !== "object") return;
  if (Array.isArray(value)) {
    for (const item of value) visit(item, currentService);
    return;
  }

  let serviceName = currentService;
  const attributes = value.resource?.attributes;
  if (Array.isArray(attributes)) {
    const serviceAttribute = attributes.find((attribute) => attribute?.key === "service.name");
    const candidate = serviceAttribute?.value?.stringValue;
    if (typeof candidate === "string") serviceName = candidate;
  }

  if (typeof value.traceId === "string" && /^[0-9a-f]{32}$/i.test(value.traceId)) {
    const traceId = value.traceId.toLowerCase();
    const summary = jsonTraces.get(traceId) ?? { attributes: {}, serviceNames: new Set(), spanNames: new Set() };
    if (serviceName) summary.serviceNames.add(serviceName);
    if (typeof value.name === "string") summary.spanNames.add(value.name);
    if (Array.isArray(value.attributes)) {
      for (const attribute of value.attributes) {
        if (
          (attribute?.key === "biwork.local_exec_request_id" || attribute?.key === "biwork.run_id") &&
          typeof attribute.value?.stringValue === "string"
        ) {
          summary.attributes[attribute.key] = attribute.value.stringValue;
        }
      }
    }
    jsonTraces.set(traceId, summary);
  }

  for (const child of Object.values(value)) visit(child, serviceName);
}

function summary() {
  return {
    binary_payload_count: binaryPayloads.length,
    binary_backend_payload_count: binaryPayloads.filter((payload) => payload.includes(Buffer.from("bibi-work-backend")))
      .length,
    traces: [...jsonTraces.entries()].map(([traceId, trace]) => ({
      trace_id: traceId,
      attributes: trace.attributes,
      service_names: [...trace.serviceNames].sort(),
      span_names: [...trace.spanNames].sort(),
      rust_binary_match: binaryPayloads.some((payload) => payload.includes(Buffer.from(traceId, "hex"))),
    })),
  };
}

const server = createServer((req, res) => {
  if (req.method === "POST" && req.url === "/reset") {
    binaryPayloads.length = 0;
    jsonTraces.clear();
    res.writeHead(204).end();
    return;
  }
  if (req.method === "GET" && req.url === "/summary") {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify(summary()));
    return;
  }
  if (req.method === "GET" && req.url?.startsWith("/contains?")) {
    const traceId = new URL(req.url, "http://127.0.0.1").searchParams.get("trace_id") ?? "";
    const validTraceId = /^[0-9a-f]{32}$/i.test(traceId);
    res.writeHead(validTraceId ? 200 : 400, { "content-type": "application/json" });
    res.end(
      JSON.stringify({
        binary_match: validTraceId && binaryPayloads.some((payload) => payload.includes(Buffer.from(traceId, "hex"))),
        json_match: validTraceId && jsonTraces.has(traceId.toLowerCase()),
      }),
    );
    return;
  }
  if (req.method !== "POST" || req.url !== "/v1/traces") {
    res.writeHead(404).end();
    return;
  }

  const chunks = [];
  let size = 0;
  req.on("data", (chunk) => {
    size += chunk.length;
    if (size > maxBodyBytes) {
      req.destroy(new Error("OTLP fixture payload exceeds limit"));
      return;
    }
    chunks.push(chunk);
  });
  req.on("end", () => {
    const payload = Buffer.concat(chunks);
    const contentType = String(req.headers["content-type"] ?? "");
    if (contentType.includes("json")) {
      try {
        visit(JSON.parse(payload.toString("utf8")));
      } catch {
        res.writeHead(400).end();
        return;
      }
    } else {
      const contentEncoding = String(req.headers["content-encoding"] ?? "").toLowerCase();
      binaryPayloads.push(contentEncoding === "gzip" ? gunzipSync(payload) : payload);
    }
    res.writeHead(200, { "content-type": "application/json" });
    res.end("{}");
  });
});

server.listen(port, "127.0.0.1", () => {
  console.log(`OTLP fixture listening on http://127.0.0.1:${port}`);
});

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => server.close(() => process.exit(0)));
}
