// Holds `sockets` WebSocket clients open, each sending a handoff heartbeat every 30 s like Wanda
// does, plus `qps` mixed GraphQL reads. Reports latency percentiles and unexpected closes.
//
//   node run.mjs http://127.0.0.1:8700 1500 50 120
//   (base, sockets, extra GraphQL queries per second, seconds, first account index)
// LOADTEST_JSON=1 prints raw results instead, for merging several runner processes.
import { readFileSync } from "node:fs";

const [base = "http://127.0.0.1:8700", sockets = 1500, qps = 50, seconds = 120, offset = 0] = process.argv.slice(2);
const tokens = JSON.parse(readFileSync(new URL("tokens.json", import.meta.url)));
const wsBase = base.replace(/^http/, "ws");
const lat = { heartbeat: [], query: [] };
let errors = 0, closes = 0, frames = 0, resyncs = 0, opened = 0;
let stopping = false;

async function gql(token, query, variables, bucket) {
  const t0 = performance.now();
  try {
    const res = await fetch(`${base}/graphql`, {
      method: "POST",
      headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
      body: JSON.stringify({ query, variables }),
    });
    const body = await res.json();
    if (!res.ok || body.errors) errors++;
  } catch { errors++; }
  lat[bucket].push(performance.now() - t0);
}

const HEARTBEAT = `mutation H($input: HandoffInput!) { updateHandoff(input: $input) }`;
const READS = [
  `{ me { __typename } }`,
  `{ friends { __typename } }`,
  `{ friendsNowPlaying { username } }`,
];

function client(u, i) {
  const ws = new WebSocket(`${wsBase}/ws/sync?device=${u.device}`);
  ws.onopen = () => {
    opened++;
    ws.send(JSON.stringify({ msg_type: "AUTH", payload: { token: u.token, device: u.device } }));
  };
  ws.onmessage = (e) => {
    frames++;
    if (String(e.data).includes('"resync_required":true')) resyncs++;
  };
  ws.onclose = () => { if (!stopping) closes++; };
  const beat = () => gql(u.token, HEARTBEAT, { input: {
    userId: u.username, trackUri: "load://t", trackTitle: "Load", artistName: "Test",
    positionMs: 1000 * i, durationMs: 200000, isPlaying: true, deviceId: u.device,
  } }, "heartbeat");
  // Spread the first beats over the interval, as real devices are.
  setTimeout(() => { beat(); setInterval(beat, 30_000); }, Math.random() * 30_000);
  return ws;
}

const conns = [];
for (let i = 0; i < Number(sockets); i++) {
  conns.push(client(tokens[(Number(offset) + i) % tokens.length], i));
  if (i % 100 === 99) await new Promise((r) => setTimeout(r, 200));
}
const reads = setInterval(() => {
  for (let k = 0; k < Number(qps) / 10; k++) {
    const u = tokens[Math.floor(Math.random() * tokens.length)];
    gql(u.token, READS[k % READS.length], {}, "query");
  }
}, 100);

await new Promise((r) => setTimeout(r, Number(seconds) * 1000));
stopping = true;
clearInterval(reads);
conns.forEach((c) => c.close());

if (process.env.LOADTEST_JSON) {
  console.log(JSON.stringify({ lat, opened, closes, frames, resyncs, errors }));
  process.exit(0);
}
const pct = (a, p) => a.length ? a.sort((x, y) => x - y)[Math.floor(a.length * p)].toFixed(1) : "-";
for (const [k, a] of Object.entries(lat)) {
  console.log(`${k.padEnd(9)} n=${a.length} p50=${pct(a, 0.5)}ms p95=${pct(a, 0.95)}ms p99=${pct(a, 0.99)}ms`);
}
console.log(`sockets opened=${opened} unexpected_closes=${closes} frames=${frames} resyncs=${resyncs} gql_errors=${errors}`);
process.exit(0);
