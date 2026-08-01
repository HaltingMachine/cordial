// Flag Inspector — the first real Cordial plugin.
//
// It asks Cordial which FastFlags are in effect and where each one came from,
// then logs a summary. Deliberately small: the point is that the whole path
// works end to end — manifest, grant, spawn, brokered call, real data — not that
// it does anything clever.
//
// Note what it cannot do. It runs with no Deno permissions at all: no file, no
// network, no environment, no subprocess. Everything it can reach arrives over
// stdio and was checked against its grant first. It requests `flags.read` but
// not `flags.write`, so the write call below is refused — by design, as a
// demonstration.

const enc = new TextEncoder();
const dec = new TextDecoder();

let nextId = 1;
const pending = new Map<number, (r: any) => void>();

(async () => {
  let buf = "";
  for await (const chunk of Deno.stdin.readable) {
    buf += dec.decode(chunk);
    let i: number;
    while ((i = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, i);
      buf = buf.slice(i + 1);
      if (!line.trim()) continue;
      const res = JSON.parse(line);
      pending.get(res.id)?.(res);
      pending.delete(res.id);
    }
  }
})();

function call(method: string, params: unknown = {}): Promise<any> {
  const id = nextId++;
  const p = new Promise<any>((resolve) => pending.set(id, resolve));
  Deno.stdout.write(enc.encode(JSON.stringify({ id, method, params }) + "\n"));
  return p;
}

const log = (message: string) => call("log.write", { message });

const flags = await call("flags.list");
if (flags.status !== "ok") {
  await log(`could not read flags: ${flags.status}`);
} else {
  const entries: Array<{ key: string; value: string; source: string }> = flags.result;
  await log(`${entries.length} flag override(s) in effect`);
  for (const e of entries) {
    await log(`  ${e.key} = ${e.value}  (from ${e.source})`);
  }
}

// Requested flags.read but not flags.write, so this must come back denied.
// Included so the boundary is visible in the output rather than only in a test.
const refused = await call("flags.set", { key: "FFlagAnything", value: "true" });
await log(`writing a flag came back: ${refused.status}` +
  (refused.capability ? ` (needs ${refused.capability})` : ""));
