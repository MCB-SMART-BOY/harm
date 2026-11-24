// Usage: bun experiments/predictor-cost-analysis.ts TRACE.jsonl [--trim]
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

type Row = {
  event: string;
  run?: number;
  size?: string;
  backend?: "cpu" | "gpu";
  threads?: number | null;
  [field: string]: unknown;
};
type Quantiles = {
  n: number;
  p50_ns: number;
  p95_ns: number;
  p99_ns: number;
  max_abs_error: number;
};
type PowerPhase = { n: number; mean_w: number | null; median_w: number | null };
type Power = { idle: PowerPhase; active: PowerPhase; max_abs_error: number };

const [path, mode] = Bun.argv.slice(2);
assert(path && (mode === undefined || mode === "--trim"), "expected TRACE.jsonl [--trim]");
const lines = readFileSync(path, "utf8").trim().split("\n");
const trace: Row[] = lines.map((line) => JSON.parse(line));
const metadata = trace[0];
assert.equal(metadata.event, "metadata");
assert.equal(metadata.schema_version, 1);
assert.equal(trace.at(-1)?.event, "sweep_end");
const cases = new Map<string, Row[]>();
const implementation = (row: Row) => row.backend === "gpu" ? "GPU" : `CPU/${row.threads}`;
const key = (row: Row) => `${row.size} ${implementation(row)} run ${row.run}`;
for (const row of trace) {
  if (row.run === undefined) continue;
  const id = key(row);
  const rows = cases.get(id) ?? [];
  rows.push(row);
  cases.set(id, rows);
}
assert(cases.size);

const sorted = (values: number[]) => [...values].sort((a, b) => a - b);
function median(values: number[]) {
  assert(values.length);
  const s = sorted(values);
  return (s[Math.floor((s.length - 1) / 2)] + s[Math.floor(s.length / 2)]) / 2;
}
function event(rows: Row[], name: string) {
  const matches = rows.filter((row) => row.event === name);
  assert.equal(matches.length, 1, `${key(rows[0])}: ${name}`);
  return matches[0];
}

const complete = new Map<string, { warm: Quantiles[]; cold: Quantiles[]; power: Power[] }>();
const failures: string[] = [];
let idleUtilizationN = 0;
let idleUtilizationMax: number | null = null;
for (const [id, rows] of cases) {
  const start = event(rows, "case_start");
  const end = event(rows, "case_end");
  if (end.status !== "complete") {
    failures.push(`${id}: ${end.status} ${end.error ?? ""}`.trim());
    continue;
  }
  const label = `${start.size} ${implementation(start)}`;
  const result = complete.get(label) ?? { warm: [], cold: [], power: [] };
  for (const phase of start.backend === "gpu" ? ["warm", "cold"] as const : ["warm"] as const) {
    const summary = event(rows, `${phase}_summary`) as unknown as Quantiles;
    const readings = rows.filter((row) => row.event === phase);
    assert.equal(summary.n, metadata[phase === "warm" ? "warm_samples_per_case" : "cold_samples_per_gpu_case"], id);
    assert((summary.max_abs_error as number) <= (metadata.epsilon as number), id);
    if (phase === "cold" || mode === "--trim") {
      assert.equal(readings.length, summary.n, `${id}: ${phase} raw samples`);
      const ns = sorted(readings.map((row) => row.latency_ns as number));
      for (const p of [50, 95, 99] as const) {
        assert.equal(ns[Math.ceil(ns.length * p / 100) - 1], summary[`p${p}_ns`], id);
      }
    }
    if (phase === "cold") {
      assert(readings.every((row) => (row.idle_ns as number) >= (metadata.cold_gap_min_ns as number)), id);
    }
    result[phase].push(summary);
  }
  const power = event(rows, "power_summary") as unknown as Power;
  assert(power.max_abs_error <= (metadata.epsilon as number), id);
  for (const sample of rows.filter((row) => row.event === "power" && row.phase === "idle")) {
    const { utilization_pct } = sample.reading as { utilization_pct: number | null };
    if (utilization_pct === null) continue;
    idleUtilizationN++;
    idleUtilizationMax = Math.max(idleUtilizationMax ?? utilization_pct, utilization_pct);
  }
  if (mode === "--trim") {
    for (const phase of ["idle", "active"] as const) {
      const watts = rows.filter((row) => row.event === "power" && row.phase === phase)
        .map((row) => (row.reading as { watts: number | null }).watts)
        .filter((value) => value !== null);
      assert.equal(watts.length, power[phase].n, id);
      assert.equal(median(watts), power[phase].median_w, id);
      assert(Math.abs(watts.reduce((total, value) => total + value, 0) / watts.length - power[phase].mean_w!) < 1e-12, id);
    }
  }
  result.power.push(power);
  complete.set(label, result);
}

assert.equal(failures.length, trace.at(-1)?.failed_or_incomplete_cases);
if (mode === "--trim") {
  const seen = new Map<string, number>();
  for (const [index, row] of trace.entries()) {
    if (row.event === "power" || row.event === "warm") {
      const id = `${key(row)} ${row.event} ${row.phase ?? ""}`;
      const count = (seen.get(id) ?? 0) + 1;
      seen.set(id, count);
      if (count > (row.event === "power" ? 3 : 8)) continue;
    }
    console.log(lines[index]);
  }
  process.exit(0);
}

const sizes = ["10k", "100k", "1M", "4M"];
const cpuBackends = [...new Set(trace.filter((v) => v.event === "case_start" && v.backend === "cpu")
  .map(implementation))].sort();
const backends = [...cpuBackends, "GPU"];
const us = (ns: number) => (ns / 1000).toFixed(2);
const around = (values: number[], format = us) =>
  `${format(median(values))} [${format(Math.min(...values))}, ${format(Math.max(...values))}]`;
const row = (columns: (number | string)[]) => console.log(`| ${columns.join(" | ")} |`);
const sum = (values: number[]) => values.reduce((total, value) => total + value, 0);
row(["Size", "Backend", "n", "p50 µs", "p95 µs [min, max]", "p99 µs [min, max]", "50 ms % (max p99)", "Max abs error"]);
row(Array(8).fill("---"));
for (const size of sizes) for (const backend of backends) {
  const result = complete.get(`${size} ${backend}`);
  if (!result) continue;
  const { warm, cold, power } = result;
  const maxP99 = Math.max(...warm.map((v) => v.p99_ns));
  row([
    size, backend, sum(warm.map((v) => v.n)), us(median(warm.map((v) => v.p50_ns))),
    around(warm.map((v) => v.p95_ns)), around(warm.map((v) => v.p99_ns)),
    (maxP99 / 500_000).toFixed(3),
    Math.max(...[...warm, ...cold, ...power].map((v) => v.max_abs_error)).toExponential(2),
  ]);
}

console.log();
row(["Size", "n", "p50 µs", "p95 µs [min, max]", "p99 µs [min, max]", "Δ p50 µs", "Δ p95 µs", "N p50 / p95 vs CPU/1", "50 ms % (max p99)"]);
row(Array(9).fill("---"));
for (const size of sizes) {
  const gpu = complete.get(`${size} GPU`);
  if (!gpu) continue;
  const cpu = complete.get(`${size} CPU/1`);
  function delta(p: "p50_ns" | "p95_ns") {
    const warm = median(gpu!.warm.map((v) => v[p]));
    const cold = median(gpu!.cold.map((v) => v[p]));
    const cost = cold - warm;
    const saving = cpu ? median(cpu.warm.map((v) => v[p])) - warm : 0;
    return { cost, n: saving <= 0 ? "—" : 1 + Math.floor(Math.max(0, cost) / saving) };
  }
  const p50 = delta("p50_ns");
  const p95 = delta("p95_ns");
  row([
    size, sum(gpu.cold.map((v) => v.n)), us(median(gpu.cold.map((v) => v.p50_ns))),
    around(gpu.cold.map((v) => v.p95_ns)), around(gpu.cold.map((v) => v.p99_ns)),
    us(p50.cost), us(p95.cost), `${p50.n} / ${p95.n}`,
    (Math.max(...gpu.cold.map((v) => v.p99_ns)) / 500_000).toFixed(3),
  ]);
}

console.log();
row(["Size", "Backend", "n idle / active", "Idle mean / median W", "Active mean / median W", "Active mean [min, max] W"]);
row(Array(6).fill("---"));
const watts = (v: number) => v.toFixed(2);
for (const size of sizes) for (const backend of backends) {
  const result = complete.get(`${size} ${backend}`);
  if (!result) continue;
  const { power } = result;
  const idle = power.map((v) => v.idle);
  const active = power.map((v) => v.active);
  const format = (phase: PowerPhase[]) =>
    `${watts(median(phase.map((v) => v.mean_w!)))} / ${watts(median(phase.map((v) => v.median_w!)))}`;
  row([
    size, backend, `${sum(idle.map((v) => v.n))} / ${sum(active.map((v) => v.n))}`,
    format(idle), format(active), around(active.map((v) => v.mean_w!), watts),
  ]);
}

console.log();
row(["Horizon", "Shared critical path", ...cpuBackends, "GPU warm", "GPU after queue idle"]);
row(Array(4 + cpuBackends.length).fill("---"));
for (const [horizon, horizonMs] of [["1 ms", 1], ["2 ms", 2], ["50 ms", 50], ["500 ms", 500], ["1 s", 1000]] as const) {
  for (const foreground of [true, false]) {
    const budget = Math.min(horizonMs, 50, foreground ? 2 : Infinity) * 1_000_000;
    const implementations = [
      ...cpuBackends.map((backend) => [backend, "warm"] as const),
      ["GPU", "warm"],
      ["GPU", "cold"],
    ] as const;
    row([horizon, foreground ? "Yes" : "No", ...implementations.map(([backend, phase]) => {
      return sizes.findLast((size) => {
        const values = complete.get(`${size} ${backend}`)?.[phase];
        return values?.length === metadata.runs && values.every((v) => v.p99_ns <= budget);
      }) ?? "—";
    })]);
  }
}

const cold = trace.filter((v) => v.event === "cold");
if (cold.length) {
  console.log(`\nQueue gaps: ${Math.min(...cold.map((v) => v.idle_ns as number))}–${Math.max(...cold.map((v) => v.idle_ns as number))} ns`);
  console.log(`PCI runtime states after gap: ${[...new Set(cold.map((v) => v.runtime_status_after_gap ?? "unavailable"))].join(", ")}`);
}
console.log(`Idle phase GPU utilization, retained samples: max ${idleUtilizationMax ?? "unavailable"}%, n=${idleUtilizationN}`);
if (failures.length) console.log(`\nFailures:\n${failures.join("\n")}`);
