/**
 * The Kage engine: the Rust binaries that hold the wallet, build and prove shielded transactions
 * and enforce spendguard (`x402z-mcp`, `x402z-agent`, `x402z-facilitator`). The SDK talks to
 * `x402z-mcp` over stdio (MCP, newline-delimited JSON-RPC 2.0); nothing about the wallet or the
 * policy check runs in JavaScript.
 *
 * Binaries are found, in order, at `enginePath`, `$KAGE_ENGINE_DIR`, or the platform package
 * `kage-x402-engine-<platform>-<arch>` (an optional dependency of `kage-x402`).
 */
import { spawn, type ChildProcess } from 'node:child_process';
import { existsSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { createInterface } from 'node:readline';

export type EngineBinary = 'x402z-mcp' | 'x402z-agent' | 'x402z-facilitator';

export class KageSetupError extends Error {
  readonly code = 'SETUP_REQUIRED';
}

/** The directory holding the engine binaries for this machine. Throws {@link KageSetupError} if there is none. */
export function engineDir(enginePath?: string): string {
  const tried: string[] = [];
  for (const d of [enginePath, process.env.KAGE_ENGINE_DIR]) {
    if (!d) continue;
    if (existsSync(join(d, 'x402z-mcp'))) return d;
    tried.push(d);
  }
  const pkg = `kage-x402-engine-${process.platform}-${process.arch}`;
  try {
    const req = createRequire(import.meta.url);
    const d = join(dirname(req.resolve(`${pkg}/package.json`)), 'bin');
    if (existsSync(join(d, 'x402z-mcp'))) return d;
    tried.push(d);
  } catch {
    tried.push(`${pkg} (not installed)`);
  }
  throw new KageSetupError(
    `Setup required: no Kage engine for ${process.platform}-${process.arch}. Install ${pkg}, or build the engine ` +
      `(cargo build --release -p x402z-mcp -p x402z-agent -p x402z-facilitator) and set KAGE_ENGINE_DIR to target/release. Tried: ${tried.join(', ')}`,
  );
}

export function enginePath(bin: EngineBinary, dir?: string): string {
  return join(engineDir(dir), bin);
}

interface Pending {
  resolve: (v: unknown) => void;
  reject: (e: Error) => void;
}

/** One `x402z-mcp` process and a JSON-RPC client for it. */
export class EngineProcess {
  private child: ChildProcess;
  private nextId = 1;
  private pending = new Map<number, Pending>();
  private exited: Error | undefined;
  readonly stderr: string[] = [];

  constructor(bin: string, env: Record<string, string>, onLog?: (line: string) => void) {
    // Only what the engine needs: PATH/HOME plus the Kage variables, never the whole parent env.
    const base: Record<string, string> = {};
    for (const k of ['PATH', 'HOME', 'TMPDIR', 'SystemRoot']) if (process.env[k]) base[k] = process.env[k] as string;
    this.child = spawn(bin, [], { env: { ...base, ...env }, stdio: ['pipe', 'pipe', 'pipe'] });
    createInterface({ input: this.child.stdout! }).on('line', (line) => this.onLine(line));
    createInterface({ input: this.child.stderr! }).on('line', (line) => {
      this.stderr.push(line);
      if (this.stderr.length > 200) this.stderr.shift();
      onLog?.(line);
    });
    this.child.on('exit', (code, signal) => {
      this.exited = new Error(`Kage engine exited (${signal ?? code}): ${this.stderr.slice(-3).join(' | ')}`);
      for (const p of this.pending.values()) p.reject(this.exited);
      this.pending.clear();
    });
    this.child.on('error', (e) => {
      this.exited = e;
      for (const p of this.pending.values()) p.reject(e);
      this.pending.clear();
    });
  }

  private onLine(line: string) {
    let msg: { id?: number; result?: unknown; error?: { message: string } };
    try {
      msg = JSON.parse(line);
    } catch {
      return;
    }
    if (typeof msg.id !== 'number') return;
    const p = this.pending.get(msg.id);
    if (!p) return;
    this.pending.delete(msg.id);
    if (msg.error) p.reject(new Error(`Kage engine: ${msg.error.message}`));
    else p.resolve(msg.result);
  }

  request(method: string, params: unknown = {}): Promise<unknown> {
    if (this.exited) return Promise.reject(this.exited);
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.child.stdin!.write(JSON.stringify({ jsonrpc: '2.0', id, method, params }) + '\n');
    });
  }

  notify(method: string, params: unknown = {}) {
    this.child.stdin!.write(JSON.stringify({ jsonrpc: '2.0', method, params }) + '\n');
  }

  /** Call an engine tool; returns its structured result and whether the engine flagged it as an error. */
  async call(name: string, args: Record<string, unknown> = {}): Promise<{ isError: boolean; value: Record<string, unknown> }> {
    const r = (await this.request('tools/call', { name, arguments: args })) as { isError?: boolean; structuredContent?: Record<string, unknown> };
    return { isError: r.isError === true, value: r.structuredContent ?? {} };
  }

  close() {
    this.child.stdin?.end();
    const t = setTimeout(() => this.child.kill('SIGTERM'), 2000);
    t.unref();
  }
}
