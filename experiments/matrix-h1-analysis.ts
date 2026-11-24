// Usage: bun experiments/matrix-h1-analysis.ts RUN_DIR
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";

const [run] = Bun.argv.slice(2);
assert(run, "expected A4 run directory");
const arms = ["native", "static-low", "static-high", "reactive", "oracle"];
const sessionNames = ["A", "B"];
const interval = 50_000_000;
const quota = 21;
const lines = (text: string) => text.trim().split("\n").filter(Boolean).map(line => JSON.parse(line));
const read = (path: string) => lines(readFileSync(path, "utf8"));
type Trial = {
  arm: string;
  order_index: number;
  trace_file: string;
  void_reasons: string[];
  joint_slo_pass: boolean | null;
  phase_p99_ns: { spike: number } | null;
  background_completion: { missed_deadline: boolean; elapsed_ns: number | null } | null;
  expiry_metrics: Record<string, number> | null;
};
type Block = { seed: number; reserve: boolean; complete: boolean; trials: Trial[] };

function result(session: string, startSeed: number) {
  const path = join(run, "sessions", session, "session.jsonl");
  const rows = existsSync(path) ? read(path) : [];
  const start = rows.find(row => row.kind === "matrix_session_start") ?? null;
  const end = rows.find(row => row.kind === "matrix_session_end") ?? null;
  const blocks: Block[] = rows.filter(row => row.kind === "matrix_seed_block" && row.workload === "w1");
  const complete = blocks.filter(block => block.complete);
  for (const [index, block] of blocks.entries()) {
    assert.equal(block.seed, startSeed + index, session);
    assert.equal(block.reserve, index >= quota, session);
    assert(block.trials.every(trial => arms.includes(trial.arm)));
    assert.equal(new Set(block.trials.map(trial => trial.arm)).size, block.trials.length);
    assert.equal(block.complete, block.trials.length === arms.length
      && block.trials.every(trial => trial.void_reasons.length === 0), session);
  }
  if (end) assert.equal(complete.length, end.valid_blocks[0], session);
  const wrapper = join(run, `session-a4-${session}`, "environment.jsonl");
  const environment = existsSync(wrapper) ? read(wrapper) : [];
  const residencyOK = environment.some(row => row.label === "before")
    && environment.some(row => row.label === "after")
    && environment.every(row => row.label !== "residency check failed");
  return { session, start, end, blocks, complete, residencyOK };
}

function ratio(numerator: number, denominator: number) {
  return { numerator, denominator, rate: denominator ? numerator / denominator : null };
}

function percentile(values: number[], p: number) {
  if (!values.length) return null;
  return values.toSorted((a, b) => a - b)[Math.ceil(p * (values.length - 1))];
}

function mean(values: number[]) {
  return values.length ? values.reduce((a, b) => a + b, 0) / values.length : null;
}

function median(values: number[]) {
  const sorted = values.toSorted((a, b) => a - b);
  return values.length ? (sorted[Math.floor((values.length - 1) / 2)] + sorted[Math.floor(values.length / 2)]) / 2 : null;
}

function bootstrap(strata: number[][]) {
  const n = strata.flat().length;
  if (n === 0) return null;
  let state = 20_261_124n;
  const wrap = (x: bigint) => BigInt.asUintN(64, x);
  function indexOf(size: number) {
    state = wrap(state + 0x9E3779B97F4A7C15n);
    let z = wrap((state ^ (state >> 30n)) * 0xBF58476D1CE4E5B9n);
    z = wrap((z ^ (z >> 27n)) * 0x94D049BB133111EBn);
    return Number((z ^ (z >> 31n)) % BigInt(size));
  }
  const means = Array.from({ length: 10000 }, () => {
    let sum = 0;
    for (const stratum of strata)
      for (let i = 0; i < stratum.length; i++) sum += stratum[indexOf(stratum.length)];
    return sum / n;
  });
  return [percentile(means, 0.025), percentile(means, 0.975)];
}

function score(block: Block) {
  const native = block.trials.find(trial => trial.arm === "native")!;
  const oracle = block.trials.find(trial => trial.arm === "oracle")!;
  const reference = native.phase_p99_ns?.spike;
  const test = oracle.phase_p99_ns?.spike;
  const referenceMiss = native.background_completion?.missed_deadline;
  const testMiss = oracle.background_completion?.missed_deadline;
  assert(typeof reference === "number" && typeof test === "number");
  assert(typeof referenceMiss === "boolean" && typeof testMiss === "boolean");
  assert(typeof oracle.joint_slo_pass === "boolean" && typeof native.joint_slo_pass === "boolean");
  const diffMs = (reference - test) / 1e6;
  return {
    seed: block.seed,
    diff_ms: diffMs,
    relative_diff_percent: reference > 0 ? 100 * (reference - test) / reference : null,
    oracle_pass: oracle.joint_slo_pass,
    native_pass: native.joint_slo_pass,
    oracle_miss: testMiss,
    native_miss: referenceMiss,
  };
}

function controllerTiming(session: string, trials: Trial[]) {
  const ticks: number[] = [];
  const applies: number[] = [];
  let initialWindowMisses = 0;
  let measured = 0;
  let staleApplies = 0;
  const expiry = {
    expired_proposal_count: 0, weight_change_proposal_count: 0,
    forecasts_with_expired_proposal_count: 0, forecast_count: 0,
  };
  for (const trial of trials) {
    if (!trial.expiry_metrics) continue;
    const path = join(run, "sessions", session, trial.trace_file);
    const head = Bun.spawnSync(["head", "-n", "256", path]);
    assert.equal(head.exitCode, 0, path);
    const tickRows = lines(head.stdout.toString()).filter(row => row.kind === "control_tick");
    assert.equal(tickRows.length, 240, path);
    const audit = read(path.replace(/\.jsonl$/, ".audit.jsonl"));
    for (const key of Object.keys(expiry) as Array<keyof typeof expiry>) expiry[key] += trial.expiry_metrics[key];
    const first = tickRows.find(tick => tick.applied !== null);
    if (trial.arm !== "native") {
      measured++;
      if (!first || first.applied.elapsed_ns >= interval) initialWindowMisses++;
    }
    for (const tick of tickRows) {
      assert.equal(tick.tick_lateness_ns, tick.decision_start_ns - tick.scheduled_ns, path);
      ticks.push(tick.tick_lateness_ns);
      if (tick.applied?.elapsed_ns >= tick.scheduled_ns + interval) staleApplies++;
    }
    for (const row of audit.filter(row => row.event === "apply")) {
      const { apply_started_seconds: from, readback_completed_seconds: to } = row.action_timing;
      applies.push(Math.round((to - from) * 1e9));
    }
  }
  const describe = (samples: number[]) => ({
    count: samples.length,
    p99_ns: percentile(samples, 0.99),
    max_ns: samples.reduce<number | null>((max, value) => Math.max(max ?? value, value), null),
  });
  return {
    attempts: trials.length,
    voids: trials.filter(trial => trial.void_reasons.length > 0).length,
    expired_proposals: ratio(expiry.expired_proposal_count, expiry.weight_change_proposal_count),
    forecast_expiries: ratio(expiry.forecasts_with_expired_proposal_count, expiry.forecast_count),
    initial_window_misses: ratio(initialWindowMisses, measured),
    stale_applies: staleApplies,
    late_ticks: ratio(ticks.filter(ns => ns >= interval).length, ticks.length),
    tick_lateness: describe(ticks),
    apply_to_readback: describe(applies),
  };
}

const sessions = sessionNames.map((name, i) => result(name, 2001 + 1000 * i));
const [a, b] = sessions;
const sameState = a.start && b.start && a.start.llm_state === b.start.llm_state
  && a.start.boot_id === b.start.boot_id && a.start.binary_sha256 === b.start.binary_sha256;
const gap = a.end && b.start && sameState
  ? (b.start.start_monotonic_ns - a.end.end_monotonic_ns) / 1e9
  : null;
const cohort = sessions.map(session => session.complete.map(score));
const scores = cohort.flat();
const strata = cohort.map(stratum => stratum.map(row => row.diff_ms));
const differences = strata.flat();
const meanMs = mean(differences);
const ci = bootstrap(strata);
const sd = differences.length >= 2 && meanMs !== null
  ? Math.sqrt(differences.reduce((total, x) => total + (x - meanMs) ** 2, 0) / (differences.length - 1))
  : null;
const oraclePass = ratio(scores.filter(score => score.oracle_pass).length, scores.length);
const nativePass = ratio(scores.filter(score => score.native_pass).length, scores.length);
const oracleMiss = ratio(scores.filter(score => score.oracle_miss).length, scores.length);
const nativeMiss = ratio(scores.filter(score => score.native_miss).length, scores.length);
const cpuMissDiff = cohort.map(rows => rows.map(row => 100 * (Number(row.native_miss) - Number(row.oracle_miss))));
const jointPassDiff = cohort.map(rows => rows.map(row => 100 * (Number(row.oracle_pass) - Number(row.native_pass))));
const conditions = {
  quota: sessions.every(session => session.complete.length === quota && session.end !== null),
  session_gap: gap !== null && gap >= 900,
  residency: sessions.every(session => session.residencyOK),
  ci_above_zero: (ci?.[0] ?? 0) > 0,
  mean_at_least_1_ms: meanMs !== null && meanMs >= 1,
  favorable_both_sessions: strata.every(row => (mean(row) ?? 0) > 0),
  oracle_slo_at_least_90_percent: (oraclePass.rate ?? 0) >= 0.9,
  oracle_cpu_miss_rate_no_higher_than_native: scores.length > 0 && oracleMiss.numerator <= nativeMiss.numerator,
};
console.log(JSON.stringify({
  h1_pass: Object.values(conditions).every(Boolean),
  conditions,
  gap_seconds: gap,
  paired_spike_p99_ms: {
    n: differences.length, mean: meanMs, ci_95: ci,
    dz: sd !== null && sd > 0 && meanMs !== null ? meanMs / sd : null,
    median_relative_improvement_percent: median(scores.flatMap(score =>
      score.relative_diff_percent === null ? [] : [score.relative_diff_percent])),
  },
  oracle_pass: oraclePass, native_pass: nativePass, oracle_cpu_miss: oracleMiss, native_cpu_miss: nativeMiss,
  joint_pass_gain_percentage_points: { mean: mean(jointPassDiff.flat()), ci_95: bootstrap(jointPassDiff) },
  cpu_miss_gain_percentage_points: { mean: mean(cpuMissDiff.flat()), ci_95: bootstrap(cpuMissDiff) },
  sessions: sessions.map(({ session, start, end, blocks, complete, residencyOK }, i) => ({
    session, start, end, residency_ok: residencyOK,
    complete_blocks: complete.length,
    attempted_blocks: blocks.length,
    mean_spike_p99_improvement_ms: mean(strata[i]),
    pairs: cohort[i],
    realized_order: blocks.map(({ seed, reserve, complete, trials }) =>
      ({ seed, reserve, complete, arms: trials.map(trial => trial.arm) })),
    void_ledger: blocks.flatMap(block => block.trials.filter(trial => trial.void_reasons.length > 0)
      .map(trial => ({ seed: block.seed, arm: trial.arm, reasons: trial.void_reasons }))),
    controller: Object.fromEntries(arms.map(arm => [arm, controllerTiming(session,
      blocks.flatMap(block => block.trials.filter(trial => trial.arm === arm)))])),
  })),
}, null, 2));
