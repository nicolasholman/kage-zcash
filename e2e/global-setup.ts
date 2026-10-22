// The stack is started separately (./stack.sh) so its PIDs are known and killed by PID afterwards.
// Fail loudly if it isn't up instead of testing nothing.
export const PORTS = { server: 4020, noFacilitator: 4030, demoLedger: 4040, badLedger: 4041, rateLimited: 4042, landing: 4050 };

export default async function globalSetup() {
  const urls = [
    ...[PORTS.server, PORTS.noFacilitator, PORTS.demoLedger, PORTS.badLedger, PORTS.rateLimited].map(p => `http://127.0.0.1:${p}/api/status`),
    `http://127.0.0.1:${PORTS.landing}/zec-shielded-x402/`,
  ];
  for (const u of urls) {
    const r = await fetch(u).catch(e => { throw new Error(`Kage stack not running (${u}: ${e.cause?.code || e.message}). Start it with: npm run stack`); });
    if (!r.ok) throw new Error(`${u} answered ${r.status}; restart the stack (npm run stack:stop && npm run stack)`);
  }
}
