// A minimal plugin, used by the integration test.
//
// It makes three calls: one it is granted, one it is not, and one that does not
// exist — so the test can show that "denied" and "unknown method" arrive as
// distinguishable answers rather than as one generic failure.

const enc = new TextEncoder();
const send = (req: unknown) =>
  Deno.stdout.write(enc.encode(JSON.stringify(req) + "\n"));

const lines = async function* () {
  const dec = new TextDecoder();
  let buf = "";
  for await (const chunk of Deno.stdin.readable) {
    buf += dec.decode(chunk);
    let i;
    while ((i = buf.indexOf("\n")) >= 0) {
      yield buf.slice(0, i);
      buf = buf.slice(i + 1);
    }
  }
}();

const call = async (id: number, method: string) => {
  await send({ id, method, params: {} });
  const { value } = await lines.next();
  return JSON.parse(value as string);
};

const granted = await call(1, "flags.list");
const refused = await call(2, "flags.set");
const bogus = await call(3, "flags.nonsense");

// Prove the sandbox too: with no --allow-read this must throw, and a plugin
// that could read the filesystem would make the capability model decorative.
let sandboxed = false;
try {
  Deno.readTextFileSync("/etc/passwd");
} catch {
  sandboxed = true;
}

await send({
  id: 99,
  method: "log.write",
  params: {
    granted: granted.status,
    refused: refused.status,
    refusedCapability: refused.capability ?? null,
    bogus: bogus.status,
    sandboxed,
  },
});
